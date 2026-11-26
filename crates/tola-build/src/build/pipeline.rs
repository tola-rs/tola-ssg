//! One sealed site candidate: hook stages, compilation, sealing, publication, in that order.

use anyhow::Result;

use crate::{
    compiler::SiteHost, config::SiteConfig, hooks, resources::BuildResources, site::SiteIndex,
};

use super::{
    BuildAttemptInputs, BuildCacheUpdate, BuildFailure, BuildMode, BuildRequest, BuildSession,
    SiteBuild, SiteBuildGuard, diagnostic,
};

mod compile;

#[derive(Default)]
pub(crate) struct BuildAttemptProducers {
    pub(crate) resources: BuildResources,
    pub(crate) cancellation: crate::cancellation::BuildCancellation,
    pub(crate) reuse: Option<crate::compiler::CompilationReuse>,
    pub(crate) generated_files: Vec<crate::output::GeneratedFile>,
    /// Accepted icon resources; preparation checks config, source identities, bytes, and dir membership.
    pub(crate) icons: Option<crate::icon::IconInventory>,
    pub(crate) images: Option<std::sync::Arc<crate::image::output::ImageOutputs>>,
    pub(crate) seo: Option<std::sync::Arc<crate::seo::SeoCompilation>>,
    pub(crate) reference_reuse: Option<crate::site::references::References>,
    pub(crate) previous_bundle_entries: Option<tola_typst::BundleEntries>,
    pub(crate) asset_reuse: Option<(
        crate::asset::SiteAssets,
        std::sync::Arc<[std::path::PathBuf]>,
    )>,
    /// `None` triggers discovery after before-build hooks.
    pub(crate) content_inventory: Option<std::sync::Arc<[crate::content::ContentUnit]>>,
    pub(crate) inputs: crate::compiler::BuildInputs,
    pub(crate) font_read_paths: Vec<std::path::PathBuf>,
    pub(crate) source_hooks: crate::hooks::SourceHookOutputs,
    pub(crate) failed_hook_outputs: Option<crate::hooks::FailedHookOutputs>,
    pub(crate) previous_hook_reuse: Option<crate::hooks::ReusableHookOutputs>,
    pub(crate) prepared_hooks: crate::hooks::ReadyHooks,
    pub(crate) report: Option<super::BuildReport>,
}

impl BuildAttemptProducers {
    pub(crate) fn into_inputs(self) -> BuildAttemptInputs {
        BuildAttemptInputs {
            compiler: self.inputs,
            font_read_paths: self.font_read_paths,
            source_hooks: self.source_hooks,
            failed_hook_outputs: self.failed_hook_outputs,
            report: self.report,
        }
    }
}

/// Build the entire site from one root Bundle for inspection or custom scheduling.
pub fn build_site(config: &SiteConfig, mode: BuildMode) -> Result<SiteBuild, BuildFailure> {
    let req = BuildRequest::new(mode);
    let mut session = BuildSession::new();
    SiteBuildGuard::build(
        &mut session,
        std::sync::Arc::new(config.clone()),
        req,
        || {},
    )
    .map(SiteBuildGuard::release)
}

/// Derivation generations a completed build retains: the current and one earlier.
const RETAINED_DERIVATION_GENERATIONS: usize = 2;

/// Ordering: attempt reports precede the stopping failure; cancelled failures report no diagnostics.
fn attach_attempt_diagnostics(
    err: anyhow::Error,
    mut attempt_diag: Vec<tola_diagnostic::Diagnostic>,
    root: &std::path::Path,
) -> anyhow::Error {
    if crate::cancellation::is_cancelled(&err) || attempt_diag.is_empty() {
        return err;
    }
    for d in diagnostic::for_error(&err, root) {
        if !attempt_diag.contains(&d) {
            attempt_diag.push(d);
        }
    }
    tola_diagnostic::DiagnosticError::attach(err, attempt_diag).into()
}

