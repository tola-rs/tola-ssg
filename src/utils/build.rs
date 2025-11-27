//! Content and asset processing.
//!
//! Handles compilation of Typst files to HTML and asset copying/optimization.

use crate::utils::watch::wait_until_stable;
use crate::{
    config::{ExtractSvgType, SiteConfig},
    log, run_command, run_command_with_stdin,
    utils::slug::{slugify_fragment, slugify_path},
};
use anyhow::{Context, Result, anyhow};
use dashmap::DashSet;
use lru::LruCache;
use quick_xml::{
    Reader, Writer,
    events::{BytesEnd, BytesStart, BytesText, Event, attributes::Attribute},
};
use rayon::prelude::*;
use std::borrow::Cow;
use std::num::NonZeroUsize;
use std::sync::{Arc, LazyLock, Mutex};
use std::{
    ffi::OsString,
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    str,
    sync::OnceLock,
};

// ============================================================================
// Types and Constants
// ============================================================================

type DirCache = LazyLock<Mutex<LruCache<PathBuf, Arc<Vec<PathBuf>>>>>;
type CreatedDirCache = LazyLock<DashSet<PathBuf>>;

const PADDING_TOP_FOR_SVG: f32 = 5.0;
const PADDING_BOTTOM_FOR_SVG: f32 = 4.0;

static ASSET_TOP_LEVELS: OnceLock<Vec<OsString>> = OnceLock::new();
static CREATED_DIRS: CreatedDirCache = LazyLock::new(DashSet::new);

pub static CONTENT_CACHE: DirCache =
    LazyLock::new(|| Mutex::new(LruCache::new(NonZeroUsize::new(50).unwrap())));
pub static ASSETS_CACHE: DirCache =
    LazyLock::new(|| Mutex::new(LruCache::new(NonZeroUsize::new(50).unwrap())));

pub const IGNORED_FILE_NAME: &[&str] = &[".DS_Store"];

/// Extracted SVG data with dimensions
struct Svg {
    data: Vec<u8>,
    size: (f32, f32),
    index: usize,
}

impl Svg {
    fn new(data: Vec<u8>, size: (f32, f32), index: usize) -> Self {
        Self { data, size, index }
    }

    /// Determine output filename based on extract type and size
    fn output_filename(&self, config: &SiteConfig) -> String {
        let use_svg = matches!(config.build.typst.svg.extract_type, ExtractSvgType::JustSvg)
            || self.data.len() < config.get_inline_max_size();

        if use_svg {
            format!("svg-{}.svg", self.index)
        } else {
            format!("svg-{}.avif", self.index)
        }
    }

    /// Check if this SVG should be kept as SVG (not compressed to AVIF)
    fn should_keep_as_svg(&self, config: &SiteConfig) -> bool {
        matches!(config.build.typst.svg.extract_type, ExtractSvgType::JustSvg)
            || self.data.len() < config.get_inline_max_size()
    }
}

// ============================================================================
// HTML Processing - Element Handlers
// ============================================================================

/// Context for HTML processing, avoiding repeated config access
struct HtmlContext<'a> {
    config: &'static SiteConfig,
    html_path: &'a Path,
    svg_count: usize,
    extract_svg: bool,
}

impl<'a> HtmlContext<'a> {
    fn new(config: &'static SiteConfig, html_path: &'a Path) -> Self {
        Self {
            config,
            html_path,
            svg_count: 0,
            extract_svg: !matches!(config.build.typst.svg.extract_type, ExtractSvgType::Embedded),
        }
    }
}

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

fn collect_files_vec<P>(dir_cache: &DirCache, dir: &Path, should_collect: &P) -> Result<Vec<PathBuf>>
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
            Err(e) => anyhow::bail!("XML parse error at position {}: {:?}", reader.error_position(), e),
        }
    }

    // Compress SVGs in parallel
    if ctx.extract_svg && !svgs.is_empty() {
        compress_svgs_parallel(&svgs, html_path, config)?;
    }

    Ok(writer.into_inner().into_inner())
}

