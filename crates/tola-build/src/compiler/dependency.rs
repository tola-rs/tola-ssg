//! Compiler reads, their source or Bundle readers, and invalidation from changed paths.
//!
//! A retained read is only evidence while the bytes it observed are still current, so every
//! reuse path revalidates against the filesystem before it is trusted.

mod path_index;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use tola_typst::{FileRead, ReadEvidence, ReadLocator, ReadOrigin};

use crate::cancellation::{BuildCancellation, BuildCancelled};
use path_index::DependencyPathIndex;
use tola_typst::sort_package_checks;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
/// Compilation unit whose reads determine dependency invalidation.
pub(crate) enum TypstDependencyReader {
    ContentSource(PathBuf),
    SiteProgram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RebuildPriority {
    Unchanged,
    Affected,
    Direct,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
        snapshot: Option<&CompilationDependencies>,
        changed_paths: &[PathBuf],
        inventory_may_change: bool,
    ) -> Self {
        let mut affected_readers = snapshot
            .map(|snapshot| snapshot.readers_of_paths(changed_paths))
            .unwrap_or_default();
        if let Some(snapshot) = snapshot {
            affected_readers.extend(snapshot.readers_requiring_each_build());
        }
        let direct_readers = snapshot
            .map(|snapshot| snapshot.direct_readers_for_paths(changed_paths))
            .unwrap_or_default();
        Self::classify(direct_readers, affected_readers, inventory_may_change)
    }

    /// Classify changes verified against captured reads and unsaved source versions.
    pub(crate) fn for_observed_paths(
        dependencies: &CompilationDependencies,
        changed_paths: &[PathBuf],
    ) -> Self {
        Self::classify(
            dependencies.direct_readers_for_paths(changed_paths),
            dependencies.readers_of_paths(changed_paths),
            false,
        )
    }

    fn classify(
        direct_readers: Vec<TypstDependencyReader>,
        mut affected_readers: Vec<TypstDependencyReader>,
        inventory_may_change: bool,
    ) -> Self {
        let source_direct = inventory_may_change
            || direct_readers
                .iter()
                .any(|reader| matches!(reader, TypstDependencyReader::ContentSource(_)));
        let site_direct = direct_readers
            .binary_search(&TypstDependencyReader::SiteProgram)
            .is_ok();
        affected_readers.retain(|reader| direct_readers.binary_search(reader).is_err());

        let source_state_may_change = direct_readers
            .iter()
            .chain(&affected_readers)
            .any(|reader| matches!(reader, TypstDependencyReader::ContentSource(_)));
        if source_state_may_change
            && !direct_readers.contains(&TypstDependencyReader::SiteProgram)
            && !affected_readers.contains(&TypstDependencyReader::SiteProgram)
        {
            affected_readers.push(TypstDependencyReader::SiteProgram);
        }
        normalize_readers(&mut affected_readers);

        let source_analysis = if source_direct {
            RebuildPriority::Direct
        } else if affected_readers
            .iter()
            .any(|reader| matches!(reader, TypstDependencyReader::ContentSource(_)))
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
            .filter_map(|reader| match reader {
                TypstDependencyReader::ContentSource(source) => Some(source.as_path()),
                TypstDependencyReader::SiteProgram => None,
            })
    }

    pub(crate) fn directly_changed_content_sources(&self) -> impl Iterator<Item = &Path> {
        self.direct_readers
            .iter()
            .filter_map(|reader| match reader {
                TypstDependencyReader::ContentSource(source) => Some(source.as_path()),
                TypstDependencyReader::SiteProgram => None,
            })
    }

    pub(crate) fn directly_rebuilds_site_program(&self) -> bool {
        self.direct_readers
            .contains(&TypstDependencyReader::SiteProgram)
    }
}

#[derive(Debug, Clone)]
enum DependencyReadEvidence {
    Physical {
        evidence: ReadEvidence,
        /// Absolute path named by the read, before resolving symlinks.
        logical_path: PathBuf,
        /// The target the candidate observed, so a retarget is noticed even with identical bytes.
        canonical_target: PathBuf,
        watch_paths: Vec<PathBuf>,
    },
    Virtual {
        evidence: ReadEvidence,
        process_stable: bool,
    },
}

impl DependencyReadEvidence {
    fn freeze(read: FileRead, host: &crate::compiler::TypstHost) -> anyhow::Result<Self> {
        let (evidence, origin) = read.into_evidence_and_origin();
        match origin {
            ReadOrigin::Disk(path) => {
                let logical_path = path.into_path();
                let canonical_target = std::fs::canonicalize(&logical_path).map_err(|error| {
                    anyhow::Error::new(error).context(format!(
                        "Tola could not read `{}`, which this build uses; restore it or remove the reference",
                        read_locator_display(evidence.locator())
                    ))
                })?;
                let watch_paths = physical_watch_paths(&logical_path, &canonical_target);
                Ok(Self::Physical {
                    evidence,
                    logical_path,
                    canonical_target,
                    watch_paths,
                })
            }
            ReadOrigin::Provider | ReadOrigin::NonPersistent => Ok(Self::Virtual {
                process_stable: matches!(origin, ReadOrigin::Provider)
                    && host.read_is_process_stable(evidence.locator()),
                evidence,
            }),
        }
    }

    fn requires_each_build(&self) -> bool {
        match self {
            Self::Virtual {
                evidence,
                process_stable: false,
            } => {
                // Replayable icon files are compared with the current icon collections
                // before both retained-source and complete-Bundle reuse.
                !matches!(
                    evidence.locator(),
                    tola_typst::ReadLocator::ProvidedPackage { package, path }
                        if tola_packages::is_icon_file(package, path)
                )
            }
            _ => false,
        }
    }

