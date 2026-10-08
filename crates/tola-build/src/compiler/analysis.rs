//! Source analysis: evaluate every content source's metadata repeatedly until the set stops
//! changing, and keep the result indexed for later reuse.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use typst::World;

use crate::compiler::TypstHost;
use crate::config::ResolvedSiteConfig;
use crate::content::{ContentSource, ContentUnit, SourceFileInput, SourceSet, SourcesInput};

// Compact the source-entry overlay after roughly one eighth of the sources change.
const SOURCE_ENTRY_OVERLAY_DIVISOR: usize = 8;
const MIN_SOURCE_ENTRY_OVERLAY: usize = 8;
const MAX_SOURCE_ENTRY_OVERLAY: usize = 128;

/// Rounds every metadata evaluation gets, regardless of source count.
///
/// A source may read another source's metadata through `all-sources()`, and a round propagates that
/// dependency one hop, so an acyclic chain of `n` sources needs `n` rounds plus one to confirm the
/// fixed point. The floor keeps a site whose chain is longer than it is wide from being cut off.
const MIN_METADATA_EVALUATION_ROUNDS: usize = 16;

pub(crate) struct SourceScan {
    pub(crate) snapshot: Arc<tola_typst::SourceSnapshot>,
    pub(crate) candidate_files: Arc<tola_typst::FileSnapshot>,
    pub(crate) source_set: SourceSet,
    pub(crate) diagnostics: Arc<tola_typst::Diagnostics>,
    declaration_warnings: Arc<[crate::diagnostic::Diagnostic]>,
    dependency_reads: BTreeMap<PathBuf, Vec<tola_typst::FileRead>>,
    entries: SourceEntryIndex,
    reused_sources: Arc<[PathBuf]>,
    content_identity: Arc<[ContentIdentity]>,
    sources_identity: SourcesInput,
    package_bindings: crate::package::SiteBindings,
    metadata_rounds: usize,
}

/// A source analysis that did not settle, with the world its sources resolve against.
///
/// The world holds the candidate snapshot, imports, packages, and fonts the analysis read, so an
/// editor still answers about the files those sources name after the analysis stopped.
pub(crate) struct SourceAnalysisFailure {
    /// The world the analyzed sources resolve against, absent when it could not be built.
    pub(crate) world: Option<Arc<tola_typst::TypstWorld>>,
    /// The failure the caller reports.
    pub(crate) error: anyhow::Error,
}

impl SourceAnalysisFailure {
    /// The failure a caller that needs a settled analysis reports.
    pub(crate) fn into_error(self) -> anyhow::Error {
        self.error
    }
}

impl std::fmt::Debug for SourceAnalysisFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SourceAnalysisFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct SourceAnalysisErrors {
    errors: Vec<(PathBuf, tola_typst::CompileError)>,
}

impl std::fmt::Display for SourceAnalysisErrors {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "source analysis failed for {} {}",
            self.errors.len(),
            if self.errors.len() == 1 {
                "file"
            } else {
                "files"
            }
        )
    }
}

impl std::error::Error for SourceAnalysisErrors {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.errors
            .first()
            .map(|(_, error)| error as &(dyn std::error::Error + 'static))
    }
}

struct SourceAnalysisEntry {
    id: crate::content::ContentId,
    layout: crate::content::ContentSourceLayout,
    source: PathBuf,
    evidence: Vec<tola_typst::FileRead>,
    package_checks: Vec<tola_typst::PackageCheck>,
    diagnostics: tola_typst::Diagnostics,
    package_bindings: crate::package::SiteBindings,
    sources_input: Option<SourcesInput>,
    source_reads: Vec<(typst::syntax::RootedPath, Option<Arc<SourceFileInput>>)>,
    declaration: Option<crate::content::SourceMetadataDeclaration>,
    /// Whether this source's own markup still calls the retired label spelling.
    declares_with_label: bool,
}

/// The name of the input one source-analysis scan runs under, written into its capture header.
///
/// The header's identity is returned but not compared yet: the conservative attempt memo that
/// replaces it with the digest of every input of the candidate is a later step.
const ANALYSIS_INPUT_IDENTITY: &str = "source-analysis";

type SourceEntryMap = BTreeMap<PathBuf, Arc<SourceAnalysisEntry>>;

/// Deterministic source-entry table with a shared base and bounded overlay.
/// Cloning is O(1); lookup takes at most two ordered-map lookups. Large overlays
/// are compacted into the base.
#[derive(Clone, Default)]
struct SourceEntryIndex {
    base: Arc<SourceEntryMap>,
    overlay: Arc<SourceEntryMap>,
    ordered_paths: Arc<[PathBuf]>,
}

impl SourceEntryIndex {
    fn from_ordered(entries: Vec<Arc<SourceAnalysisEntry>>, previous: Option<&Self>) -> Self {
        let Some(previous) = previous.filter(|previous| {
            previous.ordered_paths.len() == entries.len()
                && previous
                    .ordered_paths
                    .iter()
                    .zip(&entries)
                    .all(|(path, entry)| path == &entry.source)
        }) else {
            let paths = entries
                .iter()
                .map(|entry| entry.source.clone())
                .collect::<Vec<_>>();
            let base = entries
                .into_iter()
                .map(|entry| (entry.source.clone(), entry))
                .collect();
            return Self {
                base: Arc::new(base),
                overlay: Arc::default(),
                ordered_paths: paths.into(),
            };
        };

        let mut overlay = previous.overlay.as_ref().clone();
        for entry in entries {
            let path = entry.source.as_path();
            if previous
                .get(path)
                .is_some_and(|cached| Arc::ptr_eq(cached, &entry))
            {
                continue;
            }
            if previous
                .base
                .get(path)
                .is_some_and(|base| Arc::ptr_eq(base, &entry))
            {
                overlay.remove(path);
            } else {
                overlay.insert(entry.source.clone(), entry);
            }
        }

        let overlay_limit = (previous.ordered_paths.len() / SOURCE_ENTRY_OVERLAY_DIVISOR)
            .clamp(MIN_SOURCE_ENTRY_OVERLAY, MAX_SOURCE_ENTRY_OVERLAY);
        if overlay.len() > overlay_limit {
            let base = previous
                .ordered_paths
                .iter()
                .map(|path| {
                    (
                        path.clone(),
                        overlay
                            .get(path)
                            .or_else(|| previous.base.get(path))
                            .expect("source entry exists in base or overlay")
                            .clone(),
                    )
                })
                .collect();
            Self {
                base: Arc::new(base),
                overlay: Arc::default(),
                ordered_paths: Arc::clone(&previous.ordered_paths),
            }
        } else {
            Self {
                base: Arc::clone(&previous.base),
                overlay: Arc::new(overlay),
                ordered_paths: Arc::clone(&previous.ordered_paths),
            }
        }
    }

    fn get(&self, source: &Path) -> Option<&Arc<SourceAnalysisEntry>> {
        self.overlay.get(source).or_else(|| self.base.get(source))
    }

    fn entries(&self) -> impl Iterator<Item = &Arc<SourceAnalysisEntry>> {
        self.ordered_paths
            .iter()
            .map(|path| self.get(path).expect("ordered source entry exists"))
    }

    fn reused_from(&self, previous: Option<&Self>) -> Arc<[PathBuf]> {
        let Some(previous) = previous else {
            return Arc::default();
        };
        self.entries()
            .filter(|entry| {
                previous
                    .get(&entry.source)
                    .is_some_and(|cached| Arc::ptr_eq(cached, entry))
            })
            .map(|entry| entry.source.clone())
            .collect::<Vec<_>>()
            .into()
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ContentIdentity {
    id: crate::content::ContentId,
    root: PathBuf,
    source: PathBuf,
    layout: crate::content::ContentSourceLayout,
}

impl ContentIdentity {
    fn from_unit(unit: &ContentUnit) -> Self {
        // Reuse discovery's canonical paths; another canonicalization could
        // observe a changed filesystem and adds a syscall to each cache lookup.
        Self {
            id: unit.id.clone(),
            root: unit.root.clone(),
            source: unit.source.clone(),
            layout: unit.layout,
        }
    }

    fn matches(&self, unit: &ContentUnit) -> bool {
        self.id == unit.id
            && self.root == unit.root
            && self.source == unit.source
            && self.layout == unit.layout
    }
}

fn normalize_analysis_path(path: &Path) -> PathBuf {
    crate::filesystem::normalize_path(path)
}

enum SourceRoundResult {
    Complete {
        source: PathBuf,
        entry: Arc<SourceAnalysisEntry>,
        accessed: tola_typst::AccessedDeps,
        reused_reader: Option<crate::compiler::TypstDependencyReader>,
    },
    Failed {
        source: PathBuf,
        accessed: tola_typst::AccessedDeps,
        error: Box<tola_typst::CompileError>,
        /// The declaration the source had already written before it stopped.
        declared: Option<crate::content::SourceMetadataDeclaration>,
    },
    Cancelled {
        accessed: tola_typst::AccessedDeps,
    },
}

#[derive(Clone)]
pub(crate) struct SourceAnalysisCache {
    diagnostics: Arc<tola_typst::Diagnostics>,
    declaration_warnings: Arc<[crate::diagnostic::Diagnostic]>,
    content_identity: Arc<[ContentIdentity]>,
    sources_identity: SourcesInput,
    package_bindings: crate::package::SiteBindings,
    entries: SourceEntryIndex,
    reused_sources: Arc<[PathBuf]>,
    snapshot: Arc<tola_typst::SourceSnapshot>,
    invalidated_sources: Arc<BTreeSet<PathBuf>>,
    snapshot_refresh_paths: Arc<[PathBuf]>,
    metadata_rounds: usize,
}

#[derive(Clone, Copy, Default)]
pub(crate) enum SourceAnalysisReuse<'a> {
    #[default]
    None,
    Retained {
        cache: &'a SourceAnalysisCache,
        dependencies: &'a crate::compiler::CompilationDependencies,
    },
}

impl<'a> SourceAnalysisReuse<'a> {
    pub(crate) const fn retained(
        cache: &'a SourceAnalysisCache,
        dependencies: &'a crate::compiler::CompilationDependencies,
    ) -> Self {
        Self::Retained {
            cache,
            dependencies,
        }
    }

    const fn cache(self) -> Option<&'a SourceAnalysisCache> {
        match self {
            Self::None => None,
            Self::Retained { cache, .. } => Some(cache),
        }
    }

    pub(crate) const fn dependencies(self) -> Option<&'a crate::compiler::CompilationDependencies> {
        match self {
            Self::None => None,
            Self::Retained { dependencies, .. } => Some(dependencies),
        }
    }
}

impl SourceAnalysisCache {
    pub(crate) fn from_scan(scan: &SourceScan) -> Self {
        Self {
            diagnostics: Arc::clone(&scan.diagnostics),
            declaration_warnings: Arc::clone(&scan.declaration_warnings),
            content_identity: Arc::clone(&scan.content_identity),
            sources_identity: scan.sources_identity.clone(),
            package_bindings: scan.package_bindings.clone(),
            entries: scan.entries.clone(),
            reused_sources: Arc::clone(&scan.reused_sources),
            snapshot: Arc::clone(&scan.snapshot),
            invalidated_sources: Arc::default(),
            snapshot_refresh_paths: Arc::default(),
            metadata_rounds: scan.metadata_rounds,
        }
    }

    pub(crate) fn metadata_rounds(&self) -> usize {
        self.metadata_rounds
    }

    pub(crate) fn matches_content(
        &self,
        root: &Path,
        package_bindings: &crate::package::SiteBindings,
        content: &[ContentUnit],
        sources: &SourceSet,
    ) -> bool {
        self.snapshot.root() == root
            && self.sources_identity == *sources.inputs()
            && self.package_bindings.matches(package_bindings)
            && self.matches_content_identity(content)
    }

    pub(crate) fn diagnostics(&self) -> &tola_typst::Diagnostics {
        &self.diagnostics
    }

    /// Warnings source analysis classified itself, which keep the code they were built with.
    pub(crate) fn declaration_warnings(&self) -> &[crate::diagnostic::Diagnostic] {
        &self.declaration_warnings
    }

    fn entry_for_source(
        &self,
        source: &crate::content::ContentSource,
    ) -> Option<&Arc<SourceAnalysisEntry>> {
        self.entries
            .get(source.source())
            .filter(|entry| entry.belongs_to(source))
    }

