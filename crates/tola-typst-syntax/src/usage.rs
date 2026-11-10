//! Local liveness of one source's bindings.
//!
//! The name index answers what a source declares and how a spelling resolves; it does not order
//! the stores and reads of a binding's value, so it cannot say that a binding is never read or
//! that a stored value is never observed. This module derives that order from the source's own
//! control flow while keeping declaration identity in [`SourceNames`].

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;

use typst_syntax::ast::{self, AstNode};
use typst_syntax::{LinkedNode, Span, SyntaxKind};

use crate::names::{DeclarationKind, OccurrenceKind, SelectedInterfaces, SourceNames};

/// One store whose value no reachable read observes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadStore {
    /// Index into [`SourceNames::declarations`].
    pub declaration: usize,
    /// The write occurrence's bytes in the source; for a declaration, its name.
    pub range: Range<usize>,
}

/// The local liveness one source establishes on its own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Liveness {
    /// `let` and `for` bindings no reachable read reaches, in declaration order.
    pub unused_bindings: Vec<usize>,
    /// Stores no reachable read observes, in byte order.
    pub dead_stores: Vec<DeadStore>,
}

impl SourceNames {
    /// The local bindings no read reaches and the stores no read observes.
    ///
    /// `self` must be the index of the parsed source this answer describes: every declaration
    /// index and range in the answer addresses that source. `selected` licenses the file scope
    /// exactly as [`SourceNames::reportable_unread_imports`] does. A scope-0 binding another
    /// source may select keeps the module's read of its value after evaluation, so it is never
    /// unused and its final store stays live; an unresolved index withholds them all. A name
    /// declared more than once at the file scope exports its final visible binding only; earlier
    /// declarations of that name are ordinary locals.
    ///
    /// The answer is a backward liveness over the source's own control flow, one region per
    /// function body (file, closure, `context`). Branch joins and loop back edges are solved to a
    /// fixed point, `break`, `continue`, and `return` end their paths, and statements behind a
    /// direct jump are not reached. An assignment reads its right-hand side before it accesses
    /// its target, and a reassignment pattern reads its item targets at their own place. A
    /// closure's defaults are evaluated before the reads it captures, and the reads happen at
    /// its creation, matching Typst's capture by value; a named function's read of itself is
    /// provided by its callee scope rather than captured; the write the compiler refuses through
    /// a captured binding is not a store. Short-circuit operands are may paths. Only `let` and
    /// `for` bindings are answered: parameters are never reported, and imports remain the
    /// unused-import report's. A store whose binding no reachable read reaches is reported in
    /// [`Liveness::unused_bindings`] alone.
    ///
    /// `cancelled` is polled between the analysis phases and inside its loops. `None` means the
    /// analysis was cancelled and carries no partial answer; no other condition produces `None`.
    /// A source that does not parse cleanly answers no findings, and so does one whose loop
    /// header leaves a `break` or `continue` to a loop this analysis cannot model: a transient
    /// syntax error or an unmodelled header never invents a finding.
    pub fn liveness(
        &self,
        selected: &SelectedInterfaces,
        cancelled: impl Fn() -> bool,
    ) -> Option<Liveness> {
        LiveSource::new(self, selected, &cancelled).run()
    }
}

/// How many visited nodes pass between cancellation polls inside a large scan.
const CANCELLATION_POLL_INTERVAL: usize = 256;

/// One body and its control flow; region zero is the file itself.
#[derive(Default)]
struct Region {
    parent: usize,
    body: Range<usize>,
    /// Where the closure or `context` is created in the parent region.
    creation: usize,
    creation_node: Option<usize>,
    nodes: Vec<Node>,
    entry: usize,
    exit: usize,
    loops: Vec<(usize, usize)>,
}

/// One evaluation step that observes or replaces a binding's value.
enum Action {
    Use(usize),
    Store {
        declaration: usize,
        range: Range<usize>,
        report: bool,
    },
}

/// One flow node: its ordered actions and where control may continue.
#[derive(Default)]
struct Node {
    actions: Vec<Action>,
    successors: Vec<usize>,
}

/// One syntactic write target, before the declaration it stores into is known.
struct WriteSite {
    declaration: Option<usize>,
    range: Range<usize>,
    read: bool,
    real: bool,
}

struct LiveSource<'a> {
    names: &'a SourceNames,
    selected: &'a SelectedInterfaces,
    cancelled: &'a dyn Fn() -> bool,

    regions: Vec<Region>,
    /// Region ids by body start; equal starts keep the innermost region last.
    region_order: Vec<usize>,
    creation_regions: BTreeMap<usize, usize>,
    writes: BTreeMap<usize, WriteSite>,
    /// Every identifier's bytes by its span, so lookups stay off the tree.
    spans: HashMap<Span, Range<usize>>,
    /// Reads by their own identifier bytes: `(start, end)` -> declarations.
    reads: BTreeMap<(usize, usize), Vec<usize>>,
    /// Captured reads by the creation offset of the closure or `context`.
    captures: BTreeMap<usize, Vec<usize>>,
    init_nodes: Vec<Option<(usize, usize)>>,
    exported: BTreeSet<usize>,
    suppressed: BTreeSet<usize>,
    /// Whether the walk currently evaluates a loop header, whose break stays unmodelled.
    in_header: bool,
    halt: bool,
    uncertain: bool,
    visits: usize,
}