    fn watch_paths(&self) -> &[PathBuf] {
        match self {
            Self::Physical { watch_paths, .. } => watch_paths,
            Self::Virtual { .. } => &[],
        }
    }

    fn evidence(&self) -> &ReadEvidence {
        match self {
            Self::Physical { evidence, .. } | Self::Virtual { evidence, .. } => evidence,
        }
    }
}

/// Exact reads observed by source analysis and Bundle compilation.
#[derive(Debug, Clone)]
pub(crate) struct CompilationDependencies(Arc<DependencyReads>);

#[derive(Debug, Clone)]
pub(crate) struct DependencyReads {
    content_sources: std::collections::BTreeSet<PathBuf>,
    bundle_entries: std::collections::BTreeSet<PathBuf>,
    reader_evidence: std::collections::BTreeMap<TypstDependencyReader, DependencyReaderEvidence>,
    package_checks: Vec<tola_typst::PackageCheck>,
    path_index: DependencyPathIndex,
}

#[derive(Debug, Clone)]
struct DependencyReaderEvidence {
    reads: Vec<DependencyReadEvidence>,
    package_checks: Vec<tola_typst::PackageCheck>,
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
        host: &crate::compiler::TypstHost,
        reused: Option<ReusedDependencyReaders<'_>>,
    ) -> anyhow::Result<Self> {
        let root = crate::filesystem::normalize_path(root);
        let reused_readers = reused
            .as_ref()
            .map_or(&[][..], |reused| reused.readers)
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        let observed_package_checks = package_checks
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let mut frozen_readers = std::collections::BTreeMap::new();
        for (reader, reads, mut reader_package_checks) in reader_evidence {
            sort_package_checks(&mut reader_package_checks);
            let frozen = if reused_readers.contains(&reader) {
                let frozen = reused
                    .as_ref()
                    .and_then(|reused| reused.dependencies.reader_evidence.get(&reader))
                    .cloned()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Tola could not reuse the files the previous build read; run the build again"
                        )
                    })?;
                anyhow::ensure!(
                    frozen.package_checks == reader_package_checks,
                    "Tola could not reuse the packages the previous build read; run the build again"
                );
                frozen
            } else {
                let reads = reads
                    .into_iter()
                    .map(|read| DependencyReadEvidence::freeze(read, host))
                    .collect::<anyhow::Result<Vec<_>>>()?;
                DependencyReaderEvidence {
                    reads,
                    package_checks: reader_package_checks,
                }
            };
            frozen_readers.insert(reader, frozen);
        }
        let frozen_package_check_set = frozen_readers
            .values()
            .flat_map(|reader| reader.package_checks.iter().cloned())
            .collect::<std::collections::HashSet<_>>();
        if frozen_package_check_set != observed_package_checks {
            return Err(anyhow::anyhow!(
                "Tola could not tell which part of the build read a package; run the build again"
            ));
        }
        let mut frozen_package_checks = frozen_package_check_set.into_iter().collect::<Vec<_>>();
        sort_package_checks(&mut frozen_package_checks);
        let path_index = DependencyPathIndex::build(&root, &frozen_readers, &frozen_package_checks);
        Ok(Self(Arc::new(DependencyReads {
            content_sources: content_sources
                .into_iter()
                .map(|path| crate::filesystem::normalize_path(&path))
                .collect(),
            bundle_entries: bundle_entries
                .into_iter()
                .map(|path| crate::filesystem::normalize_path(&path))
                .collect(),
            reader_evidence: frozen_readers,
            package_checks: frozen_package_checks,
            path_index,
        })))
    }

    /// Add reads made by successful native resource realization to its Bundle reader.
    /// Observations already retained by the native call do not refreeze or copy caches.
    pub(crate) fn with_additional_program_reads(
        &self,
        root: &Path,
        reads: impl IntoIterator<Item = FileRead>,
        package_checks: impl IntoIterator<Item = tola_typst::PackageCheck>,
        host: &crate::compiler::TypstHost,
    ) -> anyhow::Result<Self> {
        let previous = self
            .reader_evidence
            .get(&TypstDependencyReader::SiteProgram);
        let observed = previous
            .into_iter()
            .flat_map(|reader| {
                reader.reads.iter().map(|read| {
                    let path = match read {
                        DependencyReadEvidence::Physical { logical_path, .. } => {
                            Some(logical_path.as_path())
                        }
                        DependencyReadEvidence::Virtual { .. } => None,
                    };
                    (read.evidence(), path)
                })
            })
            .collect::<std::collections::HashSet<_>>();
        let mut added = std::collections::HashSet::new();
        let added_reads = reads
            .into_iter()
            .filter(|read| {
                let path = match read.origin() {
                    ReadOrigin::Disk(path) => Some(path.as_path()),
                    ReadOrigin::Provider | ReadOrigin::NonPersistent => None,
                };
                !observed.contains(&(read.evidence(), path)) && added.insert(read.clone())
            })
            .map(|read| DependencyReadEvidence::freeze(read, host))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut checks = previous
            .map(|reader| reader.package_checks.clone())
            .unwrap_or_default();
        checks.extend(package_checks);
        sort_package_checks(&mut checks);
        if added_reads.is_empty() && previous.is_some_and(|reader| reader.package_checks == checks)
        {
            return Ok(self.clone());
        }
        let mut updated = self.clone();
        let inner = Arc::make_mut(&mut updated.0);
        let program = inner
            .reader_evidence
            .entry(TypstDependencyReader::SiteProgram)
            .or_insert_with(|| DependencyReaderEvidence {
                reads: Vec::new(),
                package_checks: Vec::new(),
            });
        program.reads.extend(added_reads);
        program.package_checks = checks;
        inner.package_checks = inner
            .reader_evidence
            .values()
            .flat_map(|reader| reader.package_checks.iter().cloned())
            .collect();
        sort_package_checks(&mut inner.package_checks);
        inner.path_index =
            DependencyPathIndex::build(root, &inner.reader_evidence, &inner.package_checks);
        Ok(updated)
    }

    /// Borrow every successful observation, including memoized native resource requests.
    pub(crate) fn read_evidence(&self) -> impl Iterator<Item = &ReadEvidence> {
        self.all_reads().map(DependencyReadEvidence::evidence)
    }

    /// Compare the bytes a reusable read observed against the bytes available now.
    ///
    /// A read recorded as process-stable is skipped: its locator already determines its bytes for
    /// this process. Every other virtual read must match byte for byte.
    pub(crate) fn virtual_reads_match(
        &self,
        matches: impl Fn(&ReadEvidence) -> bool,
        cancellation: &BuildCancellation,
    ) -> Result<bool, BuildCancelled> {
        cancellation.ensure_active()?;
        let mut checked = std::collections::HashSet::new();
        for read in self.all_reads() {
            cancellation.ensure_active()?;
            if let DependencyReadEvidence::Virtual {
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

    fn direct_readers_for_paths(&self, changed_paths: &[PathBuf]) -> Vec<TypstDependencyReader> {
        let mut readers = std::collections::BTreeSet::new();
        let changed_paths = changed_paths
            .iter()
            .map(|path| crate::filesystem::normalize_path(path))
            .collect::<std::collections::BTreeSet<_>>();
        let mut covered: Option<&Path> = None;
        for changed in &changed_paths {
            if covered.is_some_and(|ancestor| changed.starts_with(ancestor)) {
                continue;
            }
            covered = Some(changed.as_path());
            let range = || {
                (
                    std::ops::Bound::Included(changed.as_path()),
                    std::ops::Bound::Unbounded,
                )
            };
            readers.extend(
                self.content_sources
                    .range::<Path, _>(range())
                    .take_while(|source| source.starts_with(changed))
                    .cloned()
                    .map(TypstDependencyReader::ContentSource),
            );
            if self
                .bundle_entries
                .range::<Path, _>(range())
                .next()
                .is_some_and(|entry| entry.starts_with(changed))
            {
                readers.insert(TypstDependencyReader::SiteProgram);
            }
        }
        readers.into_iter().collect()
    }

    pub(crate) fn readers_of_paths(&self, changed_paths: &[PathBuf]) -> Vec<TypstDependencyReader> {
        self.path_index
            .readers_for(changed_paths, &self.reader_evidence)
    }

    fn readers_requiring_each_build(&self) -> Vec<TypstDependencyReader> {
        self.reader_evidence
            .iter()
            .filter(|(_, reader)| {
                reader
                    .reads
                    .iter()
                    .any(DependencyReadEvidence::requires_each_build)
            })
            .map(|(reader, _)| reader.clone())
            .collect()
    }

    fn all_reads(&self) -> impl Iterator<Item = &DependencyReadEvidence> {
        self.reader_evidence
            .values()
            .flat_map(|reader| &reader.reads)
    }

    /// Revalidate physical reads immediately before the output is written.
    ///
    /// This fence runs after the candidate's paths have been attached to the
    /// watcher. Virtual and non-persistent reads do not participate in it.
    pub(crate) fn physical_reads_are_fresh(
        &self,
        boundary: &tola_typst::SourceBoundary,
        cancellation: &BuildCancellation,
    ) -> Result<bool, BuildCancelled> {
        self.changed_physical_paths(boundary, cancellation)
            .map(|paths| paths.is_empty())
    }

    /// Paths whose current bytes or targets differ from the compilation's observations.
    pub(crate) fn changed_physical_paths(
        &self,
        boundary: &tola_typst::SourceBoundary,
        cancellation: &BuildCancellation,
    ) -> Result<Vec<PathBuf>, BuildCancelled> {
        cancellation.ensure_active()?;
        let started = std::time::Instant::now();
        let mut reads_by_logical_path = std::collections::HashMap::new();
        let mut distinct_reads = Vec::new();
        let mut changed = std::collections::BTreeSet::new();
        for read in self.all_reads() {
            cancellation.ensure_active()?;
            let DependencyReadEvidence::Physical {
                evidence,
                logical_path,
                canonical_target,
                ..
            } = read
            else {
                continue;
            };
            let identity = (evidence.digest(), canonical_target);
            match reads_by_logical_path.entry(logical_path.as_path()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(identity);
                    distinct_reads.push(read);
                }
                std::collections::hash_map::Entry::Occupied(entry) if entry.get() == &identity => {}
                std::collections::hash_map::Entry::Occupied(_) => {
                    changed.insert(logical_path.clone());
                    changed.insert(canonical_target.clone());
                }
            }
        }
        let collected = started.elapsed();
        // Every verification reads and hashes its own file, so they run on the CPU pool: the
        // fence would otherwise walk thousands of files one at a time.
        let verified = distinct_reads
            .par_iter()
            .map(|read| physical_read_is_fresh(read, boundary, cancellation))
            .collect::<Vec<_>>();
        tracing::debug!(target: "tola::compile",
            fence_reads = distinct_reads.len(),
            fence_collect_ms = collected.as_secs_f64() * 1000.0,
            fence_verify_ms = (started.elapsed() - collected).as_secs_f64() * 1000.0,
            "revalidated physical reads");
        for (read, fresh) in distinct_reads.into_iter().zip(verified) {
            if !fresh? {
                changed.extend(read.watch_paths().iter().cloned());
            }
        }
        cancellation.ensure_active()?;
        Ok(changed.into_iter().collect())
    }

    /// Package candidates whose availability or selected directory changed.
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
        let mut paths = self
            .all_reads()
            .flat_map(|read| read.watch_paths().iter().cloned())
            .collect::<Vec<_>>();
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
        inputs.file_reads_mut().record(
            observed
                .reads
                .iter()
                .flat_map(|read| read.watch_paths().iter().cloned()),
        );
        inputs.record_package_checks(observed.package_checks.iter().cloned());
    }

    pub(crate) fn record_all_reads(&self, inputs: &mut crate::compiler::BuildInputs) {
        inputs.file_reads_mut().record(
            self.all_reads()
                .flat_map(|read| read.watch_paths().iter().cloned()),
        );
        inputs.record_package_checks(self.package_checks.iter().cloned());
    }

    pub(crate) fn package_checks(&self) -> &[tola_typst::PackageCheck] {
        &self.package_checks
    }
}

