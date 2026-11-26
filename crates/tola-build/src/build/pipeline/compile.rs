use anyhow::{Context, Result};

use crate::{compiler::SiteHost, config::SiteConfig, mode::BuildMode};

use super::{BuildAttemptProducers, CompiledSiteOutputs};

struct ProducerInputs {
    content: std::sync::Arc<[crate::content::ContentUnit]>,
    icons: crate::icon::IconInventory,
    host: SiteHost,
}

struct CompiledRootBundle {
    site_program: crate::compiler::SiteProgramCache,
    deps: crate::compiler::CompilationDependencies,
    source_analysis: crate::compiler::analysis::SourceAnalysisCache,
}

fn reexport_cached_site_program(
    cfg: &SiteConfig,
    cached: &crate::compiler::SiteProgramCache,
    cancel: &crate::cancellation::BuildCancellation,
) -> Result<crate::compiler::SiteProgramCache> {
    let compilation = std::sync::Arc::clone(&cached.compilation);
    let exported = crate::compiler::bundle::export(
        cfg,
        &compilation,
        &cached.world,
        &cancel.bundle_cancellation(),
        Some(&cached.bundle_entries),
    )?;
    let mut outputs = crate::output::graph::OutputGraphBuilder::new();
    crate::compiler::outputs::insert_bundle(
        &mut outputs,
        exported.bundle_entries.iter().cloned(),
        cached.minified_languages,
        cancel,
    )?;
    Ok(crate::compiler::SiteProgramCache {
        root: cached.root.clone(),
        documents: exported.documents,
        outputs: outputs.finish(),
        diagnostics: exported.diagnostics,
        payload_diagnostics: cached.payload_diagnostics.clone(),
        pretty_html: !cfg.build.minify.html,
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
    site: &crate::modules::LoadedSite,
    host: &SiteHost,
    mode: BuildMode,
    warnings: &mut tola_typst::Diagnostics,
    diag: &mut Vec<tola_diagnostic::Diagnostic>,
    producers: &mut BuildAttemptProducers,
) -> Result<CompiledSiteOutputs> {
    let cfg = site.config();
    let cancel = producers.cancellation.clone();
    let (
        configured_assets,
        ProducerInputs {
            content,
            icons,
            host,
        },
    ) = prepare_beside_configured_assets(site, host, producers, &cancel)?;
    let bindings = crate::package::SiteBindings::from_config(
        cfg,
        configured_assets.asset_urls(cfg)?,
        Some(mode),
    );
    let CompiledRootBundle {
        site_program,
        deps,
        source_analysis,
    } = compile_root_bundle(site, &host, &content, bindings, producers, warnings, diag)?;
    diag.extend_from_slice(&site_program.payload_diagnostics);
    let mut outputs = crate::output::graph::OutputGraphBuilder::new();
    insert_code_stylesheet(&mut outputs, cfg.build.minify.css)?;
    producers.cancellation.ensure_active()?;
    outputs.insert_graph(&site_program.outputs)?;
    collect_configured_assets(&mut outputs, &configured_assets, &producers.cancellation)?;
    diag.extend(
        configured_assets
            .minify_warnings()
            .into_iter()
            .map(crate::asset::AssetMinifyWarning::diagnostic),
    );
    let (images, image_reads) = crate::image::output::ImageOutputs::prepare(
        &site_program.world,
        deps.read_evidence(),
        producers.images.as_deref(),
        cfg.root(),
        &producers.resources,
        &mut producers.inputs,
        &cancel,
    )?;
    let deps = deps.with_additional_program_reads(
        cfg.root(),
        image_reads.reads,
        image_reads.package_checks,
        &host,
    )?;
    images.insert_into(&mut outputs, &cancel)?;
    let published_icons = crate::icon::output::IconOutputs::prepare(
        deps.read_evidence(),
        &icons.collections(),
        &producers.cancellation,
    )?;
    published_icons.insert_into(&mut outputs, &producers.cancellation)?;
    let seo = collect_seo_outputs(
        cfg,
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
        producer_caches: crate::build::RetainedProducerCaches {
            host,
            dependencies: deps,
            source_analysis,
            site_program,
            configured_assets,
            icons,
            images,
            seo,
            hook_outputs: producers.source_hooks.clone(),
            prepared_hooks: std::mem::take(&mut producers.prepared_hooks),
            hook_reuse: crate::hooks::ReusableHookOutputs::default(),
        },
        content,
        reference_reuse: producers.reference_reuse.clone(),
    })
}

/// A configured asset's bytes decide its address, so the Bundle waits for the inventory.
fn prepare_beside_configured_assets(
    site: &crate::modules::LoadedSite,
    host: &SiteHost,
    producers: &mut BuildAttemptProducers,
    cancel: &crate::cancellation::BuildCancellation,
) -> Result<(crate::asset::SiteAssets, ProducerInputs)> {
    let cfg = site.config();
    let asset_reuse = producers.asset_reuse.take();
    let subscriber = tracing::dispatcher::get_default(Clone::clone);
    let span = tracing::Span::current();
    // Run I/O-bound asset/content/icon prep beside each other to keep the CPU pool free.
    std::thread::scope(
        |scope| -> Result<(crate::asset::SiteAssets, ProducerInputs)> {
            let configured_assets = scope.spawn(|| {
                let _subscriber = tracing::dispatcher::set_default(&subscriber);
                let _entered = span.enter();
                let started = std::time::Instant::now();
                let (prev, changes) = asset_reuse
                    .as_ref()
                    .map_or((None, &[][..]), |(assets, paths)| {
                        (Some(assets), paths.as_ref())
                    });
                let assets = crate::asset::build_configured_assets(
                    cfg,
                    site.applied_assets(),
                    &producers.resources.source_boundary(cfg),
                    cancel,
                    prev,
                    changes,
                );
                tracing::debug!(target: "tola::compile",
                    assets_ms = started.elapsed().as_secs_f64() * 1000.0,
                    "prepared configured assets");
                assets
            });
            let producer_inputs = scope.spawn(|| -> Result<ProducerInputs> {
                let _subscriber = tracing::dispatcher::set_default(&subscriber);
                let _entered = span.enter();
                cancel.ensure_active()?;
                producers
                    .resources
                    .source_boundary(cfg)
                    .check(&cfg.build.content_dir)?;
                let content = match &producers.content_inventory {
                    Some(content) => std::sync::Arc::clone(content),
                    None => crate::content::discover_content_units_for_config_with_cancellation(
                        cfg, cancel,
                    )?
                    .into(),
                };
                let icons = crate::package::prepare_icons(
                    cfg,
                    &producers.resources,
                    cancel,
                    &mut producers.icons,
                    &Default::default(),
                )?;
                let host = host.with_icons(icons.collections());
                Ok(ProducerInputs {
                    content,
                    icons,
                    host,
                })
            });
            let configured_assets = configured_assets
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))?;
            let producer_inputs = producer_inputs
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))?;
            cancel.ensure_active()?;
            Ok((configured_assets, producer_inputs))
        },
    )
}

