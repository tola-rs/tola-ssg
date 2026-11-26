//! Local liveness of one source's bindings.
//!
//! Derives control-flow store/read order while keeping declaration identity in [`SourceNames`].

mod cfg;
mod scan;
mod solve;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;

use typst_syntax::{LinkedNode, Span};

use crate::names::{SelectedInterfaces, SourceNames};

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
    /// Backward liveness over one region per body (file, closure, `context`). `selected`
    /// licenses scope-0 exports; an unresolved index withholds all scope-0 exports. Returns `None`
    /// only when `cancelled` fires, and empty [`Liveness`] when parse errors or unmodelled loop
    /// header breaks prevent sound analysis.
    pub fn liveness(
        &self,
        selected: &SelectedInterfaces,
        cancelled: impl Fn() -> bool,
    ) -> Option<Liveness> {
        LiveSource::new(self, selected, &cancelled).run()
    }
}

const CANCELLATION_POLL_INTERVAL: usize = 256;

#[derive(Default)]
struct Region {
    parent: usize,
    body: Range<usize>,
    creation: usize,
    creation_node: Option<usize>,
    nodes: Vec<Node>,
    entry: usize,
    exit: usize,
    loops: Vec<(usize, usize)>,
}

enum Action {
    Use(usize),
    Store {
        declaration: usize,
        range: Range<usize>,
        report: bool,
    },
}

#[derive(Default)]
struct Node {
    actions: Vec<Action>,
    successors: Vec<usize>,
}

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
    /// Region ids ordered by body start, innermost last on ties.
    region_order: Vec<usize>,
    creation_regions: BTreeMap<usize, usize>,
    writes: BTreeMap<usize, WriteSite>,
    spans: HashMap<Span, Range<usize>>,
    reads: BTreeMap<(usize, usize), Vec<usize>>,
    captures: BTreeMap<usize, Vec<usize>>,
    init_nodes: Vec<Option<(usize, usize)>>,
    exported: BTreeSet<usize>,
    suppressed: BTreeSet<usize>,
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

    fn order_regions(&mut self) {
        let mut order: Vec<usize> = (0..self.regions.len()).collect();
        order.sort_by_key(|&region| {
            let body = &self.regions[region].body;
            (body.start, std::cmp::Reverse(body.end))
        });
        self.region_order = order;
    }

    fn region_at(&self, offset: usize) -> usize {
        let idx = self
            .region_order
            .partition_point(|&region| self.regions[region].body.start <= offset);
        if idx == 0 {
            return 0;
        }
        let mut region = self.region_order[idx - 1];
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
}
