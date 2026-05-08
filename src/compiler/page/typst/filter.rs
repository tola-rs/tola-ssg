//! Draft filtering for Typst files.

use std::path::{Path, PathBuf};

use typst_batch::prelude::*;

use super::iterative::{MAX_METADATA_SCAN_ITERATIONS, scan_single_with_current_in_store};
use crate::compiler::page::TypstBatcher;
use crate::compiler::page::format::{ScannedHeading, ScannedPage, ScannedPageLink};
use crate::config::SiteConfig;
use crate::core::LinkOrigin;
use crate::package::build_filter_inputs_with_site;
use crate::page::{HashStabilityTracker, PageKind, PageMeta, StabilityDecision, StoredPageMap};

/// Result of Typst draft filtering, includes batcher for reuse
pub struct TypstFilterResult<'a> {
    /// Number of draft files filtered out.
    pub draft_count: usize,
    /// Batcher for reuse in subsequent compilation.
    pub batcher: Option<TypstBatcher<'a>>,
    /// Pre-scanned page data (metadata + kind) for non-draft files.
    pub scanned: Vec<ScannedPage>,
    /// Errors encountered during scan phase.
    pub errors: Vec<(PathBuf, typst_batch::CompileError)>,
}

impl<'a> TypstFilterResult<'a> {
    fn new(
        draft_count: usize,
        batcher: Option<TypstBatcher<'a>>,
        scanned: Vec<ScannedPage>,
        errors: Vec<(PathBuf, typst_batch::CompileError)>,
    ) -> Self {
        Self {
            draft_count,
            batcher,
            scanned,
            errors,
        }
    }

    fn empty(draft_count: usize, batcher: Option<TypstBatcher<'a>>) -> Self {
        Self::new(draft_count, batcher, vec![], vec![])
    }
}

fn build_scan_inputs(config: &SiteConfig, store: &StoredPageMap) -> Option<typst_batch::Inputs> {
    // Filter phase: only inject phase + optional site info.
    // pages/current remain intentionally unavailable in this stage.
    build_filter_inputs_with_site(config, store).ok()
}

fn parse_meta(meta_json: Option<serde_json::Value>) -> Option<PageMeta> {
    meta_json.and_then(|v| serde_json::from_value(v).ok())
}

fn to_scanned_page(
    path: &Path,
    scan: typst_batch::ScanResult,
    label: &str,
    store: &StoredPageMap,
    config: &SiteConfig,
) -> ScannedPage {
    let meta = parse_meta(scan.metadata(label));

    // Feed metadata into local scan store so iterative re-scan can resolve
    // dynamic fields with up-to-date pages/permalinks.
    if let Some(meta_for_store) = meta.clone() {
        let _ = store.apply_meta_for_source(path, meta_for_store, config);
    }

    let kind = PageKind::from_packages(scan.accessed_packages());
    let links = scan
        .links()
        .into_iter()
        .map(|link| ScannedPageLink::new(link.dest, LinkOrigin::from(link.source)))
        .filter(ScannedPageLink::is_page_candidate)
        .collect();
    let headings = scan
        .headings()
        .into_iter()
        .map(|h| ScannedHeading {
            level: h.level,
            text: h.text,
            supplement: h.supplement,
        })
        .collect();

    ScannedPage {
        path: path.to_path_buf(),
        meta,
        kind,
        links,
        headings,
    }
}

fn filter_drafts_impl<'a>(
    files: &[&PathBuf],
    root: &'a Path,
    label: &str,
    config: &SiteConfig,
) -> TypstFilterResult<'a> {
    if files.is_empty() {
        return TypstFilterResult::empty(0, None);
    }

    // Use an isolated store for scan-time metadata convergence.
    // This avoids mutating page storage during pre-scan.
    let store = StoredPageMap::new();

    let Some(inputs) = build_scan_inputs(config, &store) else {
        return TypstFilterResult::empty(0, None);
    };

    let mut builder = Compiler::new(root).into_batch();
    builder = builder.with_inputs_obj(inputs);

    let batcher = match builder.with_snapshot_from(files) {
        Ok(b) => b,
        Err(_) => return TypstFilterResult::empty(0, None), // On prepare error, include all
    };

    let scan_results = match batcher.batch_scan(files) {
        Ok(results) => results,
        Err(_) => return TypstFilterResult::empty(0, Some(batcher)), // On batch error, include all
    };

    let mut slots: Vec<Option<ScannedPage>> = vec![None; files.len()];
    let mut errors: Vec<Option<(PathBuf, typst_batch::CompileError)>> =
        (0..files.len()).map(|_| None).collect();
    let mut iterative_indices: Vec<usize> = Vec::new();

    for (idx, (path, result)) in files.iter().zip(scan_results).enumerate() {
        match result {
            Ok(scan) => {
                let page = to_scanned_page(path, scan, label, &store, config);
                if page.kind.is_iterative() {
                    iterative_indices.push(idx);
                }
                slots[idx] = Some(page);
            }
            Err(e) => {
                // Retry with per-file visible inputs so @tola/current-dependent
                // templates can scan successfully in startup/filter paths.
                match scan_single_with_current_in_store(root, path, config, &store) {
                    Ok(scan) => {
                        let page = to_scanned_page(path, scan, label, &store, config);
                        if page.kind.is_iterative() {
                            iterative_indices.push(idx);
                        }
                        slots[idx] = Some(page);
                    }
                    Err(_) => {
                        errors[idx] = Some(((*path).clone(), e));
                    }
                }
            }
        }
    }

    if !iterative_indices.is_empty() {
        let mut stability = HashStabilityTracker::with_oscillation_detection(store.pages_hash());

        for iteration in 0..MAX_METADATA_SCAN_ITERATIONS {
            for &idx in &iterative_indices {
                // Keep previously failed pages as errors.
                if errors[idx].is_some() {
                    continue;
                }

                let path = files[idx];
                match scan_single_with_current_in_store(root, path, config, &store) {
                    Ok(scan) => {
                        slots[idx] = Some(to_scanned_page(path, scan, label, &store, config));
                    }
                    Err(e) => {
                        slots[idx] = None;
                        errors[idx] = Some(((*path).clone(), e));
                    }
                }
            }

            match stability.decide(store.pages_hash(), iteration, MAX_METADATA_SCAN_ITERATIONS) {
                StabilityDecision::Converged => {
                    crate::debug!("scan"; "converged after {} iteration(s)", iteration + 1);
                    break;
                }
                StabilityDecision::Oscillating => {
                    crate::log!(
                        "warning";
                        "scan metadata oscillating (cycle detected), stopping after {} iterations",
                        iteration + 1
                    );
                    break;
                }
                StabilityDecision::MaxIterationsReached => {
                    crate::log!(
                        "warning";
                        "scan metadata did not converge after {} iterations",
                        MAX_METADATA_SCAN_ITERATIONS
                    );
                }
                StabilityDecision::Continue => {}
            }
        }
    }

    let mut scanned = Vec::new();
    let mut draft_count = 0usize;
    for page in slots.into_iter().flatten() {
        if page.meta.as_ref().is_some_and(|m| m.draft) {
            draft_count += 1;
            continue;
        }
        scanned.push(page);
    }
    let errors = errors.into_iter().flatten().collect();

    TypstFilterResult::new(draft_count, Some(batcher), scanned, errors)
}