pub(super) fn build_site_inner(
    site: &crate::modules::LoadedSite,
    req: &BuildRequest,
    producers: &mut BuildAttemptProducers,
    executor: &hooks::HookExecutor,
    prepare_compiler: impl FnOnce(&mut BuildAttemptProducers, bool) -> Result<SiteHost>,
) -> Result<SiteBuild> {
    let cfg = site.config();
    let started = std::time::Instant::now();
    let mut hook_diag = Vec::new();
    let built = (|| {
        producers.cancellation.ensure_active()?;
        let before_build = if req.hook_execution.runs() {
            hooks::run_before_build_hooks_with_cancellation(
                cfg,
                req.mode,
                producers.resources.input_scope(),
                &mut producers.prepared_hooks,
                Some(&producers.cancellation),
                &mut hook_diag,
                executor,
            )
            .map_err(|failure| {
                producers.source_hooks = failure.completed().clone();
                producers.failed_hook_outputs = failure.partial_outputs().cloned();
                anyhow::Error::new(failure)
            })?
        } else {
            producers.prepared_hooks = hooks::ReadyHooks::default();
            hooks::BeforeBuildExecution::empty()
        };
        let hooks_executed = before_build.hooks_executed();
        producers.source_hooks = before_build.into_source_hooks();
        let before_build_elapsed = started.elapsed();
        producers.cancellation.ensure_active()?;

        let host = prepare_compiler(producers, hooks_executed)?;
        let built = build_site_candidate(
            site,
            &host,
            req,
            producers,
            BeforeBuildTiming {
                started,
                elapsed: before_build_elapsed,
            },
            std::mem::take(&mut hook_diag),
            executor,
        );
        producers.font_read_paths = host.font_read_paths();
        host.evict_stale_file_cache_entries(SiteHost::RETAINED_FILE_CACHE_EPOCHS);
        typst::comemo::evict(RETAINED_DERIVATION_GENERATIONS);
        built
    })();
    built.map_err(|err| {
        let err = attach_attempt_diagnostics(err, hook_diag, cfg.root());
        diagnostic::with_site(err, cfg.root())
    })
}

struct BeforeBuildTiming {
    started: std::time::Instant,
    elapsed: std::time::Duration,
}

fn build_site_candidate(
    site: &crate::modules::LoadedSite,
    host: &SiteHost,
    req: &BuildRequest,
    producers: &mut BuildAttemptProducers,
    before_build: BeforeBuildTiming,
    mut diag: Vec<tola_diagnostic::Diagnostic>,
    executor: &hooks::HookExecutor,
) -> Result<SiteBuild> {
    let cfg = site.config();
    let mut warnings = tola_typst::Diagnostics::new();
    producers.cancellation.ensure_active()?;
    // Capture discovery identity before later hooks can change inputs.
    let content_root = tola_input::normalize_existing_prefix(&cfg.build.content_dir);
    let sealed = (|| -> Result<_> {
        let producers_started = before_build.started.elapsed();
        let mut compiled = compile::compile_site_program(
            site,
            host,
            req.mode,
            &mut warnings,
            &mut diag,
            producers,
        )?;
        diag.extend(crate::compiler::warning_diagnostics(cfg, &mut warnings));
        tola_diagnostic::coalesce_reports(&mut diag);
        tola_diagnostic::ensure_reports_succeed(&diag)?;
        compiled.add_generated_files(
            std::mem::take(&mut producers.generated_files),
            &producers.cancellation,
        )?;
        let mut hook_cfg = cfg.clone();
        if let Some(runner) = hook_cfg.hook_runner.take() {
            let assets = compiled.producer_caches.configured_assets.clone();
            hook_cfg.hook_runner = Some(runner.with_assets(assets, cfg)?);
        }
        let hook_cfg = std::sync::Arc::new(hook_cfg);
        let compiled = compiled.generate_outputs(
            &hook_cfg,
            req,
            site.scope(),
            producers,
            &mut diag,
            executor,
        )?;
        let compile_elapsed = before_build.started.elapsed();
        producers.cancellation.ensure_active()?;
        let candidate = compiled.seal(
            &hook_cfg,
            &producers.cancellation,
            req.reference_checks,
            &mut diag,
            producers.report.as_mut(),
        )?;
        producers.cancellation.ensure_active()?;
        let total_elapsed = before_build.started.elapsed();
        tracing::debug!(
            target: "tola::compile",
            before_build_ms = before_build.elapsed.as_secs_f64() * 1000.0,
            resources_ms = (producers_started - before_build.elapsed).as_secs_f64() * 1000.0,
            producers_ms = (compile_elapsed - producers_started).as_secs_f64() * 1000.0,
            seal_ms = (total_elapsed - compile_elapsed).as_secs_f64() * 1000.0,
            total_ms = total_elapsed.as_secs_f64() * 1000.0,
            "completed site candidate phases"
        );
        Ok((candidate, hook_cfg))
    })();
    diag.extend(crate::compiler::warning_diagnostics(cfg, &mut warnings));
    tola_diagnostic::coalesce_reports(&mut diag);
    let (candidate, hook_cfg) = match sealed {
        Ok(candidate) => candidate,
        Err(err) => return Err(attach_attempt_diagnostics(err, diag, cfg.root())),
    };
    Ok(SiteBuild {
        config: hook_cfg,
        mode: req.mode,
        cancellation: producers.cancellation.clone(),
        hook_executor: executor.clone(),
        index: candidate.index,
        graph: candidate.graph,
        cache_update: BuildCacheUpdate::new(
            candidate.producer_caches,
            candidate.content,
            content_root,
            candidate.references.clone(),
        ),
        diagnostics: diag,
        references: candidate.references,
        source_hooks: producers.source_hooks.clone(),
        report: producers.report.take(),
        modules: site.prepared().clone(),
        scope: site.scope(),
    })
}