fn compile_root_bundle(
    site: &crate::modules::LoadedSite,
    host: &SiteHost,
    content: &[crate::content::ContentUnit],
    bindings: crate::package::SiteBindings,
    producers: &mut BuildAttemptProducers,
    warnings: &mut tola_typst::Diagnostics,
    diag: &mut Vec<tola_diagnostic::Diagnostic>,
) -> Result<CompiledRootBundle> {
    let cfg = site.config();
    let bundle_cancel = producers.cancellation.bundle_cancellation();
    let pkg_bindings = bindings.prepare_modules(site.prepared())?;
    let reuse = producers.reuse.take();
    let reuse = reuse.as_ref();
    let analysis_reuse = reuse
        .map(crate::compiler::CompilationReuse::source_analysis_reuse)
        .unwrap_or_default();
    let accepted_deps = analysis_reuse.dependencies();
    if let Some(crate::compiler::CompilationReuse::SiteProgram {
        site_program: cached,
        dependencies: deps,
        source_analysis,
    }) = reuse
    {
        let sources = crate::content::SourceSet::without_metadata(content, cfg)?;
        if cached.matches_site(cfg)
            && cached.matches_minification(cfg)
            && source_analysis.matches_content(cfg.root(), &pkg_bindings, content, &sources)
            && deps
                .virtual_reads_match(|ev| host.virtual_read_matches(ev), &producers.cancellation)?
        {
            warnings.extend_distinct(source_analysis.diagnostics());
            warnings.extend_distinct(&cached.diagnostics);
            let site_program = if cached.matches_export_config(cfg) {
                (**cached).clone()
            } else {
                reexport_cached_site_program(cfg, cached, &producers.cancellation)?
            };
            deps.record_all_reads(&mut producers.inputs);
            tracing::debug!(target: "tola::compile", "reused background SiteProgram producer");
            diag.extend_from_slice(source_analysis.declaration_warnings());
            return Ok(CompiledRootBundle {
                site_program,
                deps: (**deps).clone(),
                source_analysis: (**source_analysis).clone(),
            });
        }
    }
    let started = std::time::Instant::now();
    let evaluated = crate::compiler::bundle::evaluate(
        cfg,
        host,
        content,
        pkg_bindings,
        &bundle_cancel,
        analysis_reuse,
        &mut producers.inputs,
    )
    .map_err(crate::compiler::analysis::SourceAnalysisFailure::into_error)?;
    let sources = evaluated.sources();
    let source_analysis = evaluated.source_analysis().clone();
    tracing::debug!(target: "tola::compile",
        source_eval_ms = started.elapsed().as_secs_f64() * 1000.0,
        metadata_rounds = source_analysis.metadata_rounds(),
        "evaluated source metadata");
    warnings.extend_distinct(source_analysis.diagnostics());
    diag.extend_from_slice(source_analysis.declaration_warnings());
    let reused_readers = source_analysis.reused_dependency_readers();
    let mut bundle_outputs = crate::output::graph::OutputGraphBuilder::new();
    let prev_bundle_entries = producers.previous_bundle_entries.take();
    let mut realized = crate::compiler::bundle::realize(
        cfg,
        host,
        &bundle_cancel,
        &evaluated,
        &mut producers.inputs,
    )
    .map_err(crate::compiler::bundle::RealizeFailure::into_error)?;
    warnings.extend_distinct(realized.compilation.diagnostics());
    let mut payload_diag = Vec::new();
    let minified_langs = crate::compiler::minified_languages(cfg);
    crate::compiler::minify_generated_payloads(
        &mut realized.compilation,
        &realized.world,
        minified_langs,
        &mut payload_diag,
        &bundle_cancel,
    )
    .map_err(|tola_typst::HtmlRawTextError::Cancelled| tola_typst::CompileError::Cancelled)
    .context("Tola could not minify the CSS or JavaScript the generated HTML includes")?;
    let compilation = std::sync::Arc::new(realized.compilation);
    let compiled = crate::compiler::bundle::export(
        cfg,
        &compilation,
        &realized.world,
        &bundle_cancel,
        prev_bundle_entries.as_ref(),
    )?;
    crate::compiler::outputs::insert_bundle(
        &mut bundle_outputs,
        compiled.bundle_entries.iter().cloned(),
        minified_langs,
        &producers.cancellation,
    )?;

    tracing::debug!(
        target: "tola::compile",
        source_units = evaluated.source_read_count(),
        program_reads = compilation.file_reads().len(),
        "compiled root Bundle dependencies"
    );
    let content_sources = sources
        .sources()
        .iter()
        .map(|src| src.source().to_path_buf());
    let reused_deps = accepted_deps
        .map(|deps| crate::compiler::ReusedDependencyReaders::new(deps, &reused_readers));
    let deps = crate::compiler::CompilationDependencies::new(
        cfg.root(),
        content_sources,
        std::iter::once(cfg.build.entry.clone()),
        evaluated.dependency_readers(&compilation),
        evaluated.package_checks(&compilation).cloned(),
        host,
        reused_deps,
    )?;
    let site_program = crate::compiler::SiteProgramCache {
        root: tola_input::normalize_path(cfg.root()),
        documents: compiled.documents,
        outputs: bundle_outputs.finish(),
        diagnostics: compiled.diagnostics,
        payload_diagnostics: payload_diag,
        pretty_html: !cfg.build.minify.html,
        minified_languages: minified_langs,
        entry: cfg.build.entry.clone(),
        bundle_entries: compiled.bundle_entries,
        compilation: compiled.compilation,
        world: compiled.world,
    };
    Ok(CompiledRootBundle {
        site_program,
        deps,
        source_analysis,
    })
}

