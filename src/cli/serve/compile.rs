//! On-demand page compilation for progressive serving.
//!
//! Delegates to the central CompileScheduler for priority-based compilation.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;

use crate::compiler::dependency::global as dep_graph;
use crate::compiler::scheduler::{CompileResult, SCHEDULER};
use crate::config::SiteConfig;
use crate::core::Priority;
use crate::freshness::mtime::{get_mtime, is_newer_than};
use crate::page::CompiledPage;

/// Ensure Typst is initialized (lazy, only triggered on first on-demand compile)
fn ensure_typst_initialized(config: &SiteConfig) {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let font_dirs = crate::cli::build::collect_font_dirs(config);
        let nested_mappings =
            crate::compiler::page::typst::build_nested_mappings(&config.build.assets.nested);
        crate::compiler::page::typst::init_typst_with_mappings(
            &font_dirs,
            config.get_root().to_path_buf(),
            nested_mappings,
        );
    });
}

/// Compile a single page on-demand and write it to disk
///
/// Returns the output file path for serving via `respond_file`
/// Uses High priority to ensure user requests are processed first
pub fn compile_on_demand(source: &Path, config: &SiteConfig) -> Result<PathBuf> {
    ensure_typst_initialized(config);

    let page = CompiledPage::from_paths(source, config)?;

    // Check if output is fresh (newer than source AND all dependencies)
    if is_output_fresh(source, &page.route.output_file) {
        return Ok(page.route.output_file);
    }

    // Delegate to scheduler with Active priority (highest)
    match SCHEDULER.compile(source.to_path_buf(), Priority::Active) {
        CompileResult::Success(output) => Ok(output),
        CompileResult::Failed(error) => Err(anyhow::anyhow!("{}", error)),
        CompileResult::Skipped => Err(anyhow::anyhow!(
            "page skipped (draft?): {}",
            source.display()
        )),
    }
}

/// Check if output is fresh (newer than source and all dependencies)
fn is_output_fresh(source: &Path, output: &Path) -> bool {
    // Output must exist and be newer than source
    if !is_newer_than(output, source) {
        crate::debug!("fresh"; "{}: output older than source", source.display());
        return false;
    }

    // Check dependencies (templates, utils, etc.)
    if let Some(deps) = dep_graph::uses(source) {
        let output_mtime = get_mtime(output);
        for dep in &deps {
            // If any dependency is newer than output, output is stale
            if let (Some(out_time), Some(dep_time)) = (output_mtime, get_mtime(dep))
                && dep_time > out_time
            {
                crate::debug!("fresh"; "{}: dep {} is newer", source.display(), dep.display());
                return false;
            }
        }
        crate::debug!("fresh"; "{}: fresh (checked {} deps)", source.display(), deps.len());
    } else {
        crate::debug!("fresh"; "{}: fresh (no deps recorded)", source.display());
    }

    true
}
