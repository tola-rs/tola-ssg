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
    pub(crate) icons: Option<crate::icon::IconSnapshot>,
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

fn build_site_candidate(
    config: &ResolvedSiteConfig,
    typst_host: &crate::compiler::TypstHost,
    mode: BuildMode,
    hook_execution: crate::hooks::HookExecution,
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
        let compiled_outputs = compiled_outputs.generate_outputs(
            config,
            mode,
            hook_execution.clone(),
            &producers.cancellation,
        )?;
        let compile_elapsed = build_started.elapsed();
        producers.cancellation.ensure_active()?;
        let candidate =
            compiled_outputs.seal(config, &producers.cancellation, &mut build_diagnostics)?;
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
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> Result<Self> {
        if !hook_execution.runs() {
            return Ok(self);
        }
        let generated =
            crate::hooks::generate_outputs(config, self.outputs.outputs(), mode, cancellation)?;
        for ownership in generated.root_ownerships {
            self.outputs.own_root(ownership)?;
        }
        for output in generated.outputs {
            self.outputs.insert(output)?;
        }
        Ok(self)
    }

    pub(super) fn seal(
        self,
        config: &ResolvedSiteConfig,
        cancellation: &crate::cancellation::BuildCancellation,
        diagnostics: &mut Vec<crate::diagnostic::Diagnostic>,
    ) -> Result<SealedSiteCandidate> {
        cancellation.ensure_active()?;
        let graph = self.outputs.finish();
        cancellation.ensure_active()?;
        let index = SiteIndex::from_output_graph(&graph, self.documents, self.html_inventories)?;
        cancellation.ensure_active()?;
        let references = crate::site::references::References::from_site(
            &index,
            &config.url_mount(),
            &self.producer_caches.site_program.world,
            cancellation,
            self.reference_reuse.as_ref(),
        )?;
        cancellation.ensure_active()?;
        let reference_diagnostics =
            crate::site::references::diagnostic::diagnostics(&references, config, cancellation)?;
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
        },
        content,
        reference_reuse: producers.reference_reuse.clone(),
    })
}

/// Producer values prepared beside the configured asset inventory, before the
/// root Bundle compiles.
struct PreparedProducerInputs {
    content: std::sync::Arc<[crate::content::ContentUnit]>,
    icons: crate::icon::IconSnapshot,
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
    use crate::config::section::build::BeforeBuildHookConfig;
    use crate::config::section::{
        AssetFileDeclaration, AssetTreeDeclaration, AssetUrl, AssetUrlPrefix,
    };
    use std::fs;
    use tempfile::TempDir;