fn insert_code_stylesheet(
    outputs: &mut crate::output::graph::OutputGraphBuilder,
    minify: bool,
) -> Result<()> {
    let css = tola_packages::TolaPackage::Code
        .file(std::path::Path::new(tola_packages::CODE_STYLESHEET_FILE))
        .context("Tola could not read the code stylesheet `@tola/code` provides")?;
    let css = if minify {
        tola_minify::minify_css(&css)
            .context("Tola could not minify the code stylesheet `@tola/code` provides")?
    } else {
        css.into_owned()
    };
    outputs.insert_system(
        "code-stylesheet",
        &tola_packages::code_stylesheet_output(),
        crate::output::semantics::OutputDeclaration::opaque(
            crate::output::semantics::ResponseMediaType::CSS,
        ),
        css.into_bytes(),
    )?;
    Ok(())
}

fn collect_configured_assets(
    outputs: &mut crate::output::graph::OutputGraphBuilder,
    configured_assets: &crate::asset::SiteAssets,
    cancel: &crate::cancellation::BuildCancellation,
) -> Result<()> {
    for (asset, bytes) in configured_assets.entries() {
        cancel.ensure_active()?;
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
    cfg: &SiteConfig,
    outputs: &mut crate::output::graph::OutputGraphBuilder,
    compilation: &std::sync::Arc<tola_typst::BundleCompilation>,
    world: &tola_typst::TypstWorld,
    cancel: &crate::cancellation::BuildCancellation,
    warnings: &mut tola_typst::Diagnostics,
    prev: Option<&std::sync::Arc<crate::seo::SeoCompilation>>,
) -> Result<std::sync::Arc<crate::seo::SeoCompilation>> {
    cancel.ensure_active()?;
    let started = std::time::Instant::now();
    let rendered =
        crate::seo::SeoCompilation::prepare(cfg, compilation, cancel, prev).map_err(|err| {
            let mut diag = match err {
                crate::seo::RenderError::Declaration(err) => {
                    tola_typst::Diagnostics::resolve(world, &[err.source_diagnostic()])
                }
                crate::seo::RenderError::HtmlExport(err) => tola_typst::Diagnostics::resolve(
                    world,
                    err.raw_diagnostics()
                        .expect("HTML export errors retain source diagnostics"),
                ),
                crate::seo::RenderError::Cancelled(err) => return anyhow::Error::new(err),
                crate::seo::RenderError::Compiler(err) => return anyhow::Error::new(err),
            };
            attach_importers(&mut diag, compilation);
            anyhow::Error::new(tola_typst::CompileError::from_diagnostics(diag))
        })?;
    cancel.ensure_active()?;
    tracing::debug!(target: "tola::compile",
        seo_ms = started.elapsed().as_secs_f64() * 1000.0,
        "rendered SEO outputs");
    let mut diag = tola_typst::Diagnostics::resolve(world, rendered.warnings());
    attach_importers(&mut diag, compilation);
    warnings.extend_distinct(&diag);
    for out in rendered.outputs() {
        cancel.ensure_active()?;
        outputs.insert_system(
            out.producer,
            out.url.as_str().trim_start_matches('/'),
            out.declaration.clone(),
            std::sync::Arc::clone(&out.bytes),
        )?;
    }
    Ok(rendered)
}

/// Name the site files behind late SEO diagnostics that point into a package.
fn attach_importers(
    diag: &mut tola_typst::Diagnostics,
    compilation: &tola_typst::BundleCompilation,
) {
    diag.attach_imported_by(|pkg| compilation.files_importing(pkg).to_vec());
}