/// Collected producer outputs. Generated files and output commands must be added
/// before sealing the graph.
pub(super) struct CompiledSiteOutputs {
    outputs: crate::output::graph::OutputGraphBuilder,
    documents: Vec<crate::compiler::documents::RealizedPage>,
    producer_caches: crate::build::RetainedProducerCaches,
    content: std::sync::Arc<[crate::content::ContentUnit]>,
    reference_reuse: Option<crate::site::references::References>,
}

impl CompiledSiteOutputs {
    pub(super) fn add_generated_files(
        &mut self,
        files: Vec<crate::output::GeneratedFile>,
        cancel: &crate::cancellation::BuildCancellation,
    ) -> Result<()> {
        for file in files {
            cancel.ensure_active()?;
            self.outputs.insert(file.into_file())?;
        }
        Ok(())
    }

    pub(super) fn generate_outputs(
        mut self,
        cfg: &SiteConfig,
        req: &BuildRequest,
        scope: tola_input::InputScope,
        producers: &mut BuildAttemptProducers,
        diag: &mut Vec<tola_diagnostic::Diagnostic>,
        executor: &hooks::HookExecutor,
    ) -> Result<Self> {
        let prev = producers.previous_hook_reuse.take();
        if !req.hook_execution.runs() {
            return Ok(self);
        }
        let generated = crate::hooks::generate_outputs(
            cfg,
            self.outputs.outputs(),
            req.mode,
            scope,
            prev.as_ref(),
            &producers.cancellation,
            diag,
            executor,
        )?;
        for ownership in generated.root_ownerships {
            self.outputs.own_root(ownership)?;
        }
        for out in generated.outputs {
            self.outputs.insert(out)?;
        }
        self.producer_caches.hook_reuse = generated.reuse;
        Ok(self)
    }

    pub(super) fn seal(
        self,
        cfg: &SiteConfig,
        cancel: &crate::cancellation::BuildCancellation,
        reference_checks: super::ReferenceChecks,
        diag: &mut Vec<tola_diagnostic::Diagnostic>,
        mut report: Option<&mut super::BuildReport>,
    ) -> Result<SealedSiteCandidate> {
        cancel.ensure_active()?;
        let graph = self.outputs.finish();
        if let Some(report) = report.as_deref_mut() {
            report.record_outputs(&graph, cancel)?;
        }
        cancel.ensure_active()?;
        let index = SiteIndex::from_output_graph(&graph, self.documents)?;
        if let Some(report) = report.as_deref_mut() {
            report.record_index(&index, cfg.root(), cancel)?;
        }
        cancel.ensure_active()?;
        let references = crate::site::references::References::from_site(
            &index,
            cfg.url_mount(),
            &self.producer_caches.site_program.world,
            cancel,
            self.reference_reuse.as_ref(),
        )?;
        if let Some(report) = report {
            report.record_references(&references, cancel)?;
        }
        cancel.ensure_active()?;
        if reference_checks == super::ReferenceChecks::Full {
            let ref_diag =
                crate::site::references::diagnostic::diagnostics(&references, cfg, cancel)?;
            if ref_diag
                .iter()
                .any(|d| d.severity == tola_diagnostic::Severity::Error)
            {
                return Err(tola_diagnostic::DiagnosticError::new(
                    "a page links to a target this site does not publish",
                    ref_diag,
                )
                .into());
            }
            diag.extend(ref_diag);
        }
        if crate::output::PageAvailability::from_outputs(graph.outputs())
            == crate::output::PageAvailability::Empty
        {
            diag.push(super::diagnostic::no_pages_diagnostic(cfg));
        }
        let publishes_404 = graph.outputs().iter().any(|out| {
            out.path().as_str() == "404.html"
                && out.kind() == crate::output::graph::OutputKind::HtmlDocument
        });
        if !publishes_404 {
            diag.push(super::diagnostic::not_found_missing_diagnostic(cfg));
        }
        Ok(SealedSiteCandidate {
            graph,
            index,
            producer_caches: self.producer_caches,
            references,
            content: self.content,
        })
    }
}

/// Complete output bytes and site index, with an immutable output graph.
pub(super) struct SealedSiteCandidate {
    pub(super) graph: crate::output::graph::OutputGraph,
    pub(super) index: SiteIndex,
    pub(super) producer_caches: crate::build::RetainedProducerCaches,
    pub(super) references: crate::site::references::References,
    pub(super) content: std::sync::Arc<[crate::content::ContentUnit]>,
}

#[cfg(test)]
mod tests;