    pub(crate) fn matches_package_bindings(
        &self,
        package_bindings: &crate::package::SiteBindings,
    ) -> bool {
        self.package_bindings.matches(package_bindings)
    }

    pub(crate) fn matches_content_identity(&self, content: &[ContentUnit]) -> bool {
        self.content_identity.len() == content.len()
            && self
                .content_identity
                .iter()
                .zip(content)
                .all(|(identity, unit)| identity.matches(unit))
    }

    pub(crate) fn prepare_for_rebuild(
        mut self,
        decision: &crate::compiler::RebuildDecision,
        site_entry: Option<&Path>,
    ) -> Self {
        self.invalidated_sources = Arc::new(
            decision
                .invalidated_content_sources()
                .map(normalize_analysis_path)
                .collect(),
        );
        let mut snapshot_refresh_paths = decision
            .directly_changed_content_sources()
            .map(normalize_analysis_path)
            .collect::<Vec<_>>();
        if decision.directly_rebuilds_site_program() {
            snapshot_refresh_paths.extend(site_entry.map(normalize_analysis_path));
        }
        snapshot_refresh_paths.sort_unstable();
        snapshot_refresh_paths.dedup();
        self.snapshot_refresh_paths = snapshot_refresh_paths.into();
        self
    }

    pub(crate) fn reused_dependency_readers(&self) -> Vec<crate::compiler::TypstDependencyReader> {
        self.reused_sources
            .iter()
            .cloned()
            .map(crate::compiler::TypstDependencyReader::ContentSource)
            .collect()
    }

    fn may_reuse_entry(&self, source: &Path) -> bool {
        !self.invalidated_sources.contains(source)
    }
}

fn content_identity(content: &[ContentUnit]) -> Arc<[ContentIdentity]> {
    content
        .iter()
        .map(ContentIdentity::from_unit)
        .collect::<Vec<_>>()
        .into()
}

impl SourceAnalysisEntry {
    fn belongs_to(&self, source: &crate::content::ContentSource) -> bool {
        self.source == source.source()
            && self.id == *source.id()
            && self.layout == source.source_layout()
    }

    fn inputs_match(
        &self,
        package_bindings: &crate::package::SiteBindings,
        sources: &SourcesInput,
    ) -> bool {
        self.package_bindings.matches(package_bindings)
            && self
                .sources_input
                .as_ref()
                .is_none_or(|cached| cached == sources)
            && self
                .source_reads
                .iter()
                .all(|(file, cached)| sources.get(file) == cached.as_ref())
    }
}

impl SourceScan {
    pub(crate) fn package_bindings(&self) -> &crate::package::SiteBindings {
        &self.package_bindings
    }

    pub(crate) fn dependency_reads(
        &self,
    ) -> Vec<(
        PathBuf,
        Vec<tola_typst::FileRead>,
        Vec<tola_typst::PackageCheck>,
    )> {
        self.dependency_reads
            .iter()
            .map(|(source, reads)| {
                let mut evidence = reads.clone();
                let mut seen = std::collections::HashSet::new();
                evidence.retain(|read| seen.insert(read.clone()));
                let checks = &self
                    .entries
                    .get(source)
                    .expect("a converged source retains its read and package evidence")
                    .package_checks;
                (source.clone(), evidence, checks.clone())
            })
            .collect()
    }
}

impl SourceAnalysisErrors {
    fn into_error(self, root: &Path, completed: &tola_typst::Diagnostics) -> anyhow::Error {
        let root = normalize_analysis_path(root);
        let mut diagnostics = Vec::new();
        let mut retain = |diagnostic| {
            if !diagnostics.contains(&diagnostic) {
                diagnostics.push(diagnostic);
            }
        };
        for diagnostic in completed {
            retain(super::diagnostic::resolved_diagnostic(
                diagnostic.clone(),
                &root,
                crate::codes::typst::COMPILE,
            ));
        }
        for (path, error) in &self.errors {
            // Source analysis only accepts sources mapped into the site root. A source that no
            // longer maps there keeps its diagnostics without a location rather than an absolute
            // path the author cannot use.
            let relative = super::diagnostic::site_relative(path, &root);
            if let Some(items) = error.diagnostics() {
                for item in items {
                    let mut diagnostic = item.clone();
                    if diagnostic.location.path.is_none() {
                        diagnostic.location.path = relative.clone();
                    }
                    retain(super::diagnostic::resolved_diagnostic(
                        diagnostic,
                        &root,
                        crate::codes::typst::COMPILE,
                    ));
                }
                continue;
            }
            if let Some(items) = super::diagnostic::compile_failure_diagnostics(error, &root) {
                for item in items {
                    retain(item);
                }
                continue;
            }
            let diagnostic = crate::diagnostic::Diagnostic::new(
                crate::codes::typst::SOURCE_ANALYSIS,
                crate::diagnostic::Severity::Error,
                "Tola could not read this source's metadata",
            )
            .with_help(
                "Rerun with `--log-file tola.log`, then report this at \
                 https://github.com/tola-rs/tola-ssg/issues",
            );
            retain(match relative {
                Some(relative) => diagnostic.with_path(relative),
                None => diagnostic,
            });
        }
        let error = anyhow::Error::new(self);
        anyhow::Error::new(crate::diagnostic::DiagnosticError::attach(
            error,
            diagnostics,
        ))
    }
}

pub(crate) fn analyze(
    config: &ResolvedSiteConfig,
    host: &TypstHost,
    content: &[ContentUnit],
    package_bindings: crate::package::SiteBindings,
    reuse: SourceAnalysisReuse<'_>,
    cancellation: &tola_typst::BundleCancellation,
    inputs: &mut crate::compiler::BuildInputs,
) -> Result<SourceScan, SourceAnalysisFailure> {
    let result = scan(
        config,
        host,
        content,
        package_bindings,
        reuse,
        cancellation,
        inputs,
    );
    result.map_err(|failure| SourceAnalysisFailure {
        world: failure.world,
        error: super::diagnostic::with_diagnostics(
            failure.error,
            config.get_root(),
            crate::codes::typst::SOURCE_ANALYSIS,
        ),
    })
}

fn scan(
    config: &ResolvedSiteConfig,
    host: &TypstHost,
    content: &[ContentUnit],
    package_bindings: crate::package::SiteBindings,
    reuse: SourceAnalysisReuse<'_>,
    cancellation: &tola_typst::BundleCancellation,
    inputs: &mut crate::compiler::BuildInputs,
) -> Result<SourceScan, SourceAnalysisFailure> {
    let mut source_set = SourceSet::without_metadata(content, config)
        .map_err(|error| SourceAnalysisFailure { world: None, error })?;
    let round_limit = source_set
        .sources()
        .len()
        .saturating_add(1)
        .max(MIN_METADATA_EVALUATION_ROUNDS);
    let sources_identity = source_set.inputs().clone();
    let entry_reuse = reuse.cache().filter(|cache| {
        cache.snapshot.root() == config.get_root()
            && cache.matches_package_bindings(&package_bindings)
    });
    let content_identity = reuse
        .cache()
        .filter(|reuse| reuse.matches_content_identity(content))
        .map(|reuse| Arc::clone(&reuse.content_identity))
        .unwrap_or_else(|| content_identity(content));
    let mut snapshot_roots = source_set
        .sources()
        .iter()
        .map(|source| source.source().to_path_buf())
        .collect::<Vec<_>>();
    snapshot_roots.push(normalize_analysis_path(&config.build.entry));
    snapshot_roots.sort_unstable();
    snapshot_roots.dedup();
    let files = host.file_resolver();
    let loaded = match entry_reuse {
        Some(reuse) => reuse.snapshot.refresh_with_files_cancellable(
            &snapshot_roots,
            &reuse.snapshot_refresh_paths,
            config.get_root(),
            Arc::clone(&files),
            cancellation,
        ),
        None => tola_typst::SourceSnapshot::build_with_files_cancellable(
            &snapshot_roots,
            config.get_root(),
            Arc::clone(&files),
            cancellation,
        ),
    };
    let loaded = match loaded {
        Ok(loaded) => loaded,
        Err(failure) => {
            inputs.record_accessed(failure.accessed());
            let error = if failure.is_cancelled() {
                anyhow::Error::new(tola_typst::CompileError::cancelled())
            } else {
                let (error, _) = failure.into_error_and_accessed();
                anyhow::Error::new(tola_typst::CompileError::from(error))
            };
            // No snapshot means no candidate files, so the failure names no world.
            return Err(SourceAnalysisFailure { world: None, error });
        }
    };
    inputs.record_accessed(loaded.accessed());
    let (snapshot, _) = loaded.into_snapshot_and_accessed();
    let snapshot = Arc::new(snapshot);
    let candidate_files = host.candidate_files(Arc::clone(&snapshot));
    // Held outside the closure so a failure still names the sources whose world it hands back.
    let mut sources_input = sources_identity.clone();
    let outcome = (|| -> anyhow::Result<SourceScan> {
        let mut history: std::collections::HashMap<u128, Vec<SourcesInput>> =
            std::collections::HashMap::new();
        let mut completed_rounds = 0usize;
        let mut previous_round_entries: Option<SourceEntryIndex> = None;
        let mut source_records = sources_input.to_source_records();

        loop {
            let library = package_bindings.library(source_records.clone());
            let round = source_set
                .sources()
                .par_iter()
                .map(|source| {
                    analyze_source(
                        config,
                        host,
                        source,
                        previous_round_entries.as_ref(),
                        entry_reuse,
                        &library,
                        &candidate_files,
                        &package_bindings,
                        &sources_input,
                        cancellation,
                    )
                })
                .collect::<Vec<_>>();

            for outcome in round.iter().filter_map(|outcome| outcome.as_ref().ok()) {
                let accessed = match outcome {
                    SourceRoundResult::Complete { accessed, .. }
                    | SourceRoundResult::Failed { accessed, .. }
                    | SourceRoundResult::Cancelled { accessed } => accessed,
                };
                inputs.record_accessed(accessed);
                if let SourceRoundResult::Complete {
                    reused_reader: Some(reader),
                    ..
                } = outcome
                {
                    reuse
                        .dependencies()
                        .expect("retained source analysis has dependency evidence")
                        .record_reader_reads(inputs, reader);
                }
            }

            let mut diagnostics = tola_typst::Diagnostics::new();
            // Only the converged round supplies publication dependencies.
            // Earlier rounds, including failures, remain recovery inputs.
            let mut dependency_reads = BTreeMap::new();
            let mut errors = Vec::new();
            let mut failed_declarations = BTreeMap::new();
            let mut has_source_dependent_errors = false;
            let mut entries = Vec::with_capacity(source_set.sources().len());
            for outcome in round {
                let outcome = outcome?;
                match outcome {
                    SourceRoundResult::Complete {
                        source,
                        entry,
                        accessed,
                        reused_reader: _,
                    } => {
                        record_reads(&mut dependency_reads, &source, accessed.reads);
                        diagnostics.extend(&entry.diagnostics);
                        entries.push(entry);
                    }
                    SourceRoundResult::Failed {
                        source,
                        accessed,
                        error,
                        declared,
                    } => {
                        has_source_dependent_errors |= reads_tola_capability(
                            &accessed.reads,
                            tola_packages::TolaPackage::Source,
                        ) || accessed.reads.iter().any(|read| {
                            crate::package::source_query_file(read.evidence().locator()).is_some()
                        });
                        if declared.is_some() {
                            failed_declarations.insert(source.clone(), declared);
                        }
                        errors.push((source, *error));
                    }
                    SourceRoundResult::Cancelled { .. } => {
                        return Err(anyhow::Error::new(tola_typst::CompileError::cancelled()));
                    }
                }
            }

            let previous_entries = previous_round_entries
                .as_ref()
                .or_else(|| entry_reuse.map(|cache| &cache.entries));
            let entries = SourceEntryIndex::from_ordered(entries, previous_entries);
            let mut declarations = entries
                .entries()
                .map(|entry| (entry.source.clone(), entry.declaration.clone()))
                .collect::<BTreeMap<_, _>>();
            // A source that stopped after declaring still reports this round's declaration.
            declarations.extend(failed_declarations);
            let next_source_set = source_set.with_metadata(&declarations);
            let next_sources_input = next_source_set.inputs().clone();
            let converged = next_sources_input == sources_input;

            // A source-dependent expression may become valid after another
            // source contributes its metadata. Retry only while that input changes.
            if !errors.is_empty() && (converged || !has_source_dependent_errors) {
                return Err(
                    SourceAnalysisErrors { errors }.into_error(config.get_root(), &diagnostics)
                );
            }

            if converged {
                let reused_sources = entries.reused_from(entry_reuse.map(|cache| &cache.entries));
                return Ok(SourceScan {
                    snapshot: Arc::clone(&snapshot),
                    candidate_files: Arc::clone(&candidate_files),
                    source_set: next_source_set,
                    diagnostics: Arc::new(diagnostics),
                    declaration_warnings: declaration_warnings(&entries, config.get_root()),
                    dependency_reads,
                    entries,
                    reused_sources,
                    content_identity: Arc::clone(&content_identity),
                    sources_identity: sources_identity.clone(),
                    package_bindings: package_bindings.clone(),
                    metadata_rounds: completed_rounds + 1,
                });
            }
            let next_source_records = next_sources_input.to_source_records();
            if history
                .get(&next_source_records.semantic_digest())
                .is_some_and(|inputs| inputs.contains(&next_sources_input))
            {
                anyhow::bail!(
                    "these sources keep changing each other's `<tola-meta>` values, so Tola could \
                     not settle the site metadata: {}",
                    changed_metadata_sources(&source_set, &next_source_set, config.get_root()),
                );
            }
            if completed_rounds + 1 >= round_limit {
                anyhow::bail!(
                    "Tola could not settle the site metadata after {round_limit} reads; these \
                     sources change their `<tola-meta>` values every time: {}",
                    changed_metadata_sources(&source_set, &next_source_set, config.get_root()),
                );
            }
            history
                .entry(source_records.semantic_digest())
                .or_default()
                .push(sources_input.clone());
            completed_rounds += 1;
            previous_round_entries = Some(entries);
            source_set = next_source_set;
            sources_input = next_sources_input;
            source_records = next_source_records;
        }
    })();
    outcome.map_err(|error| {
        let world = if crate::cancellation::is_cancelled(&error) {
            None
        } else {
            site_world(
                config,
                host,
                &package_bindings,
                &sources_input,
                &candidate_files,
                cancellation,
            )
        };
        SourceAnalysisFailure { world, error }
    })
}

