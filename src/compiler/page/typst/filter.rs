//! Draft filtering for Typst files.

use std::path::{Path, PathBuf};

use typst_batch::prelude::*;

use crate::compiler::page::format::{DraftFilter, FilterResult, ScannedHeading, ScannedPage};
use crate::compiler::page::{Typst, TypstBatcher};
use crate::config::SiteConfig;
use crate::package::Phase;
use crate::package::TolaPackage;
use crate::page::{PageKind, PageMeta};

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

fn build_scan_inputs(root: &Path, config: Option<&SiteConfig>) -> Option<typst_batch::Inputs> {
    let mut combined = serde_json::Map::new();
    combined.insert(
        Phase::input_key().to_string(),
        serde_json::json!(Phase::Filter.as_str()),
    );

    // Keep @tola/site available during scan phase so expressions like
    // `info.extra.foo` don't fail before visible compilation.
    if let Some(config) = config {
        let site_info_json = serde_json::to_value(&config.site.info)
            .unwrap_or(serde_json::Value::Object(Default::default()));
        combined.insert(TolaPackage::Site.input_key(), site_info_json);
    }

    typst_batch::Inputs::from_json_with_content(&serde_json::Value::Object(combined), root).ok()
}

fn filter_drafts_impl<'a>(
    files: &[&PathBuf],
    root: &'a Path,
    label: &str,
    config: Option<&SiteConfig>,
) -> TypstFilterResult<'a> {
    if files.is_empty() {
        return TypstFilterResult::empty(0, None);
    }

    let mut builder = Compiler::new(root).into_batch();
    builder = match build_scan_inputs(root, config) {
        Some(inputs) => builder.with_inputs_obj(inputs),
        None => builder.with_inputs([(Phase::input_key(), Phase::Filter.as_str())]),
    };

    let batcher = match builder.with_snapshot_from(files) {
        Ok(b) => b,
        Err(_) => return TypstFilterResult::empty(0, None), // On prepare error, include all
    };

    let scan_results = match batcher.batch_scan(files) {
        Ok(results) => results,
        Err(_) => return TypstFilterResult::empty(0, Some(batcher)), // On batch error, include all
    };

    let mut scanned = Vec::new();
    let mut errors = Vec::new();
    let mut draft_count = 0;

    for (path, result) in files.iter().zip(scan_results) {
        match result {
            Ok(scan) => {
                let meta_json = scan.metadata(label);
                let meta: Option<PageMeta> = meta_json
                    .as_ref()
                    .and_then(|v| serde_json::from_value(v.clone()).ok());

                // Check if draft
                let is_draft_page = meta.as_ref().map(|m| m.draft).unwrap_or(false);
                if is_draft_page {
                    draft_count += 1;
                    continue;
                }

                // Determine page kind from accessed packages
                let kind = PageKind::from_packages(scan.accessed_packages());

                // Extract internal page links (site-root links only)
                let links: Vec<String> = scan
                    .links()
                    .into_iter()
                    .filter(|link| link.is_site_root())
                    .map(|link| link.dest)
                    .collect();

                // Extract headings
                let headings: Vec<ScannedHeading> = scan
                    .headings()
                    .into_iter()
                    .map(|h| ScannedHeading {
                        level: h.level,
                        text: h.text,
                        supplement: h.supplement,
                    })
                    .collect();

                scanned.push(ScannedPage {
                    path: (*path).clone(),
                    meta,
                    kind,
                    links,
                    headings,
                });
            }
            Err(e) => {
                // Collect error for reporting
                errors.push(((*path).clone(), e));
            }
        }
    }

    TypstFilterResult::new(draft_count, Some(batcher), scanned, errors)
}

/// Filter Typst files, removing drafts
///
/// Also collects metadata and detects iterative pages (importing @tola/pages or @tola/current)
/// for pre-scan optimization
///
/// Returns batcher for reuse in subsequent compilation phases
pub fn filter_drafts<'a>(files: &[&PathBuf], root: &'a Path, label: &str) -> TypstFilterResult<'a> {
    filter_drafts_impl(files, root, label, None)
}

/// Filter Typst drafts with explicit site config injection.
///
/// This keeps `@tola/site` behavior consistent between filter and visible phases.
pub fn filter_drafts_with_config<'a>(
    files: &[&PathBuf],
    root: &'a Path,
    label: &str,
    config: &SiteConfig,
) -> TypstFilterResult<'a> {
    filter_drafts_impl(files, root, label, Some(config))
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

// DraftFilter trait implementation (without batcher, for generic use)
impl DraftFilter for Typst {
    type Extra = ();

    fn filter_drafts<'a>(
        files: Vec<&'a PathBuf>,
        root: &'a Path,
        label: &str,
    ) -> FilterResult<'a, Self::Extra> {
        let result = filter_drafts(&files, root, label);
        // Note: batcher is discarded here. Use filter_drafts() directly to get batcher.
        let non_draft_files: Vec<_> = result
            .scanned
            .iter()
            .filter_map(|s| files.iter().find(|f| ***f == s.path).copied())
            .collect();
        FilterResult::new(non_draft_files, result.draft_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::page::typst::init::init_typst;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_filter_drafts_with_config_injects_site_extra() {
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

        init_typst(&[]);

        let mut config = SiteConfig::default();
        config.set_root(root);
        config
            .site
            .info
            .extra
            .insert("custom0".into(), toml::Value::String("ABC".into()));

        let files = vec![page];
        let refs = files.iter().collect::<Vec<_>>();
        let result = filter_drafts_with_config(&refs, root, "tola-meta", &config);

        assert!(
            result.errors.is_empty(),
            "unexpected scan errors: {:?}",
            result.errors.len()
        );
        assert_eq!(result.scanned.len(), 1);
    }
}