/// Sort and deduplicate readers; `RebuildDecision` binary-searches them afterwards.
fn normalize_readers(readers: &mut Vec<TypstDependencyReader>) {
    readers.sort_unstable();
    readers.dedup();
}

fn physical_read_is_fresh(
    read: &DependencyReadEvidence,
    boundary: &tola_typst::SourceBoundary,
    cancellation: &BuildCancellation,
) -> Result<bool, BuildCancelled> {
    cancellation.ensure_active()?;
    let DependencyReadEvidence::Physical {
        evidence,
        logical_path,
        canonical_target,
        ..
    } = read
    else {
        return Ok(true);
    };
    if boundary.check(logical_path).is_err() {
        return Ok(false);
    }
    if !std::fs::canonicalize(logical_path)
        .is_ok_and(|current_target| current_target == *canonical_target)
    {
        cancellation.ensure_active()?;
        return Ok(false);
    }
    if !std::fs::read(logical_path)
        .is_ok_and(|bytes| tola_typst::ContentDigest::of(&bytes) == evidence.digest())
    {
        cancellation.ensure_active()?;
        return Ok(false);
    }
    cancellation.ensure_active()?;
    let matches = std::fs::canonicalize(logical_path)
        .is_ok_and(|current_target| current_target == *canonical_target);
    cancellation.ensure_active()?;
    Ok(matches)
}