/// The world one site's sources resolve against, or `None` when it could not be built.
///
/// Built from the candidate snapshot and the bindings that named those sources — the world a root
/// program compiles in — so a failure still hands an editor the imports, packages, and fonts the
/// sources resolve.
fn site_world(
    config: &ResolvedSiteConfig,
    host: &TypstHost,
    package_bindings: &crate::package::SiteBindings,
    sources: &SourcesInput,
    candidate_files: &Arc<tola_typst::FileSnapshot>,
    cancellation: &tola_typst::BundleCancellation,
) -> Option<Arc<tola_typst::TypstWorld>> {
    let library = package_bindings.library(sources.to_source_records());
    host.world(
        config.get_root(),
        &config.build.entry,
        &library,
        Arc::clone(candidate_files),
        cancellation,
    )
    .ok()
    .map(Arc::new)
}

fn changed_metadata_sources(previous: &SourceSet, next: &SourceSet, root: &Path) -> String {
    previous
        .sources()
        .iter()
        .zip(next.sources())
        .filter(|(previous, next)| previous.metadata() != next.metadata())
        .map(|(_, source)| crate::filesystem::display_path(source.source(), root))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The one warning naming every source that still declares metadata through the label.
///
/// A source lands here when this round's native channel declared nothing for it and its own
/// markup still calls the retired spelling: that source resolves with no metadata. Sources keep
/// the order of the source table, so one site reports one stable warning however many rounds it
/// ran. The note names at most three sources and counts the rest.
fn declaration_warnings(
    entries: &SourceEntryIndex,
    root: &Path,
) -> Arc<[crate::diagnostic::Diagnostic]> {
    let sources = entries
        .entries()
        .filter(|entry| entry.declaration.is_none() && entry.declares_with_label)
        .map(|entry| crate::filesystem::display_path(&entry.source, root))
        .collect::<Vec<_>>();
    if sources.is_empty() {
        return Arc::default();
    }
    let warning = crate::diagnostic::Diagnostic::new(
        crate::codes::source::DECLARATION_DEPRECATED,
        crate::diagnostic::Severity::Warning,
        "the `<tola-meta>` label does not declare source metadata",
    )
    .with_help(
        "Import `tola-meta` from `@tola/source:0.0.0` and replace \
         `metadata(...) <tola-meta>` with `tola-meta(...)`",
    )
    .with_note(crate::diagnostic::bounded_listing(
        &sources,
        ("file", "files"),
    ));
    Arc::from([warning])
}

#[allow(clippy::too_many_arguments)]
fn analyze_source(
    config: &ResolvedSiteConfig,
    host: &TypstHost,
    source: &ContentSource,
    previous_round_entries: Option<&SourceEntryIndex>,
    reuse: Option<&SourceAnalysisCache>,
    library: &tola_packages::library::SiteLibrary,
    candidate_files: &Arc<tola_typst::FileSnapshot>,
    package_bindings: &crate::package::SiteBindings,
    sources_input: &SourcesInput,
    cancellation: &tola_typst::BundleCancellation,
) -> anyhow::Result<SourceRoundResult> {
    cancellation.ensure_active().map_err(anyhow::Error::new)?;
    let source_path = source.source().to_path_buf();
    if let Some(entry) = previous_round_entries
        .and_then(|entries| entries.get(source.source()))
        .filter(|entry| {
            entry.belongs_to(source) && entry.inputs_match(package_bindings, sources_input)
        })
    {
        return Ok(SourceRoundResult::Complete {
            source: source_path.clone(),
            accessed: tola_typst::AccessedDeps {
                reads: entry.evidence.clone(),
                package_checks: entry.package_checks.clone(),
                ..tola_typst::AccessedDeps::default()
            },
            reused_reader: None,
            entry: Arc::clone(entry),
        });
    }
    if let Some((entry, accessed)) = reuse.and_then(|cache| {
        cache
            .may_reuse_entry(source.source())
            .then(|| cache.entry_for_source(source))
            .flatten()
            .and_then(|entry| {
                entry
                    .inputs_match(package_bindings, sources_input)
                    .then(|| entry_candidate_accessed(entry, candidate_files))
                    .flatten()
                    .map(|accessed| (entry, accessed))
            })
    }) {
        return Ok(SourceRoundResult::Complete {
            source: source_path.clone(),
            accessed,
            reused_reader: Some(crate::compiler::TypstDependencyReader::ContentSource(
                source_path,
            )),
            entry: Arc::clone(entry),
        });
    }

    let world = host
        .world(
            config.get_root(),
            source.source(),
            library,
            Arc::clone(candidate_files),
            cancellation,
        )
        .map_err(tola_typst::CompileError::from)
        .map_err(anyhow::Error::new)?;
    // This round's declaration is what the scan's channel has: the host writes its header
    // before evaluation, and every declaration is an event the source's own call wrote.
    let header = tola_packages::CaptureHeader::new(
        world.main(),
        ANALYSIS_INPUT_IDENTITY,
        tola_packages::CaptureMode::Collect,
    );
    let observed = tola_typst::scan_world_observed(&world, [header.value()]);
    let (captured, analyzed) = observed.into_parts();
    match analyzed {
        Ok(scan) => {
            let accessed = scan.accessed().clone();
            if cancellation.is_cancelled() {
                return Ok(SourceRoundResult::Cancelled { accessed });
            }
            let reads = accessed.reads.clone();
            let uses_sources = reads_tola_capability(&reads, tola_packages::TolaPackage::Source);
            let source_reads = reads
                .iter()
                .filter_map(|read| {
                    let file = crate::package::source_query_file(read.evidence().locator())?;
                    let cached = sources_input.get(&file).cloned();
                    Some((file, cached))
                })
                .collect();
            let declaration = match tola_packages::decode_capture(&captured, &header, scan.source())
            {
                Ok(capture) => capture.declaration,
                Err(error) => {
                    return Ok(SourceRoundResult::Failed {
                        source: source_path.clone(),
                        accessed,
                        error: Box::new(unreadable_declaration_error(&world, &scan, error)),
                        declared: None,
                    });
                }
            };
            let declaration = match declaration {
                tola_packages::Declaration::Absent => None,
                tola_packages::Declaration::One(declared) => {
                    Some(crate::content::SourceMetadataDeclaration {
                        metadata: crate::metadata::SourceMetadata::from_dict(declared.metadata),
                        range: declared.range,
                    })
                }
                tola_packages::Declaration::Duplicate(declared) => {
                    return Ok(SourceRoundResult::Failed {
                        source: source_path.clone(),
                        accessed,
                        error: Box::new(duplicate_declaration_error(&world, &scan, &declared)),
                        declared: None,
                    });
                }
            };
            let declares_with_label = declaration.is_none() && has_label_declaration(&scan);
            if cancellation.is_cancelled() {
                return Ok(SourceRoundResult::Cancelled { accessed });
            }
            Ok(SourceRoundResult::Complete {
                source: source_path.clone(),
                accessed,
                reused_reader: None,
                entry: Arc::new(SourceAnalysisEntry {
                    id: source.id().clone(),
                    layout: source.source_layout(),
                    source: source_path,
                    evidence: reads,
                    package_checks: scan.package_checks().to_vec(),
                    diagnostics: scan.diagnostics().clone(),
                    package_bindings: package_bindings.clone(),
                    sources_input: uses_sources.then(|| sources_input.clone()),
                    source_reads,
                    declaration,
                    declares_with_label,
                }),
            })
        }
        Err(failure) => {
            let (accessed, error) = failure.into_parts();
            if cancellation.is_cancelled() || error.is_cancelled() {
                return Ok(SourceRoundResult::Cancelled { accessed });
            }
            // A source that stops after declaring still contributes that declaration, so a reader
            // that was only waiting for the value can settle on the next pass.
            let declared = declared_before_failure(&captured, &header, &world);
            Ok(SourceRoundResult::Failed {
                source: source_path,
                accessed,
                error: Box::new(error),
                declared,
            })
        }
    }
}

/// Whether one source still declares metadata through the retired label spelling.
///
/// A direct outermost call in the source's own markup — drilling through parentheses — whose
/// evaluated element has the `<tola-meta>` label. The callee's name is deliberately not
/// checked, so an alias or a renamed import warns too. A declaration built inside a function body
/// or a show rule is not a direct call: it neither registers nor warns.
fn has_label_declaration(scan: &tola_typst::ScanResult) -> bool {
    use typst::syntax::ast::{self, AstNode};

    let markup = scan
        .source()
        .root()
        .cast::<ast::Markup>()
        .expect("an evaluated Typst source has a markup root");
    let mut label_calls = markup
        .exprs()
        .filter_map(|mut expression| {
            while let ast::Expr::Parenthesized(group) = expression {
                expression = group.expr();
            }
            match expression {
                ast::Expr::FuncCall(call) => Some(call.span()),
                _ => None,
            }
        })
        .collect::<HashSet<_>>();

    // Match AST positions with evaluated types. Consume each span once so eager whole-document
    // transformations cannot report one call twice.
    scan.metadata_declarations(crate::metadata::SOURCE_METADATA_LABEL)
        .into_iter()
        .any(|declaration| label_calls.remove(&declaration.span()))
}

/// The declaration one failed source had already written into its capture.
///
/// The scan's capture survives the failure, so a source that declares and then stops still hands
/// this round the dictionary it declared.
fn declared_before_failure(
    captured: &tola_typst::CapturedValues,
    header: &tola_packages::CaptureHeader,
    world: &tola_typst::TypstWorld,
) -> Option<crate::content::SourceMetadataDeclaration> {
    let Ok(source) = world.source(world.main()) else {
        return None;
    };
    let Ok(capture) = tola_packages::decode_capture(captured, header, &source) else {
        return None;
    };
    match capture.declaration {
        tola_packages::Declaration::One(declared) => {
            Some(crate::content::SourceMetadataDeclaration {
                metadata: crate::metadata::SourceMetadata::from_dict(declared.metadata),
                range: declared.range,
            })
        }
        _ => None,
    }
}

/// The failure of one source whose declaration events the protocol refuses.
///
/// A capture that does not follow the protocol refuses the whole round: the source is never read
/// as declaring nothing.
fn unreadable_declaration_error(
    world: &tola_typst::TypstWorld,
    scan: &tola_typst::ScanResult,
    error: tola_packages::CaptureError,
) -> tola_typst::CompileError {
    let diagnostic = typst::diag::SourceDiagnostic::error(
        typst::syntax::DiagSpan::from_range(scan.source().id(), 0..0),
        "Tola could not read this source's metadata declarations",
    )
    .with_hint("Rebuild the site; report this if it repeats")
    .with_hint(error.to_string());
    let mut diagnostics = tola_typst::Diagnostics::resolve(world, &[diagnostic.into()]);
    diagnostics.extend(scan.diagnostics());
    tola_typst::CompileError::Compilation { diagnostics }
}

/// The failure of one source that declares its metadata more than once.
fn duplicate_declaration_error(
    world: &tola_typst::TypstWorld,
    scan: &tola_typst::ScanResult,
    declared: &[tola_packages::DeclaredMeta],
) -> tola_typst::CompileError {
    // The second declaration is the one that made this source ambiguous.
    let range = declared
        .get(1)
        .or_else(|| declared.first())
        .map(|declared| declared.range.clone())
        .unwrap_or(0..0);
    let diagnostic = typst::diag::SourceDiagnostic::error(
        typst::syntax::DiagSpan::from_range(scan.source().id(), range),
        format!("this source declares its metadata {} times", declared.len()),
    )
    .with_hint("Keep one `#tola-meta((...))` call; a source declares once");
    let mut diagnostics = tola_typst::Diagnostics::resolve(world, &[diagnostic.into()]);
    diagnostics.extend(scan.diagnostics());
    tola_typst::CompileError::Compilation { diagnostics }
}

fn record_reads(
    reads: &mut BTreeMap<PathBuf, Vec<tola_typst::FileRead>>,
    source: &Path,
    evidence: impl IntoIterator<Item = tola_typst::FileRead>,
) {
    reads
        .entry(source.to_path_buf())
        .or_default()
        .extend(evidence);
}

fn entry_candidate_accessed(
    entry: &SourceAnalysisEntry,
    candidate: &tola_typst::FileSnapshot,
) -> Option<tola_typst::AccessedDeps> {
    let mut accessed = tola_typst::AccessedDeps::default();
    for read in &entry.evidence {
        let id = file_id_for_locator(read.evidence().locator())?;
        let observed = candidate.matching_accessed(id, read)?;
        accessed.reads.extend(observed.reads);
        accessed.disk_reads.extend(observed.disk_reads);
        accessed.package_checks.extend(observed.package_checks);
    }
    tola_typst::sort_package_checks(&mut accessed.package_checks);
    accessed.package_checks.dedup();
    (accessed.package_checks == entry.package_checks).then_some(accessed)
}

fn file_id_for_locator(locator: &tola_typst::ReadLocator) -> Option<typst::syntax::FileId> {
    use typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};

    match locator {
        tola_typst::ReadLocator::Root(path) | tola_typst::ReadLocator::ProvidedRoot(path) => {
            Some(tola_typst::file_id(path))
        }
        tola_typst::ReadLocator::Package { package, path }
        | tola_typst::ReadLocator::ProvidedPackage { package, path } => {
            let path = VirtualPath::new(path.to_str()?).ok()?;
            Some(FileId::new(RootedPath::new(
                VirtualRoot::Package(package.clone()),
                path,
            )))
        }
        tola_typst::ReadLocator::NonPersistent(_) => None,
    }
}

