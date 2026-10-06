//! The read evidence one successful build leaves behind, the readers it attributes each read
//! to, and the decision a changed path produces: reuse, re-analyze, or rebuild.
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
        snapshot: Option<&PublishedDependencies>,
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

/// Exact reads observed while producing one successfully published site.
#[derive(Debug, Clone)]
pub(crate) struct PublishedDependencies(Arc<PublishedDependencyReads>);

#[derive(Debug, Clone)]
pub(crate) struct PublishedDependencyReads {
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

impl std::ops::Deref for PublishedDependencies {
    type Target = PublishedDependencyReads;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub(crate) struct ReusedDependencyReaders<'a> {
    dependencies: &'a PublishedDependencies,
    readers: &'a [TypstDependencyReader],
}

impl<'a> ReusedDependencyReaders<'a> {
    pub(crate) fn new(
        dependencies: &'a PublishedDependencies,
        readers: &'a [TypstDependencyReader],
    ) -> Self {
        Self {
            dependencies,
            readers,
        }
    }
}

impl PublishedDependencies {
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
        Ok(Self(Arc::new(PublishedDependencyReads {
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
        host: &crate::compiler::TypstHost,
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
                && !host.virtual_read_matches(evidence)
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
        cancellation.ensure_active()?;
        let started = std::time::Instant::now();
        // One verification per logical path: the first read that claims a path proves its bytes,
        // and a later read claiming the same path must agree on what was observed.
        let mut reads_by_logical_path = std::collections::HashMap::new();
        let mut distinct_reads = Vec::new();
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
            let identity = PhysicalReadIdentity {
                evidence,
                logical_path,
                canonical_target,
            };
            match reads_by_logical_path.entry(logical_path.as_path()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(identity);
                    distinct_reads.push(read);
                }
                std::collections::hash_map::Entry::Occupied(entry) if entry.get() == &identity => {}
                // One candidate cannot be fresh when its producers observed
                // different bytes or targets through the same logical path.
                std::collections::hash_map::Entry::Occupied(_) => return Ok(false),
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
        for fresh in verified {
            if !fresh? {
                return Ok(false);
            }
        }
        cancellation.ensure_active()?;
        Ok(true)
    }

    pub(crate) fn package_checks_are_fresh(
        &self,
        cancellation: &BuildCancellation,
    ) -> Result<bool, BuildCancelled> {
        cancellation.ensure_active()?;
        for check in &self.package_checks {
            cancellation.ensure_active()?;
            let fresh = package_check_is_fresh(check);
            cancellation.ensure_active()?;
            if !fresh {
                return Ok(false);
            }
        }
        cancellation.ensure_active()?;
        Ok(true)
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
        let Some(published) = self.reader_evidence.get(reader) else {
            return;
        };
        inputs.file_reads_mut().record(
            published
                .reads
                .iter()
                .flat_map(|read| read.watch_paths().iter().cloned()),
        );
        inputs.record_published_package_checks(published.package_checks.iter().cloned());
    }

    pub(crate) fn record_all_reads(&self, inputs: &mut crate::compiler::BuildInputs) {
        inputs.file_reads_mut().record(
            self.all_reads()
                .flat_map(|read| read.watch_paths().iter().cloned()),
        );
        inputs.record_published_package_checks(self.package_checks.iter().cloned());
    }

    pub(crate) fn package_checks(&self) -> &[tola_typst::PackageCheck] {
        &self.package_checks
    }
}

#[derive(PartialEq, Eq)]
struct PhysicalReadIdentity<'a> {
    evidence: &'a ReadEvidence,
    logical_path: &'a Path,
    canonical_target: &'a Path,
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

    use super::*;
    use crate::compiler::tests::compiler_host;
    use tola_typst::ReadLocator;
    fn retained_dependencies(
        root: &Path,
        content_sources: impl IntoIterator<Item = PathBuf>,
        bundle_entries: impl IntoIterator<Item = PathBuf>,
        reader_evidence: std::collections::BTreeMap<
            TypstDependencyReader,
            DependencyReaderEvidence,
        >,
        package_checks: Vec<tola_typst::PackageCheck>,
    ) -> PublishedDependencies {
        let root = crate::filesystem::normalize_path(root);
        PublishedDependencies(Arc::new(PublishedDependencyReads {
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
    ) -> PublishedDependencies {
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
            .collect::<std::collections::BTreeMap<_, _>>();
        retained_dependencies(
            &root,
            content_sources,
            [site_program.to_path_buf()],
            reader_evidence,
            Vec::new(),
        )
    }

    fn snapshot(
        root: &Path,
        content_sources: impl IntoIterator<Item = PathBuf>,
        site_program: &Path,
        source_evidence: Vec<ReadEvidence>,
        site_evidence: Vec<ReadEvidence>,
    ) -> PublishedDependencies {
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

    #[test]
    fn changed_root_file_selects_its_readers() {
        let snapshot = snapshot(
            Path::new("/site"),
            [PathBuf::from("/site/content/post.typ")],
            Path::new("/site/site.typ"),
            vec![
                evidence("content/post.typ"),
                evidence("templates/document.typ"),
            ],
            vec![evidence("site.typ"), evidence("templates/document.typ")],
        );

        assert_eq!(
            snapshot.readers_of_paths(&[PathBuf::from("/site/templates/document.typ")]),
            vec![
                TypstDependencyReader::ContentSource(PathBuf::from("/site/content/post.typ")),
                TypstDependencyReader::SiteProgram
            ]
        );
        assert!(
            snapshot
                .readers_of_paths(&[PathBuf::from("/site/assets/unread.svg")])
                .is_empty()
        );
    }

    #[test]
    fn asset_read_touches_only_the_program() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("templates")).unwrap();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        for path in [
            "templates/document.typ",
            "templates/note.typ",
            "site.typ",
            "assets/a.svg",
        ] {
            std::fs::write(root.join(path), path).unwrap();
        }
        let snapshot = dependencies(
            root,
            [root.join("content/a.typ"), root.join("content/b.typ")],
            &root.join("site.typ"),
            [
                (
                    TypstDependencyReader::ContentSource(root.join("content/a.typ")),
                    vec![evidence("templates/document.typ")],
                ),
                (
                    TypstDependencyReader::ContentSource(root.join("content/b.typ")),
                    vec![evidence("templates/note.typ")],
                ),
                (
                    TypstDependencyReader::SiteProgram,
                    vec![evidence("site.typ"), evidence("assets/a.svg")],
                ),
            ],
        );

        assert_eq!(
            snapshot.readers_of_paths(&[root.join("templates/document.typ")]),
            vec![TypstDependencyReader::ContentSource(
                root.join("content/a.typ")
            )]
        );
        assert_eq!(
            snapshot.readers_of_paths(&[root.join("assets/a.svg")]),
            vec![TypstDependencyReader::SiteProgram]
        );

        let decision =
            RebuildDecision::for_paths(Some(&snapshot), &[root.join("assets/a.svg")], false);
        assert_eq!(decision.source_analysis, RebuildPriority::Unchanged);
        assert_eq!(decision.site_program, RebuildPriority::Affected);
        assert!(decision.direct_readers.is_empty());
        assert_eq!(
            decision.affected_readers,
            vec![TypstDependencyReader::SiteProgram]
        );
    }

    #[test]
    fn directory_change_hits_reads_beneath_it() {
        let snapshot = snapshot(
            Path::new("/site"),
            std::iter::empty(),
            Path::new("/site/site.typ"),
            vec![evidence("templates/document.typ")],
            vec![evidence("site.typ")],
        );

        assert_eq!(
            snapshot.readers_of_paths(&[PathBuf::from("/site/templates")]),
            vec![TypstDependencyReader::ContentSource(PathBuf::from(
                "/site/content/source.typ"
            ))]
        );
        assert!(
            snapshot
                .readers_of_paths(&[PathBuf::from("/site/templates-old")])
                .is_empty()
        );
    }

    #[test]
    fn package_read_matches_its_physical_file() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let package_root = root.join("packages");
        let package_file = package_root.join("preview/demo/1.2.3/lib.typ");
        std::fs::create_dir_all(package_file.parent().unwrap()).unwrap();
        std::fs::write(&package_file, "#let helper = 1").unwrap();

        let mut config = crate::config::tests::load_test_config(root, "");
        config.build.entry = root.join("site.typ");
        config.build.content_dir = root.join("content");
        config.package_locations =
            tola_typst::PackageLocations::from_absolute_roots(Some(package_root.clone()), None)
                .unwrap();
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
            std::collections::BTreeMap::from([(
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
        let decision = RebuildDecision::for_paths(Some(&snapshot), &[package_file], false);
        assert_eq!(decision.source_analysis, RebuildPriority::Affected);
    }

    #[test]
    fn changed_source_selects_the_analysis() {
        let snapshot = snapshot(
            Path::new("/site"),
            [PathBuf::from("/site/content/post.typ")],
            Path::new("/site/program.typ"),
            Vec::new(),
            Vec::new(),
        );
        let decision = RebuildDecision::for_paths(
            Some(&snapshot),
            &[PathBuf::from("/site/content/post.typ")],
            false,
        );
        assert_eq!(decision.source_analysis, RebuildPriority::Direct);
        assert_eq!(decision.site_program, RebuildPriority::Affected);
        assert_eq!(
            decision.direct_readers,
            vec![TypstDependencyReader::ContentSource(PathBuf::from(
                "/site/content/post.typ"
            ))]
        );
        assert_eq!(
            decision.affected_readers,
            vec![TypstDependencyReader::SiteProgram]
        );
    }

    #[test]
    fn parent_directory_rebuilds_its_sources() {
        let snapshot = snapshot(
            Path::new("/site"),
            [
                PathBuf::from("/site/content/post.typ"),
                PathBuf::from("/site/content-other/post.typ"),
            ],
            Path::new("/site/program/site.typ"),
            Vec::new(),
            Vec::new(),
        );

        let content_decision = RebuildDecision::for_paths(
            Some(&snapshot),
            &[
                PathBuf::from("/site/content"),
                PathBuf::from("/site/content/post.typ"),
                PathBuf::from("/site/content"),
            ],
            false,
        );
        assert_eq!(content_decision.source_analysis, RebuildPriority::Direct);
        assert_eq!(content_decision.site_program, RebuildPriority::Affected);
        assert_eq!(
            content_decision.direct_readers,
            vec![TypstDependencyReader::ContentSource(PathBuf::from(
                "/site/content/post.typ"
            )),]
        );

        let site_decision =
            RebuildDecision::for_paths(Some(&snapshot), &[PathBuf::from("/site/program")], false);
        assert_eq!(site_decision.site_program, RebuildPriority::Direct);
    }

    #[test]
    fn analysis_change_reaches_the_site_program() {
        let snapshot = snapshot(
            Path::new("/site"),
            [PathBuf::from("/site/content/post.typ")],
            Path::new("/site/program.typ"),
            vec![evidence("templates/document.typ")],
            vec![evidence("site.typ")],
        );
        let decision = RebuildDecision::for_paths(
            Some(&snapshot),
            &[PathBuf::from("/site/templates/document.typ")],
            false,
        );
        assert_eq!(decision.source_analysis, RebuildPriority::Affected);
        assert_eq!(decision.site_program, RebuildPriority::Affected);
        assert!(decision.direct_readers.is_empty());
        assert_eq!(
            decision.affected_readers,
            vec![
                TypstDependencyReader::ContentSource(PathBuf::from("/site/content/post.typ")),
                TypstDependencyReader::SiteProgram,
            ]
        );
    }

    #[test]
    fn source_set_membership_change_rebuilds_it() {
        let decision = |inventory: &[&str], changed: &str| {
            let snapshot = snapshot(
                Path::new("/site"),
                inventory.iter().map(|path| PathBuf::from(*path)),
                Path::new("/site/program.typ"),
                Vec::new(),
                Vec::new(),
            );
            RebuildDecision::for_paths(Some(&snapshot), &[PathBuf::from(changed)], true)
        };

        let added = decision(&["/site/content/a.typ"], "/site/content/b.typ");
        assert_eq!(added.source_analysis, RebuildPriority::Direct);
        assert_eq!(added.site_program, RebuildPriority::Affected);
        assert!(added.direct_readers.is_empty());
        assert!(added.affected_readers.is_empty());

        let deleted = decision(
            &["/site/content/a.typ", "/site/content/b.typ"],
            "/site/content/b.typ",
        );
        assert_eq!(deleted.source_analysis, RebuildPriority::Direct);
        assert_eq!(deleted.site_program, RebuildPriority::Affected);
        assert_eq!(
            deleted.direct_readers,
            vec![TypstDependencyReader::ContentSource(PathBuf::from(
                "/site/content/b.typ"
            ))]
        );
        assert_eq!(
            deleted.affected_readers,
            vec![TypstDependencyReader::SiteProgram]
        );
    }

    #[test]
    fn root_read_event_selects_the_program() {
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

        let background = RebuildDecision::for_paths(Some(&snapshot), &[], false);
        assert!(background.reuses_site_program());
        let changed = RebuildDecision::for_paths(Some(&snapshot), &[template], false);
        assert_eq!(changed.site_program, RebuildPriority::Affected);
    }

    #[test]
    fn mutation_makes_physical_reads_stale() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let template = root.join("template.typ");
        std::fs::write(&template, "#let helper = 1").unwrap();
        let snapshot = snapshot(
            root,
            std::iter::empty(),
            &root.join("site.typ"),
            vec![ReadEvidence::new(
                ReadLocator::ProvidedRoot("volatile.typ".into()),
                b"virtual",
            )],
            vec![ReadEvidence::new(
                ReadLocator::Root("template.typ".into()),
                b"#let helper = 1",
            )],
        );

        assert!(
            snapshot
                .physical_reads_are_fresh(
                    &tola_typst::SourceBoundary::default(),
                    &BuildCancellation::default()
                )
                .unwrap()
        );
        let decision = RebuildDecision::for_paths(Some(&snapshot), &[], false);
        assert_eq!(decision.source_analysis, RebuildPriority::Affected);

        std::fs::write(&template, "#let helper = 2").unwrap();
        assert!(
            !snapshot
                .physical_reads_are_fresh(
                    &tola_typst::SourceBoundary::default(),
                    &BuildCancellation::default()
                )
                .unwrap()
        );
        std::fs::remove_file(template).unwrap();
        assert!(
            !snapshot
                .physical_reads_are_fresh(
                    &tola_typst::SourceBoundary::default(),
                    &BuildCancellation::default()
                )
                .unwrap()
        );
    }

    #[test]
    fn package_check_notices_better_root() {
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
        let prepared = store.prepare(&package).unwrap();
        let checks = prepared.checks();

        assert!(checks.iter().all(package_check_is_fresh));
        std::fs::create_dir_all(data.join("preview/demo/1.0.0")).unwrap();
        assert!(!checks.iter().all(package_check_is_fresh));
    }

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
        let program_unit = TypstDependencyReader::SiteProgram;
        let reader_evidence = [
            (
                source_unit.clone(),
                DependencyReaderEvidence {
                    reads: Vec::new(),
                    package_checks: vec![first.clone()],
                },
            ),
            (
                program_unit.clone(),
                DependencyReaderEvidence {
                    reads: Vec::new(),
                    package_checks: vec![second.clone()],
                },
            ),
        ]
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
        let package_checks = vec![first.clone(), second];
        let previous = retained_dependencies(
            root,
            std::iter::empty::<PathBuf>(),
            std::iter::empty::<PathBuf>(),
            reader_evidence,
            package_checks,
        );
        let config = crate::config::tests::load_test_config(root, "");
        let host = compiler_host(&config).unwrap();

        let published = PublishedDependencies::new(
            root,
            std::iter::empty(),
            std::iter::empty(),
            [
                (source_unit.clone(), Vec::new(), vec![first.clone()]),
                (program_unit, Vec::new(), Vec::new()),
            ],
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

    #[cfg(unix)]
    #[test]
    fn package_check_notices_symlink_retarget() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let data = root.join("package-data");
        let first = root.join("first");
        let second = root.join("second");
        std::fs::create_dir_all(data.join("preview/demo")).unwrap();
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let selected = data.join("preview/demo/1.0.0");
        symlink(&first, &selected).unwrap();
        let locations =
            tola_typst::PackageLocations::from_absolute_roots(Some(data), None).unwrap();
        let store =
            tola_typst::PackageStore::new(locations, tola_typst::PackageFetchPolicy::LocalOnly);
        let package = "@preview/demo:1.0.0".parse().unwrap();
        let prepared = store.prepare(&package).unwrap();
        let checks = prepared.checks();

        assert!(checks.iter().all(package_check_is_fresh));
        std::fs::remove_file(&selected).unwrap();
        symlink(&second, &selected).unwrap();
        assert!(!checks.iter().all(package_check_is_fresh));
    }

    #[test]
    fn every_physical_read_gates_the_write() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let source = root.join("source-dependency.typ");
        let program = root.join("program-dependency.typ");
        std::fs::write(&source, "source").unwrap();
        std::fs::write(&program, "program").unwrap();
        let source_unit = TypstDependencyReader::ContentSource(root.join("content/post.typ"));
        let evidence = || {
            [
                (
                    source_unit.clone(),
                    vec![ReadEvidence::new(
                        ReadLocator::Root("source-dependency.typ".into()),
                        b"source",
                    )],
                ),
                (
                    TypstDependencyReader::SiteProgram,
                    vec![ReadEvidence::new(
                        ReadLocator::Root("program-dependency.typ".into()),
                        b"program",
                    )],
                ),
            ]
        };
        let published = dependencies(
            root,
            [root.join("content/post.typ")],
            &root.join("site.typ"),
            evidence(),
        );

        assert!(
            published
                .physical_reads_are_fresh(
                    &tola_typst::SourceBoundary::default(),
                    &BuildCancellation::default()
                )
                .unwrap()
        );

        let shared = published.clone();
        assert!(Arc::ptr_eq(&shared.0, &published.0));

        std::fs::write(&source, "changed source").unwrap();
        assert!(
            !shared
                .physical_reads_are_fresh(
                    &tola_typst::SourceBoundary::default(),
                    &BuildCancellation::default()
                )
                .unwrap()
        );
        std::fs::write(&source, "source").unwrap();
        std::fs::write(&program, "changed program").unwrap();
        assert!(
            !published
                .physical_reads_are_fresh(
                    &tola_typst::SourceBoundary::default(),
                    &BuildCancellation::default()
                )
                .unwrap()
        );
    }

    #[test]
    fn conflicting_bytes_make_reads_stale() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let template = root.join("template.typ");
        std::fs::write(&template, "first").unwrap();
        let snapshot = snapshot(
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
            !snapshot
                .physical_reads_are_fresh(
                    &tola_typst::SourceBoundary::default(),
                    &BuildCancellation::default()
                )
                .unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_retarget_alone_is_stale() {
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

        assert!(
            snapshot
                .physical_reads_are_fresh(
                    &tola_typst::SourceBoundary::default(),
                    &BuildCancellation::default()
                )
                .unwrap()
        );
        let watched = snapshot.physical_read_paths();
        let normalized_root = crate::filesystem::normalize_path(root);
        assert!(watched.contains(&normalized_root.join("template.typ")));
        assert!(watched.contains(&std::fs::canonicalize(&first).unwrap()));

        std::fs::remove_file(&logical).unwrap();
        symlink(&second, &logical).unwrap();
        assert!(
            !snapshot
                .physical_reads_are_fresh(
                    &tola_typst::SourceBoundary::default(),
                    &BuildCancellation::default()
                )
                .unwrap()
        );
    }

    #[test]
    fn shared_reads_yield_one_watch_path() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let template = root.join("template.typ");
        std::fs::write(&template, "#let helper = 1").unwrap();
        let read =
            || ReadEvidence::new(ReadLocator::Root("template.typ".into()), b"#let helper = 1");
        let snapshot = snapshot(
            root,
            std::iter::empty(),
            &root.join("site.typ"),
            vec![read()],
            vec![read()],
        );

        assert_eq!(
            snapshot.physical_read_paths(),
            vec![crate::filesystem::normalize_path(&template)]
        );
    }

    #[test]
    fn icon_read_matches_current_bytes() {
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
        assert!(RebuildDecision::for_paths(Some(&dependencies), &[], false).reuses_site_program());
        assert!(
            dependencies
                .virtual_reads_match(&original_host, &cancellation)
                .unwrap()
        );
        assert!(
            dependencies
                .virtual_reads_match(&host.with_icons(collections("red", "green")), &cancellation)
                .unwrap()
        );
        assert!(
            !dependencies
                .virtual_reads_match(&host.with_icons(collections("tan", "blue")), &cancellation)
                .unwrap()
        );
        assert!(
            !dependencies
                .virtual_reads_match(&host.with_icons(Arc::default()), &cancellation)
                .unwrap()
        );
    }

    #[test]
    fn unstable_virtual_reads_force_rebuild() {
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
            .collect::<std::collections::BTreeMap<_, _>>();
            retained_dependencies(
                config.get_root(),
                std::iter::empty::<PathBuf>(),
                [config.get_root().join("site.typ")],
                reader_evidence,
                Vec::new(),
            )
        };

        let priority = |snapshot: PublishedDependencies| {
            RebuildDecision::for_paths(Some(&snapshot), &[], false).site_program
        };
        assert_eq!(
            priority(snapshot(ReadLocator::ProvidedPackage {
                package: "@tola/document:0.0.0".parse().unwrap(),
                path: "lib.typ".into(),
            })),
            RebuildPriority::Unchanged
        );
        // Every source observation belongs to `@tola/source`: the package-wide capability
        // read and the per-file lexical identity read. Both are immutable descriptors, so
        // neither forces its producer to rebuild.
        assert_eq!(
            priority(snapshot(ReadLocator::ProvidedPackage {
                package: tola_packages::TolaPackage::Source.spec(),
                path: ".tola-capability".into(),
            })),
            RebuildPriority::Unchanged
        );
        assert_eq!(
            priority(snapshot(ReadLocator::ProvidedPackage {
                package: tola_packages::TolaPackage::Source.spec(),
                path: ".tola-source/content/document.typ".into(),
            })),
            RebuildPriority::Unchanged
        );
        // The same path under `@tola/document` is not an observation, so it is an ordinary
        // unstable virtual read. Ownership of the descriptor is what makes the difference,
        // and an import-only package file is not a source observation either.
        assert_eq!(
            priority(snapshot(ReadLocator::ProvidedPackage {
                package: tola_packages::TolaPackage::Document.spec(),
                path: ".tola-source/content/document.typ".into(),
            })),
            RebuildPriority::Affected
        );
        assert_eq!(
            priority(snapshot(ReadLocator::ProvidedPackage {
                package: tola_packages::TolaPackage::Document.spec(),
                path: "lib.typ".into(),
            })),
            RebuildPriority::Unchanged
        );
        assert_eq!(
            priority(snapshot(ReadLocator::ProvidedRoot("internal.typ".into()))),
            RebuildPriority::Affected
        );
        assert_eq!(
            priority(snapshot(ReadLocator::ProvidedRoot("unknown.typ".into()))),
            RebuildPriority::Affected
        );
        assert_eq!(
            priority(snapshot(ReadLocator::NonPersistent("<generated>".into()))),
            RebuildPriority::Affected
        );
    }
}