#[inline]
fn create_xml_reader(content: &[u8]) -> Reader<&[u8]> {
    let mut reader = Reader::from_reader(content);
    reader.config_mut().trim_text(false);
    reader.config_mut().enable_all_checks(false);
    reader
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

// ============================================================================
// Element Writers
// ============================================================================

fn write_html_with_lang(
    elem: &BytesStart<'_>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    config: &SiteConfig,
) -> Result<()> {
    let mut elem = elem.to_owned();
    elem.push_attribute(("lang", config.base.language.as_str()));
    writer.write_event(Event::Start(elem))?;
    Ok(())
}

fn write_heading_with_slugified_id(
    elem: &BytesStart<'_>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    config: &'static SiteConfig,
) -> Result<()> {
    let attrs: Vec<Attribute> = elem
        .attributes()
        .flatten()
        .map(|attr| {
            let key = attr.key;
            let value = if key.as_ref() == b"id" {
                let v = str::from_utf8(attr.value.as_ref()).unwrap_or_default();
                slugify_fragment(v, config).into_bytes().into()
            } else {
                attr.value
            };
            Attribute { key, value }
        })
        .collect();
    
    let elem = elem.to_owned().with_attributes(attrs);
    writer.write_event(Event::Start(elem))?;
    Ok(())
}

fn write_element_with_processed_links(
    elem: &BytesStart<'_>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    config: &'static SiteConfig,
) -> Result<()> {
    let attrs: Result<Vec<Attribute>> = elem
        .attributes()
        .flatten()
        .map(|attr| {
            let key = attr.key;
            let value = if key.as_ref() == b"href" || key.as_ref() == b"src" {
                process_link_value(&attr.value, config)?
            } else {
                attr.value
            };
            Ok(Attribute { key, value })
        })
        .collect();

    let elem = elem.to_owned().with_attributes(attrs?);
    writer.write_event(Event::Start(elem))?;
    Ok(())
}

fn process_link_value<'a>(value: &Cow<'a, [u8]>, config: &'static SiteConfig) -> Result<Cow<'a, [u8]>> {
    let value_str = str::from_utf8(value.as_ref())?;
    let processed = match value_str.bytes().next() {
        Some(b'/') => process_absolute_link(value_str, config)?,
        Some(b'#') => process_fragment_link(value_str, config)?,
        Some(_) => process_relative_or_external_link(value_str)?,
        None => anyhow::bail!("empty link URL found in typst file"),
    };
    Ok(processed.into_bytes().into())
}

// ============================================================================
// SVG Extraction and Processing
// ============================================================================

fn extract_svg_element(
    reader: &mut Reader<&[u8]>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    elem: &BytesStart<'_>,
    ctx: &mut HtmlContext<'_>,
) -> Result<Option<Svg>> {
    // Filter and transform SVG attributes
    let attrs: Vec<_> = elem
        .attributes()
        .flatten()
        .filter_map(|attr| match attr.key.as_ref() {
            b"height" => adjust_height_attr(attr).ok(),
            b"viewBox" => adjust_viewbox_attr(attr).ok(),
            _ => Some(attr),
        })
        .collect();

    // Capture SVG content
    let svg_content = capture_svg_content(reader, &attrs)?;
    
    // Parse and optimize SVG
    let (svg_data, size) = optimize_svg(&svg_content, ctx.config)?;
    
    // Write img placeholder to HTML
    let svg_index = ctx.svg_count;
    ctx.svg_count += 1;
    
    let svg = Svg::new(svg_data, size, svg_index);
    write_svg_img_placeholder(writer, &svg, ctx)?;

    Ok(Some(svg))
}

fn capture_svg_content(reader: &mut Reader<&[u8]>, attrs: &[Attribute<'_>]) -> Result<Vec<u8>> {
    let mut svg_writer = Writer::new(Cursor::new(Vec::with_capacity(4096)));
    svg_writer.write_event(Event::Start(BytesStart::new("svg").with_attributes(attrs.iter().cloned())))?;

    let mut depth = 1u32;
    loop {
        let event = reader.read_event()?;
        match &event {
            Event::Start(_) => depth += 1,
            Event::End(e) if e.name().as_ref() == b"svg" => {
                depth -= 1;
                if depth == 0 {
                    svg_writer.write_event(event)?;
                    break;
                }
            }
            Event::End(_) => depth -= 1,
            _ => {}
        }
        svg_writer.write_event(event)?;
    }

    Ok(svg_writer.into_inner().into_inner())
}

fn optimize_svg(svg_content: &[u8], config: &SiteConfig) -> Result<(Vec<u8>, (f32, f32))> {
    let opt = usvg::Options {
        dpi: config.build.typst.svg.dpi,
        ..Default::default()
    };
    let tree = usvg::Tree::from_data(svg_content, &opt)
        .context("Failed to parse SVG")?;
    
    let write_opt = usvg::WriteOptions {
        indent: usvg::Indent::None,
        ..Default::default()
    };
    let optimized = tree.to_string(&write_opt);
    let size = parse_svg_dimensions(&optimized).unwrap_or((0.0, 0.0));
    
    Ok((optimized.into_bytes(), size))
}

fn write_svg_img_placeholder(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    svg: &Svg,
    ctx: &HtmlContext<'_>,
) -> Result<()> {
    let svg_filename = svg.output_filename(ctx.config);
    let svg_path = ctx.html_path.parent().unwrap().join(&svg_filename);
    let src = svg_path
        .strip_prefix(&ctx.config.build.output)
        .map(|p| format!("/{}", p.display()))
        .unwrap_or_else(|_| svg_filename);

    let scale = ctx.config.get_scale();
    let (w, h) = svg.size;
    let style = format!("width:{}px;height:{}px;", w / scale, h / scale);
    
    let mut img = BytesStart::new("img");
    img.push_attribute(("src", src.as_str()));
    img.push_attribute(("style", style.as_str()));
    writer.write_event(Event::Start(img))?;
    
    Ok(())
}

fn adjust_height_attr(attr: Attribute<'_>) -> Result<Attribute<'_>> {
    let height_str = str::from_utf8(attr.value.as_ref())?;
    let height: f32 = height_str.trim_end_matches("pt").parse()?;
    let new_height = height + PADDING_TOP_FOR_SVG;
    
    Ok(Attribute {
        key: attr.key,
        value: format!("{new_height}pt").into_bytes().into(),
    })
}

fn adjust_viewbox_attr(attr: Attribute<'_>) -> Result<Attribute<'_>> {
    let viewbox_str = str::from_utf8(attr.value.as_ref())?;
    let parts: Vec<f32> = viewbox_str
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    
    if parts.len() != 4 {
        anyhow::bail!("Invalid viewBox format");
    }
    
    let new_viewbox = format!(
        "{} {} {} {}",
        parts[0],
        parts[1] - PADDING_TOP_FOR_SVG,
        parts[2],
        parts[3] + PADDING_BOTTOM_FOR_SVG + PADDING_TOP_FOR_SVG
    );
    
    Ok(Attribute {
        key: attr.key,
        value: new_viewbox.into_bytes().into(),
    })
}

/// Parse width and height from SVG string (fast string search, no regex)
fn parse_svg_dimensions(svg_data: &str) -> Option<(f32, f32)> {
    let width = extract_attr_value(svg_data, "width=\"")?.parse().ok()?;
    let height = extract_attr_value(svg_data, "height=\"")?.parse().ok()?;
    Some((width, height))
}

#[inline]
fn extract_attr_value<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let start = s.find(prefix)? + prefix.len();
    let end = s[start..].find('"')? + start;
    Some(&s[start..end])
}

// ============================================================================
// SVG Compression (Parallel)
// ============================================================================

fn compress_svgs_parallel(svgs: &[Svg], html_path: &Path, config: &'static SiteConfig) -> Result<()> {
    let parent = html_path.parent().context("Invalid html path")?;
    let relative_path = html_path
        .strip_prefix(&config.build.output)
        .map(|p| p.to_string_lossy())
        .unwrap_or_default();
    let relative_path = relative_path.trim_end_matches("index.html");
    let scale = config.get_scale();

    svgs.par_iter().try_for_each(|svg| {
        log!("svg"; "in {relative_path}: compress svg-{}", svg.index);
        
        let svg_path = parent.join(svg.output_filename(config));
        compress_single_svg(svg, &svg_path, scale, config)?;
        
        log!("svg"; "in {relative_path}: finish compressing svg-{}", svg.index);
        Ok(())
    })
}

fn compress_single_svg(svg: &Svg, output_path: &Path, scale: f32, config: &SiteConfig) -> Result<()> {
    if svg.should_keep_as_svg(config) {
        return fs::write(output_path, &svg.data).map_err(Into::into);
    }

    match &config.build.typst.svg.extract_type {
        ExtractSvgType::Embedded => Ok(()),
        ExtractSvgType::JustSvg => fs::write(output_path, &svg.data).map_err(Into::into),
        ExtractSvgType::Magick => compress_with_magick(output_path, &svg.data, scale),
        ExtractSvgType::Ffmpeg => compress_with_ffmpeg(output_path, &svg.data),
        ExtractSvgType::Builtin => compress_with_builtin(output_path, &svg.data, svg.size, scale),
    }
}

fn compress_with_magick(output_path: &Path, svg_data: &[u8], scale: f32) -> Result<()> {
    let density = (scale * 96.0).to_string();
    let mut stdin = run_command_with_stdin!(
        ["magick"];
        "-background", "none", "-density", density, "-", output_path
    )?;
    stdin.write_all(svg_data)?;
    Ok(())
}

fn compress_with_ffmpeg(output_path: &Path, svg_data: &[u8]) -> Result<()> {
    let mut stdin = run_command_with_stdin!(
        ["ffmpeg"];
        "-f", "svg_pipe",
        "-frame_size", "1000000000",
        "-i", "pipe:",
        "-filter_complex", "[0:v]split[color][alpha];[alpha]alphaextract[alpha];[color]format=yuv420p[color]",
        "-map", "[color]",
        "-c:v:0", "libsvtav1",
        "-pix_fmt", "yuv420p",
        "-svtav1-params", "preset=4:still-picture=1",
        "-map", "[alpha]",
        "-c:v:1", "libaom-av1",
        "-pix_fmt", "gray",
        "-still-picture", "1",
        "-strict", "experimental",
        "-c:v", "libaom-av1",
        "-y", output_path
    )?;
    stdin.write_all(svg_data)?;
    Ok(())
}

fn compress_with_builtin(
    output_path: &Path,
    svg_data: &[u8],
    size: (f32, f32),
    scale: f32,
) -> Result<()> {
    let (width, height) = ((size.0 * scale) as usize, (size.1 * scale) as usize);

    let pixmap: Vec<_> = svg_data
        .chunks(4)
        .map(|chunk| ravif::RGBA8::new(chunk[0], chunk[1], chunk[2], chunk[3]))
        .collect();

    let img = ravif::Encoder::new()
        .with_quality(90.0)
        .with_speed(4)
        .encode_rgba(ravif::Img::new(&pixmap, width, height))?;

    fs::write(output_path, img.avif_file)?;
    Ok(())
}

// ============================================================================
// Link Processing
// ============================================================================

fn process_absolute_link(value: &str, config: &'static SiteConfig) -> Result<String> {
    let base_path = &config.build.base_path;
    
    if is_asset_link(value, config) {
        return Ok(format!("/{}{}", base_path.display(), value));
    }
    
    let (path, fragment) = value.split_once('#').unwrap_or((value, ""));
    let slugified_path = slugify_path(path, config);
    
    let mut result = format!("/{}", base_path.join(&slugified_path).display());
    if !fragment.is_empty() {
        result.push('#');
        result.push_str(&slugify_fragment(fragment, config));
    }
    Ok(result)
}

fn process_fragment_link(value: &str, config: &'static SiteConfig) -> Result<String> {
    Ok(format!("#{}", slugify_fragment(&value[1..], config)))
}

fn process_relative_or_external_link(value: &str) -> Result<String> {
    Ok(if is_external_link(value) {
        value.to_string()
    } else {
        format!("../{value}")
    })
}

// ============================================================================
// Head Section Processing
// ============================================================================

fn write_head_content(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    config: &'static SiteConfig,
) -> Result<()> {
    let head = &config.build.head;
    let base_path = &config.build.base_path;

    // Title
    if !config.base.title.is_empty() {
        write_text_element(writer, "title", &config.base.title)?;
    }

    // Description meta tag
    if !config.base.description.is_empty() {
        write_meta_tag(writer, "description", &config.base.description)?;
    }

    // Favicon
    if let Some(icon) = &head.icon {
        write_icon_link(writer, icon, base_path)?;
    }

    // Stylesheets
    for style in &head.styles {
        let href = compute_asset_href(style, base_path)?;
        write_stylesheet_link(writer, &href)?;
    }

    // Tailwind stylesheet
    if config.build.tailwind.enable
        && let Some(input) = &config.build.tailwind.input
    {
        let href = compute_stylesheet_href(input, config)?;
        write_stylesheet_link(writer, &href)?;
    }

    // Scripts
    for script in &head.scripts {
        let src = compute_asset_href(script.path(), base_path)?;
        write_script_element(writer, &src, script.is_defer(), script.is_async())?;
    }

    // Raw HTML elements (trusted input)
    for raw in &head.elements {
        writer.get_mut().write_all(raw.as_bytes())?;
    }

    writer.write_event(Event::End(BytesEnd::new("head")))?;
    Ok(())
}

#[inline]
fn write_text_element(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    tag: &str,
    text: &str,
) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new(tag)))?;
    writer.write_event(Event::Text(BytesText::new(text)))?;
    writer.write_event(Event::End(BytesEnd::new(tag)))?;
    Ok(())
}