fn reads_tola_capability(
    evidence: &[tola_typst::FileRead],
    expected: tola_packages::TolaPackage,
) -> bool {
    evidence
        .iter()
        .any(|read| expected.owns_capability_read(read.evidence().locator()))
}

#[cfg(test)]
mod tests {
    use crate::compiler::tests::{compiler_host, configure_site, metadata_field, source_metadata};

    fn content_units(
        config: &crate::config::ResolvedSiteConfig,
    ) -> anyhow::Result<Vec<crate::content::ContentUnit>> {
        crate::content::discover_content_units_for_config_with_cancellation(
            config,
            &crate::cancellation::BuildCancellation::default(),
        )
    }

    use std::fs;

    use tempfile::TempDir;
    use typst::foundations::{Array, Dict, IntoValue, Value};

    use super::*;

    fn analyzed_entry<'a>(scan: &'a SourceScan, source: &Path) -> &'a Arc<SourceAnalysisEntry> {
        scan.entries
            .get(&normalize_analysis_path(source))
            .expect("analyzed entry exists")
    }

    fn cached_entry<'a>(
        cache: &'a SourceAnalysisCache,
        source: &Path,
    ) -> Option<&'a Arc<SourceAnalysisEntry>> {
        cache.entries.get(&normalize_analysis_path(source))
    }

    fn scan_default(
        config: &ResolvedSiteConfig,
        host: &TypstHost,
        content: &[ContentUnit],
        reuse: Option<&SourceAnalysisCache>,
    ) -> anyhow::Result<SourceScan> {
        let published = reuse.map(|_| {
            crate::compiler::CompilationDependencies::new(
                config.get_root(),
                content.iter().map(|unit| unit.source.clone()),
                std::iter::once(config.build.entry.clone()),
                std::iter::empty(),
                std::iter::empty(),
                host,
                None,
            )
            .unwrap()
        });
        let reuse = match (reuse, published.as_ref()) {
            (Some(cache), Some(dependencies)) => SourceAnalysisReuse::retained(cache, dependencies),
            (None, None) => SourceAnalysisReuse::None,
            _ => unreachable!("source analysis test reuse is paired"),
        };
        let mut inputs = crate::compiler::BuildInputs::default();
        scan(
            config,
            host,
            content,
            crate::package::SiteBindings::from_config(config, Default::default()),
            reuse,
            &tola_typst::BundleCancellation::default(),
            &mut inputs,
        )
        .map_err(SourceAnalysisFailure::into_error)
    }

    /// The scanned result for a site whose `content/` holds `sources`.
    fn scan_content(sources: &[(&str, &str)]) -> (TempDir, SourceScan) {
        let (directory, scan) = scan_result(sources);
        (directory, scan.unwrap())
    }

    #[test]
    fn capability_read_requires_the_owned_file() {
        let physical = tola_typst::ReadEvidence::new(
            tola_typst::ReadLocator::Package {
                package: tola_packages::TolaPackage::Source.spec(),
                path: PathBuf::from(".tola-capability"),
            },
            b"physical",
        );
        let virtual_read = tola_typst::ReadEvidence::new(
            tola_typst::ReadLocator::ProvidedPackage {
                package: tola_packages::TolaPackage::Source.spec(),
                path: PathBuf::from(".tola-capability"),
            },
            b"virtual",
        );
        let import_only = tola_typst::ReadEvidence::new(
            tola_typst::ReadLocator::ProvidedPackage {
                package: tola_packages::TolaPackage::Source.spec(),
                path: PathBuf::from("lib.typ"),
            },
            b"import-only",
        );
        let wrong_package = tola_typst::ReadEvidence::new(
            tola_typst::ReadLocator::ProvidedPackage {
                package: tola_packages::TolaPackage::Document.spec(),
                path: PathBuf::from(".tola-capability"),
            },
            b"wrong-package",
        );
        let root_read = tola_typst::ReadEvidence::new(
            tola_typst::ReadLocator::Root(PathBuf::from("sources.typ")),
            b"root",
        );

        let sources = tola_packages::TolaPackage::Source;
        assert!(sources.owns_capability_read(physical.locator()));
        assert!(sources.owns_capability_read(virtual_read.locator()));
        assert!(!sources.owns_capability_read(import_only.locator()));
        assert!(!sources.owns_capability_read(wrong_package.locator()));
        assert!(!sources.owns_capability_read(root_read.locator()));
    }

    #[test]
    fn importing_alone_does_not_bind_sources() {
        let (directory, scan) = scan_content(&[(
            "post.typ",
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: [Post]))"#,
        )]);
        let analyzed = analyzed_entry(&scan, &directory.path().join("content/post.typ"));

        assert!(analyzed.sources_input.is_none());
        assert!(analyzed.evidence.iter().any(|read| {
            matches!(
                read.evidence().locator(),
                tola_typst::ReadLocator::ProvidedPackage { package, path }
                    if tola_packages::TolaPackage::from_spec(package)
                        == Some(tola_packages::TolaPackage::Source)
                        && path == Path::new("lib.typ")
            )
        }));
        assert!(!analyzed.evidence.iter().any(|read| {
            tola_packages::TolaPackage::Source.owns_capability_read(read.evidence().locator())
        }));
    }

    #[test]
    fn package_check_records_availability() {
        for (name, installed) in [("example", true), ("missing", false)] {
            let directory = TempDir::new().unwrap();
            let root = directory.path();
            let content = root.join("content");
            let package_root = root.join("packages");
            let entry = root.join("site.typ");
            fs::create_dir_all(&content).unwrap();
            fs::write(
                content.join("post.typ"),
                format!("#import \"@local/{name}:1.0.0\": value\n#value"),
            )
            .unwrap();
            fs::write(&entry, "").unwrap();
            if installed {
                let package_dir = package_root.join("local").join(name).join("1.0.0");
                fs::create_dir_all(&package_dir).unwrap();
                fs::write(package_dir.join("lib.typ"), "#let value = [from package]").unwrap();
                fs::write(
                    package_dir.join("typst.toml"),
                    format!(
                        "[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nentrypoint = \"lib.typ\"\nauthors = [\"Tola tests\"]\n"
                    ),
                )
                .unwrap();
            }

            let mut config = crate::config::tests::load_test_config(root, "");
            configure_site(&mut config, &content, &entry);
            config.package_locations =
                tola_typst::PackageLocations::from_absolute_roots(Some(package_root.clone()), None)
                    .unwrap();
            let units = content_units(&config).unwrap();
            let host = compiler_host(&config).unwrap();
            let mut inputs = crate::compiler::BuildInputs::default();

            let outcome = scan(
                &config,
                &host,
                &units,
                crate::package::SiteBindings::from_config(&config, Default::default()),
                SourceAnalysisReuse::None,
                &tola_typst::BundleCancellation::default(),
                &mut inputs,
            );
            if installed {
                outcome.unwrap();
            } else {
                let error = outcome
                    .err()
                    .expect("missing source package must fail analysis")
                    .into_error();
                assert!(
                    error
                        .chain()
                        .any(|cause| cause.is::<tola_typst::CompileError>()),
                    "{error:#}"
                );
            }

            let (_, package_checks) = inputs.into_parts();
            assert_eq!(package_checks.len(), 1);
            let check = &package_checks[0];
            assert_eq!(check.package().to_string(), format!("@local/{name}:1.0.0"));
            assert_eq!(check.tier(), tola_typst::PackageTier::Data);
            assert_eq!(
                check.availability(),
                if installed {
                    tola_typst::PackageAvailability::Present
                } else {
                    tola_typst::PackageAvailability::Missing
                }
            );
            let candidate = package_root.join("local").join(name).join("1.0.0");
            assert_eq!(check.candidate(), candidate.as_path());
            assert_eq!(check.was_selected(), installed);
        }
    }

    #[test]
    fn invalid_metadata_keeps_its_reads() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let content = root.join("content");
        let metadata = root.join("metadata.json");
        let entry = root.join("site.typ");
        fs::create_dir_all(&content).unwrap();
        fs::write(&metadata, r#"["not a dictionary"]"#).unwrap();
        fs::write(
            content.join("post.typ"),
            r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((json("../metadata.json")))
#let d = decimal(0.1)"#,
        )
        .unwrap();
        fs::write(&entry, "").unwrap();

        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &entry);
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let mut inputs = crate::compiler::BuildInputs::default();

        let error = analyze(
            &config,
            &host,
            &units,
            crate::package::SiteBindings::from_config(&config, Default::default()),
            SourceAnalysisReuse::None,
            &tola_typst::BundleCancellation::default(),
            &mut inputs,
        )
        .err()
        .expect("invalid source metadata must fail analysis")
        .into_error();
        let paths = inputs.into_parts().0;
        let diagnostics = crate::diagnostic::attached(&error).unwrap();
        assert_eq!(
            diagnostics[0].location.as_ref().unwrap().path,
            "content/post.typ"
        );
        assert_eq!(diagnostics[0].location.as_ref().unwrap().line, Some(2));
        assert!(
            diagnostics[0].message.contains("dictionary"),
            "{diagnostics:#?}"
        );
        // The declaration fails before the later `decimal(0.1)` runs, so no float warning follows.
        assert!(
            paths.contains(&crate::filesystem::normalize_path(&metadata)),
            "{paths:#?}"
        );
    }

    fn prepare_cache_for_changes(
        cache: SourceAnalysisCache,
        config: &ResolvedSiteConfig,
        content: &[ContentUnit],
        reads: Vec<(
            PathBuf,
            Vec<tola_typst::FileRead>,
            Vec<tola_typst::PackageCheck>,
        )>,
        changed_paths: &[PathBuf],
        inventory_may_change: bool,
    ) -> SourceAnalysisCache {
        let host = compiler_host(config).unwrap();
        let package_checks = reads
            .iter()
            .flat_map(|(_, _, checks)| checks.iter().cloned());
        let dependencies = crate::compiler::CompilationDependencies::new(
            config.get_root(),
            content.iter().map(|unit| unit.source.clone()),
            std::iter::once(config.build.entry.clone()),
            reads.iter().map(|(source, reads, checks)| {
                (
                    crate::compiler::TypstDependencyReader::ContentSource(source.clone()),
                    reads.clone(),
                    checks.clone(),
                )
            }),
            package_checks,
            &host,
            None,
        )
        .unwrap();
        let decision = crate::compiler::RebuildDecision::for_paths(
            Some(&dependencies),
            changed_paths,
            inventory_may_change,
        );
        cache.prepare_for_rebuild(&decision, Some(&config.build.entry))
    }

    #[test]
    fn single_source_change_replaces_one_entry() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        for index in 0..64 {
            fs::write(content.join(format!("post-{index:04}.typ")), "Plain").unwrap();
        }
        let changed_source = content.join("post-0000.typ");
        let entry = root.join("site.typ");
        fs::write(&entry, "").unwrap();

        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &entry);
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let first = scan_default(&config, &host, &units, None).unwrap();
        let first_changed = Arc::clone(analyzed_entry(&first, &changed_source));
        let reads = first.dependency_reads();
        let cache = SourceAnalysisCache::from_scan(&first);

        fs::write(&changed_source, "Changed").unwrap();
        let prepared = prepare_cache_for_changes(
            cache,
            &config,
            &units,
            reads,
            std::slice::from_ref(&changed_source),
            false,
        );
        let published_base = Arc::clone(&prepared.entries.base);

        let changed = scan_default(&config, &host, &units, Some(&prepared)).unwrap();

        assert!(!Arc::ptr_eq(
            analyzed_entry(&changed, &changed_source),
            &first_changed
        ));
        assert!(Arc::ptr_eq(&published_base, &changed.entries.base));
        assert_eq!(changed.entries.overlay.len(), 1);
    }

    #[test]
    fn open_metadata_stays_native_content() {
        let (_directory, result) = scan_content(&[(
            "post/index.typ",
            r#"#set text(fill: red)
#import "@tola/source:0.0.0": tola-meta
#tola-meta((
  title: [Post],
  summary: [这是 #strike[旧内容]、*重点*，请看 #link("/new/")[新方案]。],
  pinned: true,
))"#,
        )]);

        assert_eq!(
            metadata_field(&result, "index.typ", "pinned"),
            true.into_value()
        );
        let typst::foundations::Value::Content(summary) =
            metadata_field(&result, "index.typ", "summary")
        else {
            panic!("source summary must retain native content");
        };
        assert_eq!(summary.plain_text(), "这是 旧内容、重点，请看 新方案。");
    }

    #[test]
    fn each_source_owns_its_declaration() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let content = root.join("content");
        fs::create_dir(&content).unwrap();
        fs::write(
            root.join("values.typ"),
            "#let attributes(title) = (title: title, pin: true)",
        )
        .unwrap();
        fs::write(
            content.join("child.typ"),
            r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: [Child]))