impl<'a> LiveSource<'a> {
    fn new(
        names: &'a SourceNames,
        selected: &'a SelectedInterfaces,
        cancelled: &'a dyn Fn() -> bool,
    ) -> Self {
        Self {
            names,
            selected,
            cancelled,
            regions: vec![Region {
                body: 0..names.source().text().len(),
                ..Region::default()
            }],
            region_order: Vec::new(),
            creation_regions: BTreeMap::new(),
            writes: BTreeMap::new(),
            spans: HashMap::new(),
            reads: BTreeMap::new(),
            captures: BTreeMap::new(),
            init_nodes: vec![None; names.declarations().len()],
            exported: BTreeSet::new(),
            suppressed: BTreeSet::new(),
            in_header: false,
            halt: false,
            uncertain: false,
            visits: 0,
        }
    }

    fn run(mut self) -> Option<Liveness> {
        self.poll()?;
        let root = LinkedNode::new(self.names.source().root());
        self.scan(&root, 0);
        self.poll()?;
        if self.halt {
            return None;
        }
        if self.uncertain {
            return Some(Liveness::default());
        }
        self.order_regions();
        self.finish_writes()?;
        self.collect_uses()?;
        self.lower_region(0, &root);
        self.poll()?;
        if self.halt {
            return None;
        }
        if self.uncertain {
            return Some(Liveness::default());
        }
        self.append_export_uses();
        self.solve()
    }

    fn poll(&self) -> Option<()> {
        if (self.cancelled)() { None } else { Some(()) }
    }

    /// Polls cancellation once every [`CANCELLATION_POLL_INTERVAL`] visited items.
    fn poll_every(&self, scanned: &mut usize) -> Option<()> {
        *scanned += 1;
        if scanned.is_multiple_of(CANCELLATION_POLL_INTERVAL) {
            self.poll()?;
        }
        Some(())
    }

    fn tick(&mut self) {
        self.visits += 1;
        if self.visits.is_multiple_of(CANCELLATION_POLL_INTERVAL) && (self.cancelled)() {
            self.halt = true;
        }
    }

