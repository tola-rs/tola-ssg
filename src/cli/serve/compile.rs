//! On-demand page compilation for progressive serving.
//!
//! Delegates to the central CompileScheduler for priority-based compilation.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;

use crate::compiler::scheduler::{CompileResult, SCHEDULER};
use crate::config::SiteConfig;
use crate::core::Priority;
use crate::page::CompiledPage;

/// Ensure Typst is initialized (lazy, only triggered on first on-demand compile).
fn ensure_typst_initialized(config: &SiteConfig) {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let font_dirs = crate::cli::build::collect_font_dirs(config);
        crate::compiler::page::typst::init_typst(&font_dirs);
    });
}

/// Compile a single page on-demand and write it to disk.
///
/// Returns the output file path for serving via `respond_file`.
/// Uses High priority to ensure user requests are processed first.
pub fn compile_on_demand(source: &Path, config: &SiteConfig) -> Result<PathBuf> {
    ensure_typst_initialized(config);

    // Check if already compiled (output file exists on disk)
    let page = CompiledPage::from_paths(source, config)?;
    if page.route.output_file.exists() {
        return Ok(page.route.output_file);
    }

    // Delegate to scheduler with Active priority (highest)
    match SCHEDULER.compile(source.to_path_buf(), Priority::Active) {
        CompileResult::Success(output) => Ok(output),
        CompileResult::Failed(error) => Err(anyhow::anyhow!("{}", error)),
        CompileResult::Skipped => Err(anyhow::anyhow!("page skipped (draft?): {}", source.display())),
    }
}