Child body"#,
        )
        .unwrap();
        fs::write(
            content.join("parent.typ"),
            r#"#import "../values.typ": attributes
#import "@tola/source:0.0.0": tola-meta
#tola-meta((attributes([Parent])))
#include "child.typ""#,
        )
        .unwrap();
        let entry = root.join("site.typ");
        fs::write(&entry, "").unwrap();
        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &entry);
        let units = content_units(&config).unwrap();
        let scanned =
            scan_default(&config, &compiler_host(&config).unwrap(), &units, None).unwrap();
        for (filename, title) in [("child.typ", "Child"), ("parent.typ", "Parent")] {
            let typst::foundations::Value::Content(content) =
                metadata_field(&scanned, filename, "title")
            else {
                panic!("title must remain native content");
            };
            assert_eq!(content.plain_text(), title);
        }
    }

    #[test]
    fn analysis_keeps_distinct_source_names() {
        let (_directory, result) = scan_content(&[("a b.typ", "First"), ("a-b.typ", "Second")]);

        assert_eq!(
            result
                .source_set
                .sources()
                .iter()
                .map(|source| source.id().to_string())
                .collect::<Vec<_>>(),
            ["a b.typ", "a-b.typ"],
        );
    }

    #[test]
    fn each_source_owns_its_physical_reads() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let data = root.join("data");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&data).unwrap();
        fs::write(content.join("a.typ"), "#read(\"/data/a.txt\")").unwrap();
        fs::write(content.join("b.typ"), "#read(\"/data/b.txt\")").unwrap();
        fs::write(data.join("a.txt"), "A").unwrap();
        fs::write(data.join("b.txt"), "B").unwrap();
        fs::write(root.join("site.typ"), "").unwrap();

        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &root.join("site.typ"));
        let units = content_units(&config).unwrap();
        let scan = scan_default(&config, &compiler_host(&config).unwrap(), &units, None).unwrap();
        let reads_for = |source: &Path| {
            scan.dependency_reads()
                .into_iter()
                .find(|(path, _, _)| path == &crate::filesystem::normalize_path(source))
                .map(|(_, reads, _)| reads)
                .unwrap()
        };
        let reads_path = |reads: &[tola_typst::FileRead], expected: &str| {
            reads.iter().any(|read| {
                matches!(
                    read.evidence().locator(),
                    tola_typst::ReadLocator::Root(path) if path == Path::new(expected)
                )
            })
        };

        let a_reads = reads_for(&content.join("a.typ"));
        let b_reads = reads_for(&content.join("b.typ"));
        assert!(reads_path(&a_reads, "data/a.txt"));
        assert!(!reads_path(&a_reads, "data/b.txt"));
        assert!(reads_path(&b_reads, "data/b.txt"));
        assert!(!reads_path(&b_reads, "data/a.txt"));
    }

    #[test]
    fn only_the_reader_reanalyzes() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let data = root.join("data");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&data).unwrap();
        let a = content.join("a.typ");
        let b = content.join("b.typ");
        let plain = content.join("plain.typ");
        let a_data = data.join("a.txt");
        fs::write(&a, "#read(\"/data/a.txt\")").unwrap();
        fs::write(&b, "#read(\"/data/b.txt\")").unwrap();
        fs::write(&plain, "plain").unwrap();
        fs::write(&a_data, "A1").unwrap();
        fs::write(data.join("b.txt"), "B1").unwrap();
        fs::write(root.join("site.typ"), "").unwrap();

        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &root.join("site.typ"));
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let first = scan_default(&config, &host, &units, None).unwrap();
        let reads = first.dependency_reads();
        let cache = SourceAnalysisCache::from_scan(&first);
        let ids = [&a, &b, &plain]
            .into_iter()
            .map(|path| (path.clone(), Arc::clone(analyzed_entry(&first, path))))
            .collect::<std::collections::BTreeMap<_, _>>();

        let background_cache =
            prepare_cache_for_changes(cache.clone(), &config, &units, reads.clone(), &[], false);
        let background = scan_default(&config, &host, &units, Some(&background_cache)).unwrap();
        for (path, analyzed) in &ids {
            assert!(Arc::ptr_eq(analyzed_entry(&background, path), analyzed));
        }

        fs::write(&a_data, "A2").unwrap();
        let cache = prepare_cache_for_changes(
            cache,
            &config,
            &units,
            reads,
            std::slice::from_ref(&a_data),
            false,
        );
        let changed = scan_default(&config, &host, &units, Some(&cache)).unwrap();

        assert!(!Arc::ptr_eq(analyzed_entry(&changed, &a), &ids[&a]));
        assert!(Arc::ptr_eq(analyzed_entry(&changed, &b), &ids[&b]));
        assert!(Arc::ptr_eq(analyzed_entry(&changed, &plain), &ids[&plain]));
    }

    #[test]
    fn shared_change_rebuilds_only_readers() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let data = root.join("data");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&data).unwrap();
        let first_reader = content.join("first.typ");
        let second_reader = content.join("second.typ");
        let plain = content.join("plain.typ");
        let shared = data.join("shared.txt");
        fs::write(&first_reader, "#read(\"/data/shared.txt\")").unwrap();
        fs::write(&second_reader, "#read(\"/data/shared.txt\")").unwrap();
        fs::write(&plain, "Plain").unwrap();
        fs::write(&shared, "one").unwrap();
        fs::write(root.join("site.typ"), "").unwrap();

        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &root.join("site.typ"));
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let first = scan_default(&config, &host, &units, None).unwrap();
        let ids = [&first_reader, &second_reader, &plain]
            .into_iter()
            .map(|path| (path.clone(), Arc::clone(analyzed_entry(&first, path))))
            .collect::<BTreeMap<_, _>>();
        let reads = first.dependency_reads();
        let cache = SourceAnalysisCache::from_scan(&first);

        fs::write(&shared, "two").unwrap();
        let prepared = prepare_cache_for_changes(
            cache,
            &config,
            &units,
            reads,
            std::slice::from_ref(&shared),
            false,
        );
        let changed = scan_default(&config, &host, &units, Some(&prepared)).unwrap();

        assert!(!Arc::ptr_eq(
            analyzed_entry(&changed, &first_reader),
            &ids[&first_reader]
        ));
        assert!(!Arc::ptr_eq(
            analyzed_entry(&changed, &second_reader),
            &ids[&second_reader]
        ));
        assert!(Arc::ptr_eq(analyzed_entry(&changed, &plain), &ids[&plain]));
        let cache = SourceAnalysisCache::from_scan(&changed);
        assert_eq!(
            cache.reused_dependency_readers(),
            [crate::compiler::TypstDependencyReader::ContentSource(
                crate::filesystem::normalize_path(&plain),
            )]
            .to_vec()
        );
    }

    #[test]
    fn failed_analysis_retains_source_warnings() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        let document = content.join("document.typ");
        let other = content.join("other.typ");
        fs::write(&document, "#let d = decimal(0.1)").unwrap();
        fs::write(&other, "#let d = decimal(0.2)").unwrap();
        fs::write(root.join("site.typ"), "").unwrap();
        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &root.join("site.typ"));
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let successful = scan_default(&config, &host, &units, None).unwrap();
        let cache = SourceAnalysisCache::from_scan(&successful);
        fs::write(&document, "#let d = decimal(0.1)\n#missing").unwrap();
        let cache = prepare_cache_for_changes(
            cache,
            &config,
            &units,
            successful.dependency_reads(),
            std::slice::from_ref(&document),
            false,
        );

        for reuse in [None, Some(&cache)] {
            let error = scan_default(&config, &host, &units, reuse).err().unwrap();
            let diagnostics = crate::diagnostic::attached(&error).unwrap();
            let warnings = diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.severity == crate::diagnostic::Severity::Warning)
                .collect::<Vec<_>>();
            assert_eq!(warnings.len(), 2);
            assert_eq!(
                warnings
                    .iter()
                    .map(|warning| warning.location.as_ref().unwrap().path.as_str())
                    .collect::<BTreeSet<_>>(),
                BTreeSet::from(["content/document.typ", "content/other.typ"]),
            );
            let errors = diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.severity == crate::diagnostic::Severity::Error)
                .collect::<Vec<_>>();
            assert_eq!(errors.len(), 1);
            assert_eq!(
                errors[0].location.as_ref().unwrap().path,
                "content/document.typ"
            );
        }
    }

    #[test]
    fn reports_each_source_error() {
        let dir = TempDir::new().unwrap();
        let content = dir.path().join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("one.typ"), "#let broken =").unwrap();
        fs::write(content.join("two.typ"), "#let broken =").unwrap();
        fs::write(dir.path().join("site.typ"), "").unwrap();

        let mut config = crate::config::tests::load_test_config(dir.path(), "");
        configure_site(&mut config, &content, &dir.path().join("site.typ"));
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let error = scan_default(&config, &host, &units, None)
            .err()
            .expect("source errors unexpectedly passed analysis");
        let errors = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<SourceAnalysisErrors>())
            .expect("all source errors remain available");
        assert_eq!(errors.errors.len(), 2);
        assert_eq!(
            errors.errors[0].0,
            crate::filesystem::normalize_path(&content.join("one.typ"))
        );
        assert_eq!(
            errors.errors[1].0,
            crate::filesystem::normalize_path(&content.join("two.typ"))
        );
        let first_cause = std::error::Error::source(errors)
            .unwrap()
            .downcast_ref::<tola_typst::CompileError>()
            .unwrap();
        assert!(std::ptr::eq(first_cause, &errors.errors[0].1));
        let diagnostics =
            crate::diagnostic::attached(&error).expect("source diagnostics should remain attached");
        assert_eq!(diagnostics.len(), 2);
        let diagnostic = &diagnostics[0];
        assert_eq!(
            diagnostic
                .location
                .as_ref()
                .map(|location| location.path.as_str()),
            Some("content/one.typ")
        );
        assert!(
            error
                .chain()
                .any(|cause| cause.downcast_ref::<tola_typst::CompileError>().is_some())
        );
        assert_eq!(diagnostic.severity, crate::diagnostic::Severity::Error);
        assert_eq!(
            diagnostics[1]
                .location
                .as_ref()
                .map(|location| location.path.as_str()),
            Some("content/two.typ"),
        );

        let mut inputs = crate::compiler::BuildInputs::default();
        analyze(
            &config,
            &host,
            &units,
            crate::package::SiteBindings::from_config(&config, Default::default()),
            SourceAnalysisReuse::None,
            &tola_typst::BundleCancellation::default(),
            &mut inputs,
        )
        .err()
        .expect("source errors unexpectedly passed analysis");
        let paths = inputs.into_parts().0;
        for source in ["one.typ", "two.typ"] {
            assert!(paths.contains(&crate::filesystem::normalize_path(&content.join(source))));
        }
    }

    /// The `source_origins` dictionary the library of one scan binds, keyed by source path.
    fn source_origins_of(scan: &SourceScan) -> Dict {
        let library = scan
            .package_bindings()
            .library(scan.source_set.inputs().to_source_records());
        let shared = library.shared();
        let Value::Module(system) = shared
            .global
            .scope()
            .get("sys")
            .expect("the library defines sys")
            .read()
        else {
            panic!("sys is a module");
        };
        let Value::Dict(inputs) = system.scope().get("inputs").expect("sys has inputs").read()
        else {
            panic!("sys.inputs is a dictionary");
        };
        let Value::Dict(origins) = inputs
            .get("__tola_source_origins")
            .expect("the host binds the source origins")
        else {
            panic!("the source origins are a dictionary");
        };
        origins.clone()
    }

    fn origin_file(origin: &Dict) -> typst::syntax::RootedPath {
        origin
            .get("file")
            .expect("a source origin names its file")
            .clone()
            .cast::<typst::syntax::RootedPath>()
            .expect("a source origin's file is a path")
    }

    #[test]
    fn source_origin_records_declaration_range() {
        let declaring =
            "= Post\n\n#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Post\"))\n";
        for (file, text, call) in [
            (
                "posts/deep.typ",
                declaring,
                Some("tola-meta((title: \"Post\"))"),
            ),
            ("absent.typ", "= Absent\n", None),
        ] {
            let (_directory, scan) = scan_content(&[(file, text)]);
            let origins = source_origins_of(&scan);
            let origin = origins
                .get(file)
                .unwrap_or_else(|_| panic!("`{file}` has an origin"))
                .clone()
                .cast::<Dict>()
                .expect("a source origin is a dictionary");
            assert_eq!(
                origin_file(&origin).vpath().get_with_slash(),
                format!("/content/{file}")
            );
            match call {
                Some(call) => {
                    let bounds = origin
                        .get("range")
                        .expect("a source origin declares a range")
                        .clone()
                        .cast::<Array>()
                        .expect("a declaration range is an array")
                        .iter()
                        .map(|bound| {
                            bound
                                .clone()
                                .cast::<i64>()
                                .expect("a range bound is an integer")
                                as usize
                        })
                        .collect::<Vec<_>>();
                    let start = text.find(call).expect("the source writes the call");
                    assert_eq!(bounds, [start, start + call.len()]);
                }
                None => assert_eq!(origin.get("range").unwrap(), &Value::None),
            }
        }
    }

    #[test]
    fn deferred_metadata_is_never_declared() {
        for (name, source) in [
            (
                "contextual",
                r#"#import "@tola/document:0.0.0": current-document
#context {
  let ctx = current-document()
  [#import "@tola/source:0.0.0": tola-meta
#tola-meta((contextual: true))]
}"#,
            ),
            (
                "selector show",
                r#"#show heading: it => [#import "@tola/source:0.0.0": tola-meta
#tola-meta((selector-show: true))#it]
= Heading"#,
            ),
        ] {
            let (_directory, result) = scan_content(&[("post.typ", source)]);

            assert!(
                result.source_set.sources()[0].metadata().is_none(),
                "{name} metadata was treated as declared source metadata"
            );
        }
    }

    #[test]
    fn declared_attributes_keep_native_values() {
        let directory = TempDir::new().unwrap();
        let content = directory.path().join("content");
        fs::create_dir(&content).unwrap();
        fs::write(
            directory.path().join("theme.typ"),
            r#"#let attributes() = (
  title: [Document],
  callback: value => value,
  card: [#import "@tola/source:0.0.0": tola-meta
#tola-meta((nested: true))],
)
#let declaration() = [#import "@tola/source:0.0.0": tola-meta
#tola-meta((helper: true))]"#,
        )
        .unwrap();
        fs::write(
            content.join("post.typ"),
            r#"#import "../theme.typ": attributes, declaration
#import "@tola/source:0.0.0": tola-meta
#tola-meta(attributes())
#let local() = [#metadata((local: true)) <tola-meta>]
#declaration()
#local()
#[#metadata((nested: true)) <tola-meta>]
#metadata([#metadata((stored: true)) <tola-meta>]) <cache>
#context [#metadata((contextual: true)) <tola-meta>]
= Body"#,
        )
        .unwrap();
        fs::write(directory.path().join("site.typ"), "").unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let units = content_units(&config).unwrap();

        let scanned =
            scan_default(&config, &compiler_host(&config).unwrap(), &units, None).unwrap();

        let fields = source_metadata(&scanned, "post.typ");
        assert_eq!(
            fields.get("title").unwrap(),
            &typst::text::TextElem::packed("Document").into_value()
        );
        assert!(matches!(
            fields.get("callback").unwrap(),
            typst::foundations::Value::Func(_)
        ));
        assert!(matches!(
            fields.get("card").unwrap(),
            typst::foundations::Value::Content(_)
        ));
    }

    /// One site whose `content/` holds `sources`, and the scan of it, keeping a failure to assert.
    fn scan_result(sources: &[(&str, &str)]) -> (TempDir, anyhow::Result<SourceScan>) {
        let directory = TempDir::new().unwrap();
        let content = directory.path().join("content");
        fs::create_dir_all(&content).unwrap();
        for (name, source) in sources {
            let path = content.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source).unwrap();
        }
        let entry = directory.path().join("site.typ");
        fs::write(&entry, "").unwrap();

        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        configure_site(&mut config, &content, &entry);
        let units = content_units(&config).unwrap();
        (
            directory,
            scan_default(&config, &compiler_host(&config).unwrap(), &units, None),
        )
    }

    /// The rendered error of a scan the case expected to fail.
    fn failure(scan: anyhow::Result<SourceScan>) -> String {
        match scan {
            Ok(_) => panic!("the scan was expected to fail"),
            Err(error) => format!("{error:#}"),
        }
    }

    #[test]
    fn declared_body_still_runs() {
        let source = "#import \"@tola/source:0.0.0\": tola-meta\n\
                      #tola-meta((title: \"Document\", draft: true))\n\
                      #panic(\"body must be evaluated\")";
        let directory = TempDir::new().unwrap();
        let content = directory.path().join("content");
        fs::create_dir(&content).unwrap();
        fs::write(content.join("post.typ"), source).unwrap();
        fs::write(directory.path().join("site.typ"), "").unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();

        let rendered = failure(scan_default(&config, &host, &units, None));

        assert!(rendered.contains("body must be evaluated"), "{rendered}");
    }

    #[test]
    fn native_declaration_registers_metadata() {
        let (directory, scan) = scan_content(&[(
            "post.typ",
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Post\"))\n= Post",
        )]);

        let entry = analyzed_entry(&scan, &directory.path().join("content/post.typ"));
        let declaration = entry.declaration.as_ref().expect("the call declared");
        assert_eq!(
            declaration.metadata.to_typst_dict().get("title").unwrap(),
            &Value::Str("Post".into())
        );
        let range = declaration.range.clone();
        let text = fs::read_to_string(directory.path().join("content/post.typ")).unwrap();
        assert_eq!(&text[range], "tola-meta((title: \"Post\"))");
        assert!(scan.declaration_warnings.is_empty());
    }

    #[test]
    fn label_warning_identifies_sources() {
        for names in [&["alpha", "beta"][..], &["a", "b", "c", "d", "e"][..]] {
            let sources = names
                .iter()
                .map(|name| {
                    (
                        format!("{name}.typ"),
                        format!("#metadata((title: \"{name}\")) <tola-meta>"),
                    )
                })
                .collect::<Vec<_>>();
            let sources = sources
                .iter()
                .map(|(name, source)| (name.as_str(), source.as_str()))
                .collect::<Vec<_>>();
            let (_directory, scan) = scan_content(&sources);

            assert_eq!(scan.declaration_warnings.len(), 1);
            let warning = &scan.declaration_warnings[0];
            assert_eq!(warning.code, crate::codes::source::DECLARATION_DEPRECATED);
            for (index, name) in names.iter().enumerate() {
                assert_eq!(
                    warning.notes[0].contains(&format!("content/{name}.typ")),
                    index < 3,
                );
            }
        }
    }

    #[test]
    fn retried_analysis_warns_once() {
        let (_directory, scan) = scan_content(&[
            (
                "alpha.typ",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Alpha\"))",
            ),
            (
                "beta.typ",
                "#metadata((title: \"Beta\")) <tola-meta>\n= Beta",
            ),
        ]);

        assert!(scan.metadata_rounds > 1, "alpha declares in a later round");
        assert_eq!(scan.declaration_warnings.len(), 1);
        assert!(scan.declaration_warnings[0].notes[0].contains("content/beta.typ"));
    }

    #[test]
    fn repeated_metadata_declarations_fail() {
        let (_directory, scan) = scan_result(&[(
            "post.typ",
            "#import \"@tola/source:0.0.0\": tola-meta\n\
             #tola-meta((first: true))\n\
             #tola-meta((second: true))",
        )]);

        let error = scan
            .err()
            .expect("duplicate source metadata unexpectedly succeeded");

        assert!(
            format!("{error:#}").contains("this source declares its metadata 2 times"),
            "{error:#}"
        );
        let diagnostics = crate::diagnostic::attached(&error).unwrap();
        assert_eq!(diagnostics[0].location.as_ref().unwrap().line, Some(3));
    }

    #[test]
    fn saturated_declarations_fail_closed() {
        let mut source = String::from("#import \"@tola/source:0.0.0\": tola-meta\n");
        for index in 0..10 {
            source.push_str(&format!("#tola-meta((index: {index}))\n"));
        }
        let (_directory, scan) = scan_result(&[("post.typ", &source)]);

        let rendered = failure(scan);

        assert!(
            rendered.contains("could not read this source's metadata declarations"),
            "{rendered}"
        );
    }

    #[test]
    fn dependent_declaration_settles_after_retry() {
        let (_directory, scan) = scan_content(&[
            (
                "alpha.typ",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: [Alpha]))\n= Alpha",
            ),
            (
                "beta.typ",
                "#import \"@tola/source:0.0.0\": all-sources, tola-meta\n\
                 #let alpha = all-sources().find(source => source.id == \"alpha.typ\")\n\
                 #tola-meta((title: alpha.meta.title))\n\
                 = Beta",
            ),
        ]);

        assert!(
            scan.metadata_rounds > 1,
            "beta.typ fails until alpha's metadata arrives"
        );
        assert_eq!(
            metadata_field(&scan, "beta.typ", "title"),
            typst::text::TextElem::packed("Alpha").into_value()
        );
    }

    /// The read-only detector's verdict for every shape a declaration can be written in.
    ///
    /// A renamed import (`#import …: metadata as m`) is the same shape as the aliased binding
    /// below: detection is by evaluated element and call site, never by the callee's name.
    #[test]
    fn declaration_shapes_warn_by_call_site() {
        for (name, sources, warns) in [
            (
                "plain label",
                &[("post.typ", "#metadata((title: \"A\")) <tola-meta>")][..],
                true,
            ),
            (
                "parenthesized",
                &[("post.typ", "#(metadata((title: \"A\"))) <tola-meta>")][..],
                true,
            ),
            (
                "aliased binding",
                &[(
                    "post.typ",
                    "#let m = metadata\n#m((title: \"A\")) <tola-meta>",
                )][..],
                true,
            ),
            (
                "shadowed native",
                &[(
                    "post.typ",
                    "#let tola-meta = metadata\n#tola-meta((title: \"A\")) <tola-meta>",
                )][..],
                true,
            ),
            (
                "wrapper body",
                &[(
                    "post.typ",
                    "#let decl(d) = metadata(d)\n#decl((title: \"A\")) <tola-meta>",
                )][..],
                false,
            ),
            (
                "imported helper",
                &[
                    ("helper.typ", "#let declare(d) = metadata(d)"),
                    (
                        "post.typ",
                        "#import \"helper.typ\": declare\n#declare((title: \"A\")) <tola-meta>",
                    ),
                ][..],
                false,
            ),
            (
                "local wrapper",
                &[(
                    "post.typ",
                    "#let wrap(value) = [#metadata(value) <tola-meta>]\n#wrap((title: \"A\"))",
                )][..],
                false,
            ),
            (
                "show rule",
                &[(
                    "post.typ",
                    "#show heading: it => [#metadata((show: true)) <tola-meta>#it]\n= Heading",
                )][..],
                false,
            ),
            (
                "placed binding",
                &[(
                    "post.typ",
                    "#let marker = metadata((title: \"A\"))\n#marker <tola-meta>",
                )][..],
                false,
            ),
        ] {
            let (directory, scan) = scan_content(sources);

            let entry = analyzed_entry(&scan, &directory.path().join("content/post.typ"));
            assert!(entry.declaration.is_none(), "{name} declares no metadata");
            assert_eq!(
                scan.declaration_warnings.len(),
                usize::from(warns),
                "{name}"
            );
            if warns {
                let warning = &scan.declaration_warnings[0];
                assert_eq!(warning.severity, crate::diagnostic::Severity::Warning);
                assert_eq!(warning.code, crate::codes::source::DECLARATION_DEPRECATED);
            }
        }
    }

    #[test]
    fn failed_declaration_reaches_the_next_round() {
        let (directory, scan) = scan_content(&[
            (
                "alpha.typ",
                "#import \"@tola/source:0.0.0\": all-sources, tola-meta\n\
                 #tola-meta((title: [Alpha]))\n\
                 #let beta = all-sources().find(source => source.id == \"beta.typ\")\n\
                 #let stopped = beta.meta.title",
            ),
            (
                "beta.typ",
                "#import \"@tola/source:0.0.0\": all-sources, tola-meta\n\
                 #let alpha = all-sources().find(source => source.id == \"alpha.typ\")\n\
                 #tola-meta((title: alpha.meta.title))",
            ),
        ]);

        assert!(
            scan.metadata_rounds > 1,
            "both sources wait for each other's declarations"
        );
        let alpha = analyzed_entry(&scan, &directory.path().join("content/alpha.typ"));
        assert_eq!(
            alpha
                .declaration
                .as_ref()
                .expect("alpha declared before it stopped")
                .metadata
                .to_typst_dict()
                .get("title")
                .unwrap(),
            &typst::text::TextElem::packed("Alpha").into_value()
        );
        let beta = analyzed_entry(&scan, &directory.path().join("content/beta.typ"));
        assert_eq!(
            beta.declaration
                .as_ref()
                .expect("beta declared once alpha's value arrived")
                .metadata
                .to_typst_dict()
                .get("title")
                .unwrap(),
            &typst::text::TextElem::packed("Alpha").into_value()
        );
    }

    #[test]
    fn dependent_metadata_converges() {
        for (name, declaring, dependent) in [
            (
                "any-source",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: [First]))",
                r#"#import "@tola/source:0.0.0": all-sources
#let ready = all-sources().any(source => source.meta != none and source.meta.at("title", default: none) == [First])
#import "@tola/source:0.0.0": tola-meta
#tola-meta((ready: ready))"#,
            ),
            (
                "by id across the fixed point",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: [Alpha]))",
                r#"#import "@tola/source:0.0.0": all-sources
