//! Content file scanning for validation.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::cli::common::{batch_scan_typst, scan_markdown_file};
use crate::compiler::family::Indexed;
use crate::config::SiteConfig;
use crate::core::LinkKind;
use tola_vdom::Document;

/// A link extracted from a content file
#[derive(Debug, Clone)]
pub struct ScannedLink {
    /// Link destination.
    pub dest: String,
    /// Source attribute or element type (e.g., "href", "src", "Link").
    pub attr: String,
}

impl ScannedLink {
    /// Classify this link.
    #[inline]
    pub fn kind(&self) -> LinkKind<'_> {
        LinkKind::parse(&self.dest)
    }

    /// Check if this is an HTTP/HTTPS link.
    #[inline]
    #[allow(unused)]
    pub fn is_http(&self) -> bool {
        LinkKind::is_http(&self.dest)
    }

    /// Check if this is a site-root link (starts with `/`).
    #[inline]
    #[allow(unused)]
    pub fn is_site_root(&self) -> bool {
        matches!(self.kind(), LinkKind::SiteRoot(_))
    }

    /// Check if this is a file-relative link.
    #[inline]
    #[allow(unused)]
    pub fn is_file_relative(&self) -> bool {
        matches!(self.kind(), LinkKind::FileRelative(_))
    }
}

/// Result of scanning a single file
pub struct ScanResult {
    /// Source file path (relative to root).
    pub source: String,
    /// All links found in the file.
    pub links: Vec<ScannedLink>,
    /// Indexed VDOM for fragment validation (Markdown only).
    pub indexed_vdom: Option<Document<Indexed>>,
}

/// Scan Typst files in batch (for link extraction, not metadata)
/// Uses default Phase::Filter so pages() in content body returns empty silently
pub fn scan_typst_batch(files: &[&PathBuf], root: &Path) -> Vec<Option<ScanResult>> {
    let scans = batch_scan_typst(files, root);

    scans
        .into_iter()
        .zip(files)
        .map(|(scan, file)| {
            scan.map(|s| {
                let source = file
                    .strip_prefix(root)
                    .unwrap_or(file)
                    .to_string_lossy()
                    .to_string();
                ScanResult {
                    source,
                    links: s
                        .links()
                        .into_iter()
                        .map(|l| typst_link_to_scanned(&l))
                        .collect(),
                    indexed_vdom: None,
                }
            })
        })
        .collect()
}

/// Convert typst-batch Link to ScannedLink
fn typst_link_to_scanned(link: &typst_batch::Link) -> ScannedLink {
    ScannedLink {
        dest: link.dest.clone(),
        attr: format!("{:?}", link.source),
    }
}

/// Scan a Markdown file for links
pub fn scan_markdown(file: &Path, root: &Path, config: &SiteConfig) -> Result<ScanResult> {
    let result = scan_markdown_file(file, config)?;

    let source = file
        .strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .to_string();

    let links = extract_links_from_vdom(&result.indexed_vdom);

    Ok(ScanResult {
        source,
        links,
        indexed_vdom: Some(result.indexed_vdom),
    })
}

/// Extract all links from an indexed VDOM document
fn extract_links_from_vdom(doc: &Document<Indexed>) -> Vec<ScannedLink> {
    const URL_ATTRS: [&str; 4] = ["href", "src", "poster", "data"];

    let mut links = Vec::new();

    for elem in doc.elements() {
        for attr in URL_ATTRS {
            if let Some(dest) = elem.get_attr(attr)
                && !dest.is_empty()
            {
                links.push(ScannedLink {
                    dest: dest.to_string(),
                    attr: attr.to_string(),
                });
            }
        }
    }

    links
}