    /// A check that reuses its source analysis leaves a build's published bytes untouched.
    ///
    /// The check owns no output; this states the other half of its contract at the layer that does:
    /// whatever the check derived or reused, the tree a build publishes over the same site is
    /// byte-for-byte what it published before.
    #[test]
    fn reused_check_leaves_published_bytes_unchanged() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("content")).unwrap();
        for index in 0..8 {
            fs::write(
                root.join(format!("content/doc-{index:02}.typ")),
                format!("= Document {index}\n\nBody {index}.\n"),
            )
            .unwrap();
        }
        fs::write(
            root.join("site.typ"),
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": route, route-to-output, slugify
#for source in all-sources() {
  let route = route(source.route-segments.map(segment => slugify(segment)))
  document(route-to-output(route), format: "html")[#include source.file]
}
"#,
        )
        .unwrap();
        let mut config = site_config(root);
        config.build.publish_dir = root.join("public");

        build_and_publish(&config).unwrap();
        let cold = published_files(&config);
        assert!(!cold.is_empty(), "the build published nothing to compare");

        let cancellation = crate::cancellation::BuildCancellation::new();
        let mut session =
            crate::check::SourceDiagnosticSession::new(std::sync::Arc::new(config.clone()));
        session.inspect(Vec::new(), &cancellation).unwrap();
        session.inspect(Vec::new(), &cancellation).unwrap();
        assert!(
            session.reused_source_analysis(),
            "the second revision reused the first revision's analysis"
        );

        build_and_publish(&config).unwrap();
        assert_eq!(
            cold,
            published_files(&config),
            "a reused check changed what the build published"
        );
    }

    /// Every file under the output root, by relative path, with the bytes it holds.
    fn published_files(config: &ResolvedSiteConfig) -> Vec<(String, Vec<u8>)> {
        fn collect(
            root: &std::path::Path,
            directory: &std::path::Path,
            files: &mut Vec<(String, Vec<u8>)>,
        ) {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    collect(root, &path, files);
                } else {
                    let relative = path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    files.push((relative, fs::read(&path).unwrap()));
                }
            }
        }
        let mut files = Vec::new();
        collect(
            &config.build.publish_dir,
            &config.build.publish_dir,
            &mut files,
        );
        files.sort_by(|left, right| left.0.cmp(&right.0));
        files
    }

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

    #[test]
    fn mismatched_media_types_report_errors() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("content")).unwrap();
        let site_program = root.join("site.typ");
        fs::write(
            &site_program,
            r#"#document("index.html", html.html(
  html.head(
    html.link(rel: "stylesheet", href: "/opaque.bin")
    + html.link(rel: "stylesheet", href: "/module.js")
    + html.script(src: "/site.css")
  )
  + html.body[Site]
))
#asset("opaque.bin", "")
#asset("module.js", "")
#asset("site.css", "")"#,
        )
        .unwrap();
        let config = site_config(root);

        let error = build_failure(&config);
        let diagnostics = crate::diagnostic::attached(&error).expect("reference diagnostics");

        assert_eq!(diagnostics.len(), 3);
        assert!(diagnostics.iter().all(|diagnostic| {
            diagnostic.code == "reference.resource_media_mismatch"
                && diagnostic.severity == crate::diagnostic::Severity::Error
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("application/octet-stream")
                && diagnostic.message.contains("text/css")
        }));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("text/javascript")
                && diagnostic.message.contains("text/css")
        }));
    }

    /// The identity of the bytes this build published at `path`.
    fn published_identity(graph: &crate::output::graph::OutputGraph, path: &str) -> String {
        tola_typst::ContentDigest::of(output_bytes(graph, path)).to_hex()
    }

    #[test]
    fn cache_busting_tracks_published_bytes() {
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
            Case {
                name: "output validation",
                files: &[
                    ("templates/shared.typ", "#let duplicate = [Duplicate]"),
                    (
                        "site.typ",
                        "#import \"templates/shared.typ\": duplicate\n#document(\"index.html\")[\n  #duplicate\n  #html.div(id: \"same\")\n  #html.div(id: \"same\")\n]",
                    ),
                ],
                recorded: &["site.typ", "templates/shared.typ"],
                failure: "more than once",
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
    fn failed_output_produces_no_candidate() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(
            root.join("site.typ"),
            r#"#document("index.html")[Current]
#document("bad.html")[#html.script("</script>")]"#,
        )
        .unwrap();

        let config = site_config(root);

        let error = match build_site_with_host(
            &config,
            &compiler_host(&config).unwrap(),
            BuildMode::Production,
            &mut BuildAttemptProducers::default(),
        ) {
            Ok(_) => panic!("invalid background HTML unexpectedly produced a site candidate"),
            Err(error) => error,
        };
        assert!(
            format!("{error:#}").contains("cannot contain its own closing tag"),
            "{error:#}"
        );
    }

    #[test]
    fn candidate_reports_warnings() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(
            root.join("site.typ"),
            r#"#document("index.html")[
#show strong: none
= Real
#context {
  let count = query(heading).len()
  count * [= Generated]
}
]"#,
        )
        .unwrap();

        let config = site_config(root);
        let candidate = build_site_with_host(
            &config,
            &compiler_host(&config).unwrap(),
            BuildMode::Production,
            &mut BuildAttemptProducers::default(),
        )
        .unwrap();

        assert!(
            candidate
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.severity == crate::diagnostic::Severity::Warning)
        );
    }

    #[test]
    fn export_failure_retains_source_diagnostics() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("content")).unwrap();
        fs::write(
            root.join("site.typ"),
            r#"#document("index.html")[#text(font: "missing-font-qxyz")[Hi]]
#document("file.pdf", format: "pdf")[#pdf.attach("bad.txt", bytes("hi"), mime-type: "invalid")]"#,
        )
        .unwrap();
        let config = site_config(root);
        let error = build_site_with_host(
            &config,
            &compiler_host(&config).unwrap(),
            BuildMode::Production,
            &mut BuildAttemptProducers::default(),
        )
        .err()
        .expect("the attachment cannot export with this MIME type");
        let diagnostics = crate::diagnostic::attached(&error).unwrap();
        let export = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == crate::codes::typst::BUNDLE_EXPORT)
            .unwrap();
        assert_eq!(export.severity, crate::diagnostic::Severity::Error);
        let location = export.location.as_ref().unwrap();
        assert_eq!(location.path, "site.typ");
        assert_eq!(location.line, Some(2));
        assert!(location.range.is_some());
        assert!(location.source_lines[0].text.contains("pdf.attach"));
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == crate::diagnostic::Severity::Warning
                && diagnostic
                    .location
                    .as_ref()
                    .is_some_and(|location| location.line == Some(1))
                && diagnostic.message.contains("missing-font-qxyz")
        }));
        assert!(error.chain().any(|cause| {
            matches!(
                cause.downcast_ref::<tola_typst::CompileError>(),
                Some(tola_typst::CompileError::BundleExport { .. })
            )
        }));
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

    #[cfg(unix)]
    #[test]
    fn configured_asset_keeps_logical_path() {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        let physical = root.join("physical.bin");
        let logical = root.join("logical.bin");
        fs::write(&physical, b"asset").unwrap();
        symlink(&physical, &logical).unwrap();
        let entry = root.join("site.typ");
        fs::write(&entry, "#document(\"index.html\")[Hello]").unwrap();
        let mut config = site_config(root);
        config.assets.files = vec![AssetFileDeclaration::new(
            &logical,
            AssetUrl::parse("/download.bin").unwrap(),
        )];

        let build = build_site(&config, BuildMode::Production).unwrap();
        let output = build
            .graph
            .outputs()
            .iter()
            .find(|output| output.path().as_str() == "download.bin")
            .unwrap();
        let crate::output::owner::OutputOwner::ConfiguredAsset { source } = output.owner() else {
            panic!(
                "configured output has the wrong owner: {:?}",
                output.owner()
            );
        };
        assert_eq!(source, &std::path::absolute(&logical).unwrap());
        assert_ne!(source, &fs::canonicalize(&logical).unwrap());
    }

    /// ASCII slugification reads each source name in the site's declared language: 東京
    /// slugs as `toukyou` under `ja`, not the Chinese `dong-jing`.
    #[test]
    fn slugify_reads_site_language() {
        let program = r#"#import "@tola/address:0.0.0": route, route-to-output, slugify
#import "@tola/site:0.0.0": site
#import "@tola/source:0.0.0": all-sources

#for source in all-sources() {
  document(route-to-output(route(source.route-segments.map(segment => slugify(segment, mode: "ascii", language: site.language.lang)))))[#source.id]
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
    fn every_build_publishes_minified_code_stylesheet() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("content")).unwrap();
        let entry = root.join("site.typ");
        fs::write(&entry, r#"#document("index.html", title: [Home])[Home]"#).unwrap();
        let config = site_config(root);

        let build = build_site(&config, BuildMode::Production).unwrap();
        let path = tola_packages::code_stylesheet_output();
        let output = build
            .graph
            .outputs()
            .iter()
            .find(|output| output.path().as_str() == path)
            .unwrap_or_else(|| panic!("every build publishes `{path}`"));

        assert_eq!(
            output.declaration().media_type(),
            &crate::output::semantics::ResponseMediaType::CSS
        );
        let stylesheet = String::from_utf8(output_bytes(&build.graph, &path).to_vec()).unwrap();
        assert!(
            stylesheet.contains(":where(.tola-code span)"),
            "{stylesheet}"
        );
        assert!(stylesheet.contains("--tola-code-color"), "{stylesheet}");
        assert!(!stylesheet.contains("/*"), "{stylesheet}");
        assert!(!stylesheet.contains('\n'), "{stylesheet}");
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
    fn package_declaration_error_names_importing_file() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("content")).unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            r#"#import "@tola/web:0.0.0": feed
#document("index.html", title: [Home])[Home]
#feed(entries: ((published: datetime(year: 2026, month: 9, day: 1)),))"#,
        )
        .unwrap();
        let config = site_config_from(
            root,
            "[site]\norigin = \"https://example.test\"\ntitle = \"Notes\"\ndescription = \"Published notes\"",
        );

        let error = build_failure(&config);
        let diagnostics = error_diagnostics(&error, config.get_root());
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.message.contains("/entries/0/target"))
            .unwrap_or_else(|| panic!("missing feed declaration diagnostic: {diagnostics:#?}"));

        let location = diagnostic
            .location
            .as_ref()
            .expect("the declaration error resolves a location");
        assert!(
            location.path.contains("@tola/web"),
            "unexpected diagnostic source: {}",
            location.path
        );
        assert_eq!(diagnostic.imported_by, ["site.typ"]);
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

    #[test]
    fn imported_routes_emit_sibling_outputs() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let routes = root.join("routes");
        let output = root.join("public");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&routes).unwrap();
        fs::write(
            root.join("site.typ"),
            r#"#import "routes/routes.typ": emit-routes
#emit-routes()
"#,
        )
        .unwrap();
        fs::write(
            routes.join("routes.typ"),
            r#"#let emit-routes() = include "generated.typ""#,
        )
        .unwrap();
        fs::write(
            routes.join("generated.typ"),
            r#"#for slug in ("alpha", "beta") {
  document(slug + "/index.html", [Route #slug])
}
#asset("generated/routes.txt", "alpha,beta")
"#,
        )
        .unwrap();

        let config = site_config(root);

        let build = build_and_publish(&config).unwrap();

        for path in ["alpha/index.html", "beta/index.html"] {
            assert!(
                has_output(&build.graph, path),
                "missing Bundle document {path}"
            );
            assert!(
                output.join(path).is_file(),
                "unpublished Bundle document {path}"
            );
        }
        assert_eq!(
            output_bytes(&build.graph, "generated/routes.txt"),
            b"alpha,beta"
        );
        assert_eq!(
            fs::read(output.join("generated/routes.txt")).unwrap(),
            b"alpha,beta"
        );
    }

    #[test]
    fn taxonomy_generator_builds_tag_documents() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let routes = root.join("routes");
        let output = root.join("public");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&routes).unwrap();
        fs::write(
            content.join("alpha.typ"),
            r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: [Alpha], tags: ("rust", "typst")))"#,
        )
        .unwrap();
        fs::write(
            content.join("beta.typ"),
            r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: [Beta], tags: ("rust",)))"#,
        )
        .unwrap();
        fs::write(
            routes.join("taxonomy.typ"),
            r#"#let emit-taxonomies(sources) = {
  for tag in ("rust", "typst") {
    let members = sources.filter(source => source.meta.tags.contains(tag))
    if members.len() > 0 {
      let names = members.map(source => source.meta.title).join(", ")
      document("tags/" + tag + "/index.html", [#names])
    }
  }
}
"#,
        )
        .unwrap();
        fs::write(
            root.join("site.typ"),
            r#"#import "@tola/source:0.0.0": all-sources
#import "routes/taxonomy.typ": emit-taxonomies
#emit-taxonomies(all-sources())
"#,
        )
        .unwrap();

        let config = site_config(root);

        let build = build_and_publish(&config).unwrap();

        let rust = fs::read_to_string(output.join("tags/rust/index.html")).unwrap();
        assert!(rust.contains("Alpha"), "{rust}");
        assert!(rust.contains("Beta"), "{rust}");
        let typst = fs::read_to_string(output.join("tags/typst/index.html")).unwrap();
        assert!(typst.contains("Alpha"), "{typst}");
        assert!(!typst.contains("Beta"), "{typst}");

        assert_eq!(build.index.address().pages().len(), 2);
    }

    #[test]
    fn nested_document_construction_fails() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let routes = root.join("routes");
        let output = root.join("public");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&routes).unwrap();
        fs::create_dir_all(&output).unwrap();
        fs::write(output.join("sentinel.txt"), "unchanged").unwrap();
        fs::write(
            routes.join("nested.typ"),
            r#"#let emit-inner() = document("inner/index.html")[Inner]"#,
        )
        .unwrap();
        fs::write(
            root.join("site.typ"),
            r#"#import "routes/nested.typ": emit-inner
#document("outer/index.html")[#emit-inner()]
"#,
        )
        .unwrap();

        let config = site_config(root);

        let error = build_failure(&config);
        let diagnostics = error_diagnostics(&error, config.get_root());
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.message
                    == "constructing a document is only supported in the bundle target"
            })
            .unwrap_or_else(|| {
                panic!("missing native nested-document diagnostic: {diagnostics:#?}")
            });
        let location = diagnostic
            .location
            .as_ref()
            .expect("native diagnostic should resolve the imported source");
        assert!(
            std::path::Path::new(&location.path).ends_with("routes/nested.typ"),
            "unexpected diagnostic source: {}",
            location.path
        );
        assert_eq!(
            fs::read_to_string(output.join("sentinel.txt")).unwrap(),
            "unchanged"
        );
        assert!(!output.join("outer/index.html").exists());
        assert!(!output.join("inner/index.html").exists());
    }

    #[test]
    fn failed_bundle_reports_the_label_declaration() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let routes = root.join("routes");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&routes).unwrap();
        fs::write(
            content.join("post.typ"),
            "#metadata((title: \"Post\")) <tola-meta>",
        )
        .unwrap();
        fs::write(
            routes.join("nested.typ"),
            r#"#let emit-inner() = document("inner/index.html")[Inner]"#,
        )
        .unwrap();
        fs::write(
            root.join("site.typ"),
            r#"#import "routes/nested.typ": emit-inner
#document("outer/index.html")[#emit-inner()]
"#,
        )
        .unwrap();

        let config = site_config(root);

        let error = build_failure(&config);
        let diagnostics = error_diagnostics(&error, config.get_root());

        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.code == crate::codes::source::DECLARATION_DEPRECATED
                    && diagnostic.severity == crate::diagnostic::Severity::Warning
            }),
            "{diagnostics:#?}"
        );
    }

    #[test]
    fn site_package_receives_resolved_site() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let output = root.join("public");
        fs::create_dir_all(&content).unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            r#"#import "@tola/site:0.0.0": site
