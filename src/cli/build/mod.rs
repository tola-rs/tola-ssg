//! Site building orchestration.
//!
//! Build pipeline phases:
//! - **Init** - Typst warm-up, output directory, cache clear
//! - **Pre Hooks** - User-defined pre-build commands
//! - **Atomic CSS** - Generate configured atomic stylesheet
//! - **Collect** - Gather content files and assets
//! - **Assets** - Process configured assets with freshness checks
//! - **Compile** - Content compilation
//! - **Iterative** - Rebuild iterative pages with complete metadata
//! - **Post-process** - CNAME, HTML 404, cleanup
//! - **Post Hooks** - User-defined post-build commands
//! - **Finalize** - Cache persistence, warnings, logging

mod pipeline;

use crate::{
    address::SiteIndex,
    compiler::page::{Pages, WarningCollector},
    config::SiteConfig,
    core::BuildMode,
    freshness::{self, ContentHash},
    hooks, logger,
    utils::plural_count,
};
use anyhow::Result;

/// Build the entire site using two-phase compilation
///
/// Pipeline: init -> pre-hooks -> atomic CSS -> collect -> assets -> compile -> iterative -> post-process -> post-hooks -> finalize
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

    // Process assets, then compile content.
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
        logger::log(
            "build",
            format_args!(
                "{} skipped",
                plural_count(metadata.stats.drafts_skipped, "draft")
            ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SiteConfig;
    use crate::config::section::build::{HookConfig, WatchMode};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn clean_build_keeps_pre_hook_public_asset_output() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let styles = root.join("assets/styles");
        let output = root.join("public");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&styles).unwrap();
        fs::write(styles.join("tailwind.css"), "@import \"tailwindcss\";").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(root);
        config.config_path = root.join("tola.toml");
        config.build.content = content;
        config.build.output = output.clone();
        config.build.clean = true;
        config.build.hooks.pre.push(HookConfig {
            enable: true,
            name: Some("tailwind".into()),
            command: vec![
                "sh".into(),
                "-c".into(),
                "mkdir -p public/styles && printf compiled > public/styles/tailwind.css".into(),
            ],
            watch: WatchMode::Disabled,
            quiet: true,
        });

        let pages = build_site(BuildMode::DEVELOPMENT, &config, &SiteIndex::new(), true).unwrap();

        assert!(pages.items.is_empty());
        assert_eq!(
            fs::read_to_string(output.join("styles/tailwind.css")).unwrap(),
            "compiled"
        );
    }
}