fn package_check_is_fresh(check: &tola_typst::PackageCheck) -> bool {
    match check.availability() {
        tola_typst::PackageAvailability::Missing => std::fs::metadata(check.candidate())
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
        tola_typst::PackageAvailability::Present => {
            std::fs::metadata(check.candidate()).is_ok_and(|metadata| metadata.is_dir())
                && check.canonical_target().is_some_and(|expected| {
                    std::fs::canonicalize(check.candidate())
                        .is_ok_and(|current| current == expected)
                })
        }
        tola_typst::PackageAvailability::NotDirectory => false,
        tola_typst::PackageAvailability::Unreadable => false,
    }
}

fn physical_watch_paths(logical_path: &Path, canonical_target: &Path) -> Vec<PathBuf> {
    let mut paths = vec![logical_path.to_path_buf(), canonical_target.to_path_buf()];
    paths.sort_unstable();
    paths.dedup();
    paths
}

/// Name a read the way the author wrote it: a site path, or an imported package file.
///
/// Physical reads keep the Typst-relative spelling the resolver recorded, so no host directory
/// reaches a rendered message.
fn read_locator_display(locator: &ReadLocator) -> String {
    match locator {
        ReadLocator::Root(path) => path.display().to_string(),
        ReadLocator::Package { package, path } => {
            format!("{package}/{}", path.display())
        }
        ReadLocator::ProvidedRoot(path) => path.display().to_string(),
        ReadLocator::ProvidedPackage { package, path } => {
            format!("{package}/{}", path.display())
        }
        ReadLocator::NonPersistent(_) => "a generated file".to_owned(),
    }
}

#[cfg(test)]
mod tests {

    use std::collections::BTreeMap;

    use super::*;
    use crate::compiler::tests::compiler_host;
    use tola_typst::ReadLocator;

    fn retained_dependencies(
        root: &Path,
        content_sources: impl IntoIterator<Item = PathBuf>,
        bundle_entries: impl IntoIterator<Item = PathBuf>,
        reader_evidence: BTreeMap<TypstDependencyReader, DependencyReaderEvidence>,
        package_checks: Vec<tola_typst::PackageCheck>,
    ) -> CompilationDependencies {
        let root = crate::filesystem::normalize_path(root);
        CompilationDependencies(Arc::new(DependencyReads {
            path_index: DependencyPathIndex::build(&root, &reader_evidence, &package_checks),
            content_sources: content_sources
                .into_iter()
                .map(|path| crate::filesystem::normalize_path(&path))
                .collect(),
            bundle_entries: bundle_entries
                .into_iter()
                .map(|path| crate::filesystem::normalize_path(&path))
                .collect(),
            reader_evidence,
            package_checks,
        }))
    }

    fn evidence(path: &str) -> ReadEvidence {
        ReadEvidence::new(ReadLocator::Root(path.into()), path.as_bytes())
    }

    fn physical_evidence(
        logical_path: PathBuf,
        canonical_target: PathBuf,
        evidence: ReadEvidence,
    ) -> DependencyReadEvidence {
        DependencyReadEvidence::Physical {
            watch_paths: physical_watch_paths(&logical_path, &canonical_target),
            logical_path,
            canonical_target,
            evidence,
        }
    }

    fn producer_read(root: &Path, evidence: ReadEvidence) -> DependencyReadEvidence {
        if let ReadLocator::Root(relative) = evidence.locator() {
            let logical_path = logical_read_path(&root.join(relative));
            if let Ok(canonical_target) = std::fs::canonicalize(&logical_path) {
                return physical_evidence(logical_path, canonical_target, evidence);
            }
        }
        DependencyReadEvidence::Virtual {
            process_stable: false,
            evidence,
        }
    }