#document("index.html")[#site.title #site.base-path]"#,
        )
        .unwrap();

        let config = site_config_from(
            root,
            "[site]\ntitle = \"Configured Title\"\norigin = \"https://example.test\"\nbase-path = \"/docs/\"",
        );

        build_and_publish(&config).unwrap();

        let html = fs::read_to_string(output.join("index.html")).unwrap();
        assert!(html.contains("Configured Title"), "{html}");
        assert!(html.contains("/docs/"), "{html}");
    }

    #[test]
    fn writes_stylesheet() {
        if !is_hook_child_for("before-build") {
            return;
        }
        fs::create_dir_all("generated").unwrap();
        fs::write("generated/site.css", "compiled").unwrap();
    }

    #[test]
    fn hook_generated_input_publishes_asset() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let generated = root.join("generated");
        let output = root.join("public");
        fs::create_dir_all(&content).unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": route, route-to-output

#let published = all-sources()

#for source in published {
  document(route-to-output(route(source.route-segments)))[#include source.file]
}
"#,
        )
        .unwrap();

        let mut config = site_config(root);
        config.assets.trees = vec![AssetTreeDeclaration::new(
            &generated,
            AssetUrlPrefix::parse("/styles").unwrap(),
        )];
        config.build.hooks.before_build.push(BeforeBuildHookConfig {
            name: "generate stylesheet".into(),
            command: hook_child_command("build::pipeline::tests::writes_stylesheet"),
            dev: crate::config::section::build::DevParticipation::Skip,
            generates: vec!["generated/site.css".into()],
            ..BeforeBuildHookConfig::default()
        });

        build_and_publish(&config).unwrap();
        assert_eq!(
            fs::read_to_string(output.join("styles/site.css")).unwrap(),
            "compiled"
        );
    }

    #[test]
    fn generated_document_keeps_native_title() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            r#"#document("tags/rust/index.html", title: [Native Rust])[
  #import "@tola/source:0.0.0": tola-meta