#[inline]
fn write_meta_tag(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    name: &str,
    content: &str,
) -> Result<()> {
    let mut elem = BytesStart::new("meta");
    elem.push_attribute(("name", name));
    elem.push_attribute(("content", content));
    writer.write_event(Event::Empty(elem))?;
    Ok(())
}

#[inline]
fn write_icon_link(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    icon: &Path,
    base_path: &Path,
) -> Result<()> {
    let href = compute_asset_href(icon, base_path)?;
    let mime_type = get_icon_mime_type(icon);
    
    let mut elem = BytesStart::new("link");
    elem.push_attribute(("rel", "shortcut icon"));
    elem.push_attribute(("href", href.as_str()));
    elem.push_attribute(("type", mime_type));
    writer.write_event(Event::Empty(elem))?;
    Ok(())
}

#[inline]
fn write_stylesheet_link(writer: &mut Writer<Cursor<Vec<u8>>>, href: &str) -> Result<()> {
    let mut elem = BytesStart::new("link");
    elem.push_attribute(("rel", "stylesheet"));
    elem.push_attribute(("href", href));
    writer.write_event(Event::Empty(elem))?;
    Ok(())
}

fn write_script_element(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    src: &str,
    defer: bool,
    async_attr: bool,
) -> Result<()> {
    let mut elem = BytesStart::new("script");
    elem.push_attribute(("src", src));
    if defer {
        elem.push_attribute(("defer", ""));
    }
    if async_attr {
        elem.push_attribute(("async", ""));
    }
    writer.write_event(Event::Start(elem))?;
    // Space ensures proper HTML parsing of script tags
    writer.write_event(Event::Text(BytesText::new(" ")))?;
    writer.write_event(Event::End(BytesEnd::new("script")))?;
    Ok(())
}