    fn physical_read(path: &Path, evidence: ReadEvidence) -> DependencyReadEvidence {
        let logical_path = logical_read_path(path);
        let canonical_target = std::fs::canonicalize(&logical_path).unwrap();
        physical_evidence(logical_path, canonical_target, evidence)
    }

    fn logical_read_path(path: &Path) -> PathBuf {
        let absolute = std::path::absolute(path).unwrap();
        let Some(name) = absolute.file_name() else {
            return absolute;
        };
        absolute
            .parent()
            .and_then(|parent| std::fs::canonicalize(parent).ok())
            .map_or(absolute.clone(), |parent| parent.join(name))
    }

    fn dependencies(
        root: &Path,
        content_sources: impl IntoIterator<Item = PathBuf>,
        site_program: &Path,
        reader_evidence: impl IntoIterator<Item = (TypstDependencyReader, Vec<ReadEvidence>)>,
    ) -> CompilationDependencies {
        let root = crate::filesystem::normalize_path(root);
        let reader_evidence = reader_evidence
            .into_iter()
            .map(|(reader, reads)| {
                (
                    reader,
                    DependencyReaderEvidence {
                        reads: reads
                            .into_iter()
                            .map(|read| producer_read(&root, read))
                            .collect(),
                        package_checks: Vec::new(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        retained_dependencies(
            &root,
            content_sources,
            [site_program.to_path_buf()],
            reader_evidence,
            Vec::new(),
        )
    }

    /// A reader's path as the decision reports it: a reader carries what the filesystem resolves
    /// its read to, which gains the current drive on Windows.
    fn reader_path(path: &str) -> PathBuf {
        crate::filesystem::normalize_path(Path::new(path))
    }

    fn snapshot(
        root: &Path,
        content_sources: impl IntoIterator<Item = PathBuf>,
        site_program: &Path,
        source_evidence: Vec<ReadEvidence>,
        site_evidence: Vec<ReadEvidence>,
    ) -> CompilationDependencies {
        let content_sources = content_sources.into_iter().collect::<Vec<_>>();
        let source = content_sources
            .first()
            .cloned()
            .unwrap_or_else(|| root.join("content/source.typ"));
        dependencies(
            root,
            content_sources,
            site_program,
            [
                (
                    TypstDependencyReader::ContentSource(source),
                    source_evidence,
                ),
                (TypstDependencyReader::SiteProgram, site_evidence),
            ],
        )
    }

    /// A changed path selects exactly the readers that read it, and the decision names them.
    ///
    /// One row per read shape: a path two readers share, an asset only the site program reads,
    /// a package file beneath its directory, and a source that is itself an input.
    #[test]
    fn changed_paths_select_their_readers() {
        let root = Path::new("/site");
        let snapshot = snapshot(
            root,
            [PathBuf::from("/site/content/post.typ")],
            Path::new("/site/site.typ"),
            vec![
                evidence("content/post.typ"),
                evidence("templates/document.typ"),
            ],
            vec![evidence("site.typ"), evidence("templates/document.typ")],
        );

        for (changed, readers) in [
            (
                "/site/templates/document.typ",
                vec![
                    TypstDependencyReader::ContentSource(PathBuf::from("/site/content/post.typ")),
                    TypstDependencyReader::SiteProgram,
                ],
            ),
            ("/site/site.typ", vec![TypstDependencyReader::SiteProgram]),
            ("/site/assets/unread.svg", Vec::new()),
            (
                "/site/templates",
                vec![
                    TypstDependencyReader::ContentSource(PathBuf::from("/site/content/post.typ")),
                    TypstDependencyReader::SiteProgram,
                ],
            ),
            ("/site/templates-old", Vec::new()),
        ] {
            assert_eq!(
                snapshot.readers_of_paths(&[PathBuf::from(changed)]),
                readers,
                "{changed}"
            );
        }
    }

    /// A changed path's priority follows what read it: an input rebuilds the analysis directly,
    /// a read it depends on marks it affected, and a source's own change is direct.
    #[test]
    fn changed_paths_set_rebuild_priority() {
        let snapshot = snapshot(
            Path::new("/site"),
            [PathBuf::from("/site/content/post.typ")],
            Path::new("/site/program.typ"),
            vec![evidence("templates/document.typ")],
            vec![evidence("site.typ")],
        );

        for (changed, source_analysis, site_program, affected) in [
            (
                "/site/content/post.typ",
                RebuildPriority::Direct,
                RebuildPriority::Affected,
                vec![TypstDependencyReader::SiteProgram],
            ),
            (
                "/site/templates/document.typ",
                RebuildPriority::Affected,
                RebuildPriority::Affected,
                vec![
                    TypstDependencyReader::ContentSource(PathBuf::from("/site/content/post.typ")),
                    TypstDependencyReader::SiteProgram,
                ],
            ),
        ] {
            let decision =
                RebuildDecision::for_paths(Some(&snapshot), &[PathBuf::from(changed)], false);
            assert_eq!(decision.source_analysis, source_analysis, "{changed}");
            assert_eq!(decision.site_program, site_program, "{changed}");
            assert_eq!(decision.affected_readers, affected, "{changed}");
        }

        // A parent directory rebuilds the sources beneath it, while a sibling prefix does not.
        let parent =
            RebuildDecision::for_paths(Some(&snapshot), &[PathBuf::from("/site/content")], false);
        assert_eq!(parent.source_analysis, RebuildPriority::Direct);
        assert_eq!(
            parent.direct_readers,
            vec![TypstDependencyReader::ContentSource(reader_path(
                "/site/content/post.typ"
            ))]
        );

        // An inventory change rebuilds the analysis without naming any reader of a changed path.
        let inventory = RebuildDecision::for_paths(
            Some(&snapshot),
            &[PathBuf::from("/site/content/added.typ")],
            true,
        );
        assert_eq!(inventory.source_analysis, RebuildPriority::Direct);
        assert!(inventory.direct_readers.is_empty());
    }

    /// Physical reads are fresh while their bytes and targets are unchanged, and stale as soon as
    /// either moves: an edit, a removal, or a symlink retarget with the same bytes.
    #[test]
    fn physical_reads_turn_stale_on_change() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let template = root.join("template.typ");
        std::fs::write(&template, "#let helper = 1").unwrap();
        let snapshot = snapshot(
            root,
            std::iter::empty(),
            &root.join("site.typ"),
            Vec::new(),
            vec![ReadEvidence::new(
                ReadLocator::Root("template.typ".into()),
                b"#let helper = 1",
            )],
        );
        let boundary = tola_typst::SourceBoundary::default();
        let cancellation = BuildCancellation::default();
        let fresh = || {
            snapshot
                .physical_reads_are_fresh(&boundary, &cancellation)
                .unwrap()
        };

        assert!(fresh());
        assert!(RebuildDecision::for_paths(Some(&snapshot), &[], false).reuses_site_program());

        std::fs::write(&template, "#let helper = 2").unwrap();
        assert!(!fresh(), "an edited read is stale");

        std::fs::remove_file(&template).unwrap();
        assert!(!fresh(), "a removed read is stale");
    }

    /// The same bytes reached through a different target are still stale: identity is the target,
    /// not the content.
    #[cfg(unix)]
    #[test]
    fn retargeted_read_is_stale() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let first = root.join("first.typ");
        let second = root.join("second.typ");
        let logical = root.join("template.typ");
        std::fs::write(&first, b"same bytes").unwrap();
        std::fs::write(&second, b"same bytes").unwrap();
        symlink(&first, &logical).unwrap();
        let snapshot = snapshot(
            root,
            std::iter::empty(),
            &root.join("site.typ"),
            Vec::new(),
            vec![ReadEvidence::new(
                ReadLocator::Root("template.typ".into()),
                b"same bytes",
            )],
        );
        let boundary = tola_typst::SourceBoundary::default();
        let cancellation = BuildCancellation::default();

        assert!(
            snapshot
                .physical_reads_are_fresh(&boundary, &cancellation)
                .unwrap()
        );
        // A read is watched at both its logical path and the target it resolves to.
        let watched = snapshot.physical_read_paths();
        assert!(watched.contains(&crate::filesystem::normalize_path(root).join("template.typ")));
        assert!(watched.contains(&std::fs::canonicalize(&first).unwrap()));

        std::fs::remove_file(&logical).unwrap();
        symlink(&second, &logical).unwrap();
        assert!(
            !snapshot
                .physical_reads_are_fresh(&boundary, &cancellation)
                .unwrap(),
            "a retargeted read is stale even when the bytes match"
        );
    }

    /// Two readers that observed different bytes for one path make the compilation stale.
    #[test]
    fn conflicting_reads_are_stale() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let template = root.join("template.typ");
        std::fs::write(&template, "first").unwrap();
        let boundary = tola_typst::SourceBoundary::default();
        let cancellation = BuildCancellation::default();

        let conflicting = snapshot(
            root,
            [root.join("content/post.typ")],
            &root.join("site.typ"),
            vec![ReadEvidence::new(
                ReadLocator::Root("template.typ".into()),
                b"first",
            )],
            vec![ReadEvidence::new(
                ReadLocator::Root("template.typ".into()),
                b"second",
            )],
        );
        assert!(
            !conflicting
                .physical_reads_are_fresh(&boundary, &cancellation)
                .unwrap(),
            "readers that saw different bytes are stale"
        );
    }

    /// A path several readers share is watched once.
    #[test]
    fn shared_read_is_watched_once() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let template = root.join("template.typ");
        std::fs::write(&template, "first").unwrap();
        let boundary = tola_typst::SourceBoundary::default();
        let cancellation = BuildCancellation::default();

        let read = || ReadEvidence::new(ReadLocator::Root("template.typ".into()), b"first");
        let shared = snapshot(
            root,
            std::iter::empty(),
            &root.join("site.typ"),
            vec![read()],
            vec![read()],
        );
        assert_eq!(
            shared.physical_read_paths(),
            vec![crate::filesystem::normalize_path(&template)]
        );
        assert!(
            shared
                .physical_reads_are_fresh(&boundary, &cancellation)
                .unwrap()
        );
    }

    /// A package file is watched through the physical file it resolves to, so a change inside
    /// the package directory reaches the reader that read it.
    #[test]
    fn package_read_matches_its_physical_file() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let package_root = root.join("packages");
        let package_file = package_root.join("preview/demo/1.2.3/lib.typ");
        std::fs::create_dir_all(package_file.parent().unwrap()).unwrap();
        std::fs::write(&package_file, "#let helper = 1").unwrap();

        let package_read = ReadEvidence::new(
            ReadLocator::Package {
                package: "@preview/demo:1.2.3".parse().unwrap(),
                path: PathBuf::from("lib.typ"),
            },
            b"#let helper = 1",
        );
        let reader = TypstDependencyReader::ContentSource(root.join("content/source.typ"));
        let snapshot = retained_dependencies(
            root,
            std::iter::empty::<PathBuf>(),
            [root.join("site.typ")],
            BTreeMap::from([(
                reader.clone(),
                DependencyReaderEvidence {
                    reads: vec![physical_read(&package_file, package_read)],
                    package_checks: Vec::new(),
                },
            )]),
            Vec::new(),
        );

        assert_eq!(
            snapshot.readers_of_paths(&[package_root.join("preview/demo")]),
            vec![reader]
        );
        assert_eq!(
            RebuildDecision::for_paths(Some(&snapshot), &[package_file], false).source_analysis,
            RebuildPriority::Affected
        );
    }

