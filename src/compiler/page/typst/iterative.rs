//! Shared helpers for iterative Typst scan flows.

use std::path::{Path, PathBuf};

use typst_batch::prelude::*;

use crate::config::SiteConfig;
use crate::package::build_visible_inputs_for_source;
use crate::page::StoredPageMap;

/// Default maximum iterations for metadata convergence during scan.
pub const MAX_METADATA_SCAN_ITERATIONS: usize = 5;

/// Scan a single file with per-file `@tola/current` using global page store.
pub fn scan_single_with_current(
    root: &Path,
    host: &super::TypstHost,
    file: &PathBuf,
    config: &SiteConfig,
    store: &StoredPageMap,
) -> Result<typst_batch::ScanResult, typst_batch::CompileError> {
    scan_single_with_current_in_store(root, host, file, config, store)
}

/// Scan a single file with per-file `@tola/current` using the provided page store.
pub(super) fn scan_single_with_current_in_store(
    root: &Path,
    host: &super::TypstHost,
    file: &PathBuf,
    config: &SiteConfig,
    store: &StoredPageMap,
) -> Result<typst_batch::ScanResult, typst_batch::CompileError> {
    let inputs = build_visible_inputs_for_source(config, store, file).map_err(|e| {
        typst_batch::CompileError::html_export(format!(
            "failed to build scan inputs for {}: {}",
            file.display(),
            e
        ))
    })?;

    let single = [file];
    let scanner = host
        .batch_scanner(root)
        .with_inputs_obj(inputs)
        .with_snapshot_from(&single)?;

    match scanner.batch_scan(&single)? {
        mut results if !results.is_empty() => results.pop().expect("non-empty result"),
        _ => Err(typst_batch::CompileError::html_export(format!(
            "single-file scan returned no result for {}",
            file.display()
        ))),
    }
}
