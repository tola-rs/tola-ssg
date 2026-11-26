//! Source analysis: settle every content source's metadata and keep the result reusable.

mod round;
mod settlement;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tola_input::normalize_path;
use tola_typst::Diagnostics;

use crate::compiler::SiteHost;
use crate::config::SiteConfig;
use crate::content::{ContentSource, ContentUnit, SourceSet, SourcesInput};

use round::{AnalysisRound, AnalyzedSource, SourceEntryIndex};
use settlement::{Settlement, SourceReads, settle_metadata_rounds};

const MIN_METADATA_EVALUATION_ROUNDS: usize = 16;
const ANALYSIS_INPUT_IDENTITY: &str = "source-analysis";

pub(crate) struct SourceScan {
    pub(crate) sources: Arc<tola_typst::ExplicitSources>,
    pub(crate) candidate_files: Arc<tola_typst::FileView>,
    pub(crate) source_set: SourceSet,
    pub(crate) diagnostics: Arc<Diagnostics>,
    declaration_warnings: Arc<[tola_diagnostic::Diagnostic]>,
    dependency_reads: Vec<SourceReads>,
    entries: SourceEntryIndex,
    reused_sources: Arc<[PathBuf]>,
    content_identity: Arc<[ContentUnit]>,
    sources_identity: SourcesInput,
    package_bindings: crate::package::SiteBindings,
    metadata_rounds: usize,
}

impl SourceScan {
    pub(crate) fn package_bindings(&self) -> &crate::package::SiteBindings {
        &self.package_bindings
    }

    pub(crate) fn dependency_reads(&self) -> Vec<SourceReads> {
        self.dependency_reads.clone()
    }
}

/// A source analysis that did not settle, with the world its sources resolve against.
pub(crate) struct SourceAnalysisFailure {
    pub(crate) world: Option<Arc<tola_typst::TypstWorld>>,
    pub(crate) error: anyhow::Error,
}

impl SourceAnalysisFailure {
    pub(crate) fn into_error(self) -> anyhow::Error {
        self.error
    }
}

impl std::fmt::Debug for SourceAnalysisFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceAnalysisFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub(crate) struct SourceAnalysisCache {
    diagnostics: Arc<Diagnostics>,
    declaration_warnings: Arc<[tola_diagnostic::Diagnostic]>,
    content_identity: Arc<[ContentUnit]>,
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
        bindings: &crate::package::SiteBindings,
        content: &[ContentUnit],
        srcs: &SourceSet,
    ) -> bool {
        self.sources.root() == root
            && self.sources_identity == *srcs.inputs()
            && self.package_bindings.matches(bindings)
            && self.matches_content_identity(content)
    }

    pub(crate) fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }

    pub(crate) fn declaration_warnings(&self) -> &[tola_diagnostic::Diagnostic] {
        &self.declaration_warnings
    }

    fn entry_for_source(&self, src: &ContentSource) -> Option<&Arc<AnalyzedSource>> {
        self.entries
            .get(src.source())
            .filter(|entry| entry.belongs_to(src))
    }

    pub(crate) fn matches_package_bindings(&self, bindings: &crate::package::SiteBindings) -> bool {
        self.package_bindings.matches(bindings)
    }

    pub(crate) fn matches_content_identity(&self, content: &[ContentUnit]) -> bool {
        self.content_identity.as_ref() == content
    }

    pub(crate) fn prepare_for_rebuild(
        mut self,
        decision: &crate::compiler::RebuildDecision,
        site_entry: Option<&Path>,
    ) -> Self {
        self.invalidated_sources = Arc::new(
            decision
                .invalidated_content_sources()
                .map(normalize_path)
                .collect(),
        );
        let mut refresh_paths: Vec<_> = decision
            .directly_changed_content_sources()
            .map(normalize_path)
            .collect();
        if decision.directly_rebuilds_site_program() {
            refresh_paths.extend(site_entry.map(normalize_path));
        }
        refresh_paths.sort_unstable();
        refresh_paths.dedup();
        self.source_refresh_paths = refresh_paths.into();
        self
    }

    pub(crate) fn reused_dependency_readers(&self) -> Vec<crate::compiler::TypstDependencyReader> {
        self.reused_sources
            .iter()
            .cloned()
            .map(crate::compiler::TypstDependencyReader::ContentSource)
            .collect()
    }

    pub(super) fn may_reuse_entry(&self, src: &Path) -> bool {
        !self.invalidated_sources.contains(src)
    }
}

pub(crate) fn analyze(
    cfg: &SiteConfig,
    host: &SiteHost,
    content: &[ContentUnit],
    bindings: crate::package::SiteBindings,
    reuse: SourceAnalysisReuse<'_>,
    cancellation: &tola_typst::BundleCancellation,
    inputs: &mut crate::compiler::BuildInputs,
) -> Result<SourceScan, SourceAnalysisFailure> {
    scan(cfg, host, content, bindings, reuse, cancellation, inputs).map_err(|failure| {
        SourceAnalysisFailure {
            world: failure.world,
            error: super::diagnostic::with_diagnostics(
                failure.error,
                cfg.root(),
                tola_diagnostic::codes::typst::SOURCE_ANALYSIS,
            ),
        }
    })
}

