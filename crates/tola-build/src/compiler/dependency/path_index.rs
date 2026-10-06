//! Reverse lookup from changed paths to published dependency readers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use tola_typst::ReadLocator;

use super::{DependencyReadEvidence, DependencyReaderEvidence, TypstDependencyReader};

/// Readers are indexed by directory ancestor and by lexical symlink alias.
#[derive(Debug, Clone, Default)]
pub(super) struct DependencyPathIndex {
    readers: BTreeMap<PathBuf, Vec<usize>>,
    package_candidates: BTreeSet<PathBuf>,
}

impl DependencyPathIndex {
    pub(super) fn build(
        root: &Path,
        reader_evidence: &BTreeMap<TypstDependencyReader, DependencyReaderEvidence>,
        package_checks: &[tola_typst::PackageCheck],
    ) -> Self {
        let mut index = Self::default();
        for (reader_index, published) in reader_evidence.values().enumerate() {
            for read in &published.reads {
                match read {
                    DependencyReadEvidence::Physical {
                        logical_path,
                        canonical_target,
                        ..
                    } => {
                        index.add_reader_identity(logical_path, reader_index);
                        index.add_reader_identity(canonical_target, reader_index);
                    }
                    DependencyReadEvidence::Virtual { evidence, .. } => {
                        if let ReadLocator::Root(relative) = evidence.locator() {
                            index.add_reader_path(&root.join(relative), reader_index);
                        }
                    }
                }
            }
        }
        for check in package_checks {
            index.add_package_path(check.candidate());
        }
        for readers in index.readers.values_mut() {
            readers.sort_unstable();
            readers.dedup();
        }
        index
    }

    fn add_reader_path(&mut self, path: &Path, reader_index: usize) {
        for alias in path_aliases(path) {
            self.add_reader_identity(&alias, reader_index);
        }
    }

    fn add_reader_identity(&mut self, path: &Path, reader_index: usize) {
        let path = absolute_lexical_path(path);
        for ancestor in path.ancestors() {
            self.readers
                .entry(ancestor.to_path_buf())
                .or_default()
                .push(reader_index);
        }
    }

    fn add_package_path(&mut self, path: &Path) {
        for alias in path_aliases(path) {
            self.package_candidates
                .extend(alias.ancestors().map(Path::to_path_buf));
        }
    }

    pub(super) fn readers_for(
        &self,
        changed_paths: &[PathBuf],
        reader_evidence: &BTreeMap<TypstDependencyReader, DependencyReaderEvidence>,
    ) -> Vec<TypstDependencyReader> {
        let mut reader_indices = BTreeSet::new();
        for alias in changed_paths.iter().flat_map(|path| path_aliases(path)) {
            if self.package_candidates.contains(&alias) {
                return reader_evidence.keys().cloned().collect();
            }
            if let Some(readers) = self.readers.get(&alias) {
                reader_indices.extend(readers.iter().copied());
            }
        }
        reader_evidence
            .keys()
            .enumerate()
            .filter(|(index, _)| reader_indices.contains(index))
            .map(|(_, reader)| reader.clone())
            .collect()
    }
}

fn path_aliases(path: &Path) -> impl Iterator<Item = PathBuf> {
    let lexical = crate::filesystem::lexical_path_identity(path);
    let normalized = crate::filesystem::normalize_path(path);
    let distinct = lexical != normalized;
    std::iter::once(lexical).chain(distinct.then_some(normalized))
}

fn absolute_lexical_path(path: &Path) -> PathBuf {
    let lexical = crate::filesystem::lexical_path_identity(path);
    if lexical.is_absolute() {
        lexical
    } else {
        std::path::absolute(lexical).unwrap_or_else(|_| path.to_path_buf())
    }
}
