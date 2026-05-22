//! Single page pre-scan for hot reload.

use std::path::Path;

use typst_batch::WithInputs;

use crate::config::SiteConfig;
use crate::core::{ContentKind, LinkOrigin};
use crate::page::{PageKind, PageMeta, StoredPageMap};

use super::{ScannedHeading, ScannedPageLink};
use crate::logger;

/// Scanned data from a single page.
#[derive(Debug, Clone, Default)]
pub struct SinglePageScanData {
    /// Parsed page metadata from scan.
    pub meta: Option<PageMeta>,
    /// Document headings.
    pub headings: Vec<ScannedHeading>,
    /// Internal page link candidates.
    pub links: Vec<ScannedPageLink>,
}

/// Scan a single page to extract metadata, headings, and links.
///
/// Hot reload uses this before compilation to update `@tola/current` data.
pub fn scan_single_page(
    path: &Path,
    config: &SiteConfig,
    host: &crate::compiler::page::TypstHost,
    store: &StoredPageMap,
) -> SinglePageScanData {
    let kind = match ContentKind::from_path(path) {
        Some(k) => k,
        None => return SinglePageScanData::default(),
    };

    match kind {
        ContentKind::Typst => scan_typst_page(path, config, host, store),
        ContentKind::Markdown => scan_markdown_page(path),
    }
}

/// Scan only enough page data to classify direct vs iterative compilation.
pub fn scan_page_kind(
    path: &Path,
    config: &SiteConfig,
    host: &crate::compiler::page::TypstHost,
    store: &StoredPageMap,
) -> Option<PageKind> {
    match ContentKind::from_path(path)? {
        ContentKind::Typst => {
            let scan = scan_typst(path, config, host, store)?;
            Some(PageKind::from_packages(scan.accessed_packages()))
        }
        ContentKind::Markdown => Some(PageKind::Direct),
    }
}

fn scan_typst(
    path: &Path,
    config: &SiteConfig,
    host: &crate::compiler::page::TypstHost,
    store: &StoredPageMap,
) -> Option<typst_batch::ScanResult> {
    let root = config.get_root();
    let mut scanner = host.scanner(root);

    if let Ok(inputs) = crate::package::build_visible_inputs_for_source(config, store, path) {
        scanner = scanner.with_inputs_obj(inputs);
    }

    match scanner.scan(path) {
        Ok(scan) => Some(scan),
        Err(e) => {
            logger::debug(
                "scan",
                format_args!("typst page scan failed for {}: {}", path.display(), e),
            );
            None
        }
    }
}

fn scan_typst_page(
    path: &Path,
    config: &SiteConfig,
    host: &crate::compiler::page::TypstHost,
    store: &StoredPageMap,
) -> SinglePageScanData {
    let label = &config.build.meta.label;
    let Some(scan) = scan_typst(path, config, host, store) else {
        return SinglePageScanData::default();
    };

    let meta = scan
        .metadata(label)
        .and_then(|value| serde_json::from_value(value).ok());

    let headings = scan
        .headings()
        .into_iter()
        .map(|h| ScannedHeading {
            level: h.level,
            text: h.text,
            supplement: h.supplement,
        })
        .collect();

    let links = scan
        .links()
        .into_iter()
        .map(|link| ScannedPageLink::new(link.dest, LinkOrigin::from(link.source)))
        .filter(ScannedPageLink::is_page_candidate)
        .collect();

    SinglePageScanData {
        meta,
        headings,
        links,
    }
}

fn scan_markdown_page(path: &Path) -> SinglePageScanData {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return SinglePageScanData::default(),
    };

    let meta = super::super::markdown::MarkdownMetaExtractor
        .extract_frontmatter(&content)
        .ok()
        .flatten()
        .map(|(meta, _)| meta);

    SinglePageScanData {
        meta,
        headings: super::super::markdown::extract_headings(&content),
        links: super::super::markdown::extract_links(&content),
    }
}

#[cfg(test)]
mod tests {
    use super::{scan_page_kind, scan_single_page};
    use crate::config::SiteConfig;
    use crate::page::{PageKind, StoredPageMap};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_scan_single_page_typst_keeps_relative_page_links_and_skips_images() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();

        let source = content_dir.join("post.typ");
        let image = content_dir.join("cat.svg");
        fs::write(
            &image,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"></svg>"#,
        )
        .unwrap();
        fs::write(
            &source,
            r#"#link("../about/")[About]
#image("cat.svg")
"#,
        )
        .unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;

        let store = StoredPageMap::new();
        let host = crate::compiler::page::TypstHost::for_config(&config);
        let scan = scan_single_page(&source, &config, &host, &store);

        assert!(scan.links.iter().any(|link| link.dest == "../about/"));
        assert!(!scan.links.iter().any(|link| link.dest == "cat.svg"));
    }

    #[test]
    fn scan_page_kind_keeps_plain_typst_direct() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();

        let source = content_dir.join("page.typ");
        fs::write(&source, "= Hello").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;

        let store = StoredPageMap::new();
        let host = crate::compiler::page::TypstHost::for_config(&config);

        assert_eq!(
            scan_page_kind(&source, &config, &host, &store),
            Some(PageKind::Direct)
        );
    }

    #[test]
    fn scan_page_kind_detects_iterative_typst_packages() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();

        let source = content_dir.join("page.typ");
        fs::write(
            &source,
            r#"
#import "@tola/current:0.0.0": current-permalink
#let _probe = current-permalink
= Hello
"#,
        )
        .unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.content = content_dir;

        let store = StoredPageMap::new();
        let host = crate::compiler::page::TypstHost::for_config(&config);

        assert_eq!(
            scan_page_kind(&source, &config, &host, &store),
            Some(PageKind::Iterative)
        );
    }
}
