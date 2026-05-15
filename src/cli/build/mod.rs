//! Site building orchestration.
//!
//! Build pipeline phases:
//! - **Pre Hooks** - User-defined pre-build commands
//! - **Init** - Typst warm-up, output directory, cache clear
//! - **Atomic CSS** - Generate configured atomic stylesheet
//! - **Collect** - Gather content files and assets
//! - **Compile** - Parallel content compilation + asset processing
//! - **Iterative** - Rebuild iterative pages with complete metadata
//! - **Post-process** - Flatten assets, CNAME, content assets, enhance CSS
//! - **Post Hooks** - User-defined post-build commands
//! - **Finalize** - Cache persistence, warnings, logging

mod pipeline;

use crate::{
    address::SiteIndex,
    compiler::page::{Pages, WarningCollector},
    config::SiteConfig,
    core::BuildMode,
    freshness::{self, ContentHash},
    hooks, log,
    utils::plural_count,
};
use anyhow::Result;

/// Build the entire site using two-phase compilation
///
/// Pipeline: init -> pre-hooks -> atomic CSS -> collect -> compile -> iterative -> post-process -> post-hooks -> finalize
pub fn build_site(
    mode: BuildMode,
    config: &SiteConfig,
    state: &SiteIndex,
    quiet: bool,
) -> Result<Pages> {
    let warnings = WarningCollector::new();

    // Initialize (must be before pre hooks to clean output dir first)
    let typst_host = pipeline::init_build(config)?;
    let deps_hash: ContentHash = freshness::compute_deps_hash(config);

    // Pre Hooks (after init so output dir exists and is clean)
    hooks::run_pre_hooks(config)?;

    // Native Atomic CSS runs before page compilation so generated stylesheet
    // links can use the current output hash.
    crate::css::build::build(config)?;

    // Collect files
    let files = pipeline::collect_build_files(config);
    let progress = pipeline::create_progress(&files, quiet);

    // Compile content + process assets (parallel)
    let metadata = pipeline::compile_and_process(
        mode,
        config,
        &typst_host,
        state,
        deps_hash,
        &warnings,
        progress.as_ref(),
    )?;

    // Log drafts skipped
    if !quiet && metadata.stats.has_skipped_drafts() {
        log!(
            "build";
            "{} skipped",
            plural_count(metadata.stats.drafts_skipped, "draft")
        );
    }

    // Rebuild iterative pages with complete metadata
    let pages = pipeline::rebuild_iterative_pages(
        mode,
        config,
        &typst_host,
        state,
        deps_hash,
        &metadata,
        &warnings,
    )?;

    if let Some(p) = progress {
        p.finish();
    }

    // Post-processing
    pipeline::post_process(config, quiet)?;

    // Post Hooks
    hooks::run_post_hooks(config)?;

    // Finalize
    pipeline::finalize_build(config, state, &warnings, quiet)?;

    Ok(pages)
}