fn scan(
    cfg: &SiteConfig,
    host: &SiteHost,
    content: &[ContentUnit],
    bindings: crate::package::SiteBindings,
    reuse: SourceAnalysisReuse<'_>,
    cancellation: &tola_typst::BundleCancellation,
    inputs: &mut crate::compiler::BuildInputs,
) -> Result<SourceScan, SourceAnalysisFailure> {
    let source_set =
        SourceSet::without_metadata(content, cfg).map_err(|err| SourceAnalysisFailure {
            world: None,
            error: err,
        })?;
    let sources_identity = source_set.inputs().clone();
    let entry_reuse = reuse.cache().filter(|cache| {
        cache.sources.root() == cfg.root() && cache.matches_package_bindings(&bindings)
    });
    let content_identity = match reuse.cache() {
        Some(cache) if cache.matches_content_identity(content) => {
            Arc::clone(&cache.content_identity)
        }
        _ => content.into(),
    };
    let mut src_roots: Vec<PathBuf> = source_set
        .sources()
        .iter()
        .map(|src| src.source().to_path_buf())
        .collect();
    src_roots.extend(cfg.project_entry_path().as_deref().map(normalize_path));
    src_roots.sort_unstable();
    src_roots.dedup();
    let loaded = match entry_reuse {
        Some(reuse) => reuse.sources.refresh_with_files_cancellable(
            &src_roots,
            &reuse.source_refresh_paths,
            cfg.root(),
            host.file_resolver(),
            cancellation,
        ),
        None => tola_typst::ExplicitSources::build_with_files_cancellable(
            &src_roots,
            cfg.root(),
            host.file_resolver(),
            cancellation,
        ),
    };
    let loaded = loaded.map_err(|failure| {
        inputs.record_accessed(failure.accessed());
        let error = if failure.is_cancelled() {
            anyhow::Error::new(tola_typst::CompileError::cancelled())
        } else {
            let (err, _) = failure.into_error_and_accessed();
            anyhow::Error::new(tola_typst::CompileError::from(err))
        };
        SourceAnalysisFailure { world: None, error }
    })?;
    inputs.record_accessed(loaded.accessed());
    let sources = Arc::new(loaded.into_sources_and_accessed().0);
    let candidate_files = host.candidate_files(Arc::clone(&sources));
    let round_limit = source_set
        .sources()
        .len()
        .saturating_add(1)
        .max(MIN_METADATA_EVALUATION_ROUNDS);
    let mut settlement = Settlement::new(source_set, round_limit);
    let round = AnalysisRound {
        cfg,
        host,
        reuse: entry_reuse,
        candidate_files: &candidate_files,
        bindings: &bindings,
        cancellation,
    };
    let outcome = settle_metadata_rounds(&round, &mut settlement, reuse.dependencies(), inputs);
    let (diags, dep_reads) = outcome.map_err(|err| {
        let world = if crate::cancellation::is_cancelled(&err) {
            None
        } else {
            source_analysis_world(
                cfg,
                host,
                &bindings,
                settlement.source_set.inputs(),
                &candidate_files,
                cancellation,
            )
        };
        SourceAnalysisFailure { world, error: err }
    })?;
    let declaration_warnings = declaration_warnings(&settlement.entries);
    let retained = entry_reuse.map(|cache| &cache.entries);
    let reused_sources = settlement.entries.reused_from(retained);
    Ok(SourceScan {
        sources,
        candidate_files,
        source_set: settlement.source_set,
        diagnostics: Arc::new(diags),
        declaration_warnings,
        dependency_reads: dep_reads,
        entries: settlement.entries,
        reused_sources,
        content_identity,
        sources_identity,
        package_bindings: bindings,
        metadata_rounds: settlement.rounds,
    })
}

fn source_analysis_world(
    cfg: &SiteConfig,
    host: &SiteHost,
    bindings: &crate::package::SiteBindings,
    srcs: &SourcesInput,
    candidate_files: &Arc<tola_typst::FileView>,
    cancellation: &tola_typst::BundleCancellation,
) -> Option<Arc<tola_typst::TypstWorld>> {
    let library = bindings.library(srcs.to_source_records());
    host.site_world(cfg, &library, Arc::clone(candidate_files), cancellation)
        .ok()
        .map(Arc::new)
}

fn declaration_warnings(entries: &SourceEntryIndex) -> Arc<[tola_diagnostic::Diagnostic]> {
    entries
        .entries()
        .filter_map(|entry| entry.legacy_label.clone())
        .map(|loc| {
            tola_diagnostic::Diagnostic::at_location(
                tola_diagnostic::codes::source::DECLARATION_DEPRECATED,
                tola_diagnostic::Severity::Warning,
                loc,
                "the `<tola-meta>` label does not declare source metadata",
            )
            .with_help(
                "Import `tola-meta` from `@tola/source:0.0.0` and replace \
                 `metadata(...) <tola-meta>` with `tola-meta(...)`",
            )
        })
        .collect()
}
