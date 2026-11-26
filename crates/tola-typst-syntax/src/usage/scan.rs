//! AST scanning for function regions, write sites, and local/captured uses.

use typst_syntax::ast::{self, AstNode};
use typst_syntax::{LinkedNode, SyntaxKind};

use crate::names::OccurrenceKind;

use super::{CANCELLATION_POLL_INTERVAL, LiveSource, Region, WriteSite};

impl<'a> LiveSource<'a> {
    /// Records every function region, every identifier's range, and every write target.
    pub(super) fn scan(&mut self, node: &LinkedNode, region: usize) {
        self.tick();
        if self.halt {
            return;
        }
        if node.kind() == SyntaxKind::Error {
            self.uncertain = true;
        }
        if matches!(node.kind(), SyntaxKind::Ident | SyntaxKind::MathIdent) {
            self.spans
                .entry(node.span())
                .or_insert_with(|| node.range());
        }
        let write = match node.kind() {
            SyntaxKind::Binary => node
                .cast::<ast::Binary>()
                .and_then(|bin| assignment_reads(bin.op()).zip(written_ident(bin.lhs()))),
            _ => None,
        };
        let destructured = match node.kind() {
            SyntaxKind::DestructAssignment => node
                .cast::<ast::DestructAssignment>()
                .map(|assign| assign.pattern().bindings()),
            _ => None,
        };
        // A closure or `context` body forms its own region; parameter defaults stay in the creator.
        let body = match node.kind() {
            SyntaxKind::Closure => node
                .cast::<ast::Closure>()
                .map(|closure| node.find(closure.body().span())),
            SyntaxKind::Contextual => node
                .cast::<ast::Contextual>()
                .map(|ctx| node.find(ctx.body().span())),
            _ => None,
        };
        if let Some(body) = body {
            let child = self.push_region(region, body.as_ref(), node.range().start);
            for child_node in node.children() {
                let in_body = body
                    .as_ref()
                    .is_some_and(|body| body.range() == child_node.range());
                self.scan(&child_node, if in_body { child } else { region });
                if self.halt {
                    return;
                }
            }
            return;
        }
        for child in node.children() {
            self.scan(&child, region);
            if self.halt {
                return;
            }
        }
        // Recorded after scanning children so write-target spans are already indexed.
        if let Some((read, ident)) = write {
            self.record_write(ident, read);
        }
        if let Some(names) = destructured {
            for name in names {
                self.record_write(name, false);
            }
        }
    }

    fn push_region(&mut self, parent: usize, body: Option<&LinkedNode>, creation: usize) -> usize {
        let id = self.regions.len();
        self.regions.push(Region {
            parent,
            body: body.map_or(creation..creation, |body| body.range()),
            creation,
            ..Region::default()
        });
        self.creation_regions.insert(creation, id);
        id
    }

    fn record_write(&mut self, ident: ast::Ident<'_>, read: bool) {
        let Some(range) = self.range_of(ident.span()) else {
            return;
        };
        let start = range.start;
        self.writes.entry(start).or_insert(WriteSite {
            declaration: None,
            range,
            read,
            real: false,
        });
    }

    /// Resolves each write target and whether it stores into a binding of its own region.
    pub(super) fn finish_writes(&mut self) -> Option<()> {
        let offsets: Vec<usize> = self.writes.keys().copied().collect();
        for (idx, offset) in offsets.iter().enumerate() {
            if idx.is_multiple_of(CANCELLATION_POLL_INTERVAL) {
                self.poll()?;
            }
            let Some(decl) = self.names.declared_at(*offset) else {
                continue;
            };
            let decl_region = self.region_at(self.names.declarations()[decl].range.start);
            let write_region = self.region_at(*offset);
            let site = self
                .writes
                .get_mut(offset)
                .expect("a recorded write target");
            site.declaration = Some(decl);
            site.real = decl_region == write_region;
        }
        Some(())
    }

    /// Attributes every read to the region whose flow observes it.
    pub(super) fn collect_uses(&mut self) -> Option<()> {
        let names = self.names;
        let mut seen = 0usize;
        for occ in names.occurrences() {
            self.poll_every(&mut seen)?;
            if !matches!(occ.kind, OccurrenceKind::Name) {
                continue;
            }
            // Compound assignment targets read before storing; non-compound targets only store.
            if let Some(site) = self.writes.get(&occ.range.start)
                && !site.read
            {
                continue;
            }
            let Some(decl) = names.declared_at(occ.range.start) else {
                continue;
            };
            let spelled = &names.declarations()[decl];
            if spelled
                .recursive_body
                .as_ref()
                .is_some_and(|body| body.contains(&occ.range.start))
            {
                continue;
            }
            let read_region = self.region_at(occ.range.start);
            let decl_region = self.region_at(spelled.range.start);
            if read_region == decl_region {
                self.reads
                    .entry((occ.range.start, occ.range.end))
                    .or_default()
                    .push(decl);
            } else if let Some(inner) = self.region_below(read_region, decl_region) {
                self.captures
                    .entry(self.regions[inner].creation)
                    .or_default()
                    .push(decl);
            } else {
                self.suppressed.insert(decl);
            }
        }
        for decls in self.reads.values_mut() {
            decls.sort_unstable();
            decls.dedup();
        }
        for decls in self.captures.values_mut() {
            decls.sort_unstable();
            decls.dedup();
        }
        Some(())
    }
}

/// Whether a binary operator stores into its left-hand side; `true` when it also reads it.
pub(super) fn assignment_reads(op: ast::BinOp) -> Option<bool> {
    match op {
        ast::BinOp::Assign => Some(false),
        ast::BinOp::AddAssign
        | ast::BinOp::SubAssign
        | ast::BinOp::MulAssign
        | ast::BinOp::DivAssign => Some(true),
        _ => None,
    }
}

/// The identifier a store target writes, if it writes a binding rather than a field or method.
pub(super) fn written_ident<'a>(target: ast::Expr<'a>) -> Option<ast::Ident<'a>> {
    match target {
        ast::Expr::Ident(ident) => Some(ident),
        ast::Expr::Parenthesized(group) => written_ident(group.expr()),
        _ => None,
    }
}