#let ready = all-sources().any(source => source.id == "declaring.typ" and source.meta != none and source.meta.at("title", default: none) == [Alpha])
#import "@tola/source:0.0.0": tola-meta
#tola-meta((ready: ready))"#,
            ),
        ] {
            let (_directory, result) =
                scan_content(&[("declaring.typ", declaring), ("dependent.typ", dependent)]);

            assert_eq!(
                metadata_field(&result, "dependent.typ", "ready"),
                true.into_value(),
                "{name}"
            );
        }
    }

    #[test]
    fn long_acyclic_metadata_chain_converges() {
        let mut sources = vec![(
            "source-0.typ".to_owned(),
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((rank: 0))".to_owned(),
        )];
        for index in 1..20 {
            sources.push((
                format!("source-{index}.typ"),
                format!(
                    r#"#import "@tola/source:0.0.0": all-sources
#let previous = all-sources().find(source => source.id == "source-{}.typ")
#import "@tola/source:0.0.0": tola-meta
#tola-meta((rank: previous.meta.rank + 1))"#,
                    index - 1
                ),
            ));
        }
        let sources = sources
            .iter()
            .map(|(name, source)| (name.as_str(), source.as_str()))
            .collect::<Vec<_>>();
        let (_directory, scanned) = scan_content(&sources);

        assert_eq!(
            metadata_field(&scanned, "source-19.typ", "rank"),
            19.into_value()
        );
    }

    #[test]
    fn oscillating_metadata_reports_the_cycle() {
        let (_directory, scan) = scan_result(&[(
            "toggle.typ",
            r#"#import "@tola/source:0.0.0": all-sources
#let previous = all-sources().first().meta
#import "@tola/source:0.0.0": tola-meta
#tola-meta((flag: previous == none or not previous.flag))"#,
        )]);

        let error = scan
            .err()
            .expect("alternating source attributes have no fixed point");

        assert!(
            error
                .to_string()
                .contains("keep changing each other's `<tola-meta>` values"),
            "{error:#}"
        );
        assert!(
            error.to_string().contains("content/toggle.typ"),
            "{error:#}"
        );
    }

    #[test]
    fn only_the_broken_source_is_reported() {
        let (_directory, scan) = scan_result(&[
            (
                "alpha.typ",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Alpha\"))",
            ),
            (
                "beta.typ",
                r#"#import "@tola/source:0.0.0": all-sources
#let alpha = all-sources().find(source => source.id == "alpha.typ")
#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: alpha.meta.title))"#,
            ),
            ("gamma.typ", "#let broken ="),
        ]);

        let error = scan
            .err()
            .expect("unrelated source error must fail analysis");
        let diagnostics = crate::diagnostic::attached(&error).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].location.as_ref().unwrap().path,
            "content/gamma.typ",
        );
    }

    #[test]
    fn stored_metadata_runs_in_its_bundle() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let content = root.join("content");
        fs::create_dir(&content).unwrap();
        fs::write(
            content.join("post.typ"),
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/source:0.0.0": tola-meta
#tola-meta((
  title: "Native values",
  lookup: () => all-sources(),
  settings: () => sys.inputs,
  standard-settings: () => std.sys.inputs,
  card: [
    #set text(fill: red)
    #show strong: body => underline(body.body)
    *Native card*
  ],
  deferred: context [Size: #text.size],
  paint: gradient.linear(red, blue),
))"#,
        )
        .unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            r#"#import "@tola/source:0.0.0": all-sources
