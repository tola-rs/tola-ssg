use crate::utils::meta::PageMeta;
use crate::utils::xml::process_html;
use crate::compiler::utils::is_up_to_date;
use crate::compiler::assets::process_relative_asset;
use crate::utils::exec::FilterRule;
use crate::{config::SiteConfig, exec, log};
use anyhow::Result;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

/// Typst filter: skip known warnings.
const TYPST_FILTER: FilterRule = FilterRule::new(&[
    "warning: html export is under active development",
    "and incomplete",
    "= hint: its behaviour may change at any time",
    "= hint: do not rely on this feature for production use cases",
    "= hint: see https://github.com/typst/typst/issues/5512",
    "for more information",
    "warning: elem",
]);

pub fn process_content(
    content_path: &Path,
    config: &'static SiteConfig,
    force_rebuild: bool,
    deps_mtime: Option<SystemTime>,
    log_file: bool,
) -> Result<()> {
    if content_path.extension().is_some_and(|ext| ext == "typ") {
        process_typst_page(content_path, config, force_rebuild, deps_mtime, log_file)
    } else {
        process_relative_asset(content_path, config, force_rebuild, log_file)
    }
}

fn process_typst_page(
    path: &Path,
    config: &'static SiteConfig,
    force_rebuild: bool,
    deps_mtime: Option<SystemTime>,
    log_file: bool,
) -> Result<()> {
    // Process .typ file: get output paths, compile, and post-process
    let page = PageMeta::from_source(path.to_path_buf(), config)?;

    // Check source and dependencies (templates, utils, config)
    if !force_rebuild && is_up_to_date(path, &page.html, deps_mtime) {
        return Ok(());
    }

    if log_file {
        log!("content"; "{}", page.relative);
    }

    if let Some(parent) = page.html.parent() {
        fs::create_dir_all(parent)?;
    }

    let html_content = compile_typst(path, config)?;
    let html_content = process_html(&page.html, &html_content, config)?;

    // Minify HTML if enabled
    let html_content = if config.build.minify {
        let mut cfg = minify_html::Cfg::new();
        cfg.keep_closing_tags = true;
        cfg.keep_html_and_head_opening_tags = true;
        cfg.keep_comments = false;
        cfg.minify_css = true;
        cfg.minify_js = true;
        cfg.remove_bangs = true;
        cfg.remove_processing_instructions = true;
        minify_html::minify(html_content.as_slice(), &cfg)
    } else {
        html_content
    };

    fs::write(&page.html, html_content)?;
    Ok(())
}

fn compile_typst(content_path: &Path, config: &SiteConfig) -> Result<Vec<u8>> {
    let root = config.get_root();
    let output = exec!(
        filter=&TYPST_FILTER;
        &config.build.typst.command;
        "compile", "--features", "html", "--format", "html",
        "--font-path", root, "--root", root,
        content_path, "-"
    )?;
    Ok(output.stdout)
}
