//! Prefix collision index for owned logical output paths.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputPathConflict {
    SamePath,
    FileDirectory,
}

impl std::fmt::Display for OutputPathConflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::SamePath => {
                "the paths differ only by letter case or Unicode form, so they collide on some filesystems"
            }
            Self::FileDirectory => "one path is a file and the other is a directory",
        })
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct OutputPathIndex {
    root: OutputPathIndexNode,
}

impl OutputPathIndex {
    pub(super) fn conflict(&self, key: &[String]) -> Option<(usize, OutputPathConflict)> {
        let mut node = &self.root;
        for component in key {
            if let Some(index) = node.output {
                return Some((index, OutputPathConflict::FileDirectory));
            }
            node = node.children.get(component)?;
        }

        if let Some(index) = node.output {
            return Some((index, OutputPathConflict::SamePath));
        }
        node.first_output()
            .map(|index| (index, OutputPathConflict::FileDirectory))
    }

    /// Find the earliest inserted incompatible overlapping path. Ancestors and
    /// strict descendants have separate policies; equality always conflicts.
    pub(super) fn ownership_conflict(
        &self,
        key: &[String],
        allow_ancestor: impl Fn(usize) -> bool,
        allow_descendant: impl Fn(usize) -> bool,
    ) -> Option<usize> {
        let mut node = &self.root;
        for component in key {
            if let Some(index) = node.output {
                return (!allow_ancestor(index)).then_some(index);
            }
            node = node.children.get(component)?;
        }
        if let Some(index) = node.output {
            return Some(index);
        }
        let first = node.first?;
        if !allow_descendant(first) {
            Some(first)
        } else {
            node.other_owner.filter(|&index| !allow_descendant(index))
        }
    }

    pub(super) fn insert(
        &mut self,
        key: Vec<String>,
        index: usize,
        same_owner: impl Fn(usize, usize) -> bool,
    ) {
        let mut node = &mut self.root;
        node.record_owner(index, &same_owner);
        for component in key {
            node = node.children.entry(component).or_default();
            node.record_owner(index, &same_owner);
        }
        let previous = node.output.replace(index);
        debug_assert!(previous.is_none());
    }
}

#[derive(Debug, Clone, Default)]
struct OutputPathIndexNode {
    output: Option<usize>,
    children: BTreeMap<String, OutputPathIndexNode>,
    /// Earliest insertion in the subtree and earliest with a different owner; these
    /// two witnesses suffice for every single-owner descendant policy.
    first: Option<usize>,
    other_owner: Option<usize>,
}

impl OutputPathIndexNode {
    fn record_owner(&mut self, index: usize, same_owner: &impl Fn(usize, usize) -> bool) {
        if let Some(first) = self.first {
            if self.other_owner.is_none() && !same_owner(first, index) {
                self.other_owner = Some(index);
            }
        } else {
            self.first = Some(index);
        }
    }

    fn first_output(&self) -> Option<usize> {
        let mut pending = vec![self];
        while let Some(node) = pending.pop() {
            if let Some(index) = node.output {
                return Some(index);
            }
            pending.extend(node.children.values().rev());
        }
        None
    }
}
