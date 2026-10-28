//! Construction of one sealed site candidate: hook stages, producer compilation,
//! graph sealing, and the handoff to publication.
//!
//! The stage order is an invariant: before-build hooks may write declared inputs, so
//! compiler resources load after them, producers prepare before the root Bundle
//! compiles, and only a sealed candidate is published.

use anyhow::{Context, Result};

use crate::{
    compiler::TypstHost, config::ResolvedSiteConfig, hooks, resources::BuildResources,
    site::SiteIndex,
};

use super::{
    AcceptedFileChanges, BuildAttemptInputs, BuildCacheUpdate, BuildFailure, BuildMode,
    BuildRequest, BuildSession, SiteBuild, SiteBuildGuard, diagnostic,
};

#[derive(Default)]
pub(crate) struct BuildAttemptProducers {
    pub(crate) resources: BuildResources,
    pub(crate) cancellation: crate::cancellation::BuildCancellation,
    pub(crate) reuse: Option<crate::compiler::CompilationReuse>,
    pub(crate) generated_files: Vec<crate::output::GeneratedFile>,
    /// Accepted icon resources; preparation checks their configuration,
    /// source identities, bytes, and directory membership before reuse.
    pub(crate) icons: Option<crate::icon::IconInventory>,
    pub(crate) images: Option<std::sync::Arc<crate::image::output::ImageOutputs>>,
    pub(crate) seo: Option<std::sync::Arc<crate::seo::SeoCompilation>>,
    pub(crate) reference_reuse: Option<crate::site::references::References>,
    /// Previous successful root Bundle export for exporter-level byte reuse.
    pub(crate) previous_bundle_entries: Option<tola_typst::BundleEntries>,
    pub(crate) configured_assets: Option<crate::asset::ConfiguredAssetInventory>,
    pub(crate) configured_asset_changes: Option<AcceptedFileChanges>,
    /// Published inventory whose membership is unchanged according to filesystem
    /// events. `None` triggers discovery after before-build hooks.
    pub(crate) content_inventory: Option<std::sync::Arc<[crate::content::ContentUnit]>>,
    pub(crate) inputs: crate::compiler::BuildInputs,
    pub(crate) font_read_paths: Vec<std::path::PathBuf>,
    /// Successful source generation may be newer than the installed revision.
    pub(crate) source_hooks: crate::hooks::SourceHookOutputs,
    pub(crate) failed_hook_outputs: Option<crate::hooks::FailedHookOutputs>,
    /// Whether the hooks this attempt ran wrote exactly what the accepted revision held.
    pub(crate) hook_outputs_unchanged: bool,
    /// Reusable `generate-outputs` products from the accepted development revision.
    pub(crate) previous_hook_reuse: Option<crate::hooks::ReusableHookOutputs>,
}

impl BuildAttemptProducers {
    fn bundle_cancellation(&self) -> tola_typst::BundleCancellation {
        self.cancellation.bundle_cancellation()
    }

    /// Discard the reuse that only an observed filesystem event justified.
    ///
    /// A source generator may have rewritten the inputs those decisions were based on, so
    /// every field whose validity came from an accepted event — the compilation, the previous
    /// export, the configured-asset inventory and its changes, and the content inventory — is
    /// withdrawn. Producers that re-prove their own evidence (icons, images, SEO, reference
    /// groups) keep the reuse the caller handed them.
    fn clear_pre_scan_reuse(&mut self) {
        self.reuse = None;
        self.previous_bundle_entries = None;
        self.configured_assets = None;
        self.configured_asset_changes = None;
        self.content_inventory = None;
    }

    pub(crate) fn inputs_mut(&mut self) -> &mut crate::compiler::BuildInputs {
        &mut self.inputs
    }

    pub(crate) fn into_inputs(self) -> BuildAttemptInputs {
        BuildAttemptInputs {
            compiler: self.inputs,
            font_read_paths: self.font_read_paths,
            source_hooks: self.source_hooks,
            failed_hook_outputs: self.failed_hook_outputs,
        }
    }
}

/// Build the entire site from one root Bundle for inspection or custom scheduling.
///
/// The attempt runs the pre-publication hook stages its mode selects. Use
/// [`BuildRequest::hook_execution`] on a prepared attempt to build from
/// existing inputs instead, and [`BuildSession::build_and_write`] for ordinary
/// repeated disk builds.
pub fn build_site(config: &ResolvedSiteConfig, mode: BuildMode) -> Result<SiteBuild> {
    let request = BuildRequest::new(mode);
    let mut session = BuildSession::new();
    SiteBuildGuard::build(
        &mut session,
        std::sync::Arc::new(config.clone()),
        request,
        || {},
    )
    .map(SiteBuildGuard::release)
    .map_err(BuildFailure::into_error)
}

/// Derivation generations a completed build keeps: the one this build produced, plus one
/// earlier one, so an edit can return a file to text an earlier build already derived from.
/// Measured as in-process live heap across consecutive development rebuilds of a 300-page
/// site, a retained generation costs about 3 MB; a superseded one buys nothing beyond that
/// return.
const RETAINED_DERIVATION_GENERATIONS: usize = 2;

pub(super) fn build_site_inner(
    config: &ResolvedSiteConfig,
    mode: BuildMode,
    hook_execution: crate::hooks::HookExecution,
    reference_checks: super::ReferenceChecks,
    producers: &mut BuildAttemptProducers,
    prepare_compiler: impl FnOnce(
        &mut BuildAttemptProducers,
        bool,
    ) -> Result<crate::compiler::TypstHost>,
) -> Result<SiteBuild> {
    let build_started = std::time::Instant::now();
    let built = (|| {
        producers.cancellation.ensure_active()?;
        let before_build = hooks::run_before_build_hooks_with_cancellation(
            config,
            mode,
            hook_execution.clone(),
            Some(&producers.cancellation),
        )
        .map_err(|failure| {
            producers.source_hooks = failure.completed().clone();
            producers.failed_hook_outputs = failure.partial_outputs().cloned();
            anyhow::Error::new(failure)
        })?;
        let hooks_executed = before_build.hooks_executed();
        producers.source_hooks = before_build.into_source_hooks();
        let before_build_elapsed = build_started.elapsed();
        producers.cancellation.ensure_active()?;

        let typst_host = prepare_compiler(producers, hooks_executed)?;
        if hooks_executed && !producers.hook_outputs_unchanged {
            producers.clear_pre_scan_reuse();
        }
        let built = build_site_candidate(
            config,
            &typst_host,
            mode,
            hook_execution,
            reference_checks,
            producers,
            build_started,
            before_build_elapsed,
        );
        producers.font_read_paths = typst_host.font_read_paths();
        typst_host.evict_stale_file_cache_entries(TypstHost::RETAINED_FILE_CACHE_EPOCHS);
        typst::comemo::evict(RETAINED_DERIVATION_GENERATIONS);
        built
    })();
    built.map_err(|error| diagnostic::with_site(error, config.get_root()))
}

