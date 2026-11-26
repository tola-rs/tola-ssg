//! Reachability and backward dataflow solver for [`LiveSource`].

use std::collections::BTreeSet;

use crate::names::DeclarationKind;

use super::{Action, DeadStore, LiveSource, Liveness};

impl<'a> LiveSource<'a> {
    /// Adds the module's read of each exported scope-0 binding at the end of the file.
    pub(super) fn append_export_uses(&mut self) {
        let names = self.names;
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut decls = Vec::new();
        for &decl in names.scopes()[0].declarations.iter().rev() {
            let spelled = &names.declarations()[decl];
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
            decls.push(decl);
        }
        let exit = self.regions[0].exit;
        self.push_uses(0, exit, &decls);
        self.exported.extend(decls);
    }

    pub(super) fn solve(&mut self) -> Option<Liveness> {
        let decl_count = self.names.declarations().len();
        let words = decl_count.div_ceil(64).max(1);
        let (node_reach, region_reach) = self.compute_reachability()?;
        let used = self.collect_reachable_uses(&node_reach, &region_reach, decl_count)?;

        let mut dead_stores = Vec::new();
        for region in 0..self.regions.len() {
            if region_reach[region] {
                self.solve_region_liveness(
                    region,
                    &node_reach[region],
                    words,
                    &used,
                    &mut dead_stores,
                )?;
            }
        }
        dead_stores.sort_by_key(|store| store.range.start);

        let unused_bindings = self.collect_unused_bindings(&node_reach, &region_reach, &used);
        self.poll()?;
        Some(Liveness {
            unused_bindings,
            dead_stores,
        })
    }

    fn compute_reachability(&self) -> Option<(Vec<Vec<bool>>, Vec<bool>)> {
        let mut node_reach = Vec::with_capacity(self.regions.len());
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

        let mut region_reach = vec![false; self.regions.len()];
        region_reach[0] = true;
        for region in 1..self.regions.len() {
            let body = &self.regions[region];
            let parent = body.parent;
            region_reach[region] = region_reach[parent]
                && body
                    .creation_node
                    .is_some_and(|node| node_reach[parent].get(node).copied().unwrap_or(false));
        }
        Some((node_reach, region_reach))
    }

    fn collect_reachable_uses(
        &self,
        node_reach: &[Vec<bool>],
        region_reach: &[bool],
        decl_count: usize,
    ) -> Option<Vec<bool>> {
        let mut used = vec![false; decl_count];
        let mut scanned = 0usize;
        for (region, nodes_reached) in node_reach.iter().enumerate() {
            if !region_reach[region] {
                continue;
            }
            for (id, node) in self.regions[region].nodes.iter().enumerate() {
                if !nodes_reached[id] {
                    continue;
                }
                self.poll_every(&mut scanned)?;
                for action in &node.actions {
                    if let Action::Use(decl) = action {
                        used[*decl] = true;
                    }
                }
            }
        }
        for &decl in &self.exported {
            used[decl] = true;
        }
        Some(used)
    }

    fn solve_region_liveness(
        &self,
        region: usize,
        reach: &[bool],
        words: usize,
        used: &[bool],
        dead_stores: &mut Vec<DeadStore>,
    ) -> Option<()> {
        let nodes = &self.regions[region].nodes;
        let count = nodes.len();
        let mut preds = vec![Vec::new(); count];
        for (id, node) in nodes.iter().enumerate() {
            if !reach[id] {
                continue;
            }
            for &next in &node.successors {
                if reach[next] {
                    preds[next].push(id);
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
                    Action::Use(decl) => set_bit(&mut scratch, *decl),
                    Action::Store { declaration, .. } => clear_bit(&mut scratch, *declaration),
                }
            }
            if scratch != live[id] {
                live[id].copy_from_slice(&scratch);
                for &prev in &preds[id] {
                    if !queued[prev] {
                        queued[prev] = true;
                        worklist.push(prev);
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
                    Action::Use(decl) => set_bit(&mut scratch, *decl),
                    Action::Store {
                        declaration: decl,
                        range,
                        report,
                    } => {
                        let local = matches!(
                            &self.names.declarations()[*decl].kind,
                            DeclarationKind::Let | DeclarationKind::Loop
                        );
                        if *report
                            && local
                            && !self.suppressed.contains(decl)
                            && !bit_is_set(&scratch, *decl)
                            && used[*decl]
                        {
                            dead_stores.push(DeadStore {
                                declaration: *decl,
                                range: range.clone(),
                            });
                        }
                        clear_bit(&mut scratch, *decl);
                    }
                }
            }
        }
        Some(())
    }

    fn collect_unused_bindings(
        &self,
        node_reach: &[Vec<bool>],
        region_reach: &[bool],
        used: &[bool],
    ) -> Vec<usize> {
        let mut unused = Vec::new();
        for (decl, spelled) in self.names.declarations().iter().enumerate() {
            if !matches!(&spelled.kind, DeclarationKind::Let | DeclarationKind::Loop) {
                continue;
            }
            if self.suppressed.contains(&decl) {
                continue;
            }
            let Some((region, node)) = self.init_nodes[decl] else {
                continue;
            };
            if region_reach[region] && node_reach[region][node] && !used[decl] {
                unused.push(decl);
            }
        }
        unused
    }
}

fn live_after(successors: &[usize], live: &[Vec<u64>], out: &mut [u64]) {
    out.fill(0);
    for &next in successors {
        for (word, src) in out.iter_mut().zip(&live[next]) {
            *word |= *src;
        }
    }
}

fn set_bit(set: &mut [u64], decl: usize) {
    set[decl / 64] |= 1 << (decl % 64);
}

fn clear_bit(set: &mut [u64], decl: usize) {
    set[decl / 64] &= !(1 << (decl % 64));
}

fn bit_is_set(set: &[u64], decl: usize) -> bool {
    set[decl / 64] & (1 << (decl % 64)) != 0
}