    /// Records every function region, every identifier's range, and every write target.
    fn scan(&mut self, node: &LinkedNode, region: usize) {
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
                .and_then(|binary| assignment_reads(binary.op()).zip(written_ident(binary.lhs()))),
            _ => None,
        };
        let destructured = match node.kind() {
            SyntaxKind::DestructAssignment => node
                .cast::<ast::DestructAssignment>()
                .map(|assignment| assignment.pattern().bindings()),
            _ => None,
        };
        // A closure or `context` gives its body a region of its own; everything beside that body,
        // parameters included, belongs to the region that creates it.
        let body = match node.kind() {
            SyntaxKind::Closure => node
                .cast::<ast::Closure>()
                .map(|closure| node.find(closure.body().span())),
            SyntaxKind::Contextual => node
                .cast::<ast::Contextual>()
                .map(|contextual| node.find(contextual.body().span())),
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
        // A write target's identifier is only ranged once its own subtree was scanned.
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
    fn finish_writes(&mut self) -> Option<()> {
        let offsets: Vec<usize> = self.writes.keys().copied().collect();
        for (index, offset) in offsets.iter().enumerate() {
            if index.is_multiple_of(CANCELLATION_POLL_INTERVAL) {
                self.poll()?;
            }
            let Some(declaration) = self.names.declared_at(*offset) else {
                continue;
            };
            let declaration_region =
                self.region_at(self.names.declarations()[declaration].range.start);
            let write_region = self.region_at(*offset);
            let site = self
                .writes
                .get_mut(offset)
                .expect("a recorded write target");
            site.declaration = Some(declaration);
            site.real = declaration_region == write_region;
        }
        Some(())
    }

    /// Attributes every read to the region whose flow observes it.
    fn collect_uses(&mut self) -> Option<()> {
        let names = self.names;
        let mut seen = 0usize;
        for occurrence in names.occurrences() {
            self.poll_every(&mut seen)?;
            if !matches!(occurrence.kind, OccurrenceKind::Name) {
                continue;
            }
            // A compound assignment's target is read before it is stored, and that read is
            // attributed like any other; the store's own region only decides whether the write
            // is a store this analysis can model.
            if let Some(site) = self.writes.get(&occurrence.range.start)
                && !site.read
            {
                continue;
            }
            let Some(declaration) = names.declared_at(occurrence.range.start) else {
                continue;
            };
            let spelled = &names.declarations()[declaration];
            if spelled
                .recursive_body
                .as_ref()
                .is_some_and(|body| body.contains(&occurrence.range.start))
            {
                continue;
            }
            let read_region = self.region_at(occurrence.range.start);
            let declaration_region = self.region_at(spelled.range.start);
            if read_region == declaration_region {
                self.reads
                    .entry((occurrence.range.start, occurrence.range.end))
                    .or_default()
                    .push(declaration);
            } else if let Some(inner) = self.region_below(read_region, declaration_region) {
                self.captures
                    .entry(self.regions[inner].creation)
                    .or_default()
                    .push(declaration);
            } else {
                self.suppressed.insert(declaration);
            }
        }
        for declarations in self.reads.values_mut() {
            declarations.sort_unstable();
            declarations.dedup();
        }
        for declarations in self.captures.values_mut() {
            declarations.sort_unstable();
            declarations.dedup();
        }
        Some(())
    }

    /// The region whose parent is `ancestor`, walking up from `region`.
    fn region_below(&self, mut region: usize, ancestor: usize) -> Option<usize> {
        loop {
            let parent = self.regions[region].parent;
            if parent == ancestor {
                return Some(region);
            }
            if parent == region {
                return None;
            }
            region = parent;
        }
    }

    /// Orders regions for [`Self::region_at`]: body starts ascend, equal starts end first.
    fn order_regions(&mut self) {
        let mut order: Vec<usize> = (0..self.regions.len()).collect();
        order.sort_by_key(|&region| {
            let body = &self.regions[region].body;
            (body.start, std::cmp::Reverse(body.end))
        });
        self.region_order = order;
    }

    /// The innermost region whose body holds the offset.
    fn region_at(&self, offset: usize) -> usize {
        let index = self
            .region_order
            .partition_point(|&region| self.regions[region].body.start <= offset);
        if index == 0 {
            return 0;
        }
        let mut region = self.region_order[index - 1];
        loop {
            let body = &self.regions[region].body;
            if body.start <= offset && offset < body.end {
                return region;
            }
            if region == 0 {
                return 0;
            }
            region = self.regions[region].parent;
        }
    }

    fn range_of(&self, span: Span) -> Option<Range<usize>> {
        self.spans
            .get(&span)
            .cloned()
            .or_else(|| self.names.source().find(span).map(|node| node.range()))
    }

    fn new_node(&mut self, region: usize) -> usize {
        let nodes = &mut self.regions[region].nodes;
        nodes.push(Node::default());
        nodes.len() - 1
    }

    fn edge(&mut self, region: usize, from: usize, to: usize) {
        self.regions[region].nodes[from].successors.push(to);
    }

    /// Flushes the read at the identifier node that evaluates it.
    fn flush_read(&mut self, region: usize, node: &LinkedNode, at: usize) {
        if !matches!(node.kind(), SyntaxKind::Ident | SyntaxKind::MathIdent) {
            return;
        }
        let range = node.range();
        if let Some(declarations) = self.reads.remove(&(range.start, range.end)) {
            self.push_uses(region, at, &declarations);
        }
    }

    /// Flushes the reads a closure or `context` captures: once, at its creation.
    fn flush_captures(&mut self, region: usize, creation: usize, at: usize) {
        if let Some(declarations) = self.captures.remove(&creation) {
            self.push_uses(region, at, &declarations);
        }
    }

    /// Records one use of each declaration read at one node.
    fn push_uses(&mut self, region: usize, at: usize, declarations: &[usize]) {
        let actions = &mut self.regions[region].nodes[at].actions;
        for &declaration in declarations {
            actions.push(Action::Use(declaration));
        }
    }

    /// Builds one region's flow from its body, the whole source for region zero.
    fn lower_region(&mut self, region: usize, body: &LinkedNode) {
        let entry = self.new_node(region);
        let exit = self.new_node(region);
        self.regions[region].entry = entry;
        self.regions[region].exit = exit;
        let saved = self.in_header;
        self.in_header = false;
        let tail = self.walk(body, region, entry);
        self.in_header = saved;
        if let Some(tail) = tail {
            self.edge(region, tail, exit);
        }
    }

    /// Lowers one closure or `context` body into its own region.
    fn enter_region(&mut self, creation: usize, body: &LinkedNode, creation_node: usize) {
        let Some(&region) = self.creation_regions.get(&creation) else {
            return;
        };
        self.regions[region].creation_node = Some(creation_node);
        self.lower_region(region, body);
    }

    /// Lowers a loop body with this loop as the innermost `break` and `continue` target, so the
    /// body's tail flows back to the loop's head.
    fn lower_loop_body(
        &mut self,
        region: usize,
        head: usize,
        exit: usize,
        entry: usize,
        body: Option<LinkedNode<'_>>,
    ) {
        self.regions[region].loops.push((exit, head));
        let saved = self.in_header;
        self.in_header = false;
        let lowered = body.and_then(|body| self.walk(&body, region, entry));
        self.in_header = saved;
        self.regions[region].loops.pop();
        if let Some(tail) = lowered {
            self.edge(region, tail, head);
        }
    }

    fn walk(&mut self, node: &LinkedNode, region: usize, current: usize) -> Option<usize> {
        self.tick();
        if self.halt {
            return Some(current);
        }
        match node.kind() {
            SyntaxKind::Ident | SyntaxKind::MathIdent => {
                self.flush_read(region, node, current);
                return Some(current);
            }
            SyntaxKind::CodeBlock | SyntaxKind::ContentBlock => {
                if let Some(body) = block_body(node) {
                    return self.sequence(&body, region, current);
                }
            }
            SyntaxKind::Markup => return self.sequence(node, region, current),
            SyntaxKind::Conditional => {
                if let Some(conditional) = node.cast::<ast::Conditional>() {
                    let mut cur = current;
                    if let Some(condition) = node.find(conditional.condition().span()) {
                        cur = self.walk(&condition, region, cur)?;
                    }
                    let join = self.new_node(region);
                    if let Some(if_body) = node.find(conditional.if_body().span()) {
                        let entry = self.new_node(region);
                        self.edge(region, cur, entry);
                        if let Some(tail) = self.walk(&if_body, region, entry) {
                            self.edge(region, tail, join);
                        }
                    }
                    if let Some(else_body) = conditional
                        .else_body()
                        .and_then(|body| node.find(body.span()))
                    {
                        let entry = self.new_node(region);
                        self.edge(region, cur, entry);
                        if let Some(tail) = self.walk(&else_body, region, entry) {
                            self.edge(region, tail, join);
                        }
                    } else {
                        self.edge(region, cur, join);
                    }
                    return Some(join);
                }
            }
            SyntaxKind::ForLoop => {
                if let Some(for_loop) = node.cast::<ast::ForLoop>() {
                    let mut cur = current;
                    // A break in the header sets flow this loop may or may not consume, decided
                    // by the header's value; withhold the answer instead of guessing its owner.
                    let saved = self.in_header;
                    self.in_header = true;
                    let header = match node.find(for_loop.iterable().span()) {
                        Some(iterable) => self.walk(&iterable, region, cur),
                        None => Some(cur),
                    };
                    self.in_header = saved;
                    cur = header?;
                    let head = self.new_node(region);
                    self.edge(region, cur, head);
                    let exit = self.new_node(region);
                    let body_entry = self.new_node(region);
                    self.edge(region, head, body_entry);
                    self.edge(region, head, exit);
                    // A pattern binding happens per iteration, never on the zero-iteration exit.
                    self.store_bindings(region, body_entry, for_loop.pattern().bindings(), false);
                    let body = node.find(for_loop.body().span());
                    self.lower_loop_body(region, head, exit, body_entry, body);
                    return Some(exit);
                }
            }
            SyntaxKind::WhileLoop => {
                if let Some(while_loop) = node.cast::<ast::WhileLoop>() {
                    let head = self.new_node(region);
                    self.edge(region, current, head);
                    let saved = self.in_header;
                    self.in_header = true;
                    let condition = match node.find(while_loop.condition().span()) {
                        Some(condition) => self.walk(&condition, region, head),
                        None => Some(head),
                    };
                    self.in_header = saved;
                    let exit = self.new_node(region);
                    let body_entry = self.new_node(region);
                    match condition {
                        Some(tail) => {
                            self.edge(region, tail, body_entry);
                            self.edge(region, tail, exit);
                        }
                        None => self.edge(region, head, exit),
                    }
                    let body = node.find(while_loop.body().span());
                    self.lower_loop_body(region, head, exit, body_entry, body);
                    return Some(exit);
                }
            }
            SyntaxKind::LoopBreak | SyntaxKind::LoopContinue => {
                if self.in_header {
                    self.uncertain = true;
                }
                let target =
                    self.regions[region]
                        .loops
                        .last()
                        .map(|&(break_target, continue_target)| {
                            if node.kind() == SyntaxKind::LoopBreak {
                                break_target
                            } else {
                                continue_target
                            }
                        });
                if let Some(target) = target {
                    self.edge(region, current, target);
                }
                return None;
            }
            SyntaxKind::FuncReturn => {
                if let Some(return_) = node.cast::<ast::FuncReturn>() {
                    let mut cur = current;
                    if let Some(value) = return_.body().and_then(|body| node.find(body.span())) {
                        cur = self.walk(&value, region, cur)?;
                    }
                    let exit = self.regions[region].exit;
                    self.edge(region, cur, exit);
                }
                return None;
            }
            SyntaxKind::Closure => {
                if let Some(closure) = node.cast::<ast::Closure>() {
                    let mut cur = current;
                    for parameter in closure.params().children() {
                        let ast::Param::Named(named) = parameter else {
                            continue;
                        };
                        let Some(default) = node.find(named.expr().span()) else {
                            continue;
                        };
                        cur = self.walk(&default, region, cur)?;
                    }
                    if let Some(body) = node.find(closure.body().span()) {
                        self.flush_captures(region, node.range().start, cur);
                        self.enter_region(node.range().start, &body, cur);
                    }
                    return Some(cur);
                }
            }
            SyntaxKind::Contextual => {
                if let Some(contextual) = node.cast::<ast::Contextual>() {
                    self.flush_captures(region, node.range().start, current);
                    if let Some(body) = node.find(contextual.body().span()) {
                        self.enter_region(node.range().start, &body, current);
                    }
                    return Some(current);
                }
            }
            SyntaxKind::LetBinding => {
                if let Some(binding) = node.cast::<ast::LetBinding>() {
                    let mut cur = current;
                    if let Some(init) = binding.init().and_then(|init| node.find(init.span())) {
                        cur = self.walk(&init, region, cur)?;
                    }
                    let report = binding.init().is_some();
                    self.store_bindings(region, cur, binding.kind().bindings(), report);
                    return Some(cur);
                }
            }
            SyntaxKind::DestructAssignment => {
                if let Some(assignment) = node.cast::<ast::DestructAssignment>() {
                    let mut cur = current;
                    if let Some(value) = node.find(assignment.value().span()) {
                        cur = self.walk(&value, region, cur)?;
                    }
                    // The compiler assigns one target at a time, so a later target's index or
                    // receiver observes the values earlier targets stored.
                    if let Some(pattern) = node.find(assignment.pattern().span()) {
                        cur =
                            self.lower_destructured(region, &pattern, assignment.pattern(), cur)?;
                    }
                    return Some(cur);
                }
            }
            SyntaxKind::Binary => {
                if let Some(binary) = node.cast::<ast::Binary>() {
                    match binary.op() {
                        ast::BinOp::And | ast::BinOp::Or => {
                            let mut cur = current;
                            if let Some(lhs) = node.find(binary.lhs().span()) {
                                cur = self.walk(&lhs, region, cur)?;
                            }
                            let join = self.new_node(region);
                            self.edge(region, cur, join);
                            let rhs_entry = self.new_node(region);
                            self.edge(region, cur, rhs_entry);
                            if let Some(rhs) = node.find(binary.rhs().span())
                                && let Some(tail) = self.walk(&rhs, region, rhs_entry)
                            {
                                self.edge(region, tail, join);
                            }
                            return Some(join);
                        }
                        op => {
                            let mut cur = current;
                            // An assignment evaluates its right-hand side before it accesses its
                            // target (typst-eval `apply_assignment`), so a compound read observes
                            // the value that side stored.
                            let stores = assignment_reads(op).is_some();
                            let order = if stores {
                                [binary.rhs(), binary.lhs()]
                            } else {
                                [binary.lhs(), binary.rhs()]
                            };
                            for side in order {
                                if let Some(side) = node.find(side.span()) {
                                    cur = self.walk(&side, region, cur)?;
                                }
                            }
                            if stores && let Some(ident) = written_ident(binary.lhs()) {
                                self.store_written(region, cur, ident);
                            }
                            return Some(cur);
                        }
                    }
                }
            }
            _ => {}
        }
        let mut cur = current;
        for child in node.children() {
            cur = self.walk(&child, region, cur)?;
        }
        Some(cur)
    }

    /// Records the store each binding of one pattern makes at the node that binds it.
    fn store_bindings(
        &mut self,
        region: usize,
        node: usize,
        bindings: Vec<ast::Ident<'_>>,
        report: bool,
    ) {
        for name in bindings {
            let Some(range) = self.range_of(name.span()) else {
                continue;
            };
            let Some(declaration) = self.names.declared_at(range.start) else {
                continue;
            };
            let store_range = self.names.declarations()[declaration].range.clone();
            self.regions[region].nodes[node]
                .actions
                .push(Action::Store {
                    declaration,
                    range: store_range,
                    report,
                });
            self.init_nodes[declaration] = Some((region, node));
        }
    }

    /// Emits the store of one assignment target when it stores a binding of this region.
    fn store_written(&mut self, region: usize, node: usize, ident: ast::Ident<'_>) {
        let Some(range) = self.range_of(ident.span()) else {
            return;
        };
        let Some(site) = self.writes.get(&range.start) else {
            return;
        };
        let (Some(declaration), true) = (site.declaration, site.real) else {
            return;
        };
        let range = site.range.clone();
        self.regions[region].nodes[node]
            .actions
            .push(Action::Store {
                declaration,
                range,
                report: true,
            });
    }

    /// Lowers one reassignment pattern in the order the compiler assigns its targets.
    fn lower_destructured(
        &mut self,
        region: usize,
        pattern_node: &LinkedNode,
        pattern: ast::Pattern<'_>,
        cur: usize,
    ) -> Option<usize> {
        match pattern {
            ast::Pattern::Destructuring(destructuring) => {
                let mut cur = cur;
                for item in destructuring.items() {
                    let Some(item_node) = pattern_node.find(item.span()) else {
                        continue;
                    };
                    cur = self.lower_destructuring_item(region, &item_node, item, cur)?;
                }
                Some(cur)
            }
            ast::Pattern::Parenthesized(parenthesized) => {
                match pattern_node.find(parenthesized.pattern().span()) {
                    Some(inner) => {
                        self.lower_destructured(region, &inner, parenthesized.pattern(), cur)
                    }
                    None => Some(cur),
                }
            }
            ast::Pattern::Normal(expr) => self.lower_target(region, pattern_node, expr, cur),
            ast::Pattern::Placeholder(_) => Some(cur),
        }
    }

    /// Lowers one pattern target, emitting the store it makes when it names a binding.
    fn lower_target(
        &mut self,
        region: usize,
        target: &LinkedNode,
        expression: ast::Expr<'_>,
        cur: usize,
    ) -> Option<usize> {
        let cur = self.walk(target, region, cur)?;
        if let ast::Expr::Ident(ident) = expression {
            self.store_written(region, cur, ident);
        }
        Some(cur)
    }

    /// Lowers one pattern item after the items before it were assigned.
    fn lower_destructuring_item(
        &mut self,
        region: usize,
        item_node: &LinkedNode,
        item: ast::DestructuringItem<'_>,
        cur: usize,
    ) -> Option<usize> {
        match item {
            ast::DestructuringItem::Pattern(pattern) => {
                self.lower_destructured(region, item_node, pattern, cur)
            }
            ast::DestructuringItem::Named(named) => match item_node.find(named.pattern().span()) {
                Some(target) => self.lower_destructured(region, &target, named.pattern(), cur),
                None => Some(cur),
            },
            ast::DestructuringItem::Spread(spread) => {
                let Some(expr) = spread.sink_expr() else {
                    return Some(cur);
                };
                let Some(target) = item_node.find(expr.span()) else {
                    return Some(cur);
                };
                self.lower_target(region, &target, expr, cur)
            }
        }
    }

    /// Lowers a statement sequence, sharing one node across straight-line statements.
    fn sequence(&mut self, block: &LinkedNode, region: usize, entry: usize) -> Option<usize> {
        let mut current = Some(entry);
        for child in block.children() {
            if self.halt {
                break;
            }
            if child.kind().is_trivia() {
                continue;
            }
            let Some(cur) = current else {
                break;
            };
            current = self.walk(&child, region, cur);
        }
        current
    }

    /// Adds the module's read of each exported scope-0 binding at the end of the file.
    ///
    /// A name exports the binding its last scope-0 declaration spells; declarations of the same
    /// name before it are shadowed before the module ends.
    fn append_export_uses(&mut self) {
        let names = self.names;
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut declarations = Vec::new();
        for &declaration in names.scopes()[0].declarations.iter().rev() {
            let spelled = &names.declarations()[declaration];
            if !seen.insert(spelled.name.as_str()) {
                continue;
            }
            if !matches!(&spelled.kind, DeclarationKind::Let | DeclarationKind::Loop) {
                continue;
            }
            if self
                .selected
                .reads_nothing_from(names.source().id(), &spelled.name)
            {
                continue;
            }
            declarations.push(declaration);
        }
        let exit = self.regions[0].exit;
        self.push_uses(0, exit, &declarations);
        self.exported.extend(declarations);
    }

    fn solve(&mut self) -> Option<Liveness> {
        let regions = self.regions.len();
        let declarations = self.names.declarations().len();
        let words = declarations.div_ceil(64).max(1);

        let mut node_reach: Vec<Vec<bool>> = Vec::with_capacity(regions);
        let mut scanned = 0usize;
        for region in &self.regions {
            let mut reach = vec![false; region.nodes.len()];
            if !region.nodes.is_empty() {
                reach[region.entry] = true;
                let mut stack = vec![region.entry];
                while let Some(id) = stack.pop() {
                    self.poll_every(&mut scanned)?;
                    for &next in &region.nodes[id].successors {
                        if !reach[next] {
                            reach[next] = true;
                            stack.push(next);
                        }
                    }
                }
            }
            node_reach.push(reach);
        }

        let mut region_reach = vec![false; regions];
        region_reach[0] = true;
        for region in 1..regions {
            let body = &self.regions[region];
            let parent = body.parent;
            region_reach[region] = region_reach[parent]
                && body
                    .creation_node
                    .is_some_and(|node| node_reach[parent].get(node).copied().unwrap_or(false));
        }

        let mut used = vec![false; declarations];
        let mut scanned = 0usize;
        for region in 0..regions {
            if !region_reach[region] {
                continue;
            }
            for (id, node) in self.regions[region].nodes.iter().enumerate() {
                if !node_reach[region][id] {
                    continue;
                }
                self.poll_every(&mut scanned)?;
                for action in &node.actions {
                    if let Action::Use(declaration) = action {
                        used[*declaration] = true;
                    }
                }
            }
        }
        for &declaration in &self.exported {
            used[declaration] = true;
        }

        let mut dead_stores: Vec<DeadStore> = Vec::new();
        for region in 0..regions {
            if !region_reach[region] {
                continue;
            }
            let nodes = &self.regions[region].nodes;
            let reach = &node_reach[region];
            let count = nodes.len();
            let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); count];
            for (id, node) in nodes.iter().enumerate() {
                if !reach[id] {
                    continue;
                }
                for &next in &node.successors {
                    if reach[next] {
                        predecessors[next].push(id);
                    }
                }
            }
            let mut live = vec![vec![0u64; words]; count];
            let mut scratch = vec![0u64; words];
            let mut queued = vec![false; count];
            let mut worklist: Vec<usize> = (0..count).filter(|&id| reach[id]).collect();
            for &id in &worklist {
                queued[id] = true;
            }
            let mut stepped = 0usize;
            while let Some(id) = worklist.pop() {
                queued[id] = false;
                self.poll_every(&mut stepped)?;
                live_after(&nodes[id].successors, &live, &mut scratch);
                for action in nodes[id].actions.iter().rev() {
                    match action {
                        Action::Use(declaration) => set_bit(&mut scratch, *declaration),
                        Action::Store { declaration, .. } => clear_bit(&mut scratch, *declaration),
                    }
                }
                if scratch != live[id] {
                    live[id].copy_from_slice(&scratch);
                    for &previous in &predecessors[id] {
                        if !queued[previous] {
                            queued[previous] = true;
                            worklist.push(previous);
                        }
                    }
                }
            }
            let mut scanned = 0usize;
            for (id, node) in nodes.iter().enumerate() {
                if !reach[id] {
                    continue;
                }
                self.poll_every(&mut scanned)?;
                live_after(&node.successors, &live, &mut scratch);
                for action in node.actions.iter().rev() {
                    match action {
                        Action::Use(declaration) => set_bit(&mut scratch, *declaration),
                        Action::Store {
                            declaration,
                            range,
                            report,
                        } => {
                            let answers = matches!(
                                &self.names.declarations()[*declaration].kind,
                                DeclarationKind::Let | DeclarationKind::Loop
                            );
                            if *report
                                && answers
                                && !self.suppressed.contains(declaration)
                                && !bit_is_set(&scratch, *declaration)
                                && used[*declaration]
                            {
                                dead_stores.push(DeadStore {
                                    declaration: *declaration,
                                    range: range.clone(),
                                });
                            }
                            clear_bit(&mut scratch, *declaration);
                        }
                    }
                }
            }
        }
        dead_stores.sort_by_key(|store| store.range.start);

