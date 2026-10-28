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
    pub(crate) sources: Arc<tola_typst::ExplicitSources>,
    pub(crate) candidate_files: Arc<tola_typst::FileView>,
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
/// The world holds the candidate sources, imports, packages, and fonts the analysis read, so an
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
    sources: Arc<tola_typst::ExplicitSources>,
    invalidated_sources: Arc<BTreeSet<PathBuf>>,
    source_refresh_paths: Arc<[PathBuf]>,
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
            sources: Arc::clone(&scan.sources),
            invalidated_sources: Arc::default(),
            source_refresh_paths: Arc::default(),
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
        self.sources.root() == root
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
        let mut source_refresh_paths = decision
            .directly_changed_content_sources()
            .map(normalize_analysis_path)
            .collect::<Vec<_>>();
        if decision.directly_rebuilds_site_program() {
            source_refresh_paths.extend(site_entry.map(normalize_analysis_path));
        }
        source_refresh_paths.sort_unstable();
        source_refresh_paths.dedup();
        self.source_refresh_paths = source_refresh_paths.into();
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
        cache.sources.root() == config.get_root()
            && cache.matches_package_bindings(&package_bindings)
    });
    let content_identity = reuse
        .cache()
        .filter(|reuse| reuse.matches_content_identity(content))
        .map(|reuse| Arc::clone(&reuse.content_identity))
        .unwrap_or_else(|| content_identity(content));
    let mut source_roots = source_set
        .sources()
        .iter()
        .map(|source| source.source().to_path_buf())
        .collect::<Vec<_>>();
    source_roots.push(normalize_analysis_path(&config.build.entry));
    source_roots.sort_unstable();
    source_roots.dedup();
    let files = host.file_resolver();
    let loaded = match entry_reuse {
        Some(reuse) => reuse.sources.refresh_with_files_cancellable(
            &source_roots,
            &reuse.source_refresh_paths,
            config.get_root(),
            Arc::clone(&files),
            cancellation,
        ),
        None => tola_typst::ExplicitSources::build_with_files_cancellable(
            &source_roots,
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
            // No sources means no candidate files, so the failure names no world.
            return Err(SourceAnalysisFailure { world: None, error });
        }
    };
    inputs.record_accessed(loaded.accessed());
    let (sources, _) = loaded.into_sources_and_accessed();
    let sources = Arc::new(sources);
    let candidate_files = host.candidate_files(Arc::clone(&sources));
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
                    sources: Arc::clone(&sources),
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
/// Built from the candidate sources and the bindings that named those sources — the world a root
/// program compiles in — so a failure still hands an editor the imports, packages, and fonts the
/// sources resolve.
fn site_world(
    config: &ResolvedSiteConfig,
    host: &TypstHost,
    package_bindings: &crate::package::SiteBindings,
    sources: &SourcesInput,
    candidate_files: &Arc<tola_typst::FileView>,
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
    candidate_files: &Arc<tola_typst::FileView>,
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
    let header = tola_packages::CaptureHeader::new(world.main(), ANALYSIS_INPUT_IDENTITY);
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
    // A full channel is the one refusal an author can cause: every declaration the source wrote
    // beyond the channel's capacity was dropped, so the count is unknown but the cause is not.
    // Every other refusal means the channel and the decoder disagree, which no source program can
    // produce.
    let (message, hint) = match &error {
        tola_packages::CaptureError::Saturated => (
            "this source writes more metadata declarations than Tola can read",
            "Keep one `#tola-meta((...))` call; a source declares once",
        ),
        _ => (
            "Tola could not read this source's metadata declarations",
            "Rebuild the site; report this if it repeats",
        ),
    };
    let diagnostic = tola_typst::diagnostic::producer_error_at(
        scan.source().id(),
        0..0,
        tola_packages::DECLARATION_PROTOCOL_ORIGIN,
        &[],
        message,
        tola_typst::diagnostic::DiagnosticProjection::default(),
    )
    .with_hint(hint)
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
    candidate: &tola_typst::FileView,
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
    use crate::compiler::tests::{compiler_host, configure_site, metadata_field};

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
    use typst::foundations::IntoValue;

    use super::*;

    fn analyzed_entry<'a>(scan: &'a SourceScan, source: &Path) -> &'a Arc<SourceAnalysisEntry> {
        scan.entries
            .get(&normalize_analysis_path(source))
            .expect("analyzed entry exists")
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

    /// The rendered error of a scan the case expected to fail.
    fn rendered_failure(scan: anyhow::Result<SourceScan>) -> String {
        match scan {
            Ok(_) => panic!("the scan was expected to fail"),
            Err(error) => format!("{error:#}"),
        }
    }

    /// The diagnostics a failed scan attached, for a case that expects them.
    fn failure_diagnostics(scan: anyhow::Result<SourceScan>) -> Vec<crate::diagnostic::Diagnostic> {
        let error = scan.err().expect("the scan was expected to fail");
        crate::diagnostic::attached(&error)
            .expect("a failed scan carries diagnostics")
            .to_vec()
    }

    /// One site whose `content/` holds `sources`, and the scan of it.
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

    /// The scanned result for a site whose `content/` holds `sources`.
    fn scan_content(sources: &[(&str, &str)]) -> (TempDir, SourceScan) {
        let (directory, scan) = scan_result(sources);
        (directory, scan.unwrap())
    }

    /// A cache prepared for a rebuild from `changed_paths`, as the build loop does.
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

    /// A source declares its metadata only at an eager call site.
    ///
    /// A declaration is detected by the evaluated element and its call site, never by the
    /// callee's name, so a renamed import or a wrapper body is not one. Every shape a source can
    /// be written in is one row.
    #[test]
    fn declarations_settle_by_call_site() {
        for (name, sources, declares, warns) in [
            (
                "plain label",
                &[("post.typ", "#metadata((title: \"A\")) <tola-meta>")][..],
                false,
                true,
            ),
            (
                "parenthesized",
                &[("post.typ", "#(metadata((title: \"A\"))) <tola-meta>")][..],
                false,
                true,
            ),
            (
                "aliased binding",
                &[(
                    "post.typ",
                    "#let m = metadata\n#m((title: \"A\")) <tola-meta>",
                )][..],
                false,
                true,
            ),
            (
                "shadowed native",
                &[(
                    "post.typ",
                    "#let tola-meta = metadata\n#tola-meta((title: \"A\")) <tola-meta>",
                )][..],
                false,
                true,
            ),
            (
                "wrapper body",
                &[(
                    "post.typ",
                    "#let decl(d) = metadata(d)\n#decl((title: \"A\")) <tola-meta>",
                )][..],
                false,
                false,
            ),
            (
                "show rule",
                &[(
                    "post.typ",
                    "#show heading: it => [#metadata((show: true)) <tola-meta>#it]\n= Heading",
                )][..],
                false,
                false,
            ),
            (
                "placed binding",
                &[(
                    "post.typ",
                    "#let marker = metadata((title: \"A\"))\n#marker <tola-meta>",
                )][..],
                false,
                false,
            ),
            (
                "top level",
                &[(
                    "post.typ",
                    "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"A\"))",
                )][..],
                true,
                false,
            ),
            (
                "own function",
                &[(
                    "post.typ",
                    "#import \"@tola/source:0.0.0\": tola-meta\n#let declare() = tola-meta((title: \"Fn\"))\n#declare()",
                )][..],
                true,
                false,
            ),
        ] {
            let (directory, scan) = scan_content(sources);
            let entry = analyzed_entry(&scan, &directory.path().join("content/post.typ"));

            assert_eq!(entry.declaration.is_some(), declares, "{name} declares");
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

    /// A declaration deferred to realization registers nothing: a contextual body and a show
    /// rule both run after the analysis has settled the source.
    #[test]
    fn deferred_metadata_is_never_declared() {
        for source in [
            r#"#import "@tola/document:0.0.0": current-document
#context {
  let ctx = current-document()
  [#import "@tola/source:0.0.0": tola-meta
#tola-meta((contextual: true))]
}"#,
            r#"#show heading: it => [#import "@tola/source:0.0.0": tola-meta
#tola-meta((selector-show: true))#it]
= Heading"#,
        ] {
            let (_directory, result) = scan_content(&[("post.typ", source)]);
            assert!(result.source_set.sources()[0].metadata().is_none());
        }
    }

    /// A declaration keeps the native values it was written with, including a function and a
    /// content block, rather than flattening them to text.
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
)"#,
        )
        .unwrap();
        fs::write(
            content.join("post.typ"),
            r#"#import "../theme.typ": attributes
#import "@tola/source:0.0.0": tola-meta
#tola-meta(attributes())
= Body"#,
        )
        .unwrap();
        fs::write(directory.path().join("site.typ"), "").unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        configure_site(&mut config, &content, &directory.path().join("site.typ"));
        let units = content_units(&config).unwrap();
        let scanned =
            scan_default(&config, &compiler_host(&config).unwrap(), &units, None).unwrap();

        assert_eq!(
            metadata_field(&scanned, "post.typ", "title"),
            typst::text::TextElem::packed("Document").into_value()
        );
        assert!(matches!(
            metadata_field(&scanned, "post.typ", "callback"),
            typst::foundations::Value::Func(_)
        ));
        assert!(matches!(
            metadata_field(&scanned, "post.typ", "card"),
            typst::foundations::Value::Content(_)
        ));
    }

    /// The physical read evidence names exactly the files a source read, and nothing else.
    #[test]
    fn sources_own_their_physical_reads() {
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

        for (source, own, other) in [
            ("a.typ", "data/a.txt", "data/b.txt"),
            ("b.typ", "data/b.txt", "data/a.txt"),
        ] {
            let reads = reads_for(&content.join(source));
            assert!(reads_path(&reads, own), "{source} read {own}");
            assert!(!reads_path(&reads, other), "{source} claimed {other}");
        }
    }

    /// A change rebuilds exactly the readers of what changed, and reuses the rest.
    ///
    /// The cache is the thing under test, so each case compares entry identity before and after
    /// rather than any internal field.
    #[test]
    fn only_affected_sources_reanalyze() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let data = root.join("data");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&data).unwrap();
        let a = content.join("a.typ");
        let b = content.join("b.typ");
        let plain = content.join("plain.typ");
        let shared = data.join("shared.txt");
        fs::write(&a, "#read(\"/data/shared.txt\")").unwrap();
        fs::write(&b, "#read(\"/data/b.txt\")").unwrap();
        fs::write(&plain, "Plain").unwrap();
        fs::write(&shared, "one").unwrap();
        fs::write(data.join("b.txt"), "B").unwrap();
        fs::write(root.join("site.typ"), "").unwrap();

        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &root.join("site.typ"));
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let first = scan_default(&config, &host, &units, None).unwrap();
        let ids = [&a, &b, &plain]
            .into_iter()
            .map(|path| (path.clone(), Arc::clone(analyzed_entry(&first, path))))
            .collect::<BTreeMap<_, _>>();
        let cache = SourceAnalysisCache::from_scan(&first);

        // A shared file both sources read: both re-derive, the unrelated source does not.
        fs::write(&shared, "two").unwrap();
        let prepared = prepare_cache_for_changes(
            cache.clone(),
            &config,
            &units,
            first.dependency_reads(),
            std::slice::from_ref(&shared),
            false,
        );
        let changed = scan_default(&config, &host, &units, Some(&prepared)).unwrap();

        assert!(!Arc::ptr_eq(analyzed_entry(&changed, &a), &ids[&a]));
        assert!(Arc::ptr_eq(analyzed_entry(&changed, &b), &ids[&b]));
        assert!(Arc::ptr_eq(analyzed_entry(&changed, &plain), &ids[&plain]));
    }

    /// An unchanged revision reuses every entry.
    #[test]
    fn unchanged_revision_reuses_every_entry() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        for index in 0..8 {
            fs::write(content.join(format!("post-{index}.typ")), "Plain").unwrap();
        }
        fs::write(root.join("site.typ"), "").unwrap();

        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &root.join("site.typ"));
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let first = scan_default(&config, &host, &units, None).unwrap();
        let ids = first
            .entries
            .ordered_paths
            .iter()
            .map(|path| (path.clone(), Arc::clone(analyzed_entry(&first, path))))
            .collect::<Vec<_>>();
        let cache = SourceAnalysisCache::from_scan(&first);

        let prepared =
            prepare_cache_for_changes(cache, &config, &units, first.dependency_reads(), &[], false);
        let unchanged = scan_default(&config, &host, &units, Some(&prepared)).unwrap();

        for (path, entry) in ids {
            assert!(
                Arc::ptr_eq(analyzed_entry(&unchanged, &path), &entry),
                "{} was re-derived",
                path.display()
            );
        }
    }

    /// Every source error is reported at its own file, and each one keeps its own diagnostic.
    #[test]
    fn each_source_error_is_reported() {
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
        let scan = scan_default(&config, &host, &units, None);
        let error = scan
            .err()
            .expect("source errors unexpectedly passed analysis");
        let errors = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<SourceAnalysisErrors>())
            .expect("all source errors remain available");
        assert_eq!(errors.errors.len(), 2);

        let diagnostics =
            crate::diagnostic::attached(&error).expect("source diagnostics should remain attached");
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.location.as_ref().unwrap().path.as_str())
                .collect::<Vec<_>>(),
            ["content/one.typ", "content/two.typ"]
        );
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity == crate::diagnostic::Severity::Error)
        );
    }

    /// A failed analysis keeps the warnings of the sources that did evaluate, whichever sources
    /// the failure came from.
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

        // The same warnings must reach the author whether or not analysis was reused.
        for reuse in [None, Some(&cache)] {
            let diagnostics = failure_diagnostics(scan_default(&config, &host, &units, reuse));
            let warnings = diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.severity == crate::diagnostic::Severity::Warning)
                .map(|warning| warning.location.as_ref().unwrap().path.clone())
                .collect::<BTreeSet<_>>();
            assert_eq!(
                warnings,
                BTreeSet::from([
                    "content/document.typ".to_owned(),
                    "content/other.typ".to_owned()
                ])
            );
            assert_eq!(
                diagnostics
                    .iter()
                    .filter(|diagnostic| {
                        diagnostic.severity == crate::diagnostic::Severity::Error
                    })
                    .count(),
                1
            );
        }
    }

    /// Two sources readable only through each other settle once both declarations arrive.
    #[test]
    fn mutual_declarations_converge() {
        let (_directory, scan) = scan_content(&[
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
        for name in ["alpha.typ", "beta.typ"] {
            assert_eq!(
                metadata_field(&scan, name, "title"),
                typst::text::TextElem::packed("Alpha").into_value(),
                "{name}"
            );
        }
    }

    /// A long acyclic chain settles over as many rounds as it is long.
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

    /// A declaration that keeps changing has no fixed point, and the failure names the source.
    #[test]
    fn cycling_metadata_fails() {
        let (_directory, scan) = scan_result(&[(
            "toggle.typ",
            r#"#import "@tola/source:0.0.0": all-sources
#let previous = all-sources().first().meta
#import "@tola/source:0.0.0": tola-meta
#tola-meta((flag: previous == none or not previous.flag))"#,
        )]);

        let rendered = rendered_failure(scan);
        assert!(rendered.contains("content/toggle.typ"), "{rendered}");
    }

    /// A source that never settles is not silently accepted: the analysis gives up and names it.
    #[test]
    fn unsettled_metadata_fails() {
        let (_directory, scan) = scan_result(&[(
            "post.typ",
            r#"#import "@tola/source:0.0.0": all-sources
#let captured = all-sources()
#import "@tola/source:0.0.0": tola-meta
#tola-meta((count: () => captured.len()))"#,
        )]);

        let rendered = rendered_failure(scan);
        assert!(rendered.contains("content/post.typ"), "{rendered}");
    }

    /// A source that declares twice is refused, at the second declaration.
    #[test]
    fn repeated_declarations_fail() {
        let (_directory, scan) = scan_result(&[(
            "post.typ",
            "#import \"@tola/source:0.0.0\": tola-meta\n\
             #tola-meta((first: true))\n\
             #tola-meta((second: true))",
        )]);

        let error = scan
            .err()
            .expect("duplicate source metadata unexpectedly succeeded");
        let diagnostics = crate::diagnostic::attached(&error).unwrap();
        assert_eq!(diagnostics[0].location.as_ref().unwrap().line, Some(3));
    }

    /// Only the broken source of a site is reported.
    #[test]
    fn only_the_broken_source_is_reported() {
        let (_directory, scan) = scan_result(&[
            (
                "alpha.typ",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Alpha\"))",
            ),
            ("gamma.typ", "#let broken ="),
        ]);

        let diagnostics = failure_diagnostics(scan);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].location.as_ref().unwrap().path,
            "content/gamma.typ",
        );
    }

    /// A source that declares with the retired label spelling is warned about once, and the note
    /// names at most three sources.
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

    /// Every source sees one source set, whatever its own position in it.
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

    /// Adding a source invalidates the readers that observe the set, and not the ones that do not.
    #[test]
    fn membership_change_invalidates_readers() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        let plain = content.join("plain.typ");
        let reader = content.join("reader.typ");
        fs::write(&plain, "Plain").unwrap();
        fs::write(
            &reader,
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/source:0.0.0": tola-meta
#tola-meta((count: all-sources().len()))"#,
        )
        .unwrap();
        fs::write(root.join("site.typ"), "").unwrap();

        let mut config = crate::config::tests::load_test_config(root, "");
        configure_site(&mut config, &content, &root.join("site.typ"));
        let units = content_units(&config).unwrap();
        let host = compiler_host(&config).unwrap();
        let first = scan_default(&config, &host, &units, None).unwrap();
        let plain_entry = Arc::clone(analyzed_entry(&first, &plain));
        let reader_entry = Arc::clone(analyzed_entry(&first, &reader));
        let cache = SourceAnalysisCache::from_scan(&first);

        fs::write(content.join("added.typ"), "Added").unwrap();
        let expanded = content_units(&config).unwrap();
        let changed = scan_default(&config, &host, &expanded, Some(&cache)).unwrap();

        assert!(Arc::ptr_eq(analyzed_entry(&changed, &plain), &plain_entry));
        assert!(!Arc::ptr_eq(
            analyzed_entry(&changed, &reader),
            &reader_entry
        ));
    }

    /// Analysing a site whose source analysis was cancelled reports cancellation, not a failure.
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
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(true));
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
