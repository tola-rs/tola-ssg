//! Content and asset processing.
//!
//! Handles compilation of Typst files to HTML and asset copying/optimization.

use crate::utils::svg::{HtmlContext, Svg, compress_svgs_parallel, extract_svg_element};
use crate::utils::watch::wait_until_stable;
use crate::utils::xml::{
    create_xml_reader, write_element_with_processed_links, write_head_content,
    write_heading_with_slugified_id, write_html_with_lang,
};
use crate::{config::SiteConfig, exec, log, utils::page::PageMeta};
use anyhow::{Result, anyhow};
use quick_xml::{
    Reader, Writer,
    events::{BytesEnd, BytesStart, Event},
};
use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    time::SystemTime,
};
use walkdir::WalkDir;

// ============================================================================
// Directory Operations
// ============================================================================

/// Files to ignore during directory traversal
const IGNORED_FILES: &[&str] = &[".DS_Store"];

/// Collect all files from a directory recursively
pub fn collect_all_files(dir: &Path) -> Vec<PathBuf> {
    WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let name = e.file_name().to_str().unwrap_or_default();
            !IGNORED_FILES.contains(&name)
        })
        .map(|e| e.into_path())
        .collect()
}

// ============================================================================
// Content Processing
// ============================================================================

/// Check if destination is up-to-date compared to source and dependencies
pub fn is_up_to_date(src: &Path, dst: &Path, deps_mtime: Option<SystemTime>) -> bool {
    let Ok(src_time) = src.metadata().and_then(|m| m.modified()) else {
        return false;
    };
    let Ok(dst_time) = dst.metadata().and_then(|m| m.modified()) else {
        return false;
    };

    // Check if source is newer than destination
    if src_time > dst_time {
        return false;
    }

    // Check if any dependency is newer than destination
    if let Some(deps) = deps_mtime
        && deps > dst_time
    {
        return false;
    }

    true
}

pub fn process_content(
    content_path: &Path,
    config: &'static SiteConfig,
    force_rebuild: bool,
    deps_mtime: Option<SystemTime>,
    log_file: bool,
) -> Result<()> {
    let root = config.get_root();
    let content = &config.build.content;
    let output = &config.build.output.join(&config.build.path_prefix);

    let is_relative_asset = content_path.extension().is_some_and(|ext| ext != "typ");

    if is_relative_asset {
        let relative_asset_path = content_path
            .strip_prefix(content)?
            .to_str()
            .ok_or(anyhow!("Invalid path"))?;

        let output = output.join(relative_asset_path);

        // Relative assets don't depend on templates/config, just check source vs dest
        if !force_rebuild && is_up_to_date(content_path, &output, None) {
            return Ok(());
        }

        if log_file {
            log!("content"; "{}", relative_asset_path);
        }

        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::copy(content_path, output)?;
        return Ok(());
    }

    // Process .typ file: get output paths, compile, and post-process
    let page = PageMeta::from_source(content_path.to_path_buf(), config)?;

    // Check source and dependencies (templates, utils, config)
    if !force_rebuild && is_up_to_date(content_path, &page.html, deps_mtime) {
        return Ok(());
    }

    if log_file {
        log!("content"; "{}", page.relative);
    }

    if let Some(parent) = page.html.parent() {
        fs::create_dir_all(parent)?;
    }

    let output = exec!(&config.build.typst.command;
        "compile", "--features", "html", "--format", "html",
        "--font-path", root, "--root", root,
        content_path, "-"
    )?;

    let html_content = output.stdout;
    let html_content = process_html(&page.html, &html_content, config)?;

    let html_content = if config.build.minify {
        minify_html::minify(html_content.as_slice(), &minify_html::Cfg::new())
    } else {
        html_content
    };

    fs::write(&page.html, html_content)?;
    Ok(())
}

// ============================================================================
// Asset Processing
// ============================================================================

pub fn process_asset(
    asset_path: &Path,
    config: &'static SiteConfig,
    should_wait_until_stable: bool,
    log_file: bool,
) -> Result<()> {
    let assets = &config.build.assets;
    let output = &config.build.output.join(&config.build.path_prefix);

    let asset_extension = asset_path
        .extension()
        .unwrap_or_default()
        .to_str()
        .unwrap_or_default();
    let relative_asset_path = asset_path
        .strip_prefix(assets)?
        .to_str()
        .ok_or(anyhow!("Invalid path"))?;

    if log_file {
        log!("assets"; "{}", relative_asset_path);
    }

    let output_path = output.join(relative_asset_path);

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    if should_wait_until_stable {
        wait_until_stable(asset_path, 5)?;
    }

    match asset_extension {
        "css" if config.build.tailwind.enable => {
            let input = config.build.tailwind.input.as_ref().unwrap();
            // Config paths are already absolute, just canonicalize the runtime path
            let asset_path = asset_path.canonicalize().unwrap();
            if *input == asset_path {
                exec!(config.get_root(); &config.build.tailwind.command;
                    "-i", input, "-o", &output_path, if config.build.minify { "--minify" } else { "" }
                )?;
            } else {
                fs::copy(asset_path, &output_path)?;
            }
        }
        _ => {
            fs::copy(asset_path, &output_path)?;
        }
    }

    Ok(())
}

// ============================================================================
// HTML Processing
// ============================================================================

fn process_html(html_path: &Path, content: &[u8], config: &'static SiteConfig) -> Result<Vec<u8>> {
    let mut ctx = HtmlContext::new(config, html_path);
    let mut writer = Writer::new(Cursor::new(Vec::with_capacity(content.len())));
    let mut reader = create_xml_reader(content);
    let mut svgs = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(elem)) => {
                handle_start_element(&elem, &mut reader, &mut writer, &mut ctx, &mut svgs)?;
            }
            Ok(Event::End(elem)) => {
                handle_end_element(&elem, &mut writer, config)?;
            }
            Ok(Event::Eof) => break,
            Ok(event) => writer.write_event(event)?,
            Err(e) => anyhow::bail!(
                "XML parse error at position {}: {:?}",
                reader.error_position(),
                e
            ),
        }
    }

    // Compress SVGs in parallel
    if ctx.extract_svg && !svgs.is_empty() {
        compress_svgs_parallel(&svgs, html_path, config)?;
    }

    Ok(writer.into_inner().into_inner())
}

fn handle_start_element(
    elem: &BytesStart<'_>,
    reader: &mut Reader<&[u8]>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    ctx: &mut HtmlContext<'_>,
    svgs: &mut Vec<Svg>,
) -> Result<()> {
    match elem.name().as_ref() {
        b"html" => write_html_with_lang(elem, writer, ctx.config)?,
        b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6" => {
            write_heading_with_slugified_id(elem, writer, ctx.config)?;
        }
        b"svg" if ctx.extract_svg => {
            if let Some(svg) = extract_svg_element(reader, writer, elem, ctx)? {
                svgs.push(svg);
            }
        }
        _ => write_element_with_processed_links(elem, writer, ctx.config)?,
    }
    Ok(())
}

fn handle_end_element(
    elem: &BytesEnd<'_>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    config: &'static SiteConfig,
) -> Result<()> {
    match elem.name().as_ref() {
        b"head" => write_head_content(writer, config)?,
        _ => writer.write_event(Event::End(elem.to_owned()))?,
    }
    Ok(())
}