        let mut unused_bindings = Vec::new();
        for (declaration, spelled) in self.names.declarations().iter().enumerate() {
            if !matches!(&spelled.kind, DeclarationKind::Let | DeclarationKind::Loop) {
                continue;
            }
            if self.suppressed.contains(&declaration) {
                continue;
            }
            let Some((region, node)) = self.init_nodes[declaration] else {
                continue;
            };
            if !region_reach[region] || !node_reach[region][node] {
                continue;
            }
            if !used[declaration] {
                unused_bindings.push(declaration);
            }
        }
        self.poll()?;
        Some(Liveness {
            unused_bindings,
            dead_stores,
        })
    }
}

/// The body node of a code or content block.
fn block_body<'a>(node: &LinkedNode<'a>) -> Option<LinkedNode<'a>> {
    let span = if let Some(block) = node.cast::<ast::CodeBlock>() {
        block.body().span()
    } else {
        node.cast::<ast::ContentBlock>()?.body().span()
    };
    node.children().find(|child| child.span() == span)
}

/// Whether a binary operator stores into its left-hand side; `true` when it also reads it.
fn assignment_reads(op: ast::BinOp) -> Option<bool> {
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
fn written_ident<'a>(target: ast::Expr<'a>) -> Option<ast::Ident<'a>> {
    match target {
        ast::Expr::Ident(ident) => Some(ident),
        ast::Expr::Parenthesized(group) => written_ident(group.expr()),
        _ => None,
    }
}