// ============================================================================
// Asset Utilities
// ============================================================================

/// Get MIME type for icon based on file extension
fn get_icon_mime_type(path: &Path) -> &'static str {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| match ext.to_lowercase().as_str() {
            "ico" => "image/x-icon",
            "png" => "image/png",
            "svg" => "image/svg+xml",
            "avif" => "image/avif",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "jpg" | "jpeg" => "image/jpeg",
            _ => "image/x-icon",
        })
        .unwrap_or("image/x-icon")
}

/// Compute href for an asset path relative to base_path
fn compute_asset_href(asset_path: &Path, base_path: &Path) -> Result<String> {
    // Strip the leading "./" prefix if present
    let without_dot_prefix = asset_path
        .strip_prefix("./")
        .unwrap_or(asset_path);
    // Strip the "assets/" prefix if present to get relative path within assets
    let relative_path = without_dot_prefix
        .strip_prefix("assets/")
        .unwrap_or(without_dot_prefix);
    let path = PathBuf::from("/").join(base_path).join(relative_path);
    Ok(path.to_string_lossy().into_owned())
}

fn compute_stylesheet_href(input: &Path, config: &'static SiteConfig) -> Result<String> {
    let base_path = &config.build.base_path;
    // Config assets path is already absolute
    let assets = &config.build.assets;
    let input = input.canonicalize()?;
    let relative = input.strip_prefix(assets)?;
    let path = PathBuf::from("/").join(base_path).join(relative);
    Ok(path.to_string_lossy().into_owned())
}

