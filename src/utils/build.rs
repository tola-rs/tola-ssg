//! Content and asset processing.
//!
//! Handles compilation of Typst files to HTML and asset copying/optimization.

use crate::utils::svg::{HtmlContext, Svg, compress_svgs_parallel, extract_svg_element};
use crate::utils::watch::wait_until_stable;
use crate::utils::xml::{
    create_xml_reader, write_element_with_processed_links, write_head_content,
    write_heading_with_slugified_id, write_html_with_lang,
};
use crate::{
    config::SiteConfig,
    log, run_command,
    utils::slug::slugify_path,
};
use anyhow::{Context, Result, anyhow};
use dashmap::DashSet;
use lru::LruCache;
use quick_xml::{
    Reader, Writer,
    events::{BytesEnd, BytesStart, Event},
};
use rayon::prelude::*;
use std::num::NonZeroUsize;
use std::sync::{Arc, LazyLock, Mutex};
use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

// ============================================================================
// Types and Constants
// ============================================================================

type DirCache = LazyLock<Mutex<LruCache<PathBuf, Arc<Vec<PathBuf>>>>>;
type CreatedDirCache = LazyLock<DashSet<PathBuf>>;

static CREATED_DIRS: CreatedDirCache = LazyLock::new(DashSet::new);

pub static CONTENT_CACHE: DirCache =
    LazyLock::new(|| Mutex::new(LruCache::new(NonZeroUsize::new(50).unwrap())));
pub static ASSETS_CACHE: DirCache =
    LazyLock::new(|| Mutex::new(LruCache::new(NonZeroUsize::new(50).unwrap())));

pub const IGNORED_FILE_NAME: &[&str] = &[".DS_Store"];

// ============================================================================
// Directory Operations
// ============================================================================

pub fn _copy_dir_recursively(src: &Path, dst: &Path) -> Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst).context("[Utils] Failed to create destination directory")?;
    }

    for entry in fs::read_dir(src).context("[Utils] Failed to read source directory")? {
        let entry = entry.context("[Utils] Invalid directory entry")?;
        let entry_path = entry.path();
        let dest_path = dst.join(entry.file_name());

        if entry_path.is_dir() {
            _copy_dir_recursively(&entry_path, &dest_path)?;
        } else {
            fs::copy(&entry_path, &dest_path).with_context(|| {
                format!("[Utils] Failed to copy {entry_path:?} to {dest_path:?}")
            })?;
            log!("assets"; "{}", dest_path.display());
        }
    }

    Ok(())
}

fn collect_files_vec<P>(
    dir_cache: &DirCache,
    dir: &Path,
    should_collect: &P,
) -> Result<Vec<PathBuf>>
where
    P: Fn(&PathBuf) -> bool + Sync,
{
    if let Some(cached) = dir_cache.lock().unwrap().get(dir) {
        return Ok((**cached).clone());
    }

    let paths: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();

    let parts: Vec<Vec<PathBuf>> = paths
        .par_iter()
        .map(|path| -> Result<Vec<_>> {
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default();

            if path.is_dir() {
                collect_files_vec(dir_cache, path, should_collect)
            } else if path.is_file()
                && should_collect(path)
                && !IGNORED_FILE_NAME.contains(&file_name)
            {
                Ok(vec![path.clone()])
            } else {
                Ok(Vec::new())
            }
        })
        .collect::<Result<_>>()?;

    let files: Vec<_> = parts.into_iter().flatten().collect();

    dir_cache
        .lock()
        .unwrap()
        .put(dir.to_path_buf(), Arc::new(files.clone()));

    Ok(files)
}

pub fn collect_files<P>(dir_cache: &DirCache, dir: &Path, p: &P) -> Result<Arc<Vec<PathBuf>>>
where
    P: Fn(&PathBuf) -> bool + Sync,
{
    let files = collect_files_vec(dir_cache, dir, p)?;
    Ok(Arc::new(files))
}

