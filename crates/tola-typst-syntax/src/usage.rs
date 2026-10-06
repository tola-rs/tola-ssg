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

/// One function boundary's byte extent; region zero is the file itself.
struct Region {
    parent: usize,
    body: Range<usize>,
    /// Where the closure or `context` is created in the parent region.
    creation: usize,
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

/// One region's control flow.
#[derive(Default)]
struct Flow {
    nodes: Vec<Node>,
    entry: usize,
    exit: usize,
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
    flows: Vec<Flow>,
    built: Vec<bool>,
    creation_nodes: Vec<Option<usize>>,
    loops: Vec<Vec<(usize, usize)>>,
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
                parent: 0,
                body: 0..names.source().text().len(),
                creation: 0,
            }],
            region_order: Vec::new(),
            creation_regions: BTreeMap::new(),
            writes: BTreeMap::new(),
            spans: HashMap::new(),
            reads: BTreeMap::new(),
            captures: BTreeMap::new(),
            flows: vec![Flow::default()],
            built: vec![false],
            creation_nodes: vec![None],
            loops: vec![Vec::new()],
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
        self.lower_region(0, Some(&root));
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
        match node.kind() {
            SyntaxKind::Closure => {
                if let Some(closure) = node.cast::<ast::Closure>() {
                    let body = node.find(closure.body().span());
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
            }
            SyntaxKind::Contextual => {
                if let Some(contextual) = node.cast::<ast::Contextual>() {
                    let body = node.find(contextual.body().span());
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
            }
            _ => {}
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
        let range = body.map_or(creation..creation, |body| body.range());
        self.regions.push(Region {
            parent,
            body: range,
            creation,
        });
        self.creation_regions.insert(creation, id);
        self.flows.push(Flow::default());
        self.built.push(false);
        self.creation_nodes.push(None);
        self.loops.push(Vec::new());
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
            seen += 1;
            if seen.is_multiple_of(CANCELLATION_POLL_INTERVAL) {
                self.poll()?;
            }
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
        let flow = &mut self.flows[region];
        flow.nodes.push(Node::default());
        flow.nodes.len() - 1
    }

    fn edge(&mut self, region: usize, from: usize, to: usize) {
        self.flows[region].nodes[from].successors.push(to);
    }

    /// Flushes the read at the identifier node that evaluates it.
    fn flush_read(&mut self, region: usize, node: &LinkedNode, at: usize) {
        if !matches!(node.kind(), SyntaxKind::Ident | SyntaxKind::MathIdent) {
            return;
        }
        let range = node.range();
        if let Some(declarations) = self.reads.remove(&(range.start, range.end)) {
            let actions = &mut self.flows[region].nodes[at].actions;
            for declaration in declarations {
                actions.push(Action::Use(declaration));
            }
        }
    }

    /// Flushes the reads a closure or `context` captures: once, at its creation.
    fn flush_captures(&mut self, region: usize, creation: usize, at: usize) {
        if let Some(declarations) = self.captures.remove(&creation) {
            let actions = &mut self.flows[region].nodes[at].actions;
            for declaration in declarations {
                actions.push(Action::Use(declaration));
            }
        }
    }

    /// Builds one region's flow from its body, the whole source for region zero.
    fn lower_region(&mut self, region: usize, body: Option<&LinkedNode>) {
        let entry = self.new_node(region);
        let exit = self.new_node(region);
        self.flows[region].entry = entry;
        self.flows[region].exit = exit;
        self.built[region] = true;
        let saved = self.in_header;
        self.in_header = false;
        let tail = body.and_then(|body| self.walk(body, region, entry));
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
        self.creation_nodes[region] = Some(creation_node);
        self.lower_region(region, Some(body));
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
                    for name in for_loop.pattern().bindings() {
                        let Some(range) = self.range_of(name.span()) else {
                            continue;
                        };
                        let Some(declaration) = self.names.declared_at(range.start) else {
                            continue;
                        };
                        let store_range = self.names.declarations()[declaration].range.clone();
                        self.flows[region].nodes[body_entry]
                            .actions
                            .push(Action::Store {
                                declaration,
                                range: store_range,
                                report: false,
                            });
                        self.init_nodes[declaration] = Some((region, body_entry));
                    }
                    self.loops[region].push((exit, head));
                    let saved = self.in_header;
                    self.in_header = false;
                    let body = node.find(for_loop.body().span());
                    let lowered = body.and_then(|body| self.walk(&body, region, body_entry));
                    self.in_header = saved;
                    self.loops[region].pop();
                    if let Some(tail) = lowered {
                        self.edge(region, tail, head);
                    }
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
                    self.loops[region].push((exit, head));
                    let saved = self.in_header;
                    self.in_header = false;
                    let body = node.find(while_loop.body().span());
                    let lowered = body.and_then(|body| self.walk(&body, region, body_entry));
                    self.in_header = saved;
                    self.loops[region].pop();
                    if let Some(tail) = lowered {
                        self.edge(region, tail, head);
                    }
                    return Some(exit);
                }
            }
            SyntaxKind::LoopBreak | SyntaxKind::LoopContinue => {
                if self.in_header {
                    self.uncertain = true;
                }
                let target = self.loops[region]
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
                    let exit = self.flows[region].exit;
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
                    for name in binding.kind().bindings() {
                        let Some(range) = self.range_of(name.span()) else {
                            continue;
                        };
                        let Some(declaration) = self.names.declared_at(range.start) else {
                            continue;
                        };
                        let store_range = self.names.declarations()[declaration].range.clone();
                        self.flows[region].nodes[cur].actions.push(Action::Store {
                            declaration,
                            range: store_range,
                            report,
                        });
                        self.init_nodes[declaration] = Some((region, cur));
                    }
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
                            let order = if assignment_reads(op).is_some() {
                                [binary.rhs(), binary.lhs()]
                            } else {
                                [binary.lhs(), binary.rhs()]
                            };
                            for side in order {
                                if let Some(side) = node.find(side.span()) {
                                    cur = self.walk(&side, region, cur)?;
                                }
                            }
                            if assignment_reads(op).is_some()
                                && let Some(ident) = written_ident(binary.lhs())
                            {
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
        self.flows[region].nodes[node].actions.push(Action::Store {
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
            ast::Pattern::Normal(expr) => {
                let cur = self.walk(pattern_node, region, cur)?;
                if let ast::Expr::Ident(ident) = expr {
                    self.store_written(region, cur, ident);
                }
                Some(cur)
            }
            ast::Pattern::Placeholder(_) => Some(cur),
        }
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
                let cur = self.walk(&target, region, cur)?;
                if let ast::Expr::Ident(ident) = expr {
                    self.store_written(region, cur, ident);
                }
                Some(cur)
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
        let exit = self.flows[0].exit;
        for declaration in declarations {
            self.flows[0].nodes[exit]
                .actions
                .push(Action::Use(declaration));
            self.exported.insert(declaration);
        }
    }

    fn solve(&mut self) -> Option<Liveness> {
        let regions = self.regions.len();
        let declarations = self.names.declarations().len();
        let words = declarations.div_ceil(64).max(1);

        let mut node_reach: Vec<Vec<bool>> = Vec::with_capacity(regions);
        let mut scanned = 0usize;
        for region in 0..regions {
            if !self.built[region] {
                node_reach.push(Vec::new());
                continue;
            }
            let flow = &self.flows[region];
            let mut reach = vec![false; flow.nodes.len()];
            if !flow.nodes.is_empty() {
                reach[flow.entry] = true;
                let mut stack = vec![flow.entry];
                while let Some(id) = stack.pop() {
                    scanned += 1;
                    if scanned.is_multiple_of(CANCELLATION_POLL_INTERVAL) {
                        self.poll()?;
                    }
                    for &next in &flow.nodes[id].successors {
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
        region_reach[0] = self.built[0];
        for region in 1..regions {
            if !self.built[region] {
                continue;
            }
            let parent = self.regions[region].parent;
            region_reach[region] = region_reach[parent]
                && self.creation_nodes[region]
                    .is_some_and(|node| node_reach[parent].get(node).copied().unwrap_or(false));
        }

        let mut used = vec![false; declarations];
        let mut scanned = 0usize;
        for region in 0..regions {
            if !region_reach[region] {
                continue;
            }
            let flow = &self.flows[region];
            for (id, node) in flow.nodes.iter().enumerate() {
                if !node_reach[region][id] {
                    continue;
                }
                scanned += 1;
                if scanned.is_multiple_of(CANCELLATION_POLL_INTERVAL) {
                    self.poll()?;
                }
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
            let flow = &self.flows[region];
            let reach = &node_reach[region];
            let count = flow.nodes.len();
            let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); count];
            for (id, node) in flow.nodes.iter().enumerate() {
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
                stepped += 1;
                if stepped.is_multiple_of(CANCELLATION_POLL_INTERVAL) {
                    self.poll()?;
                }
                scratch.fill(0);
                for &next in &flow.nodes[id].successors {
                    let successor = &live[next];
                    for (word, source) in scratch.iter_mut().zip(successor) {
                        *word |= *source;
                    }
                }
                for action in flow.nodes[id].actions.iter().rev() {
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
            for (id, node) in flow.nodes.iter().enumerate() {
                if !reach[id] {
                    continue;
                }
                scanned += 1;
                if scanned.is_multiple_of(CANCELLATION_POLL_INTERVAL) {
                    self.poll()?;
                }
                scratch.fill(0);
                for &next in &node.successors {
                    let successor = &live[next];
                    for (word, source) in scratch.iter_mut().zip(successor) {
                        *word |= *source;
                    }
                }
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

    #[test]
    fn shadowed_binding_is_unused() {
        let text = "#let x = 1\n#let x = 2\n#x\n";
        let (names, liveness) = analyzed_with(text, &SelectedInterfaces::default());
        assert_eq!(liveness.unused_bindings, [0]);
        assert!(unused_names(&names, &liveness) == ["x"]);
        assert!(liveness.dead_stores.is_empty());
    }

    #[test]
    fn overwritten_stores_are_dead() {
        let text = "#let x = 0\n#(x = 1)\n#(x = 2)\n#x\n";
        let liveness = liveness(text);
        assert_eq!(
            dead_offsets(&liveness),
            [text.find("x = 0").unwrap(), text.find("x = 1").unwrap()]
        );
        assert!(liveness.unused_bindings.is_empty());
    }

    #[test]
    fn branch_overwrite_makes_store_dead() {
        let text = "#let x = 0\n#if true {\n  x = 1\n} else {\n  x = 2\n}\n#x\n";
        let liveness = liveness(text);
        assert_eq!(dead_offsets(&liveness), [text.find("x = 0").unwrap()]);
    }

    #[test]
    fn branch_read_keeps_store_live() {
        let text = "#let x = 0\n#if true { x } else { x = 1 }\n#x\n";
        let liveness = liveness(text);
        assert!(liveness.dead_stores.is_empty());
    }

    #[test]
    fn loop_carried_value_keeps_store_live() {
        let text = "#let x = 0\n#for i in (1, 2) {\n  x\n  x = i\n}\n";
        let liveness = liveness(text);
        assert!(liveness.dead_stores.is_empty());
    }

    #[test]
    fn iteration_local_overwrite_is_dead() {
        let text = "#let x = 0\n#for i in (1, 2) {\n  x = i\n  x = i + 1\n}\n#x\n";
        let liveness = liveness(text);
        assert_eq!(dead_offsets(&liveness), [text.find("x = i\n").unwrap()]);
    }

    #[test]
    fn statement_after_break_is_unreachable() {
        let text = "#let x = 0\n#for i in (1, 2) {\n  break\n  x = 1\n}\n#x\n";
        let liveness = liveness(text);
        assert!(liveness.dead_stores.is_empty());
    }

    #[test]
    fn read_after_return_is_unreachable() {
        let text = "#let f() = {\n  let y = 1\n  return\n  y\n}\n#f()\n";
        let (names, liveness) = analyzed_with(text, &SelectedInterfaces::default());
        assert_eq!(unused_names(&names, &liveness), ["y"]);
        assert!(liveness.dead_stores.is_empty());
    }

    #[test]
    fn closure_capture_reads_at_creation() {
        let text = "#let x = 0\n#let f = () => x\n#(x = 1)\n";
        let liveness = liveness(text);
        assert_eq!(dead_offsets(&liveness), [text.find("x = 1").unwrap()]);
    }

    #[test]
    fn context_capture_reads_at_creation() {
        let text = "#let x = 0\n#let c = context x\n#(x = 1)\n";
        let liveness = liveness(text);
        assert_eq!(dead_offsets(&liveness), [text.find("x = 1").unwrap()]);
    }

    #[test]
    fn parameter_default_reads_at_creation() {
        let text = "#let x = 0\n#let f = (value: x) => value\n#(x = 1)\n";
        let liveness = liveness(text);
        assert_eq!(dead_offsets(&liveness), [text.find("x = 1").unwrap()]);
    }

    #[test]
    fn self_recursive_function_is_unused() {
        let text = "#let f() = {\n  f()\n}\n";
        let (names, liveness) = analyzed_with(text, &SelectedInterfaces::default());
        assert_eq!(unused_names(&names, &liveness), ["f"]);
        assert!(liveness.dead_stores.is_empty());
    }

    #[test]
    fn externally_called_function_is_used() {
        let text = "#let f() = {\n  f()\n}\n#f()\n";
        let liveness = liveness(text);
        assert!(liveness.unused_bindings.is_empty());
        assert!(liveness.dead_stores.is_empty());
    }

    #[test]
    fn math_identifier_reads_its_binding() {
        let text = "#let alpha = 1\n$alpha$\n";
        let liveness = liveness(text);
        assert!(liveness.unused_bindings.is_empty());
    }

    #[test]
    fn math_text_stays_unread() {
        let text = "#let b = 1\n$b$\n";
        let (names, liveness) = analyzed_with(text, &SelectedInterfaces::default());
        assert_eq!(unused_names(&names, &liveness), ["b"]);
    }

    #[test]
    fn import_binding_is_not_reported() {
        let text = "#import \"lib.typ\": item\n";
        let liveness = liveness(text);
        assert!(liveness.unused_bindings.is_empty());
        assert!(liveness.dead_stores.is_empty());
    }

    #[test]
    fn parameter_is_not_reported() {
        let text = "#let f(parameter) = {\n  parameter\n}\n#f(1)\n";
        let liveness = liveness(text);
        assert!(liveness.unused_bindings.is_empty());
        assert!(liveness.dead_stores.is_empty());
    }

    #[test]
    fn compound_assignment_reads_its_target() {
        let text = "#let x = 0\n#(x += 1)\n#x\n";
        let liveness = liveness(text);
        assert!(liveness.unused_bindings.is_empty());
        assert!(liveness.dead_stores.is_empty());
    }

    #[test]
    fn write_only_binding_is_unused() {
        let text = "#let x = 0\n#(x = 1)\n";
        let (names, liveness) = analyzed_with(text, &SelectedInterfaces::default());
        assert_eq!(unused_names(&names, &liveness), ["x"]);
        assert!(liveness.dead_stores.is_empty());
    }

    #[test]
    fn export_read_keeps_the_final_value() {
        let text = "#let x = 0\n#(x = 1)\n";
        let selected = SelectedInterfaces {
            unresolved: true,
            ..SelectedInterfaces::default()
        };
        let liveness = analyzed_with(text, &selected).1;
        assert_eq!(dead_offsets(&liveness), [text.find("x = 0").unwrap()]);
    }

    #[test]
    fn shadowed_export_reads_only_the_final_binding() {
        let text = "#let value = 1\n#let value = 2\n";
        let selected = SelectedInterfaces {
            unresolved: true,
            ..SelectedInterfaces::default()
        };
        let (names, liveness) = analyzed_with(text, &selected);
        assert_eq!(liveness.unused_bindings, [0]);
        assert_eq!(unused_names(&names, &liveness), ["value"]);
        assert!(liveness.dead_stores.is_empty());
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
    fn captured_write_is_no_store() {
        let text = "#let x = 0\n#let f = () => {\n  x = 1\n}\n#x\n";
        let liveness = liveness(text);
        assert!(liveness.dead_stores.is_empty());
    }

    #[test]
    fn captured_compound_read_keeps_binding_used() {
        let text = "#let x = 0\n#let f = () => {\n  x += 1\n}\n#f()\n#(x = 2)\n";
        let liveness = liveness(text);
        assert!(liveness.unused_bindings.is_empty());
        assert_eq!(dead_offsets(&liveness), [text.find("x = 2").unwrap()]);
    }

    #[test]
    fn short_circuit_read_keeps_binding_live() {
        let text = "#let x = 0\n#let y = false and x > 0\n#(x = 1)\n";
        let liveness = liveness(text);
        assert_eq!(dead_offsets(&liveness), [text.find("x = 1").unwrap()]);
    }

    #[test]
    fn loop_binding_is_unused() {
        let text = "#for item in (1, 2) {\n}\n";
        let (names, liveness) = analyzed_with(text, &SelectedInterfaces::default());
        assert_eq!(unused_names(&names, &liveness), ["item"]);
    }

    #[test]
    fn cancellation_returns_none() {
        let names = SourceNames::new(Source::detached("#let x = 1\n#x\n".to_owned()));
        assert!(
            names
                .liveness(&SelectedInterfaces::default(), || true)
                .is_none()
        );

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
        assert!(calls.get() >= 3, "cancellation observed mid-analysis");
    }

    #[test]
    fn parse_error_answers_no_findings() {
        let text = "#let x = 1\n#let broken = (]\n";
        let liveness = liveness(text);
        assert!(liveness.unused_bindings.is_empty());
        assert!(liveness.dead_stores.is_empty());
    }

    #[test]
    fn default_writes_precede_captures() {
        let text = "#let value = 0\n#let f = (p: { value = 1; none }) => value\n#f()\n";
        let liveness = liveness(text);
        assert_eq!(dead_offsets(&liveness), [text.find("value = 0").unwrap()]);
    }

    #[test]
    fn assignment_reads_after_its_value() {
        let text = "#let value = 0\n#(value += { value = 3; 2 })\n#value\n";
        let liveness = liveness(text);
        assert_eq!(dead_offsets(&liveness), [text.find("value = 0").unwrap()]);
    }

    #[test]
    fn destruct_field_target_reads_its_base() {
        let text = "#let d = (k: 1)\n#let other = 0\n#((d.k, other) = (2, 3))\n";
        let (names, liveness) = analyzed_with(text, &SelectedInterfaces::default());
        assert_eq!(unused_names(&names, &liveness), ["other"]);
        assert!(liveness.dead_stores.is_empty());
    }

    #[test]
    fn destructuring_reads_follow_each_target() {
        let text = "#let index = 0\n#let values = (0, 0)\n#((index, values.at(index)) = (1, 2))\n#values\n";
        let liveness = liveness(text);
        assert_eq!(dead_offsets(&liveness), [text.find("index = 0").unwrap()]);
    }

    #[test]
    fn zero_iteration_loop_keeps_the_previous_value_live() {
        let text = "#let x = 0\n#for i in (1, 2) {\n  #(x = 1)\n}\n#x\n";
        let liveness = liveness(text);
        assert!(liveness.dead_stores.is_empty());
    }

    #[test]
    fn loop_header_break_answers_no_findings() {
        for text in [
            "#let before = 0\n#for i in { (); break } {\n}\n#before\n",
            "#let before = 0\n#for i in { (1,); break } {\n}\n#before\n",
            "#let before = 0\n#while { break } {\n}\n#before\n",
        ] {
            let liveness = liveness(text);
            assert!(liveness.unused_bindings.is_empty(), "{text}");
            assert!(liveness.dead_stores.is_empty(), "{text}");
        }
    }
}