    /// Two locators that read the same bytes are one read: the compilation stays fresh while
    /// both agree.
    #[test]
    fn same_bytes_from_two_locators_agree() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let file = root.join("shared.typ");
        std::fs::write(&file, "Shared").unwrap();
        let boundary = tola_typst::SourceBoundary::default();
        let cancellation = BuildCancellation::default();

        let reads = [
            ReadLocator::Root("shared.typ".into()),
            ReadLocator::ProvidedRoot("mapped.typ".into()),
        ]
        .into_iter()
        .map(|locator| physical_read(&file, ReadEvidence::new(locator, b"Shared")))
        .collect();
        let shared = retained_dependencies(
            root,
            [],
            [],
            BTreeMap::from([(
                TypstDependencyReader::SiteProgram,
                DependencyReaderEvidence {
                    reads,
                    package_checks: Vec::new(),
                },
            )]),
            Vec::new(),
        );
        assert!(
            shared
                .physical_reads_are_fresh(&boundary, &cancellation)
                .unwrap(),
            "distinct locators over the same bytes agree"
        );
    }

    /// An asset only the site program reads leaves the source analysis alone.
    #[test]
    fn asset_read_leaves_source_analysis_alone() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        for path in [
            "templates/document.typ",
            "site.typ",
            "assets/a.svg",
            "content/a.typ",
        ] {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, path).unwrap();
        }
        let asset_snapshot = dependencies(
            root,
            [root.join("content/a.typ")],
            &root.join("site.typ"),
            [
                (
                    TypstDependencyReader::ContentSource(root.join("content/a.typ")),
                    vec![evidence("templates/document.typ")],
                ),
                (
                    TypstDependencyReader::SiteProgram,
                    vec![evidence("site.typ"), evidence("assets/a.svg")],
                ),
            ],
        );
        let decision =
            RebuildDecision::for_paths(Some(&asset_snapshot), &[root.join("assets/a.svg")], false);
        assert_eq!(decision.source_analysis, RebuildPriority::Unchanged);
        assert_eq!(decision.site_program, RebuildPriority::Affected);
        assert!(decision.direct_readers.is_empty());
        assert_eq!(
            decision.affected_readers,
            vec![TypstDependencyReader::SiteProgram]
        );
    }

    /// A package check follows the packages it selected, through a better root or a retarget.
    #[test]
    fn package_checks_follow_their_selection() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let data = root.join("package-data");
        let cache = root.join("package-cache");
        std::fs::create_dir_all(cache.join("preview/demo/1.0.0")).unwrap();
        let locations =
            tola_typst::PackageLocations::from_absolute_roots(Some(data.clone()), Some(cache))
                .unwrap();
        let store =
            tola_typst::PackageStore::new(locations, tola_typst::PackageFetchPolicy::LocalOnly);
        let package = "@preview/demo:1.0.0".parse().unwrap();
        let checks = store.prepare(&package).unwrap().checks().to_vec();

        assert!(checks.iter().all(package_check_is_fresh));
        // A site package root that now holds the package is a better answer than the cache.
        std::fs::create_dir_all(data.join("preview/demo/1.0.0")).unwrap();
        assert!(!checks.iter().all(package_check_is_fresh));
    }

    /// A read the process cannot reproduce makes its producer rebuild; an owned observation
    /// descriptor does not.
    ///
    /// That ownership is the whole classification, so one row per locator kind decides it.
    #[test]
    fn virtual_reads_classify_by_ownership() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let config = crate::config::tests::load_test_config(root, "");
        let host = compiler_host(&config).unwrap();
        let snapshot = |locator: ReadLocator| {
            let process_stable = host.read_is_process_stable(&locator);
            let reader_evidence = [(
                TypstDependencyReader::SiteProgram,
                DependencyReaderEvidence {
                    reads: vec![DependencyReadEvidence::Virtual {
                        evidence: ReadEvidence::new(locator, b"virtual"),
                        process_stable,
                    }],
                    package_checks: Vec::new(),
                },
            )]
            .into_iter()
            .collect::<BTreeMap<_, _>>();
            retained_dependencies(
                config.get_root(),
                std::iter::empty::<PathBuf>(),
                [config.get_root().join("site.typ")],
                reader_evidence,
                Vec::new(),
            )
        };
        let priority = |locator: ReadLocator| {
            RebuildDecision::for_paths(Some(&snapshot(locator)), &[], false).site_program
        };

        for (locator, expected) in [
            // A package file is immutable for the process.
            (
                ReadLocator::ProvidedPackage {
                    package: "@tola/document:0.0.0".parse().unwrap(),
                    path: "lib.typ".into(),
                },
                RebuildPriority::Unchanged,
            ),
            // Both source observations belong to `@tola/source`: the package-wide capability
            // read and the per-file lexical identity read are immutable descriptors.
            (
                ReadLocator::ProvidedPackage {
                    package: tola_packages::TolaPackage::Source.spec(),
                    path: ".tola-capability".into(),
                },
                RebuildPriority::Unchanged,
            ),
            (
                ReadLocator::ProvidedPackage {
                    package: tola_packages::TolaPackage::Source.spec(),
                    path: ".tola-source/content/document.typ".into(),
                },
                RebuildPriority::Unchanged,
            ),
            // The same path under another package is an ordinary unstable virtual read.
            (
                ReadLocator::ProvidedPackage {
                    package: tola_packages::TolaPackage::Document.spec(),
                    path: ".tola-source/content/document.typ".into(),
                },
                RebuildPriority::Affected,
            ),
            (
                ReadLocator::ProvidedRoot("internal.typ".into()),
                RebuildPriority::Affected,
            ),
            (
                ReadLocator::NonPersistent("<generated>".into()),
                RebuildPriority::Affected,
            ),
        ] {
            assert_eq!(priority(locator.clone()), expected, "{locator:?}");
        }
    }

    /// A reused unit keeps the package checks it was derived from, so its evidence survives.
    #[test]
    fn reused_unit_keeps_its_package_checks() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let data = root.join("package-data");
        for package in ["first", "second"] {
            std::fs::create_dir_all(data.join(format!("preview/{package}/1.0.0"))).unwrap();
        }
        let locations =
            tola_typst::PackageLocations::from_absolute_roots(Some(data), None).unwrap();
        let store =
            tola_typst::PackageStore::new(locations, tola_typst::PackageFetchPolicy::LocalOnly);
        let first = store
            .prepare(&"@preview/first:1.0.0".parse().unwrap())
            .unwrap()
            .checks()[0]
            .clone();
        let second = store
            .prepare(&"@preview/second:1.0.0".parse().unwrap())
            .unwrap()
            .checks()[0]
            .clone();
        let source_unit = TypstDependencyReader::ContentSource(root.join("content/post.typ"));
        let reader_evidence = [
            (
                source_unit.clone(),
                DependencyReaderEvidence {
                    reads: Vec::new(),
                    package_checks: vec![first.clone()],
                },
            ),
            (
                TypstDependencyReader::SiteProgram,
                DependencyReaderEvidence {
                    reads: Vec::new(),
                    package_checks: vec![second],
                },
            ),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let previous = retained_dependencies(
            root,
            std::iter::empty::<PathBuf>(),
            std::iter::empty::<PathBuf>(),
            reader_evidence,
            vec![first.clone()],
        );
        let config = crate::config::tests::load_test_config(root, "");
        let host = compiler_host(&config).unwrap();

        let published = CompilationDependencies::new(
            root,
            std::iter::empty(),
            std::iter::empty(),
            [(source_unit.clone(), Vec::new(), vec![first.clone()])],
            std::iter::once(first.clone()),
            &host,
            Some(ReusedDependencyReaders::new(
                &previous,
                std::slice::from_ref(&source_unit),
            )),
        )
        .unwrap();

        assert_eq!(published.package_checks(), std::slice::from_ref(&first));
        let mut inputs = crate::compiler::BuildInputs::default();
        published.record_reader_reads(&mut inputs, &source_unit);
        let (_, package_checks) = inputs.into_parts();
        assert_eq!(package_checks, vec![first]);
    }

    /// An icon read is matched against the bytes the host currently holds, and its membership
    /// list against the names the collection carries.
    #[test]
    fn icon_reads_follow_their_current_bytes() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let host = compiler_host(&config).unwrap();
        let collections = |mark: &str, unused: &str| {
            let wrap = |paint: &str| {
                format!("<svg viewBox='0 0 1 1'><path fill='{paint}' d='M0 0h1v1H0z'/></svg>")
            };
            crate::compiler::tests::brand_icons(&wrap(mark), &wrap(unused))
        };
        let original = collections("red", "blue");
        let original_host = host.with_icons(Arc::clone(&original));
        let read = ReadEvidence::new(
            ReadLocator::ProvidedPackage {
                package: tola_packages::TolaPackage::Icon.spec(),
                path: ".tola-icon/brand/mark.svg".into(),
            },
            original.get("brand", "mark").unwrap().svg().as_bytes(),
        );
        assert!(!original_host.read_is_process_stable(read.locator()));
        let dependencies = snapshot(
            config.get_root(),
            std::iter::empty(),
            &config.build.entry,
            Vec::new(),
            vec![read],
        );
        let cancellation = BuildCancellation::default();
        let matches = |host: &crate::compiler::TypstHost| {
            dependencies
                .virtual_reads_match(
                    |evidence| host.virtual_read_matches(evidence),
                    &cancellation,
                )
                .unwrap()
        };
        assert!(matches(&original_host));
        // An unused icon changing does not invalidate the reader.
        assert!(matches(&host.with_icons(collections("red", "green"))));
        assert!(!matches(&host.with_icons(collections("tan", "blue"))));
        assert!(!matches(&host.with_icons(Arc::default())));

        // The name list is its own observation: a changed membership invalidates, as does none.
        let named = |names: &[&str]| {
            let mut collection = tola_icons::IconCollection::new();
            for name in names {
                collection
                    .insert_svg(name, "<svg viewBox='0 0 1 1'><path/></svg>")
                    .unwrap();
            }
            let mut collections = tola_icons::IconCollections::new();
            collections.mount("brand", collection).unwrap();
            Arc::new(collections)
        };
        let package = tola_packages::TolaPackage::Icon.spec();
        let path = std::path::Path::new(".tola-icon/brand/.names");
        let bytes = tola_packages::icon_collection_names_bytes(&original, &package, path).unwrap();
        let dependencies = snapshot(
            config.get_root(),
            std::iter::empty(),
            &config.build.entry,
            Vec::new(),
            vec![ReadEvidence::new(
                ReadLocator::ProvidedPackage {
                    package,
                    path: path.into(),
                },
                &bytes,
            )],
        );
        let matches = |collections| {
            let current = host.with_icons(collections);
            dependencies
                .virtual_reads_match(
                    |evidence| current.virtual_read_matches(evidence),
                    &cancellation,
                )
                .unwrap()
        };
        assert!(matches(Arc::clone(&original)));
        assert!(!matches(named(&["palette", "paints"])));
        assert!(!matches(named(&[])));
    }
}