fn get_asset_top_levels(assets_dir: &Path) -> &'static [OsString] {
    ASSET_TOP_LEVELS.get_or_init(|| {
        fs::read_dir(assets_dir)
            .map(|dir| dir.flatten().map(|entry| entry.file_name()).collect())
            .unwrap_or_default()
    })
}

fn is_asset_link(path: &str, config: &'static SiteConfig) -> bool {
    let asset_top_levels = get_asset_top_levels(&config.build.assets);
    
    // Extract first path component after the leading slash
    let first_component = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();
    
    asset_top_levels.iter().any(|name| name == first_component)
}

#[inline]
fn is_external_link(link: &str) -> bool {
    link.find(':').is_some_and(|pos| {
        link[..pos].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_get_icon_mime_type_ico() {
        assert_eq!(get_icon_mime_type(Path::new("favicon.ico")), "image/x-icon");
    }

    #[test]
    fn test_get_icon_mime_type_png() {
        assert_eq!(get_icon_mime_type(Path::new("icon.png")), "image/png");
    }

    #[test]
    fn test_get_icon_mime_type_svg() {
        assert_eq!(get_icon_mime_type(Path::new("logo.svg")), "image/svg+xml");
    }

    #[test]
    fn test_get_icon_mime_type_avif() {
        assert_eq!(get_icon_mime_type(Path::new("image.avif")), "image/avif");
    }

    #[test]
    fn test_get_icon_mime_type_webp() {
        assert_eq!(get_icon_mime_type(Path::new("photo.webp")), "image/webp");
    }

    #[test]
    fn test_get_icon_mime_type_gif() {
        assert_eq!(get_icon_mime_type(Path::new("animation.gif")), "image/gif");
    }

    #[test]
    fn test_get_icon_mime_type_jpeg() {
        assert_eq!(get_icon_mime_type(Path::new("photo.jpg")), "image/jpeg");
        assert_eq!(get_icon_mime_type(Path::new("photo.jpeg")), "image/jpeg");
    }

    #[test]
    fn test_get_icon_mime_type_unknown_defaults_to_ico() {
        assert_eq!(get_icon_mime_type(Path::new("file.xyz")), "image/x-icon");
    }

    #[test]
    fn test_get_icon_mime_type_no_extension_defaults_to_ico() {
        assert_eq!(get_icon_mime_type(Path::new("favicon")), "image/x-icon");
    }

    #[test]
    fn test_get_icon_mime_type_case_insensitive() {
        assert_eq!(get_icon_mime_type(Path::new("icon.PNG")), "image/png");
        assert_eq!(get_icon_mime_type(Path::new("logo.SVG")), "image/svg+xml");
        assert_eq!(get_icon_mime_type(Path::new("photo.JPEG")), "image/jpeg");
    }

    #[test]
    fn test_compute_asset_href_simple_path() {
        let result = compute_asset_href(Path::new("images/icon.png"), Path::new("")).unwrap();
        assert_eq!(result, "/images/icon.png");
    }

    #[test]
    fn test_compute_asset_href_with_dot_prefix() {
        let result = compute_asset_href(Path::new("./images/icon.png"), Path::new("")).unwrap();
        assert_eq!(result, "/images/icon.png");
    }

    #[test]
    fn test_compute_asset_href_with_assets_prefix() {
        let result = compute_asset_href(Path::new("assets/images/icon.png"), Path::new("")).unwrap();
        assert_eq!(result, "/images/icon.png");
    }

    #[test]
    fn test_compute_asset_href_with_dot_and_assets_prefix() {
        let result = compute_asset_href(Path::new("./assets/images/icon.png"), Path::new("")).unwrap();
        assert_eq!(result, "/images/icon.png");
    }

    #[test]
    fn test_compute_asset_href_with_base_path() {
        let result = compute_asset_href(Path::new("images/icon.png"), Path::new("blog")).unwrap();
        assert_eq!(result, "/blog/images/icon.png");
    }

    #[test]
    fn test_compute_asset_href_full_path_with_base() {
        let result = compute_asset_href(
            Path::new("./assets/scripts/main.js"),
            Path::new("mysite")
        ).unwrap();
        assert_eq!(result, "/mysite/scripts/main.js");
    }

    #[test]
    fn test_write_stylesheet_link() {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_stylesheet_link(&mut writer, "/styles/main.css").unwrap();
        let output = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(output.contains("link"));
        assert!(output.contains("rel=\"stylesheet\""));
        assert!(output.contains("href=\"/styles/main.css\""));
    }

    #[test]
    fn test_write_script_element_basic() {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_script_element(&mut writer, "/scripts/main.js", false, false).unwrap();
        let output = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(output.contains("<script"));
        assert!(output.contains("src=\"/scripts/main.js\""));
        assert!(output.contains("</script>"));
        assert!(!output.contains("defer"));
        assert!(!output.contains("async"));
    }

    #[test]
    fn test_write_script_element_with_defer() {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_script_element(&mut writer, "/scripts/main.js", true, false).unwrap();
        let output = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(output.contains("defer"));
        assert!(!output.contains("async"));
    }

    #[test]
    fn test_write_script_element_with_async() {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_script_element(&mut writer, "/scripts/main.js", false, true).unwrap();
        let output = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(!output.contains("defer"));
        assert!(output.contains("async"));
    }

    #[test]
    fn test_write_script_element_with_both_defer_and_async() {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_script_element(&mut writer, "/scripts/main.js", true, true).unwrap();
        let output = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(output.contains("defer"));
        assert!(output.contains("async"));
    }

    #[test]
    fn test_is_external_link_http() {
        assert!(is_external_link("http://example.com"));
        assert!(is_external_link("https://example.com/path"));
    }

    #[test]
    fn test_is_external_link_mailto() {
        assert!(is_external_link("mailto:user@example.com"));
    }

    #[test]
    fn test_is_external_link_relative_path() {
        assert!(!is_external_link("/path/to/page"));
        assert!(!is_external_link("./relative/path"));
        assert!(!is_external_link("../parent/path"));
    }

    #[test]
    fn test_is_external_link_anchor() {
        assert!(!is_external_link("#section"));
    }
}
