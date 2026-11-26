//! Compiler reads and their invalidation from changed paths; every reuse revalidates.

mod path_index;
mod reads;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use tola_typst::{FileRead, ReadOrigin, ReadRecord, sort_package_checks};

use crate::cancellation::{BuildCancellation, BuildCancelled};
use path_index::DependencyPathIndex;
use reads::{
    DepRead, DepReader, ReuseProtocolError, package_check_is_fresh, physical_read_is_fresh,
};

/// Compilation unit whose reads determine dependency invalidation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TypstDependencyReader {
    ContentSource(PathBuf),
    SiteProgram,
}

impl TypstDependencyReader {
    fn content_source(&self) -> Option<&Path> {
        match self {
            Self::ContentSource(src) => Some(src.as_path()),
            Self::SiteProgram => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RebuildPriority {
    Unchanged,
    Affected,
    Direct,
}

#[derive(Debug)]
pub(crate) struct RebuildDecision {
    pub(crate) source_analysis: RebuildPriority,
    pub(crate) site_program: RebuildPriority,
    pub(crate) direct_readers: Vec<TypstDependencyReader>,
    pub(crate) affected_readers: Vec<TypstDependencyReader>,
}

impl RebuildDecision {
    pub(crate) fn reuses_source_analysis(&self) -> bool {
        self.source_analysis == RebuildPriority::Unchanged
    }

    pub(crate) fn reuses_site_program(&self) -> bool {
        self.site_program == RebuildPriority::Unchanged
    }

    pub(crate) fn for_paths(
        evidence: Option<&CompilationDependencies>,
        changed: &[PathBuf],
        inventory_may_change: bool,
    ) -> Self {
        let Some(evidence) = evidence else {
            return Self::classify(Vec::new(), Vec::new(), inventory_may_change);
        };
        let mut affected = evidence.readers_of_paths(changed);
        affected.extend(evidence.readers_requiring_each_build());
        Self::classify(
            evidence.direct_readers_for_paths(changed),
            affected,
            inventory_may_change,
        )
    }

    /// Classify changes verified against captured reads and unsaved source versions.
    pub(crate) fn for_observed_paths(deps: &CompilationDependencies, changed: &[PathBuf]) -> Self {
        Self::classify(
            deps.direct_readers_for_paths(changed),
            deps.readers_of_paths(changed),
            false,
        )
    }

    fn classify(
        direct_readers: Vec<TypstDependencyReader>,
        mut affected_readers: Vec<TypstDependencyReader>,
        inventory_may_change: bool,
    ) -> Self {
        let src_direct = inventory_may_change
            || direct_readers
                .iter()
                .any(|r| matches!(r, TypstDependencyReader::ContentSource(_)));
        let site_direct = direct_readers
            .binary_search(&TypstDependencyReader::SiteProgram)
            .is_ok();
        affected_readers.retain(|r| direct_readers.binary_search(r).is_err());

        let src_state_may_change = direct_readers
            .iter()
            .chain(&affected_readers)
            .any(|r| matches!(r, TypstDependencyReader::ContentSource(_)));
        if src_state_may_change && !site_direct {
            affected_readers.push(TypstDependencyReader::SiteProgram);
        }
        normalize_readers(&mut affected_readers);

        let source_analysis = if src_direct {
            RebuildPriority::Direct
        } else if affected_readers
            .iter()
            .any(|r| matches!(r, TypstDependencyReader::ContentSource(_)))
        {
            RebuildPriority::Affected
        } else {
            RebuildPriority::Unchanged
        };
        Self {
            source_analysis,
            site_program: if site_direct {
                RebuildPriority::Direct
            } else if source_analysis != RebuildPriority::Unchanged
                || affected_readers.contains(&TypstDependencyReader::SiteProgram)
            {
                RebuildPriority::Affected
            } else {
                RebuildPriority::Unchanged
            },
            direct_readers,
            affected_readers,
        }
    }

    pub(crate) fn invalidated_content_sources(&self) -> impl Iterator<Item = &Path> {
        self.direct_readers
            .iter()
            .chain(&self.affected_readers)
            .filter_map(TypstDependencyReader::content_source)
    }

    pub(crate) fn directly_changed_content_sources(&self) -> impl Iterator<Item = &Path> {
        self.direct_readers
            .iter()
            .filter_map(TypstDependencyReader::content_source)
    }

    pub(crate) fn directly_rebuilds_site_program(&self) -> bool {
        self.site_program == RebuildPriority::Direct
    }
}

/// Exact reads observed by source analysis and Bundle compilation.
#[derive(Debug, Clone)]
pub(crate) struct CompilationDependencies(Arc<DependencyReads>);

#[derive(Debug, Clone)]
pub(crate) struct DependencyReads {
    content_sources: BTreeSet<PathBuf>,
    bundle_entries: BTreeSet<PathBuf>,
    reader_evidence: BTreeMap<TypstDependencyReader, DepReader>,
    package_checks: Vec<tola_typst::PackageCheck>,
    path_index: DependencyPathIndex,
}

impl std::ops::Deref for CompilationDependencies {
    type Target = DependencyReads;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub(crate) struct ReusedDependencyReaders<'a> {
    dependencies: &'a CompilationDependencies,
    readers: &'a [TypstDependencyReader],
}

impl<'a> ReusedDependencyReaders<'a> {
    pub(crate) fn new(
        dependencies: &'a CompilationDependencies,
        readers: &'a [TypstDependencyReader],
    ) -> Self {
        Self {
            dependencies,
            readers,
        }
    }
}

impl CompilationDependencies {
    pub(crate) fn new(
        root: &Path,
        content_sources: impl IntoIterator<Item = PathBuf>,
        bundle_entries: impl IntoIterator<Item = PathBuf>,
        reader_evidence: impl IntoIterator<
            Item = (
                TypstDependencyReader,
                Vec<FileRead>,
                Vec<tola_typst::PackageCheck>,
            ),
        >,
        package_checks: impl IntoIterator<Item = tola_typst::PackageCheck>,
        host: &crate::compiler::SiteHost,
        reused: Option<ReusedDependencyReaders<'_>>,
    ) -> anyhow::Result<Self> {
        let root = tola_input::normalize_path(root);
        let reused_readers: BTreeSet<_> = reused
            .as_ref()
            .map_or(&[][..], |r| r.readers)
            .iter()
            .collect();
        let observed_checks: HashSet<_> = package_checks.into_iter().collect();
        let mut frozen_readers = BTreeMap::new();
        for (reader, reads, mut reader_checks) in reader_evidence {
            sort_package_checks(&mut reader_checks);
            let frozen = if reused_readers.contains(&reader) {
                let frozen = reused
                    .as_ref()
                    .and_then(|r| r.dependencies.reader_evidence.get(&reader))
                    .cloned()
                    .ok_or(ReuseProtocolError::MissingReaderEvidence)?;
                if frozen.package_checks != reader_checks {
                    return Err(ReuseProtocolError::ChangedReaderPackages.into());
                }
                frozen
            } else {
                let reads = reads
                    .into_iter()
                    .map(|read| DepRead::freeze(read, host))
                    .collect::<Result<Vec<_>, ReuseProtocolError>>()?;
                DepReader {
                    reads,
                    package_checks: reader_checks,
                }
            };
            frozen_readers.insert(reader, frozen);
        }
        let frozen_check_set: HashSet<_> = frozen_readers
            .values()
            .flat_map(|r| r.package_checks.iter().cloned())
            .collect();
        if frozen_check_set != observed_checks {
            return Err(ReuseProtocolError::UnattributedPackageCheck.into());
        }
        let mut frozen_checks: Vec<_> = frozen_check_set.into_iter().collect();
        sort_package_checks(&mut frozen_checks);
        let path_index = DependencyPathIndex::build(&root, &frozen_readers, &frozen_checks);
        Ok(Self(Arc::new(DependencyReads {
            content_sources: content_sources
                .into_iter()
                .map(|p| tola_input::normalize_path(&p))
                .collect(),
            bundle_entries: bundle_entries
                .into_iter()
                .map(|p| tola_input::normalize_path(&p))
                .collect(),
            reader_evidence: frozen_readers,
            package_checks: frozen_checks,
            path_index,
        })))
    }

    /// Add native realization reads to the Bundle reader, keeping retained reads frozen.
    pub(crate) fn with_additional_program_reads(
        &self,
        root: &Path,
        reads: impl IntoIterator<Item = FileRead>,
        package_checks: impl IntoIterator<Item = tola_typst::PackageCheck>,
        host: &crate::compiler::SiteHost,
    ) -> anyhow::Result<Self> {
        let prev = self
            .reader_evidence
            .get(&TypstDependencyReader::SiteProgram);
        let observed: HashSet<_> = prev
            .into_iter()
            .flat_map(|r| {
                r.reads.iter().map(|read| {
                    let path = match read {
                        DepRead::Physical { logical_path, .. } => Some(logical_path.as_path()),
                        DepRead::Virtual { .. } => None,
                    };
                    (read.evidence(), path)
                })
            })
            .collect();
        let mut added = HashSet::new();
        let added_reads = reads
            .into_iter()
            .filter(|read| {
                let path = match read.origin() {
                    ReadOrigin::Disk(path) => Some(path.as_path()),
                    ReadOrigin::Provider | ReadOrigin::NonPersistent => None,
                };
                !observed.contains(&(read.evidence(), path)) && added.insert(read.clone())
            })
            .map(|read| DepRead::freeze(read, host))
            .collect::<Result<Vec<_>, ReuseProtocolError>>()?;
        let mut checks = prev.map(|r| r.package_checks.clone()).unwrap_or_default();
        checks.extend(package_checks);
        sort_package_checks(&mut checks);
        if added_reads.is_empty() && prev.is_some_and(|r| r.package_checks == checks) {
            return Ok(self.clone());
        }
        let mut updated = self.clone();
        let inner = Arc::make_mut(&mut updated.0);
        let program = inner
            .reader_evidence
            .entry(TypstDependencyReader::SiteProgram)
            .or_insert_with(|| DepReader {
                reads: Vec::new(),
                package_checks: Vec::new(),
            });
        program.reads.extend(added_reads);
        program.package_checks = checks;
        inner.package_checks = inner
            .reader_evidence
            .values()
            .flat_map(|r| r.package_checks.iter().cloned())
            .collect();
        sort_package_checks(&mut inner.package_checks);
        inner.path_index =
            DependencyPathIndex::build(root, &inner.reader_evidence, &inner.package_checks);
        Ok(updated)
    }

    pub(crate) fn read_evidence(&self) -> impl Iterator<Item = &ReadRecord> {
        self.all_reads().map(DepRead::evidence)
    }

    pub(crate) fn virtual_reads_match(
        &self,
        matches: impl Fn(&ReadRecord) -> bool,
        cancellation: &BuildCancellation,
    ) -> Result<bool, BuildCancelled> {
        cancellation.ensure_active()?;
        let mut checked = HashSet::new();
        for read in self.all_reads() {
            cancellation.ensure_active()?;
            if let DepRead::Virtual {
                evidence,
                process_stable: false,
            } = read
                && checked.insert(evidence)
                && !matches(evidence)
            {
                return Ok(false);
            }
        }
        cancellation.ensure_active()?;
        Ok(true)
    }

    fn direct_readers_for_paths(&self, changed: &[PathBuf]) -> Vec<TypstDependencyReader> {
        let mut readers = BTreeSet::new();
        let changed: BTreeSet<_> = changed
            .iter()
            .flat_map(|p| path_index::path_aliases(p))
            .collect();
        let mut covered: Option<&Path> = None;
        for path in &changed {
            if covered.is_some_and(|ancestor| path.starts_with(ancestor)) {
                continue;
            }
            covered = Some(path.as_path());
            let range = || {
                (
                    std::ops::Bound::Included(path.as_path()),
                    std::ops::Bound::Unbounded,
                )
            };
            readers.extend(
                self.content_sources
                    .range::<Path, _>(range())
                    .take_while(|src| src.starts_with(path))
                    .cloned()
                    .map(TypstDependencyReader::ContentSource),
            );
            if self
                .bundle_entries
                .range::<Path, _>(range())
                .next()
                .is_some_and(|entry| entry.starts_with(path))
            {
                readers.insert(TypstDependencyReader::SiteProgram);
            }
        }
        readers.into_iter().collect()
    }

    pub(crate) fn readers_of_paths(&self, changed: &[PathBuf]) -> Vec<TypstDependencyReader> {
        self.path_index.readers_for(changed, &self.reader_evidence)
    }

    fn readers_requiring_each_build(&self) -> Vec<TypstDependencyReader> {
        self.reader_evidence
            .iter()
            .filter(|(_, r)| r.reads.iter().any(DepRead::requires_each_build))
            .map(|(r, _)| r.clone())
            .collect()
    }

    fn all_reads(&self) -> impl Iterator<Item = &DepRead> {
        self.reader_evidence.values().flat_map(|r| &r.reads)
    }

    pub(crate) fn physical_reads_are_fresh(
        &self,
        boundary: &tola_typst::SourceBoundary,
        cancellation: &BuildCancellation,
    ) -> Result<bool, BuildCancelled> {
        self.changed_physical_paths(boundary, cancellation)
            .map(|paths| paths.is_empty())
    }

    /// Paths whose current bytes or symlink targets differ from captured observations.
    pub(crate) fn changed_physical_paths(
        &self,
        boundary: &tola_typst::SourceBoundary,
        cancellation: &BuildCancellation,
    ) -> Result<Vec<PathBuf>, BuildCancelled> {
        cancellation.ensure_active()?;
        let started = std::time::Instant::now();
        let mut by_logical = HashMap::new();
        let mut distinct = Vec::new();
        let mut changed = BTreeSet::new();
        for read in self.all_reads() {
            cancellation.ensure_active()?;
            let DepRead::Physical {
                evidence,
                logical_path,
                canonical_target,
                ..
            } = read
            else {
                continue;
            };
            let identity = (evidence.digest(), canonical_target);
            match by_logical.entry(logical_path.as_path()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(identity);
                    distinct.push(read);
                }
                std::collections::hash_map::Entry::Occupied(entry) if entry.get() == &identity => {}
                std::collections::hash_map::Entry::Occupied(_) => {
                    changed.insert(logical_path.clone());
                    changed.insert(canonical_target.clone());
                }
            }
        }
        let collected = started.elapsed();
        let verified: Vec<_> = distinct
            .par_iter()
            .map(|read| physical_read_is_fresh(read, boundary, cancellation))
            .collect();
        tracing::debug!(
            target: "tola::compile",
            fence_reads = distinct.len(),
            fence_collect_ms = collected.as_secs_f64() * 1000.0,
            fence_verify_ms = (started.elapsed() - collected).as_secs_f64() * 1000.0,
            "revalidated physical reads"
        );
        for (read, fresh) in distinct.into_iter().zip(verified) {
            if !fresh? {
                changed.extend(read.watch_paths().iter().cloned());
            }
        }
        cancellation.ensure_active()?;
        Ok(changed.into_iter().collect())
    }

    pub(crate) fn changed_package_paths(
        &self,
        cancellation: &BuildCancellation,
    ) -> Result<Vec<PathBuf>, BuildCancelled> {
        let mut changed = Vec::new();
        for check in &self.package_checks {
            cancellation.ensure_active()?;
            if !package_check_is_fresh(check) {
                changed.push(check.candidate().to_path_buf());
            }
        }
        cancellation.ensure_active()?;
        changed.sort_unstable();
        changed.dedup();
        Ok(changed)
    }

    pub(crate) fn package_checks_are_fresh(
        &self,
        cancellation: &BuildCancellation,
    ) -> Result<bool, BuildCancelled> {
        self.changed_package_paths(cancellation)
            .map(|paths| paths.is_empty())
    }

    pub(crate) fn physical_read_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<_> = self
            .all_reads()
            .flat_map(|read| read.watch_paths().iter().cloned())
            .collect();
        paths.sort_unstable();
        paths.dedup();
        paths
    }

    pub(crate) fn record_reader_reads(
        &self,
        inputs: &mut crate::compiler::BuildInputs,
        reader: &TypstDependencyReader,
    ) {
        let Some(observed) = self.reader_evidence.get(reader) else {
            return;
        };
        inputs.record_disk_reads(
            observed
                .reads
                .iter()
                .flat_map(|read| read.watch_paths().iter().cloned()),
        );
        inputs.record_package_checks(observed.package_checks.iter().cloned());
    }

    pub(crate) fn record_all_reads(&self, inputs: &mut crate::compiler::BuildInputs) {
        for reader in self.reader_evidence.keys() {
            self.record_reader_reads(inputs, reader);
        }
    }

    pub(crate) fn package_checks(&self) -> &[tola_typst::PackageCheck] {
        &self.package_checks
    }
}

fn normalize_readers(readers: &mut Vec<TypstDependencyReader>) {
    readers.sort_unstable();
    readers.dedup();
}