#tola-meta((title: [Rust], pin: true))
  = Rust
]
"#,
        )
        .unwrap();

        let config = site_config(root);

        let snapshot = build_and_publish(&config).unwrap();
        let url = tola_address::UrlPath::parse("/tags/rust/").unwrap();
        let crate::site::Resource::Page { document } =
            snapshot.index.address().get_by_url(&url).unwrap()
        else {
            panic!("expected generated document");
        };

        assert_eq!(
            document
                .properties
                .title
                .as_ref()
                .map(|title| title.to_string()),
            Some("Native Rust".to_string())
        );
    }

    #[test]
    fn documents_report_their_own_routes() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let output = root.join("public");
        fs::create_dir_all(&content).unwrap();
        fs::write(
            content.join("document.typ"),
            r#"#import "@tola/document:0.0.0": current-document
#context {
  let ctx = current-document()
  html.span(class: "current-route")[#ctx.route]
}"#,
        )
        .unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/document:0.0.0": current-document
#import "@tola/address:0.0.0": route, route-to-output

#for source in all-sources() {
  document(route-to-output(route(source.route-segments)))[#include source.file]
}

#document("tags/rust/index.html", context {
  let ctx = current-document()
  html.span(class: "current-route")[#ctx.route]
})

#let body = include "content/document.typ"
#document("first/index.html", body)
#document("second/index.html", body)
"#,
        )
        .unwrap();

        let config = site_config(root);
        build_and_publish(&config).unwrap();

        for (path, route) in [
            ("document/index.html", "/document/"),
            ("tags/rust/index.html", "/tags/rust/"),
            ("first/index.html", "/first/"),
            ("second/index.html", "/second/"),
        ] {
            let html = fs::read_to_string(output.join(path)).unwrap();
            assert!(
                html.contains(&format!(r#"class="current-route">{route}</span>"#)),
                "{html}"
            );
        }
    }

    #[test]
    fn not_found_document_uses_authored_body() {
        let dir = TempDir::new().unwrap();
        let root = crate::filesystem::normalize_path(dir.path());
        let content = root.join("content");
        let templates = root.join("templates");
        let output = root.join("public");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&templates).unwrap();
        fs::write(templates.join("not-found.typ"), "= Not Found").unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            r#"#document("404.html", format: "html", title: [Page not found])[
  #include "templates/not-found.typ"
]
"#,
        )
        .unwrap();

        let config = site_config(&root);

        build_and_publish(&config).unwrap();

        let html = fs::read_to_string(output.join("404.html")).unwrap();
        assert!(html.contains("<title>Page not found</title>"), "{html}");
        assert!(html.contains("Not Found"), "{html}");
        assert!(!output.join("404.html/index.html").exists());
    }

    #[test]
    fn parent_index_keeps_flat_children() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let posts = content.join("posts");
        let rust = posts.join("rust");
        let global_assets = root.join("global-assets");
        let exact_asset = root.join("CNAME");
        let output = root.join("public");
        fs::create_dir_all(&rust).unwrap();
        fs::create_dir_all(&global_assets).unwrap();
        fs::write(posts.join("index.typ"), "Posts").unwrap();
        fs::write(posts.join("child.typ"), "Child").unwrap();
        fs::write(rust.join("index.typ"), "Rust").unwrap();
        fs::write(global_assets.join("site.txt"), "site asset").unwrap();
        fs::write(&exact_asset, "example.com").unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": route, route-to-output