/// The live-after set of one node: every successor's live set, unioned.
fn live_after(successors: &[usize], live: &[Vec<u64>], out: &mut [u64]) {
    out.fill(0);
    for &next in successors {
        for (word, source) in out.iter_mut().zip(&live[next]) {
            *word |= *source;
        }
    }
}

/// Sets one declaration's bit in a region's live set.
fn set_bit(set: &mut [u64], declaration: usize) {
    set[declaration / 64] |= 1 << (declaration % 64);
}

/// Clears one declaration's bit in a region's live set.
fn clear_bit(set: &mut [u64], declaration: usize) {
    set[declaration / 64] &= !(1 << (declaration % 64));
}

/// Whether one declaration's bit is set in a region's live set.
fn bit_is_set(set: &[u64], declaration: usize) -> bool {
    set[declaration / 64] & (1 << (declaration % 64)) != 0
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::HashMap;

    use typst_syntax::Source;

    use super::*;
    use crate::names::SelectedNames;

    fn analyzed_with(text: &str, selected: &SelectedInterfaces) -> (SourceNames, Liveness) {
        let names = SourceNames::new(Source::detached(text.to_owned()));
        let liveness = names
            .liveness(selected, || false)
            .expect("an uncancelled analysis");
        (names, liveness)
    }

    fn liveness(text: &str) -> Liveness {
        analyzed_with(text, &SelectedInterfaces::default()).1
    }

    fn unused_names(names: &SourceNames, liveness: &Liveness) -> Vec<String> {
        liveness
            .unused_bindings
            .iter()
            .map(|&declaration| names.declarations()[declaration].name.clone())
            .collect()
    }

    fn dead_offsets(liveness: &Liveness) -> Vec<usize> {
        liveness
            .dead_stores
            .iter()
            .map(|store| store.range.start)
            .collect()
    }

    /// A binding nothing reads is reported unused, and a binding read anywhere is not — the
    /// end of the file included, which is where an exported name is read.
    #[test]
    fn unused_bindings_name_the_dead_lets() {
        for (label, text, exports_are_read, expected) in [
            (
                "shadowed",
                "#let x = 1\n#let x = 2\n#x\n",
                false,
                &["x"][..],
            ),
            (
                "after return",
                "#let f() = {\n  let y = 1\n  return\n  y\n}\n#f()\n",
                false,
                &["y"][..],
            ),
            (
                "self recursive",
                "#let f() = {\n  f()\n}\n",
                false,
                &["f"][..],
            ),
            ("write only", "#let x = 0\n#(x = 1)\n", false, &["x"][..]),
            (
                "loop binding",
                "#for item in (1, 2) {\n}\n",
                false,
                &["item"][..],
            ),
            (
                "destruct field target",
                "#let d = (k: 1)\n#let other = 0\n#((d.k, other) = (2, 3))\n",
                false,
                &["other"][..],
            ),
            (
                "shadowed export",
                "#let value = 1\n#let value = 2\n",
                true,
                &["value"][..],
            ),
        ] {
            let selected = SelectedInterfaces {
                unresolved: exports_are_read,
                ..SelectedInterfaces::default()
            };
            let (names, liveness) = analyzed_with(text, &selected);
            assert_eq!(unused_names(&names, &liveness), expected, "{label}");
        }
    }

    /// A store no read reaches is dead: overwritten, unreachable, or written after the reader
    /// that would have consumed it. The offset is the store's, so the finding points at the
    /// write the author should delete.
    #[test]
    fn dead_stores_name_unreached_writes() {
        for (label, text, dead) in [
            (
                "overwritten",
                "#let x = 0\n#(x = 1)\n#(x = 2)\n#x\n",
                &["x = 0", "x = 1"][..],
            ),
            (
                "branch overwrite",
                "#let x = 0\n#if true {\n  x = 1\n} else {\n  x = 2\n}\n#x\n",
                &["x = 0"][..],
            ),
            (
                "iteration local",
                "#let x = 0\n#for i in (1, 2) {\n  x = i\n  x = i + 1\n}\n#x\n",
                &["x = i\n"][..],
            ),
            (
                "closure capture at creation",
                "#let x = 0\n#let f = () => x\n#(x = 1)\n",
                &["x = 1"][..],
            ),
            (
                "context capture at creation",
                "#let x = 0\n#let c = context x\n#(x = 1)\n",
                &["x = 1"][..],
            ),
            (
                "parameter default at creation",
                "#let x = 0\n#let f = (value: x) => value\n#(x = 1)\n",
                &["x = 1"][..],
            ),
            (
                "short circuit read",
                "#let x = 0\n#let y = false and x > 0\n#(x = 1)\n",
                &["x = 1"][..],
            ),
            (
                "default writes precede captures",
                "#let value = 0\n#let f = (p: { value = 1; none }) => value\n#f()\n",
                &["value = 0"][..],
            ),
            (
                "assignment reads after its value",
                "#let value = 0\n#(value += { value = 3; 2 })\n#value\n",
                &["value = 0"][..],
            ),
            (
                "destructuring reads follow each target",
                "#let index = 0\n#let values = (0, 0)\n#((index, values.at(index)) = (1, 2))\n#values\n",
                &["index = 0"][..],
            ),
            (
                "captured compound write",
                "#let x = 0\n#let f = () => {\n  x += 1\n}\n#f()\n#(x = 2)\n",
                &["x = 2"][..],
            ),
        ] {
            let expected: Vec<usize> = dead
                .iter()
                .map(|needle| text.find(needle).unwrap())
                .collect();
            assert_eq!(dead_offsets(&liveness(text)), expected, "{label}");
        }
    }

    #[test]
    fn read_path_prevents_dead_store() {
        for text in [
            "#let x = 0\n#if true { x } else { x = 1 }\n#x\n",
            "#let x = 0\n#for i in (1, 2) {\n  x\n  x = i\n}\n#x\n",
            "#let x = 0\n#for i in (1, 2) {\n  break\n  x = 1\n}\n#x\n",
            "#let x = 0\n#let f = () => {\n  x = 1\n}\n#x\n",
        ] {
            assert!(liveness(text).dead_stores.is_empty(), "{text}");
        }
    }

    #[test]
    fn unreportable_binding_is_never_reported() {
        for text in [
            "#let f() = {\n  f()\n}\n#f()\n",
            "#import \"lib.typ\": item\n",
            "#let f(parameter) = 1\n#f(1)\n",
        ] {
            let liveness = liveness(text);
            assert!(liveness.unused_bindings.is_empty(), "{text}");
            assert!(liveness.dead_stores.is_empty(), "{text}");
        }
    }

    #[test]
    fn selection_index_decides_export_read() {
        let text = "#let x = 0\n#(x = 1)\n";
        let names = SourceNames::new(Source::detached(text.to_owned()));
        let unselected = names
            .liveness(&SelectedInterfaces::default(), || false)
            .expect("an uncancelled analysis");
        assert_eq!(unselected.unused_bindings, [0]);
        assert!(unselected.dead_stores.is_empty());

        let mut selected_names = SelectedNames::default();
        selected_names.names.insert("x".to_owned());
        let selected = SelectedInterfaces {
            unresolved: false,
            selected: HashMap::from([(names.source().id(), selected_names)]),
        };
        let licensed = names
            .liveness(&selected, || false)
            .expect("an uncancelled analysis");
        assert!(licensed.unused_bindings.is_empty());
        assert_eq!(dead_offsets(&licensed), [text.find("x = 0").unwrap()]);
    }

    #[test]
    fn cancellation_returns_none() {
        let names = SourceNames::new(Source::detached("#let x = 1\n#x\n".repeat(512)));
        let calls = Cell::new(0usize);
        let midway = || {
            calls.set(calls.get() + 1);
            calls.get() >= 3
        };
        assert!(
            names
                .liveness(&SelectedInterfaces::default(), midway)
                .is_none()
        );
    }

    /// Input the analysis cannot read answers nothing rather than guessing: a parse error, a
    /// loop header that breaks before yielding, and the other ways a loop yields no values.
    #[test]
    fn unanalysable_input_answers_no_findings() {
        for text in [
            "#let x = 1\n#let broken = (]\n",
            "#let before = 0\n#for i in { (); break } {\n}\n#before\n",
            "#let before = 0\n#while { break } {\n}\n#before\n",
        ] {
            let liveness = liveness(text);
            assert!(liveness.unused_bindings.is_empty(), "{text}");
            assert!(liveness.dead_stores.is_empty(), "{text}");
        }
    }
}
