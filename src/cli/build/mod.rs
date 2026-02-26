//! Site building orchestration.
//!
//! Build pipeline phases:
//! - **Pre Hooks** - User-defined pre-build commands
//! - **Init** - Typst warm-up, output repo, cache clear
//! - **Collect** - Gather content files and assets
//! - **Compile** - Parallel content compilation + asset processing
//! - **Iterative** - Rebuild iterative pages with complete metadata
//! - **Post-process** - Flatten assets, CNAME, CSS processor, enhance CSS
//! - **Post Hooks** - User-defined post-build commands
//! - **Finalize** - Cache persistence, warnings, logging

mod pipeline;
mod recompile;

use anyhow::Result;
use gix::ThreadSafeRepository;
use std::path::Path;

use crate::{
    compiler::page::Pages,
    config::SiteConfig,
    core::BuildMode,
    freshness::{self, ContentHash},
    hooks, log,
    utils::plural_count,
};

pub use recompile::recompile_files;

/// Collect font directories from config for font loading
pub fn collect_font_dirs(config: &SiteConfig) -> Vec<&Path> {
    let mut dirs: Vec<&Path> = vec![config.build.content.as_path()];
    dirs.extend(config.build.assets.nested_sources());
    dirs.extend(config.build.deps.iter().map(|p| p.as_path()));
    dirs
}

/// Build the entire site using two-phase compilation
///
/// Pipeline: pre-hooks -> init -> collect -> compile -> iterative -> post-process -> post-hooks -> finalize
pub fn build_site(
    mode: BuildMode,
    config: &SiteConfig,
    quiet: bool,
) -> Result<(ThreadSafeRepository, Pages)> {
    // Initialize (must be before pre hooks to clean output dir first)
    let repo = pipeline::init_build(config)?;
    let deps_hash: ContentHash = freshness::compute_deps_hash(config);

    // Pre Hooks (after init so output dir exists and is clean)
    hooks::run_pre_hooks(config, mode, true)?;

    // Collect files
    let files = pipeline::collect_build_files(config);
    let progress = pipeline::create_progress(&files, quiet);

    // Compile content + process assets (parallel)
    let metadata =
        pipeline::compile_and_process(mode, config, &files, deps_hash, progress.as_ref())?;

    // Log drafts skipped
    if !quiet && metadata.stats.has_skipped_drafts() {
        log!(
            "build";
            "{} skipped",
            plural_count(metadata.stats.drafts_skipped, "draft")
        );
    }

    // Rebuild iterative pages with complete metadata
    let pages = pipeline::rebuild_iterative_pages(mode, config, deps_hash, &metadata)?;

    if let Some(p) = progress {
        p.finish();
    }

    // Post-processing
    pipeline::post_process(config, quiet)?;

    // Post Hooks
    hooks::run_post_hooks(config, mode, true)?;

    // Finalize
    pipeline::finalize_build(config, quiet)?;

    Ok((repo, pages))
}