#[allow(clippy::too_many_arguments)]
fn build_site_candidate(
    config: &ResolvedSiteConfig,
    typst_host: &crate::compiler::TypstHost,
    mode: BuildMode,
    hook_execution: crate::hooks::HookExecution,
    reference_checks: super::ReferenceChecks,
    producers: &mut BuildAttemptProducers,
    build_started: std::time::Instant,
    before_build_elapsed: std::time::Duration,
) -> Result<SiteBuild> {
    let mut warnings = tola_typst::Diagnostics::new();
    let mut build_diagnostics = Vec::new();
    producers.cancellation.ensure_active()?;
    // Capture discovery identity before later hooks can change inputs.
    let content_root = crate::filesystem::normalize_existing_prefix(&config.build.content_dir);
    let checked = (|| -> Result<_> {
        let producers_started = build_started.elapsed();
        let mut compiled_outputs = compile_site_program(
            config,
            typst_host,
            &mut warnings,
            &mut build_diagnostics,
            producers,
        )?;
        compiled_outputs.add_generated_files(
            std::mem::take(&mut producers.generated_files),
            &producers.cancellation,
        )?;
        let previous_hook_reuse = producers.previous_hook_reuse.take();
        let compiled_outputs = compiled_outputs.generate_outputs(
            config,
            mode,
            hook_execution.clone(),
            previous_hook_reuse.as_ref(),
            &producers.cancellation,
        )?;
        let compile_elapsed = build_started.elapsed();
        producers.cancellation.ensure_active()?;
        let candidate = compiled_outputs.seal(
            config,
            &producers.cancellation,
            reference_checks,
            &mut build_diagnostics,
        )?;
        producers.cancellation.ensure_active()?;
        let total_elapsed = build_started.elapsed();
        tracing::debug!(
            target: "tola::compile",
            before_build_ms = before_build_elapsed.as_secs_f64() * 1000.0,
            resources_ms = (producers_started - before_build_elapsed).as_secs_f64() * 1000.0,
            producers_ms = (compile_elapsed - producers_started).as_secs_f64() * 1000.0,
            seal_ms = (total_elapsed - compile_elapsed).as_secs_f64() * 1000.0,
            total_ms = total_elapsed.as_secs_f64() * 1000.0,
            "completed site candidate phases"
        );
        Ok(candidate)
    })();
    build_diagnostics.extend(crate::compiler::warning_diagnostics(config, &mut warnings));
    let candidate = match checked {
        Ok(candidate) => candidate,
        Err(error) if crate::cancellation::is_cancelled(&error) || build_diagnostics.is_empty() => {
            return Err(error);
        }
        Err(error) => {
            build_diagnostics.extend(diagnostic::for_error(&error, config.get_root()));
            return Err(
                crate::diagnostic::DiagnosticError::attach(error, build_diagnostics).into(),
            );
        }
    };
    Ok(SiteBuild {
        config: std::sync::Arc::new(config.clone()),
        mode,
        cancellation: producers.cancellation.clone(),
        index: candidate.index,
        graph: candidate.graph,
        cache_update: BuildCacheUpdate::new(
            candidate.producer_caches,
            candidate.content,
            content_root,
            candidate.references.clone(),
        ),
        diagnostics: build_diagnostics,
        references: candidate.references,
        source_hooks: producers.source_hooks.clone(),
    })
}

/// Collected producer outputs. Generated files and output commands must be added
/// before sealing the graph.
pub(super) struct CompiledSiteOutputs {
    outputs: crate::output::graph::OutputGraphBuilder,
    documents: Vec<crate::site::HtmlPage>,
    html_inventories: std::collections::BTreeMap<
        tola_address::OutputPath,
        std::sync::Arc<tola_typst::HtmlDocumentInventory>,
    >,
    producer_caches: crate::build::RetainedProducerCaches,
    content: std::sync::Arc<[crate::content::ContentUnit]>,
    reference_reuse: Option<crate::site::references::References>,
}

