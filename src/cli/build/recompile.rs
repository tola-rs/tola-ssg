use rayon::prelude::*;
use std::path::PathBuf;

use crate::core::{BuildMode, ContentKind};

/// Recompile modified files in parallel. Returns (path, error) for failures
pub fn recompile_files(files: &[PathBuf], mode: BuildMode) -> Vec<(String, String)> {
    use crate::compiler::page::process_page;
    use crate::config::cfg;

    let config = cfg();

    crate::debug!("recompile"; "starting parallel recompile of {} files", files.len());

    // Filter to supported content types
    let content_files: Vec<_> = files
        .iter()
        .filter(|f| ContentKind::from_path(f).is_some())
        .collect();

    // Parallel compile and collect errors
    let errors: Vec<_> = content_files
        .par_iter()
        .filter_map(|file| {
            let rel_path = file
                .strip_prefix(config.get_root())
                .unwrap_or(file)
                .display()
                .to_string();

            match process_page(mode, file, &config) {
                Ok(Some(result)) => {
                    if let Some(vdom) = result.indexed_vdom {
                        crate::compiler::page::cache_vdom(&result.permalink, vdom);
                    }
                    crate::debug!("recompile"; "ok: {}", rel_path);
                    None
                }
                Ok(None) => {
                    crate::debug!("recompile"; "skipped (draft): {}", rel_path);
                    None
                }
                Err(e) => {
                    let error_msg = format!("{}", e);
                    crate::debug!("recompile"; "error: {}: {}", rel_path, error_msg);
                    Some((rel_path, error_msg))
                }
            }
        })
        .collect();

    crate::debug!("recompile"; "finished with {} errors", errors.len());
    errors
}