pub fn process_files<P, F>(
    dir_cache: &DirCache,
    dir: &Path,
    config: &'static SiteConfig,
    should_process: &P,
    f: &F,
) -> Result<()>
where
    P: Fn(&PathBuf) -> bool + Sync,
    F: Fn(&Path, &'static SiteConfig) -> Result<()> + Sync,
{
    let files = collect_files(dir_cache, dir, should_process)?;
    files.par_iter().try_for_each(|path| f(path, config))?;
    Ok(())
}

fn ensure_dir_exists(path: &Path) -> Result<()> {
    if CREATED_DIRS.insert(path.to_path_buf()) {
        fs::create_dir_all(path)?;
    }
    Ok(())
}

// ============================================================================
// Content Processing
// ============================================================================

pub fn process_content(
    content_path: &Path,
    config: &'static SiteConfig,
    should_log_newline: bool,
    force_rebuild: bool,
) -> Result<()> {
    let root = config.get_root();
    let content = &config.build.content;
    let output = &config.build.output.join(&config.build.base_path);

    let is_relative_asset = content_path.extension().is_some_and(|ext| ext != "typ");

    if is_relative_asset {
        let relative_asset_path = content_path
            .strip_prefix(content)?
            .to_str()
            .ok_or(anyhow!("Invalid path"))?;

        log!(should_log_newline; "content"; "{}", relative_asset_path);

        let output = output.join(relative_asset_path);
        ensure_dir_exists(output.parent().unwrap())?;

        if !force_rebuild
            && let (Ok(src_meta), Ok(dst_meta)) = (content_path.metadata(), output.metadata())
            && let (Ok(src_time), Ok(dst_time)) = (src_meta.modified(), dst_meta.modified())
            && src_time <= dst_time
        {
            return Ok(());
        }

        fs::copy(content_path, output)?;
        return Ok(());
    }

    // println!("{:?}, {:?}, {:?}, {:?}", root, content, output, content_path);
    let relative_post_path = content_path
        .strip_prefix(content)?
        .to_str()
        .ok_or(anyhow!("Invalid path"))?
        .strip_suffix(".typ")
        .ok_or(anyhow!("Not a .typ file"))
        .with_context(|| format!("compiling post: {:?}", content_path))?;

    log!(should_log_newline; "content"; "{}", relative_post_path);

    let output = output.join(relative_post_path);
    fs::create_dir_all(&output).unwrap();

    let html_path = if content_path.file_name().is_some_and(|p| p == "index.typ") {
        config.build.output.join("index.html")
    } else {
        output.join("index.html")
    };
    let html_path = slugify_path(&html_path, config);
    if !force_rebuild && html_path.exists() {
        let src_time = content_path.metadata()?.modified()?;
        let dst_time = html_path
            .metadata()?
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if src_time <= dst_time {
            return Ok(());
        }
    }

    let output = run_command!(&config.build.typst.command;
        "compile", "--features", "html", "--format", "html",
        "--font-path", root, "--root", root,
        content_path, "-"
    )
    // .with_context(|| format!("post path: {}", content_path.display()))
?;

    let html_content = output.stdout;
    let html_content = process_html(&html_path, &html_content, config)?;

    let html_content = if config.build.minify {
        minify_html::minify(html_content.as_slice(), &minify_html::Cfg::new())
    } else {
        html_content
    };

    fs::write(&html_path, html_content)?;
    Ok(())
}

// ============================================================================
// Asset Processing
// ============================================================================

pub fn process_asset(
    asset_path: &Path,
    config: &'static SiteConfig,
    should_wait_until_stable: bool,
    should_log_newline: bool,
) -> Result<()> {
    let assets = &config.build.assets;
    let output = &config.build.output.join(&config.build.base_path);

    let asset_extension = asset_path
        .extension()
        .unwrap_or_default()
        .to_str()
        .unwrap_or_default();
    let relative_asset_path = asset_path
        .strip_prefix(assets)?
        .to_str()
        .ok_or(anyhow!("Invalid path"))?;

    log!(should_log_newline; "assets"; "{}", relative_asset_path);

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
                run_command!(config.get_root(); &config.build.tailwind.command;
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