/// Filter Typst files, removing drafts
///
/// Also collects metadata and detects iterative pages (importing @tola/pages or @tola/current)
/// for pre-scan optimization
///
/// Returns batcher for reuse in subsequent compilation phases
pub fn filter_drafts<'a>(
    files: &[&PathBuf],
    root: &'a Path,
    label: &str,
    config: &SiteConfig,
) -> TypstFilterResult<'a> {
    filter_drafts_impl(files, root, label, config)
}

/// Check if a Typst scan result indicates a draft page
#[allow(dead_code)]
#[inline]
fn is_draft(scan: &typst_batch::ScanResult, label: &str) -> bool {
    scan.metadata(label)
        .as_ref()
        .and_then(|m| m.get("draft"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::page::typst::init_runtime;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_filter_drafts_injects_site_extra() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let page = root.join("page.typ");

        fs::write(
            &page,
            r#"
#import "@tola/site:0.0.0": info
#metadata((title: "test")) <tola-meta>
#let _x = info.extra.custom0
= Hello
"#,
        )
        .unwrap();

        init_runtime(&[], root.to_path_buf(), Vec::new());

        let mut config = SiteConfig::default();
        config.set_root(root);
        config
            .site
            .info
            .extra
            .insert("custom0".into(), toml::Value::String("ABC".into()));

        let files = [page];
        let refs = files.iter().collect::<Vec<_>>();
        let result = filter_drafts(&refs, root, "tola-meta", &config);

        assert!(
            result.errors.is_empty(),
            "unexpected scan errors: {:?}",
            result.errors
        );
        assert_eq!(result.scanned.len(), 1);
    }

    #[test]
    fn test_filter_drafts_injects_site_root() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let page = root.join("page.typ");

        fs::write(
            &page,
            r#"
#import "@tola/site:0.0.0": root
#metadata((title: root)) <tola-meta>
= Hello
"#,
        )
        .unwrap();

        init_runtime(&[], root.to_path_buf(), Vec::new());

        let mut config = SiteConfig::default();
        config.set_root(root);
        config.build.path_prefix = std::path::PathBuf::from("sub/xxx");

        let files = [page];
        let refs = files.iter().collect::<Vec<_>>();
        let result = filter_drafts(&refs, root, "tola-meta", &config);

        assert!(
            result.errors.is_empty(),
            "unexpected scan errors: {:?}",
            result.errors
        );
        assert_eq!(result.scanned.len(), 1);
    }

    #[test]
    fn test_filter_drafts_allows_current_permalink_link() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content_dir = root.join("content");
        fs::create_dir_all(&content_dir).unwrap();
        let page = content_dir.join("page.typ");

        fs::write(
            &page,
            r#"
#import "@tola/current:0.0.0": current-permalink
#metadata((title: "current-link")) <tola-meta>
#let _x = link(current-permalink)[current-permalink]
= Hello
"#,
        )
        .unwrap();

        init_runtime(&[], root.to_path_buf(), Vec::new());

        let mut config = SiteConfig::default();
        config.set_root(root);
        config.build.content = content_dir;

        let files = [page];
        let refs = files.iter().collect::<Vec<_>>();
        let result = filter_drafts(&refs, root, "tola-meta", &config);

        assert!(
            result.errors.is_empty(),
            "unexpected scan errors: {:?}",
            result.errors
        );
        assert_eq!(result.scanned.len(), 1);
    }
}