#let attributes = all-sources().first().meta
#assert.eq(type(attributes.title), str)
#let lookup = attributes.lookup
#let settings = attributes.settings
#let standard-settings = attributes.standard-settings
#import sys.inputs.at("__tola"): all-sources as host-sources
#assert.eq(lookup().first().meta.title, "Native values")
// A deferred call reaches the host through the compilation it runs in, so a stored function
// cannot observe stale sources.
#assert.eq(host-sources().first().meta.title, "Native values")
#document("index.html", format: "html")[
  #set text(size: 17pt)
  #attributes.card
  #attributes.deferred
  #rect(width: 10pt, height: 10pt, fill: attributes.paint)
]"#,
        )
        .unwrap();
        let config = crate::config::tests::load_test_config(root, "");

        let built = crate::build::build_site(&config, crate::mode::BuildMode::Production).unwrap();
        let page = built
            .graph()
            .outputs()
            .iter()
            .find(|output| output.path().as_str() == "index.html")
            .unwrap();
        let html = std::str::from_utf8(page.bytes()).unwrap();

        assert!(html.contains("Native card"), "{html}");
        assert!(html.contains("17pt"), "{html}");
    }

    #[test]
    fn captured_sources_never_false_converge() {
        let (_directory, scan) = scan_result(&[(
            "post.typ",
            r#"#import "@tola/source:0.0.0": all-sources
#let captured = all-sources()
#import "@tola/source:0.0.0": tola-meta
#tola-meta((count: () => captured.len()))"#,
        )]);

        let error = scan
            .err()
            .expect("the captured environment grows on every round");

        assert!(
            error
                .to_string()
                .contains("could not settle the site metadata after 16 reads"),
            "{error:#}"
        );
        assert!(error.to_string().contains("content/post.typ"), "{error:#}");
    }

    #[test]
    fn every_source_sees_one_source_set() {
        let metadata_source = r#"#import "@tola/source:0.0.0": all-sources, tola-meta
#tola-meta((count: all-sources().len()))"#;
        let (_directory, result) = scan_content(&[
            ("docs/guide.typ", metadata_source),
            ("api/reference.typ", metadata_source),
        ]);

        assert_eq!(result.source_set.sources().len(), 2);
        for source in result.source_set.sources() {
            let file = source.source().file_name().unwrap().to_str().unwrap();
            assert_eq!(metadata_field(&result, file, "count"), 2.into_value());
        }
    }

    #[test]
    fn only_used_icon_bytes_invalidate() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let content = root.join("content");
        fs::create_dir(&content).unwrap();
        fs::write(
            root.join("helpers.typ"),
            r#"#import "@tola/icon:0.0.0": icon-bytes"#,
        )
        .unwrap();
        fs::write(
            content.join("post.typ"),
            r#"#import "../helpers.typ": icon-bytes
#import "@tola/source:0.0.0": tola-meta
#tola-meta((svg-bytes: icon-bytes("brand:mark").len()))"#,
        )
        .unwrap();
        fs::write(root.join("site.typ"), "").unwrap();
        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &root.join("site.typ"));
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let icons = |body: &str, unused: &str| {
            let wrap = |body: &str| format!("<svg viewBox='0 0 24 24'>{body}</svg>");
            let collections = crate::compiler::tests::brand_icons(&wrap(body), &wrap(unused));
            let length = collections.get("brand", "mark").unwrap().svg().len() as i64;
            (collections, length)
        };
        let mark = "<path d='M0 0h24v24H0z'/>";
        let (first_icons, first_length) = icons(mark, "<path/>");
        let first_host = host.with_icons(first_icons);
        let first = scan_default(&config, &first_host, &units, None).unwrap();
        let cache = SourceAnalysisCache::from_scan(&first);
        let metadata = |scan: &SourceScan| metadata_field(scan, "post.typ", "svg-bytes");
        assert_eq!(metadata(&first), first_length.into_value());
        let (unused_icons, _) = icons(mark, "<circle cx='12' cy='12' r='10'/>");
        let unchanged = scan_default(
            &config,
            &host.with_icons(unused_icons),
            &units,
            Some(&cache),
        )
        .unwrap();
        assert_eq!(metadata(&unchanged), metadata(&first));
        assert_eq!(unchanged.dependency_reads, first.dependency_reads);

        let (changed_icons, changed_length) = icons(
            "<circle cx='12' cy='12' r='10'/><path d='M0 0h24v24H0z'/>",
            "<path/>",
        );
        let changed_host = host.with_icons(changed_icons);
        let changed = scan_default(&config, &changed_host, &units, Some(&cache)).unwrap();
        let cold = scan_default(&config, &changed_host, &units, None).unwrap();
        assert_eq!(metadata(&changed), changed_length.into_value());
        assert_eq!(metadata(&changed), metadata(&cold));
        assert_eq!(changed.dependency_reads, cold.dependency_reads);
        assert_ne!(changed.dependency_reads, first.dependency_reads);

        let missing = host.with_icons(Arc::default());
        assert!(scan_default(&config, &missing, &units, Some(&cache)).is_err());
        assert_eq!(
            metadata(&scan_default(&config, &changed_host, &units, Some(&cache)).unwrap()),
            metadata(&cold)
        );
    }

    #[test]
    fn membership_change_invalidates_readers() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        let plain = content.join("plain.typ");
        let first_reader = content.join("first-reader.typ");
        let second_reader = content.join("second-reader.typ");
        let reader = r#"#import "@tola/source:0.0.0": all-sources, tola-meta