#for source in all-sources() {
  document(route-to-output(route(source.route-segments)))[#include source.file]
}
#asset("generated.txt", "generated asset")
"#,
        )
        .unwrap();

        let mut config = site_config(root);
        config.assets.trees = vec![AssetTreeDeclaration::new(
            global_assets,
            AssetUrlPrefix::parse("/global").unwrap(),
        )];
        config.assets.files = vec![AssetFileDeclaration::new(
            exact_asset,
            AssetUrl::parse("/CNAME").unwrap(),
        )];

        build_and_publish(&config).unwrap();

        for path in [
            "posts/index.html",
            "posts/child/index.html",
            "posts/rust/index.html",
            "global/site.txt",
            "CNAME",
            "generated.txt",
        ] {
            assert!(output.join(path).is_file(), "missing output {path}");
        }
    }

    #[test]
    fn draft_sources_are_filtered_by_metadata() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        fs::write(
            content.join("included.typ"),
            r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: [Included], draft: false))"#,
        )
        .unwrap();
        fs::write(
            content.join("excluded.typ"),
            r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: [Excluded], draft: true))"#,
        )
        .unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": route, route-to-output
#for source in all-sources().filter(source => not source.meta.draft) {
  document(route-to-output(route(source.route-segments)))[#include source.file]
}"#,
        )
        .unwrap();

        let config = site_config(root);

        let snapshot = build_and_publish(&config).unwrap();
        assert!(
            snapshot
                .index
                .address()
                .get_by_url(&tola_address::UrlPath::parse("/included/").unwrap())
                .is_some()
        );
        assert!(
            snapshot
                .index
                .address()
                .get_by_url(&tola_address::UrlPath::parse("/excluded/").unwrap())
                .is_none()
        );
    }

    #[test]
    fn document_without_metadata_becomes_page() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        let entry = root.join("site.typ");
        fs::write(&entry, r#"#document("plain/index.html")[Plain]"#).unwrap();

        let config = site_config(root);

        let snapshot = build_and_publish(&config).unwrap();
        let url = tola_address::UrlPath::parse("/plain/").unwrap();
        let crate::site::Resource::Page { document } =
            snapshot.index.address().get_by_url(&url).unwrap()
        else {
            panic!("expected generated document");
        };

        assert!(document.properties.title.is_none());
    }

    #[test]
    fn document_keeps_native_metadata() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            r#"#document(
  "native/index.html",
  title: [Native Title],
  author: "Alice",
  description: [Native Summary],
)[Native]"#,
        )
        .unwrap();

        let config = site_config(root);

        let snapshot = build_and_publish(&config).unwrap();
        let url = tola_address::UrlPath::parse("/native/").unwrap();
        let crate::site::Resource::Page { document } =
            snapshot.index.address().get_by_url(&url).unwrap()
        else {
            panic!("expected generated document");
        };

        assert_eq!(
            document
                .properties
                .title
                .as_ref()
                .map(|title| title.to_string()),
            Some("Native Title".to_string())
        );
        assert_eq!(
            document
                .properties
                .description
                .as_ref()
                .map(|summary| summary.to_string()),
            Some("Native Summary".to_string())
        );
        assert_eq!(document.properties.author.as_slice(), &["Alice"]);
    }

    #[test]
    fn no_pages_warning_follows_html_outputs() {
        for (program, availability) in [
            ("", crate::output::PageAvailability::Empty),
            (
                "#document(\"404.html\", format: \"html\")[Missing]",
                crate::output::PageAvailability::Present,
            ),
            (
                "#asset(\"index.html\", \"ordinary asset\")",
                crate::output::PageAvailability::Empty,
            ),
            (
                "#document(\"download.pdf\", format: \"pdf\")[Download]",
                crate::output::PageAvailability::Empty,
            ),
            (
                "#document(\"plain.html\")[Plain]",
                crate::output::PageAvailability::Present,
            ),
        ] {
            let directory = TempDir::new().unwrap();
            let root = directory.path();
            let config = site_config(root);
            fs::write(&config.build.entry, program).unwrap();

            let build = build_site(&config, BuildMode::Production).unwrap();

            assert_eq!(
                crate::output::PageAvailability::from_outputs(build.graph().outputs()),
                availability,
                "{program}"
            );
            let mut diagnostics = build
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.code == crate::codes::site::NO_PAGES);
            assert_eq!(
                diagnostics.next(),
                (availability == crate::output::PageAvailability::Empty)
                    .then(|| no_pages_diagnostic(&config))
                    .as_ref(),
                "{program}"
            );
            assert!(diagnostics.next().is_none(), "{program}");
        }
    }

    #[test]
    fn not_found_warning_follows_output_kind_and_path() {
        let cases = [
            ("#document(\"404.html\", format: \"html\")[Missing]", false),
            (
                "#import \"templates/not-found.typ\": not-found-page\n\n#not-found-page()",
                false,
            ),
            (
                "#document(\"404/index.html\", format: \"html\")[Missing]",
                true,
            ),
            (
                "#document(\"index.html\")[Home]\n#asset(\"404.html\", \"ordinary asset\")",
                true,
            ),
            ("", true),
        ];
        for (program, expected) in cases {
            let directory = TempDir::new().unwrap();
            let root = directory.path();
            let config = site_config(root);
            fs::create_dir_all(root.join("templates")).unwrap();
            fs::write(
                root.join("templates/not-found.typ"),
                "#let not-found-page() = document(\"404.html\", format: \"html\")[Missing]",
            )
            .unwrap();
            fs::write(&config.build.entry, program).unwrap();

            let build = build_site(&config, BuildMode::Production).unwrap();

            let mut diagnostics = build
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.code == crate::codes::site::NOT_FOUND_MISSING);
            assert_eq!(
                diagnostics.next().map(|diagnostic| diagnostic.severity),
                expected.then_some(crate::diagnostic::Severity::Warning),
                "{program}"
            );
            assert!(diagnostics.next().is_none(), "{program}");
        }
    }

    pub(super) fn build_failure(config: &ResolvedSiteConfig) -> anyhow::Error {
        match build_and_publish(config) {
            Ok(_) => panic!("site unexpectedly built and published"),
            Err(error) => error,
        }
    }
}
