//! HTML file writing for compiled pages.
//!
//! All HTML processing (head injection, link resolution, minification)
//! happens in the VDOM pipeline. This module only handles file I/O.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::compiler::page::CompiledPage;
use crate::core::UrlPath;
use crate::embed::build::{REDIRECT_HTML, RedirectVars};
use crate::freshness::{self, ContentHash, is_fresh};
use crate::log;

/// Write a page's HTML to disk. Also copies colocated assets
pub fn write_page_html(page: &CompiledPage) -> Result<()> {
    write_page(page, true, None, false)?;

    // Copy colocated assets if present
    crate::asset::copy_colocated_assets(&page.route, true)?;

    Ok(())
}

/// Write redirect HTML files for all aliases of a page
pub fn write_redirects(page: &CompiledPage, output_dir: &Path) -> Result<()> {
    let targets = collect_redirect_targets(page);
    if targets.is_empty() {
        return Ok(());
    }

    for (alias_url, canonical_url) in targets {
        write_redirect_file(&alias_url, &canonical_url, output_dir)?;
        log!("redirect"; "{} -> {}", alias_url, canonical_url);
    }

    Ok(())
}

fn collect_redirect_targets(page: &CompiledPage) -> Vec<(UrlPath, UrlPath)> {
    let Some(meta) = &page.content_meta else {
        return Vec::new();
    };

    if meta.aliases.is_empty() {
        return Vec::new();
    }

    let canonical = page.route.permalink.clone();
    meta.aliases
        .iter()
        .map(|alias| (UrlPath::from_page(alias), canonical.clone()))
        .collect()
}

/// Build the final HTML with embedded hash marker for freshness detection
fn build_final_html(
    html_content: &[u8],
    source_hash: &ContentHash,
    deps_hash: Option<&ContentHash>,
) -> String {
    let hash_marker = freshness::build_hash_marker(source_hash, deps_hash);
    let html_str = String::from_utf8_lossy(html_content);

    if let Some(pos) = html_str.rfind("</html>") {
        format!("{}{}\n</html>", &html_str[..pos], hash_marker)
    } else {
        format!("{}\n{}", html_str, hash_marker)
    }
}

fn build_redirect_html(canonical_url: &UrlPath) -> String {
    let url_str = canonical_url.to_string();
    REDIRECT_HTML.render(&RedirectVars {
        canonical_url: &url_str,
    })
}

/// `/old-url/` -> `{output_dir}/old-url/index.html`
fn compute_redirect_output_path(alias_url: &UrlPath, output_dir: &Path) -> PathBuf {
    let relative = alias_url.to_string();
    let relative = relative.trim_start_matches('/');
    output_dir.join(relative).join("index.html")
}

/// Write a page's HTML to disk with freshness check
pub(super) fn write_page(
    page: &CompiledPage,
    clean: bool,
    deps_hash: Option<ContentHash>,
    log_file: bool,
) -> Result<()> {
    if !clean && is_fresh(&page.route.source, &page.route.output_file, deps_hash) {
        return Ok(());
    }

    if log_file {
        log!("content"; "{}", page.route.relative);
    }

    if let Some(parent) = page.route.output_file.parent() {
        fs::create_dir_all(parent)?;
    }

    let html_content = page
        .compiled_html
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Page has no compiled HTML: {:?}", page.route.source))?;

    let source_hash = freshness::compute_file_hash(&page.route.source);
    let final_html = build_final_html(html_content, &source_hash, deps_hash.as_ref());

    fs::write(&page.route.output_file, final_html)?;

    Ok(())
}

fn write_redirect_file(
    alias_url: &UrlPath,
    canonical_url: &UrlPath,
    output_dir: &Path,
) -> Result<()> {
    let output_file = compute_redirect_output_path(alias_url, output_dir);

    if let Some(parent) = output_file.parent() {
        fs::create_dir_all(parent)?;
    }

    let html = build_redirect_html(canonical_url);
    fs::write(&output_file, html)?;

    Ok(())
}