impl CompiledSiteOutputs {
    pub(super) fn add_generated_files(
        &mut self,
        files: Vec<crate::output::GeneratedFile>,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> Result<()> {
        for file in files {
            cancellation.ensure_active()?;
            self.outputs.insert(file.into_file())?;
        }
        Ok(())
    }

    pub(super) fn generate_outputs(
        mut self,
        config: &ResolvedSiteConfig,
        mode: super::BuildMode,
        hook_execution: crate::hooks::HookExecution,
        previous: Option<&crate::hooks::ReusableHookOutputs>,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> Result<Self> {
        if !hook_execution.runs() {
            self.producer_caches.hook_reuse = crate::hooks::ReusableHookOutputs::default();
            return Ok(self);
        }
        let generated = crate::hooks::generate_outputs(
            config,
            self.outputs.outputs(),
            mode,
            previous,
            cancellation,
        )?;
        for ownership in generated.root_ownerships {
            self.outputs.own_root(ownership)?;
        }
        for output in generated.outputs {
            self.outputs.insert(output)?;
        }
        self.producer_caches.hook_reuse = generated.reuse;
        Ok(self)
    }

    pub(super) fn seal(
        self,
        config: &ResolvedSiteConfig,
        cancellation: &crate::cancellation::BuildCancellation,
        reference_checks: super::ReferenceChecks,
        diagnostics: &mut Vec<crate::diagnostic::Diagnostic>,
    ) -> Result<SealedSiteCandidate> {
        cancellation.ensure_active()?;
        let graph = self.outputs.finish();
        cancellation.ensure_active()?;
        let index = SiteIndex::from_output_graph(&graph, self.documents, self.html_inventories)?;
        cancellation.ensure_active()?;
        let references = crate::site::references::References::from_site(
            &index,
            config.url_mount(),
            &self.producer_caches.site_program.world,
            cancellation,
            self.reference_reuse.as_ref(),
        )?;
        cancellation.ensure_active()?;
        if reference_checks == super::ReferenceChecks::Full {
            let reference_diagnostics = crate::site::references::diagnostic::diagnostics(
                &references,
                config,
                cancellation,
            )?;
            if reference_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == crate::diagnostic::Severity::Error)
            {
                return Err(crate::diagnostic::DiagnosticError::new(
                    "a page links to a target this site does not publish",
                    reference_diagnostics,
                )
                .into());
            }
            diagnostics.extend(reference_diagnostics);
        }
        if crate::output::PageAvailability::from_outputs(graph.outputs())
            == crate::output::PageAvailability::Empty
        {
            diagnostics.push(super::diagnostic::no_pages_diagnostic(config));
        }
        let publishes_not_found_page = graph.outputs().iter().any(|output| {
            output.path().as_str() == "404.html"
                && output.kind() == crate::output::graph::OutputKind::HtmlDocument
        });
        if !publishes_not_found_page {
            diagnostics.push(super::diagnostic::not_found_missing_diagnostic(config));
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

fn reexport_cached_site_program(
    config: &ResolvedSiteConfig,
    cached: &crate::compiler::SiteProgramCache,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<crate::compiler::SiteProgramCache> {
    let compilation = std::sync::Arc::clone(&cached.compilation);
    let exported = crate::compiler::bundle::export(
        config,
        &compilation,
        &cached.world,
        &cancellation.bundle_cancellation(),
        Some(&cached.bundle_entries),
    )?;
    let mut outputs = crate::output::graph::OutputGraphBuilder::new();
    crate::compiler::outputs::insert_reexported_bundle(
        &mut outputs,
        exported.bundle_entries.iter().cloned(),
        &cached.outputs,
    )?;
    Ok(crate::compiler::SiteProgramCache {
        root: cached.root.clone(),
        documents: exported.documents,
        html_inventories: exported.html_inventories,
        outputs: outputs.finish(),
        diagnostics: exported.diagnostics,
        payload_diagnostics: cached.payload_diagnostics.clone(),
        pretty_html: !config.build.minify.html,
        minified_languages: cached.minified_languages,
        entry: cached.entry.clone(),
        bundle_entries: exported.bundle_entries,
        compilation,
        world: std::sync::Arc::clone(&cached.world),
    })
}

/// Compile the root Bundle and collect built-in outputs with configured assets.
/// Native files and output commands join this open graph before sealing. Reused
/// and compiled Bundles retain their own read evidence and diagnostics.
pub(super) fn compile_site_program(
    config: &ResolvedSiteConfig,
    typst_host: &TypstHost,
    warnings: &mut tola_typst::Diagnostics,
    diagnostics: &mut Vec<crate::diagnostic::Diagnostic>,
    producers: &mut BuildAttemptProducers,
) -> Result<CompiledSiteOutputs> {
    let cancellation = producers.cancellation.clone();
    let previous_assets = producers.configured_assets.take();
    let configured_changes = producers.configured_asset_changes.take();
    let subscriber = tracing::dispatcher::get_default(Clone::clone);
    let span = tracing::Span::current();
    // The producers below run beside each other rather than on the CPU pool: each
    // one reads its own directories and files directly, so keeping them here is what
    // lets the pool stay available for the compilation work that follows.
    let produced = std::thread::scope(
        |scope| -> Result<(
            crate::asset::ConfiguredAssetInventory,
            PreparedProducerInputs,
        )> {
            let configured_assets = scope.spawn(|| {
                let _subscriber = tracing::dispatcher::set_default(&subscriber);
                let _entered = span.enter();
                let started = std::time::Instant::now();
                let assets = crate::asset::render_configured_asset_inventory(
                    config,
                    &producers.resources.source_boundary(config),
                    &cancellation,
                    configured_changes.as_ref().and(previous_assets.as_ref()),
                    configured_changes
                        .as_ref()
                        .map_or(&[], super::AcceptedFileChanges::paths),
                );
                tracing::debug!(target: "tola::compile",
                    assets_ms = started.elapsed().as_secs_f64() * 1000.0,
                    "prepared configured assets");
                assets
            });
            let prepared = scope.spawn(|| -> Result<PreparedProducerInputs> {
                let _subscriber = tracing::dispatcher::set_default(&subscriber);
                let _entered = span.enter();
                cancellation.ensure_active()?;
                producers
                    .resources
                    .source_boundary(config)
                    .check(&config.build.content_dir)?;
                let content = match &producers.content_inventory {
                    Some(content) => std::sync::Arc::clone(content),
                    None => crate::content::discover_content_units_for_config_with_cancellation(
                        config,
                        &cancellation,
                    )?
                    .into(),
                };
                let icons = crate::package::prepare_icons(
                    config,
                    &producers.resources,
                    &cancellation,
                    &mut producers.icons,
                    &Default::default(),
                )?;
                let typst_host = typst_host.with_icons(icons.collections());
                Ok(PreparedProducerInputs {
                    content,
                    icons,
                    typst_host,
                })
            });
            // A configured asset's published bytes decide the mounted URL its
            // declaration resolves to, so the root Bundle waits for the inventory
            // rendered beside the preparation above. That keeps an asset error ahead
            // of a producer error, and both ahead of a compiler error, independent of
            // completion order.
            let configured_assets = configured_assets
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))?;
            let prepared = prepared
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))?;
            Ok((configured_assets, prepared))
        },
    );
    cancellation.ensure_active()?;
    let (configured_asset_inventory, prepared) = produced?;
    let PreparedProducerInputs {
        content,
        icons,
        typst_host,
    } = prepared;
    let packages = crate::package::PackageInputs { icons };
    let compiled = compile_root_bundle(
        config,
        &typst_host,
        &content,
        &configured_asset_inventory,
        producers,
        warnings,
        diagnostics,
    )?;
    let CompiledRootBundle {
        site_program,
        dependencies,
        source_analysis,
    } = compiled;
    diagnostics.extend_from_slice(&site_program.payload_diagnostics);
    let mut outputs = crate::output::graph::OutputGraphBuilder::new();
    insert_code_stylesheet(&mut outputs)?;
    let icons = packages.icons;
    producers.cancellation.ensure_active()?;
    outputs.insert_graph(&site_program.outputs)?;
    collect_configured_assets(
        &mut outputs,
        &configured_asset_inventory,
        &producers.cancellation,
    )?;
    diagnostics.extend(
        configured_asset_inventory
            .minify_warnings()
            .into_iter()
            .map(crate::asset::AssetMinifyWarning::diagnostic),
    );
    let (images, image_reads) = crate::image::output::ImageOutputs::prepare(
        &site_program.world,
        dependencies.read_evidence(),
        producers.images.as_deref(),
        config.get_root(),
        &producers.resources,
        &mut producers.inputs,
        &cancellation,
    )?;
    let dependencies = dependencies.with_additional_program_reads(
        config.get_root(),
        image_reads.reads,
        image_reads.package_checks,
        &typst_host,
    )?;
    images.insert_into(&mut outputs, &cancellation)?;
    let published_icons = crate::icon::output::IconOutputs::prepare(
        dependencies.read_evidence(),
        &icons.collections(),
        &producers.cancellation,
    )?;
    published_icons.insert_into(&mut outputs, &producers.cancellation)?;
    let seo = collect_seo_outputs(
        config,
        &mut outputs,
        &site_program.compilation,
        &site_program.world,
        &producers.cancellation,
        warnings,
        producers.seo.as_ref(),
    )?;
    Ok(CompiledSiteOutputs {
        outputs,
        documents: site_program.documents.clone(),
        html_inventories: site_program.html_inventories.clone(),
        producer_caches: crate::build::RetainedProducerCaches {
            host: typst_host,
            dependencies,
            source_analysis,
            site_program,
            configured_assets: configured_asset_inventory,
            icons,
            images,
            seo,
            hook_outputs: producers.source_hooks.clone(),
            hook_reuse: crate::hooks::ReusableHookOutputs::default(),
        },
        content,
        reference_reuse: producers.reference_reuse.clone(),
    })
}

/// Producer values prepared beside the configured asset inventory, before the
/// root Bundle compiles.
struct PreparedProducerInputs {
    content: std::sync::Arc<[crate::content::ContentUnit]>,
    icons: crate::icon::IconInventory,
    typst_host: TypstHost,
}

struct CompiledRootBundle {
    site_program: crate::compiler::SiteProgramCache,
    dependencies: crate::compiler::CompilationDependencies,
    source_analysis: crate::compiler::analysis::SourceAnalysisCache,
}

fn compile_root_bundle(
    config: &ResolvedSiteConfig,
    typst_host: &TypstHost,
    content: &[crate::content::ContentUnit],
    configured_assets: &crate::asset::ConfiguredAssetInventory,
    producers: &mut BuildAttemptProducers,
    warnings: &mut tola_typst::Diagnostics,
    // Every later step of this attempt may fail, and a failed root Bundle reports no other
    // source-analysis output, so the declaration warnings join the attempt's diagnostics here.
    diagnostics: &mut Vec<crate::diagnostic::Diagnostic>,
) -> Result<CompiledRootBundle> {
    let cancellation = producers.bundle_cancellation();
    let package_bindings =
        crate::package::SiteBindings::from_config(config, configured_assets.asset_urls(config)?);
    let reuse = producers.reuse.take();
    let reuse = reuse.as_ref();
    let source_analysis_reuse = reuse
        .map(crate::compiler::CompilationReuse::source_analysis_reuse)
        .unwrap_or_default();
    let previous_dependencies = source_analysis_reuse.dependencies();
    if let Some(crate::compiler::CompilationReuse::SiteProgram {
        site_program: cached_site_program,
        dependencies,
        source_analysis,
    }) = reuse
    {
        let sources = crate::content::SourceSet::without_metadata(content, config)?;
        if cached_site_program.matches_site(config)
            && cached_site_program.matches_minification(config)
            && source_analysis.matches_content(
                config.get_root(),
                &package_bindings,
                content,
                &sources,
            )
            && dependencies.virtual_reads_match(
                |evidence| typst_host.virtual_read_matches(evidence),
                &producers.cancellation,
            )?
        {
            warnings.extend_distinct(source_analysis.diagnostics());
            warnings.extend_distinct(&cached_site_program.diagnostics);
            let site_program = if cached_site_program.matches_export_config(config) {
                (**cached_site_program).clone()
            } else {
                reexport_cached_site_program(config, cached_site_program, &producers.cancellation)?
            };
            dependencies.record_all_reads(producers.inputs_mut());
            tracing::debug!(target: "tola::compile", "reused background SiteProgram producer");
            diagnostics.extend_from_slice(source_analysis.declaration_warnings());
            return Ok(CompiledRootBundle {
                site_program,
                dependencies: (**dependencies).clone(),
                source_analysis: (**source_analysis).clone(),
            });
        }
    }
    if matches!(
        reuse,
        Some(crate::compiler::CompilationReuse::SourceAnalysis { .. })
    ) {
        tracing::debug!(target: "tola::compile", "retained source analysis entries for exact reuse");
    }
    let analysis_started = std::time::Instant::now();
    let evaluated = crate::compiler::bundle::evaluate(
        config,
        typst_host,
        content,
        package_bindings,
        &cancellation,
        source_analysis_reuse,
        producers.inputs_mut(),
    )
    .map_err(crate::compiler::analysis::SourceAnalysisFailure::into_error)?;
    let sources = evaluated.sources();
    let source_analysis = evaluated.source_analysis().clone();
    tracing::debug!(target: "tola::compile",
        source_eval_ms = analysis_started.elapsed().as_secs_f64() * 1000.0,
        metadata_rounds = source_analysis.metadata_rounds(),
        "evaluated source metadata");
    warnings.extend_distinct(source_analysis.diagnostics());
    diagnostics.extend_from_slice(source_analysis.declaration_warnings());
    let reused_dependency_readers = source_analysis.reused_dependency_readers();
    let mut bundle_outputs = crate::output::graph::OutputGraphBuilder::new();
    let previous_bundle_entries = producers.previous_bundle_entries.clone();
    let mut realized = crate::compiler::bundle::realize(
        config,
        typst_host,
        &cancellation,
        &evaluated,
        producers.inputs_mut(),
    )
    .map_err(crate::compiler::bundle::RealizeFailure::into_error)?;
    warnings.extend_distinct(realized.compilation.diagnostics());
    let mut payload_diagnostics = Vec::new();
    crate::compiler::minify_generated_payloads(
        &mut realized.compilation,
        &realized.world,
        crate::compiler::minified_languages(config),
        &mut payload_diagnostics,
        &cancellation,
    )
    .map_err(|tola_typst::HtmlRawTextError::Cancelled| tola_typst::CompileError::Cancelled)
    .context("Tola could not minify the CSS or JavaScript the generated HTML includes")?;
    let compilation = std::sync::Arc::new(realized.compilation);
    let compiled_bundle = crate::compiler::bundle::export(
        config,
        &compilation,
        &realized.world,
        &cancellation,
        previous_bundle_entries.as_ref(),
    )?;
    let documents = compiled_bundle.documents;
    let html_inventories = compiled_bundle.html_inventories;
    crate::compiler::outputs::insert_bundle(
        &mut bundle_outputs,
        compiled_bundle.bundle_entries.iter().cloned(),
        "site-program",
        &compiled_bundle.document_outputs,
    )?;
    let bundle_outputs = bundle_outputs.finish();
    let diagnostics = compiled_bundle.diagnostics;
    let bundle_entries = compiled_bundle.bundle_entries;

    tracing::debug!(
        target: "tola::compile",
        source_units = evaluated.source_read_count(),
        program_reads = compilation.file_reads().len(),
        "compiled root Bundle dependencies"
    );
    let content_sources = sources
        .sources()
        .iter()
        .map(|source| source.source().to_path_buf());
    let reader_evidence = evaluated.dependency_readers(&compilation);
    let reused_dependencies = previous_dependencies.map(|dependencies| {
        crate::compiler::ReusedDependencyReaders::new(dependencies, &reused_dependency_readers)
    });
    let dependencies = crate::compiler::CompilationDependencies::new(
        config.get_root(),
        content_sources,
        std::iter::once(config.build.entry.clone()),
        reader_evidence,
        evaluated.package_checks(&compilation).cloned(),
        typst_host,
        reused_dependencies,
    )?;
    let site_program = crate::compiler::SiteProgramCache {
        root: crate::filesystem::normalize_path(config.get_root()),
        documents,
        html_inventories,
        outputs: bundle_outputs,
        diagnostics,
        payload_diagnostics,
        pretty_html: !config.build.minify.html,
        minified_languages: crate::compiler::minified_languages(config),
        entry: config.build.entry.clone(),
        bundle_entries,
        compilation: compiled_bundle.compilation,
        world: compiled_bundle.world,
    };
    Ok(CompiledRootBundle {
        site_program,
        dependencies,
        source_analysis,
    })
}

fn insert_code_stylesheet(outputs: &mut crate::output::graph::OutputGraphBuilder) -> Result<()> {
    // The code stylesheet is published for every build: a site links it from its head, and that
    // URL has to resolve whether or not this build renders any code. Its bytes are the shared
    // consumer stylesheet, minified here once per build.
    let stylesheet = tola_packages::TolaPackage::Code
        .file(std::path::Path::new(tola_packages::CODE_STYLESHEET_FILE))
        .context("Tola could not read the code stylesheet `@tola/code` provides")?;
    let stylesheet = tola_minify::minify_css(&stylesheet)
        .context("Tola could not minify the code stylesheet `@tola/code` provides")?;
    outputs.insert_system(
        "code-stylesheet",
        &tola_packages::code_stylesheet_output(),
        crate::output::semantics::OutputDeclaration::opaque(
            crate::output::semantics::ResponseMediaType::CSS,
        ),
        stylesheet.into_bytes(),
    )?;
    Ok(())
}

fn collect_configured_assets(
    outputs: &mut crate::output::graph::OutputGraphBuilder,
    configured_assets: &crate::asset::ConfiguredAssetInventory,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<()> {
    for (asset, bytes) in configured_assets.entries() {
        cancellation.ensure_active()?;
        outputs.insert_configured_asset(
            asset.logical_source.clone(),
            asset.output.clone(),
            asset.declaration().clone(),
            std::sync::Arc::clone(bytes),
        )?;
    }
    Ok(())
}

fn collect_seo_outputs(
    config: &ResolvedSiteConfig,
    outputs: &mut crate::output::graph::OutputGraphBuilder,
    compilation: &std::sync::Arc<tola_typst::BundleCompilation>,
    world: &tola_typst::TypstWorld,
    cancellation: &crate::cancellation::BuildCancellation,
    warnings: &mut tola_typst::Diagnostics,
    previous: Option<&std::sync::Arc<crate::seo::SeoCompilation>>,
) -> Result<std::sync::Arc<crate::seo::SeoCompilation>> {
    cancellation.ensure_active()?;
    let started = std::time::Instant::now();
    let rendered = crate::seo::SeoCompilation::prepare(config, compilation, cancellation, previous)
        .map_err(|error| match error {
            crate::seo::RenderError::Declaration(error) => {
                let mut diagnostics =
                    tola_typst::Diagnostics::resolve(world, &[error.source_diagnostic()]);
                attach_importers(&mut diagnostics, compilation);
                anyhow::Error::new(tola_typst::CompileError::from_resolved(diagnostics))
            }
            crate::seo::RenderError::HtmlExport(error) => {
                let raw = error
                    .raw_diagnostics()
                    .expect("HTML export errors retain source diagnostics");
                let mut diagnostics = tola_typst::Diagnostics::resolve(world, raw);
                attach_importers(&mut diagnostics, compilation);
                anyhow::Error::new(tola_typst::CompileError::from_resolved(diagnostics))
            }
            crate::seo::RenderError::Cancelled(error) => anyhow::Error::new(error),
            crate::seo::RenderError::Compiler(error) => anyhow::Error::new(error),
        })?;
    cancellation.ensure_active()?;
    tracing::debug!(target: "tola::compile",
        seo_ms = started.elapsed().as_secs_f64() * 1000.0,
        "rendered SEO outputs");
    let mut diagnostics = tola_typst::Diagnostics::resolve(world, rendered.warnings());
    attach_importers(&mut diagnostics, compilation);
    warnings.extend_distinct(&diagnostics);
    for output in rendered.outputs() {
        cancellation.ensure_active()?;
        outputs.insert_system(
            output.producer,
            output.url.as_str().trim_start_matches('/'),
            output.declaration.clone(),
            std::sync::Arc::clone(&output.bytes),
        )?;
    }
    Ok(rendered)
}

/// Name the site files behind each late SEO diagnostic that points into a package.
///
/// A declaration a package helper writes reports at the helper's own source, so those files are
/// where the site author starts looking.
fn attach_importers(
    diagnostics: &mut tola_typst::Diagnostics,
    compilation: &tola_typst::BundleCompilation,
) {
    diagnostics.attach_imported_by(|package| compilation.files_importing(package).to_vec());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::tests::*;
    use crate::build::*;

    use crate::config::section::{
        AssetFileDeclaration, AssetTreeDeclaration, AssetUrl, AssetUrlPrefix,
    };
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn matching_media_types_resolve_references() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("content")).unwrap();
        let site_program = root.join("site.typ");
        fs::write(
            &site_program,
            r#"#document("index.html", html.html(
  html.head(
    html.link(rel: "stylesheet", href: "/site.css")
    + html.script(src: "/classic.js")
    + html.script(type: "module", src: "/module.js")
    + html.link(rel: "modulepreload", href: "/dependency.js")
  )
  + html.body[Site]
))
#asset("site.css", "body {}")
#asset("classic.js", "")
#asset("module.js", "")
#asset("dependency.js", "")"#,
        )
        .unwrap();
        let config = site_config(root);

        let build = build_site(&config, BuildMode::Production).unwrap();

        assert_eq!(build.references.references().len(), 4);
        assert!(build.references.references().all(|reference| {
            matches!(
                reference.resolution(),
                crate::site::references::ReferenceResolution::Found { .. }
            )
        }));
    }

    /// The identity of the bytes this build published at `path`.
    fn published_identity(graph: &crate::output::graph::OutputGraph, path: &str) -> String {
        tola_typst::ContentDigest::of(output_bytes(graph, path)).to_hex()
    }

    #[test]
    fn asset_url_changes_with_its_bytes() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("content")).unwrap();
        let script = root.join("app.js");
        fs::write(&script, "export const version = 1;\n").unwrap();
        let stylesheet = root.join("site.css");
        fs::write(&stylesheet, "body { color: red; }\n").unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            "#import \"@tola/address:0.0.0\": asset-url\n#document(\"index.html\")[#asset-url(\"/app.js\")]",
        )
        .unwrap();
        let mut config = site_config(root);
        config.assets.cache_busting = true;
        config.assets.files = vec![AssetFileDeclaration::new(
            &script,
            AssetUrl::parse("/app.js").unwrap(),
        )];

        let mut session = crate::build::BuildSession::new();
        let host = compiler_host(&config).unwrap();
        let first = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers::default(),
        )
        .unwrap();
        let first_identity = published_identity(&first.graph, "app.js");
        let first_html =
            String::from_utf8_lossy(output_bytes(&first.graph, "index.html")).into_owned();
        assert!(
            first_html.contains(&format!("/app.js?h={first_identity}")),
            "{first_html}"
        );
        install_build(&mut session, first);

        let mut grown = config.clone();
        grown.assets.files.push(AssetFileDeclaration::new(
            &stylesheet,
            AssetUrl::parse("/site.css").unwrap(),
        ));
        let decision = session.rebuild_decision(std::slice::from_ref(&stylesheet), false);
        let reuse = session.reusable_compilation(&decision, true).unwrap();
        let second = build_site_with_host(
            &grown,
            &compiler_host(&grown).unwrap(),
            BuildMode::Production,
            &mut BuildAttemptProducers {
                reuse: Some(reuse),
                previous_bundle_entries: session.bundle_entries(),
                configured_assets: session.configured_assets().cloned(),
                configured_asset_changes: Some(AcceptedFileChanges::from_watcher(vec![
                    stylesheet.clone(),
                ])),
                ..BuildAttemptProducers::default()
            },
        )
        .unwrap();
        assert_eq!(
            output_bytes(&second.graph, "index.html"),
            first_html.as_bytes()
        );
        install_build(&mut session, second);

        fs::write(&script, "export const version = 2;\n").unwrap();
        let decision = session.rebuild_decision(std::slice::from_ref(&script), false);
        let reuse = session.reusable_compilation(&decision, true).unwrap();
        let third = build_site_with_host(
            &grown,
            &compiler_host(&grown).unwrap(),
            BuildMode::Production,
            &mut BuildAttemptProducers {
                reuse: Some(reuse),
                previous_bundle_entries: session.bundle_entries(),
                configured_assets: session.configured_assets().cloned(),
                configured_asset_changes: Some(AcceptedFileChanges::from_watcher(vec![
                    script.clone(),
                ])),
                ..BuildAttemptProducers::default()
            },
        )
        .unwrap();
        let third_identity = published_identity(&third.graph, "app.js");
        let third_html =
            String::from_utf8_lossy(output_bytes(&third.graph, "index.html")).into_owned();

        assert_ne!(
            third_identity, first_identity,
            "changed bytes, changed identity"
        );
        assert!(
            third_html.contains(&format!("/app.js?h={third_identity}")),
            "{third_html}"
        );
        assert!(
            !third_html.contains(&format!("/app.js?h={first_identity}")),
            "{third_html}"
        );
        assert!(has_output(&third.graph, "app.js"));
        assert!(
            has_output(&third.graph, "site.css"),
            "the declaration added in between publishes its declared name"
        );
    }

    /// A failed build keeps the reads it already recorded, whichever phase failed.
    #[test]
    fn failed_builds_record_read_inputs() {
        struct Case {
            name: &'static str,
            /// Files to write under the site root, relative path first.
            files: &'static [(&'static str, &'static str)],
            /// Every file the failed build must have recorded reading.
            recorded: &'static [&'static str],
            /// Text the failure must name for the author to act on it.
            failure: &'static str,
        }
        let cases = [
            Case {
                name: "program evaluation",
                files: &[("site.typ", "#include \"templates/missing.typ\"")],
                recorded: &["site.typ", "templates/missing.typ"],
                failure: "missing.typ",
            },
            Case {
                name: "source analysis",
                files: &[
                    ("site.typ", "#document(\"index.html\")[Site]"),
                    ("content/index.typ", "#include \"../templates/missing.typ\""),
                ],
                recorded: &["templates/missing.typ"],
                failure: "missing.typ",
            },
            Case {
                name: "realization",
                files: &[
                    ("templates/shared.typ", "#let title = [Shared]"),
                    (
                        "site.typ",
                        "#import \"templates/shared.typ\": title\n#document(\"index.html\")[#title #context read(\"templates/missing.txt\")]",
                    ),
                ],
                recorded: &["site.typ", "templates/shared.typ", "templates/missing.txt"],
                failure: "missing.txt",
            },
        ];

        for case in cases {
            let directory = TempDir::new().unwrap();
            let root = directory.path();
            for (relative, body) in case.files {
                let path = root.join(relative);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, body).unwrap();
            }
            let config = site_config(root);
            let mut producers = BuildAttemptProducers::default();
            let error = match build_site_with_host(
                &config,
                &compiler_host(&config).unwrap(),
                BuildMode::Production,
                &mut producers,
            ) {
                Ok(_) => panic!("{} unexpectedly built", case.name),
                Err(error) => error,
            };

            let reads = producers.into_inputs().compiler.into_parts().0;
            for relative in case.recorded {
                let path = root.join(relative);
                let recorded = if path.exists() {
                    crate::filesystem::normalize_path(&path)
                } else {
                    crate::filesystem::normalize_existing_prefix(&path)
                };
                assert!(
                    reads.contains(&recorded),
                    "{}: {relative} not recorded",
                    case.name
                );
            }
            assert!(
                format!("{error:#}").contains(case.failure),
                "{}: {error:#}",
                case.name
            );
        }
    }

    #[test]
    fn cancelled_execution_writes_no_candidate() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(content.join("post.typ"), "Post").unwrap();
        fs::write(root.join("site.typ"), "#document(\"index.html\")[Hello]").unwrap();

        let config = site_config(root);
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        canceller.cancel();
        let mut producers = BuildAttemptProducers {
            cancellation,
            ..BuildAttemptProducers::default()
        };

        let error = match build_site_with_host(
            &config,
            &compiler_host(&config).unwrap(),
            BuildMode::Production,
            &mut producers,
        ) {
            Ok(_) => panic!("cancelled build unexpectedly produced a candidate"),
            Err(error) => error,
        };

        assert!(crate::cancellation::is_cancelled(&error));
    }

    #[test]
    fn url_syntax_in_tree_member_fails() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let content = root.join("content");
        let assets = root.join("assets");
        let output = root.join("public");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&assets).unwrap();
        fs::write(assets.join("fragment#name.txt"), b"asset").unwrap();
        let entry = root.join("site.typ");
        fs::write(&entry, "#document(\"index.html\")[Hello]").unwrap();

        let mut config = site_config(root);
        config.build.publish_dir = output.clone();
        config.assets.trees = vec![AssetTreeDeclaration::new(
            &assets,
            AssetUrlPrefix::parse("/assets").unwrap(),
        )];

        let error = build_failure(&config);
        let rendered = format!("{error:#}");
        assert!(rendered.contains("fragment#name.txt"), "{rendered}");
        assert!(!output.exists());
    }

    #[test]
    fn overlapping_asset_urls_write_nothing() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let content = root.join("content");
        let output = root.join("public");
        fs::create_dir_all(&content).unwrap();
        let first = root.join("download.bin");
        let second = root.join("readme.txt");
        fs::write(&first, b"download").unwrap();
        fs::write(&second, b"readme").unwrap();
        let entry = root.join("site.typ");
        fs::write(&entry, "#document(\"index.html\")[Hello]").unwrap();

        let mut config = site_config(root);
        config.build.publish_dir = output.clone();
        config.assets.files = vec![
            AssetFileDeclaration::new(&first, AssetUrl::parse("/download").unwrap()),
            AssetFileDeclaration::new(&second, AssetUrl::parse("/download/readme.txt").unwrap()),
        ];

        let _ = build_failure(&config);
        assert!(!output.exists());
    }

    /// ASCII slugification reads each source name in the site's declared language: 東京
    /// slugs as `toukyou` under `ja`, not the Chinese `dong-jing`.
    #[test]
    fn slugify_reads_site_language() {
        let program = r#"#import "@tola/address:0.0.0": join-route, output-of, slugify
#import "@tola/site:0.0.0": site
#import "@tola/source:0.0.0": all-sources

#for source in all-sources() {
  document(output-of(join-route(source.route-segments.map(segment => slugify(segment, mode: "ascii", language: site.language.lang)))))[#source.id]
}"#;
        let cases = [
            (
                "",
                &[
                    ("重庆.typ", "= 重庆"),
                    ("しんぶん.typ", "= しんぶん"),
                    ("한국어.typ", "= 한국어"),
                ][..],
                &[
                    "chong-qing/index.html",
                    "shinbun/index.html",
                    "hangugeo/index.html",
                ][..],
            ),
            (
                "[site]\nlanguage = \"ja\"",
                &[("東京.typ", "= 東京")][..],
                &["toukyou/index.html"][..],
            ),
        ];
        for (source, files, expected) in cases {
            let directory = TempDir::new().unwrap();
            let root = directory.path();
            let config = site_config_from(root, source);
            fs::write(&config.build.entry, program).unwrap();
            for (name, body) in files {
                fs::write(config.build.content_dir.join(name), body).unwrap();
            }

            let build = build_site(&config, BuildMode::Production).unwrap();
            let published = build
                .graph
                .outputs()
                .iter()
                .map(|output| output.path().as_str())
                .collect::<Vec<_>>();
            for route in expected {
                assert!(published.contains(route), "{route} missing: {published:?}");
            }
        }
    }

    #[test]
    fn seo_outputs_use_effective_site_url() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("content")).unwrap();
        let entry = root.join("site.typ");
        fs::write(&entry, r#"#import "@tola/web:0.0.0": feed, sitemap
#document("index.html", title: [Home])[Home]
#feed(output: "feed.xml", entries: ((id: "urn:home", target: "index.html", published: datetime(year: 2026, month: 1, day: 2)),))
#sitemap(targets: ("index.html",))"#).unwrap();
        let mut config = site_config_from(
            root,
            "[site]\norigin = \"https://example.test\"\nbase-path = \"/docs/\"",
        );
        config.site.title = "Mounted site".into();
        config.site.description = "Mounted SEO outputs".into();

        let build = build_site(&config, BuildMode::Production).unwrap();
        assert!(has_output(&build.graph, "feed.xml"));
        assert!(has_output(&build.graph, "sitemap.xml"));
        let feed = String::from_utf8_lossy(output_bytes(&build.graph, "feed.xml"));
        let sitemap = String::from_utf8_lossy(output_bytes(&build.graph, "sitemap.xml"));
        assert!(feed.contains("https://example.test/docs/"), "{feed}");
        assert!(sitemap.contains("https://example.test/docs/"), "{sitemap}");
    }

    #[test]
    fn asset_feed_url_conflict_writes_nothing() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("content")).unwrap();
        let entry = root.join("site.typ");
        fs::write(&entry, "#import \"@tola/web:0.0.0\": feed\n#document(\"index.html\")[Site]\n#feed(output: \"feed.xml\", title: [Feed], description: [Entries])").unwrap();
        let feed_source = root.join("feed.xml");
        fs::write(&feed_source, "configured feed bytes").unwrap();
        let output = root.join("public");
        let mut config = site_config_from(root, "[site]\norigin = \"https://example.test\"");
        config.build.publish_dir = output.clone();
        config.assets.files = vec![AssetFileDeclaration::new(
            &feed_source,
            AssetUrl::parse("/feed.xml").unwrap(),
        )];

        let error = build_failure(&config);
        assert!(error.chain().any(|cause| matches!(
            cause.downcast_ref::<crate::output::graph::OutputGraphError>(),
            Some(crate::output::graph::OutputGraphError::PathConflict {
                first_path, second_path, ..
            }) if first_path.as_str() == "feed.xml" && second_path.as_str() == "feed.xml"
        )));
        assert!(!output.exists());
    }

    /// A root Bundle that generates documents and assets publishes every one of them: a
    /// route loop in an imported helper, and a taxonomy over source metadata.
    #[test]
    fn generated_routes_publish_their_outputs() {
        // Documents emitted from an imported helper, beside an asset it declares.
        {
            let dir = TempDir::new().unwrap();
            let root = dir.path();
            fs::create_dir_all(root.join("content")).unwrap();
            fs::create_dir_all(root.join("routes")).unwrap();
            fs::write(
                root.join("site.typ"),
                "#import \"routes/routes.typ\": emit-routes\n#emit-routes()\n",
            )
            .unwrap();
            fs::write(
                root.join("routes/routes.typ"),
                "#let emit-routes() = include \"generated.typ\"",
            )
            .unwrap();
            fs::write(
                root.join("routes/generated.typ"),
                "#for slug in (\"alpha\", \"beta\") {\n  document(slug + \"/index.html\", [Route #slug])\n}\n#asset(\"generated/routes.txt\", \"alpha,beta\")\n",
            )
            .unwrap();
            let config = site_config(root);
            let output = config.build.publish_dir.clone();
            let build = build_and_publish(&config).unwrap();
            for path in ["alpha/index.html", "beta/index.html"] {
                assert!(has_output(&build.graph, path), "missing {path}");
                assert!(output.join(path).is_file(), "unpublished {path}");
            }
            assert_eq!(
                output_bytes(&build.graph, "generated/routes.txt"),
                b"alpha,beta"
            );
        }

        // Tag documents computed from the sources' own metadata.
        {
            let dir = TempDir::new().unwrap();
            let root = dir.path();
            fs::create_dir_all(root.join("content")).unwrap();
            fs::create_dir_all(root.join("routes")).unwrap();
            fs::write(
                root.join("content/alpha.typ"),
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: [Alpha], tags: (\"rust\", \"typst\")))",
            )
            .unwrap();
            fs::write(
                root.join("content/beta.typ"),
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: [Beta], tags: (\"rust\",)))",
            )
            .unwrap();
            fs::write(
                root.join("routes/taxonomy.typ"),
                "#let emit-taxonomies(sources) = {\n  for tag in (\"rust\", \"typst\") {\n    let members = sources.filter(source => source.meta.tags.contains(tag))\n    if members.len() > 0 {\n      let names = members.map(source => source.meta.title).join(\", \")\n      document(\"tags/\" + tag + \"/index.html\", [#names])\n    }\n  }\n}\n",
            )
            .unwrap();
            fs::write(
                root.join("site.typ"),
                "#import \"@tola/source:0.0.0\": all-sources\n#import \"routes/taxonomy.typ\": emit-taxonomies\n#emit-taxonomies(all-sources())\n",
            )
            .unwrap();
            let config = site_config(root);
            let output = config.build.publish_dir.clone();
            let build = build_and_publish(&config).unwrap();
            let rust = fs::read_to_string(output.join("tags/rust/index.html")).unwrap();
            assert!(rust.contains("Alpha") && rust.contains("Beta"), "{rust}");
            let typst = fs::read_to_string(output.join("tags/typst/index.html")).unwrap();
            assert!(
                typst.contains("Alpha") && !typst.contains("Beta"),
                "{typst}"
            );
            assert_eq!(build.index.address().pages().len(), 2);
        }
    }

    /// A document's page properties come from wherever the author wrote them: the
    /// `document` call's own arguments, or absent when they wrote none.
    #[test]
    fn page_properties_follow_document_metadata() {
        struct Case {
            program: &'static str,
            url: &'static str,
            title: Option<&'static str>,
            description: Option<&'static str>,
            author: &'static [&'static str],
        }
        for case in [
            Case {
                program: r#"#document("plain/index.html")[Plain]"#,
                url: "/plain/",
                title: None,
                description: None,
                author: &[],
            },
            Case {
                program: r#"#document("native/index.html", title: [Native Title], author: "Alice", description: [Native Summary])[Native]"#,
                url: "/native/",
                title: Some("Native Title"),
                description: Some("Native Summary"),
                author: &["Alice"],
            },
            Case {
                program: "#document(\"tags/rust/index.html\", title: [Native Rust])[\n  #import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: [Rust], pin: true))\n  = Rust\n]\n",
                url: "/tags/rust/",
                title: Some("Native Rust"),
                description: None,
                author: &[],
            },
        ] {
            let dir = TempDir::new().unwrap();
            let root = dir.path();
            fs::create_dir_all(root.join("content")).unwrap();
            fs::write(root.join("site.typ"), case.program).unwrap();
            let config = site_config(root);

            let snapshot = build_and_publish(&config).unwrap();
            let url = tola_address::UrlPath::parse(case.url).unwrap();
            let crate::site::Resource::Page { document } =
                snapshot.index.address().get_by_url(&url).unwrap()
            else {
                panic!("expected generated document at {}", case.url);
            };

            assert_eq!(
                document.properties.title.as_ref().map(|t| t.to_string()),
                case.title.map(str::to_owned),
                "{}",
                case.url
            );
            assert_eq!(
                document
                    .properties
                    .description
                    .as_ref()
                    .map(|d| d.to_string()),
                case.description.map(str::to_owned),
                "{}",
                case.url
            );
            assert_eq!(
                document.properties.author.as_slice(),
                case.author,
                "{}",
                case.url
            );
        }
    }

    pub(super) fn build_failure(config: &ResolvedSiteConfig) -> anyhow::Error {
        match build_and_publish(config) {
            Ok(_) => panic!("site unexpectedly built and published"),
            Err(error) => error,
        }
    }
}
