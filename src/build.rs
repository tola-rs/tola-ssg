//! Site building orchestration.
//!
//! Coordinates content compilation and asset processing.

use crate::{
    config::SiteConfig,
    log,
    utils::{
        build::{process_asset, process_content, process_files},
        category::get_deps_mtime,
        git,
    },
};
use anyhow::{Context, Result};
use gix::ThreadSafeRepository;
use std::{ffi::OsStr, fs, path::Path};

/// Build the entire site, processing content and assets in parallel
///
/// If `config.build.clean` is true, clears the entire output directory first.
pub fn build_site(config: &'static SiteConfig) -> Result<ThreadSafeRepository> {
    let output = &config.build.output;
    let content = &config.build.content;
    let assets = &config.build.assets;

    // Initialize output directory with git repo
    let repo = init_output_repo(output, config.build.clean)?;

    // Calculate deps mtime once for all content files
    let deps_mtime = get_deps_mtime(config);

    // Process content and assets in parallel
    let clean = config.build.clean;
    let (posts_result, assets_result) = rayon::join(
        || {
            process_files(
                content,
                config,
                |path| path.starts_with(content),
                |path, cfg| process_content(path, cfg, false, clean, deps_mtime),
            )
            .context("Failed to compile posts")
        },
        || {
            process_files(
                assets,
                config,
                |_| true,
                |path, cfg| process_asset(path, cfg, false, false),
            )
            .context("Failed to copy assets")
        },
    );

    posts_result?;
    assets_result?;

    log_build_result(output)?;

    Ok(repo)
}

/// Initialize output directory with git repository
fn init_output_repo(output: &Path, clean: bool) -> Result<ThreadSafeRepository> {
    match (output.exists(), clean) {
        (true, true) => {
            fs::remove_dir_all(output).with_context(|| {
                format!("Failed to clear output directory: {}", output.display())
            })?;
            git::create_repo(output)
        }
        (true, false) => git::open_repo(output).or_else(|_| {
            log!("git"; "{output:?} is not a git repo, creating new now");
            git::create_repo(output)
        }),
        (false, _) => git::create_repo(output),
    }
}

/// Log build result based on output directory contents
fn log_build_result(output: &Path) -> Result<()> {
    let file_count = fs::read_dir(output)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() != OsStr::new(".git"))
        .count();

    if file_count == 0 {
        log!("warn"; "Output directory is empty. Check if content files have .typ extension.");
    } else {
        log!("build"; "Successfully generated site in: {}", output.display());
    }

    Ok(())
}