#tola-meta((count: all-sources().len()))"#;
        fs::write(&plain, "Plain").unwrap();
        fs::write(&first_reader, reader).unwrap();
        fs::write(&second_reader, reader).unwrap();
        fs::write(root.join("site.typ"), "").unwrap();

        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &root.join("site.typ"));
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let first = scan_default(&config, &host, &units, None).unwrap();
        let ids = [&plain, &first_reader, &second_reader]
            .into_iter()
            .map(|path| (path.clone(), Arc::clone(analyzed_entry(&first, path))))
            .collect::<BTreeMap<_, _>>();
        let cache = SourceAnalysisCache::from_scan(&first);
        let published_base = Arc::clone(&cache.entries.base);

        fs::write(content.join("added.typ"), "Added").unwrap();
        let expanded = content_units(&config).unwrap();
        let expanded_sources = SourceSet::without_metadata(&expanded, &config).unwrap();
        assert!(!cache.matches_content(
            config.get_root(),
            &crate::package::SiteBindings::from_config(&config, Default::default()),
            &expanded,
            &expanded_sources
        ));
        let changed = scan_default(&config, &host, &expanded, Some(&cache)).unwrap();

        assert!(Arc::ptr_eq(analyzed_entry(&changed, &plain), &ids[&plain]));
        assert!(!Arc::ptr_eq(
            analyzed_entry(&changed, &first_reader),
            &ids[&first_reader]
        ));
        assert!(!Arc::ptr_eq(
            analyzed_entry(&changed, &second_reader),
            &ids[&second_reader]
        ));
        assert!(!Arc::ptr_eq(&published_base, &changed.entries.base));
        assert_eq!(changed.entries.ordered_paths.len(), expanded.len());
    }

    #[test]
    fn metadata_change_reuses_unaffected_reader() {
        let directory = TempDir::new().unwrap();
        let root = crate::filesystem::normalize_path(directory.path());
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        let document = content.join("document.typ");
        let reader = content.join("reader.typ");
        fs::write(
            &document,
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((revision: 1))",
        )
        .unwrap();
        fs::write(
            &reader,
            r#"#import "@tola/address:0.0.0": route
#import "@tola/source:0.0.0": current-source
#let source = current-source()
#import "@tola/source:0.0.0": tola-meta
#tola-meta((filename: source.filename, permalink: route(source.route-segments)))
Reader body"#,
        )
        .unwrap();
        let entry = root.join("site.typ");
        fs::write(&entry, "").unwrap();
        let mut config = crate::config::tests::load_test_config(&root, "");
        configure_site(&mut config, &content, &entry);
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let first = scan_default(&config, &host, &units, None).unwrap();
        let analyzed = analyzed_entry(&first, &reader);
        assert!(analyzed.sources_input.is_none());
        assert_eq!(analyzed.source_reads.len(), 1);
        assert_eq!(
            analyzed.source_reads[0].0.vpath().get_with_slash(),
            "/content/reader.typ"
        );
        assert!(analyzed.source_reads[0].1.is_some());
        let reader_analysis = Arc::clone(analyzed);
        let cache = SourceAnalysisCache::from_scan(&first);

        fs::write(
            &document,
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((revision: 2))",
        )
        .unwrap();
        let cache = prepare_cache_for_changes(
            cache,
            &config,
            &units,
            first.dependency_reads(),
            std::slice::from_ref(&document),
            false,
        );
        let second = scan_default(&config, &host, &units, Some(&cache)).unwrap();
        assert!(Arc::ptr_eq(
            analyzed_entry(&second, &reader),
            &reader_analysis
        ));
        assert_eq!(
            metadata_field(&second, "document.typ", "revision"),
            2.into_value()
        );

        assert_eq!(
            metadata_field(&second, "reader.typ", "filename"),
            "reader.typ".into_value()
        );
        assert_eq!(
            metadata_field(&second, "reader.typ", "permalink"),
            "/reader/".into_value()
        );
    }

    #[test]
    fn entries_with_no_reads_are_reused() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        let plain = content.join("plain.typ");
        let sources_reader = content.join("sources.typ");
        let current = content.join("current.typ");
        fs::write(&plain, "Plain").unwrap();
        fs::write(
            &sources_reader,
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/source:0.0.0": tola-meta
#tola-meta((count: all-sources().len()))"#,
        )
        .unwrap();
        fs::write(
            &current,
            r#"#import "@tola/document:0.0.0": current-document
#context [#current-document().route]"#,
        )
        .unwrap();
        let entry = root.join("site.typ");
        fs::write(&entry, "").unwrap();

        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &entry);
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let first = scan_default(&config, &host, &units, None).unwrap();
        let cache = SourceAnalysisCache::from_scan(&first);

        let plain_entry = Arc::clone(cached_entry(&cache, &plain).unwrap());
        let sources_entry = Arc::clone(cached_entry(&cache, &sources_reader).unwrap());
        let current_entry = Arc::clone(cached_entry(&cache, &current).unwrap());
        assert!(plain_entry.sources_input.is_none());
        assert!(sources_entry.sources_input.is_some());
        assert!(current_entry.sources_input.is_none());

        fs::write(content.join("added.typ"), "Added").unwrap();
        let expanded = content_units(&config).unwrap();
        let rescanned = scan_default(&config, &host, &expanded, Some(&cache)).unwrap();

        assert!(Arc::ptr_eq(
            analyzed_entry(&rescanned, &plain),
            &plain_entry
        ));
        assert!(!Arc::ptr_eq(
            analyzed_entry(&rescanned, &sources_reader),
            &sources_entry
        ));
        assert!(Arc::ptr_eq(
            analyzed_entry(&rescanned, &current),
            &current_entry
        ));
    }

    #[test]
    fn changed_program_keeps_source_metadata() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(
            content.join("post.typ"),
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: [Cached]))",
        )
        .unwrap();
        fs::write(
            root.join("site.typ"),
            "#let value = 1\n#document(\"index.html\", [#value])",
        )
        .unwrap();

        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &root.join("site.typ"));
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let first = scan_default(&config, &host, &units, None).unwrap();
        let reads = first.dependency_reads();
        let cache = SourceAnalysisCache::from_scan(&first);
        let discovered_sources = SourceSet::without_metadata(&units, &config).unwrap();
        assert!(cache.matches_content(
            config.get_root(),
            &crate::package::SiteBindings::from_config(&config, Default::default()),
            &units,
            &discovered_sources
        ));

        fs::write(
            root.join("site.typ"),
            "#let value = 2\n#document(\"index.html\", [#value])",
        )
        .unwrap();
        let prepared = prepare_cache_for_changes(
            cache.clone(),
            &config,
            &units,
            reads,
            &[root.join("site.typ")],
            false,
        );
        let reused = scan_default(&config, &host, &units, Some(&prepared)).unwrap();
        assert!(!Arc::ptr_eq(&first.snapshot, &reused.snapshot));

        assert_eq!(
            metadata_field(&reused, "post.typ", "title"),
            typst::text::TextElem::packed("Cached").into_value()
        );

        let mut changed_identity = units.clone();
        changed_identity[0].id = crate::content::ContentId::new("renamed.typ".into());
        let changed_sources = SourceSet::without_metadata(&changed_identity, &config).unwrap();
        assert!(!cache.matches_content(
            config.get_root(),
            &crate::package::SiteBindings::from_config(&config, Default::default()),
            &changed_identity,
            &changed_sources
        ));
        let mut changed_layout = units.clone();
        changed_layout[0].layout = crate::content::ContentSourceLayout::DirectoryIndex;
        let changed_sources = SourceSet::without_metadata(&changed_layout, &config).unwrap();
        assert!(!cache.matches_content(
            config.get_root(),
            &crate::package::SiteBindings::from_config(&config, Default::default()),
            &changed_layout,
            &changed_sources
        ));

        let mut changed_config = config.clone();
        changed_config.site.title = "Changed".into();
        let changed_sources = SourceSet::without_metadata(&units, &changed_config).unwrap();
        assert!(!cache.matches_content(
            config.get_root(),
            &crate::package::SiteBindings::from_config(&changed_config, Default::default()),
            &units,
            &changed_sources
        ));

        let identity = crate::package::SiteBindings::from_config(&config, Default::default());
        let library = identity.library(Default::default());
        let world = host
            .world(
                root,
                &root.join("site.typ"),
                &library,
                Arc::clone(&reused.candidate_files),
                &tola_typst::BundleCancellation::default(),
            )
            .expect("valid test world");
        let compilation =
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::default())
                .unwrap();
        let export = compilation
            .export(
                &tola_typst::BundleOptions::default(),
                &tola_typst::BundleCancellation::default(),
                None,
            )
            .unwrap();
        let entry = export
            .entries()
            .iter()
            .find(|entry| entry.path().get_with_slash() == "/index.html")
            .unwrap();
        let html = std::str::from_utf8(entry.bytes().as_slice()).unwrap();
        assert!(html.contains(">2<"), "{html}");
    }

    #[test]
    fn reused_analysis_reports_new_errors() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        let source = content.join("post.typ");
        fs::write(
            &source,
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: [First]))",
        )
        .unwrap();
        fs::write(root.join("site.typ"), "").unwrap();

        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &root.join("site.typ"));
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let first = scan_default(&config, &host, &units, None).unwrap();
        let reads = first.dependency_reads();
        let cache = SourceAnalysisCache::from_scan(&first);

        fs::write(&source, "#let broken =").unwrap();
        let cache = prepare_cache_for_changes(
            cache,
            &config,
            &units,
            reads,
            std::slice::from_ref(&source),
            false,
        );
        let error = scan_default(&config, &host, &units, Some(&cache))
            .err()
            .expect("reused analysis must expose changed source errors");
        let diagnostics = crate::diagnostic::attached(&error).unwrap();
        assert_eq!(
            diagnostics[0].location.as_ref().unwrap().path,
            "content/post.typ"
        );
        assert_eq!(diagnostics[0].severity, crate::diagnostic::Severity::Error);
    }

    #[test]
    fn reused_analysis_invents_no_diagnostics() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(
            content.join("post.typ"),
            r#"
#show strong: none
= Real <real>
#context {
  let count = query(heading).len()
  count * [= Fake <fake>]
}
"#,
        )
        .unwrap();
        fs::write(root.join("site.typ"), "").unwrap();

        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &root.join("site.typ"));
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let first = scan_default(&config, &host, &units, None).unwrap();
        assert!(first.diagnostics.is_empty());
        let cache = SourceAnalysisCache::from_scan(&first);

        let reused = scan_default(&config, &host, &units, Some(&cache)).unwrap();

        assert!(reused.diagnostics.is_empty());
    }

    #[test]
    fn cancelled_analysis_reports_cancellation() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("post.typ"), "Post").unwrap();
        fs::write(root.join("site.typ"), "").unwrap();

        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &root.join("site.typ"));
        let units = content_units(&config).unwrap();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let policy = tola_typst::BundleCancellation::default().with_cancellation(cancelled);

        let mut inputs = crate::compiler::BuildInputs::default();
        let error = match scan(
            &config,
            &compiler_host(&config).unwrap(),
            &units,
            crate::package::SiteBindings::from_config(&config, Default::default()),
            SourceAnalysisReuse::None,
            &policy,
            &mut inputs,
        ) {
            Ok(_) => panic!("cancelled source analysis unexpectedly succeeded"),
            Err(failure) => failure.into_error(),
        };

        assert!(error.chain().any(|cause| matches!(
            cause.downcast_ref::<tola_typst::CompileError>(),
            Some(tola_typst::CompileError::Cancelled)
        )));
    }
}
