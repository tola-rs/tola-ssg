//! On-demand page compilation for progressive serving.
//!
//! Delegates to the central CompileScheduler for priority-based compilation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use crate::address::SiteIndex;
use crate::compiler::dependency::global as dep_graph;
use crate::compiler::page::TypstHost;
use crate::compiler::scheduler::{CompileResult, SCHEDULER};
use crate::config::SiteConfig;
use crate::core::Priority;
use crate::freshness::mtime::{get_mtime, is_newer_than};
use crate::page::CompiledPage;

/// Compile a single page on-demand and write it to disk
///
/// Returns the output file path for serving via `respond_file`
/// Uses High priority to ensure user requests are processed first
pub fn compile_on_demand(
    source: &Path,
    config: &SiteConfig,
    typst_host: Arc<TypstHost>,
    state: Arc<SiteIndex>,
) -> Result<PathBuf> {
    let page = CompiledPage::from_paths(source, config)?;

    // Check if output is fresh (newer than source AND all dependencies)
    if is_output_fresh(source, &page.route.output_file) {
        return Ok(page.route.output_file);
    }

    prepare_stale_output_recompile(source);

    // Delegate to scheduler with Active priority (highest)
    match SCHEDULER.compile(
        source.to_path_buf(),
        Priority::Active,
        Arc::new(config.clone()),
        typst_host,
        state,
    ) {
        CompileResult::Success { output, .. } => Ok(output),
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

fn prepare_stale_output_recompile(source: &Path) {
    SCHEDULER.invalidate(source);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::section::build::assets::NestedEntry;
    use std::fs;
    use tempfile::TempDir;

    fn config_with_nested_asset(root: &Path, output_name: &str, source_dir: &Path) -> SiteConfig {
        let mut config = SiteConfig::default();
        config.set_root(root);
        config.build.content = root.join("content");
        config.build.assets.nested = vec![NestedEntry::Full {
            dir: source_dir.to_path_buf(),
            output_as: Some(output_name.to_string()),
        }];
        config
    }

    #[test]
    fn typst_host_uses_current_config_on_each_request() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        let output_name = "runtime-refresh-probe";
        let asset_dir = first.path().join("assets");
        fs::create_dir_all(first.path().join("content")).unwrap();
        fs::create_dir_all(second.path().join("content")).unwrap();
        fs::create_dir_all(&asset_dir).unwrap();
        fs::write(asset_dir.join("probe.txt"), "first").unwrap();

        let first_config = config_with_nested_asset(first.path(), output_name, &asset_dir);
        let second_asset_dir = second.path().join("assets");
        let second_config = config_with_nested_asset(second.path(), output_name, &second_asset_dir);
        let virtual_path = PathBuf::from(format!("/{output_name}/probe.txt"));

        let first_host = crate::compiler::page::TypstHost::for_config(&first_config);
        assert!(first_host.is_virtual_path(&virtual_path));

        let second_host = crate::compiler::page::TypstHost::for_config(&second_config);
        assert!(!second_host.is_virtual_path(&virtual_path));
    }
}
