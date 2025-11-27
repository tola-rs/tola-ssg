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
    sync::OnceLock,
};

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

struct Svg {
    data: Vec<u8>,
    size: (f32, f32),
}

impl Svg {
    pub fn new(data: Vec<u8>, size: (f32, f32)) -> Self {
        Self { data, size }
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

        if let (Ok(src_meta), Ok(dst_meta)) = (content_path.metadata(), output.metadata())
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
    if html_path.exists() {
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
            let input = input.canonicalize().unwrap();
            let asset_path = asset_path.canonicalize().unwrap();
            match input == asset_path {
                true => {
                    let output_path = output.canonicalize().unwrap().join(relative_asset_path);
                    run_command!(config.get_root(); &config.build.tailwind.command;
                        "-i", input, "-o", output_path, if config.build.minify { "--minify" } else { "" }
                    )?;
                }
                false => {
                    fs::copy(asset_path, &output_path)?;
                }
            }
        }
        _ => {
            fs::copy(asset_path, &output_path)?;
        }
    }

    Ok(())
}

fn process_html(html_path: &Path, content: &[u8], config: &'static SiteConfig) -> Result<Vec<u8>> {
    let mut svg_cnt = 0;
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    let mut reader = {
        let mut reader = Reader::from_reader(content);
        reader.config_mut().trim_text(false);
        reader.config_mut().enable_all_checks(false);
        reader
    };

    let mut svgs = vec![];

    loop {
        match reader.read_event() {
            Ok(Event::Start(elem)) => match elem.name().as_ref() {
                b"html" => {
                    let mut elem = elem.into_owned();
                    elem.push_attribute(("lang", config.base.language.as_str()));
                    writer.write_event(Event::Start(elem))?;
                }
                b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6" => {
                    let attrs: Vec<Attribute> = elem
                        .attributes()
                        .flatten()
                        .map(|attr| {
                            let key = attr.key;
                            let value = if key.as_ref() == b"id" {
                                let value = str::from_utf8(attr.value.as_ref()).unwrap();
                                slugify_fragment(value, config).into_bytes().into()
                            } else {
                                attr.value
                            };
                            Attribute { key, value }
                        })
                        .collect();
                    let elem = elem.to_owned().with_attributes(attrs);
                    writer.write_event(Event::Start(elem))?;
                }
                b"svg" => match config.build.typst.svg.extract_type {
                    ExtractSvgType::Embedded => writer.write_event(Event::Start(elem))?,
                    _ => {
                        let svg = process_svg_in_html(
                            html_path, &mut svg_cnt, &mut reader, &mut writer, elem, config,
                        )?;
                        svgs.push(svg);
                    }
                },
                _ => process_link_in_html(&mut writer, elem, config)?,
            },
            Ok(Event::End(elem)) => match elem.name().as_ref() {
                b"head" => process_head_in_html(&mut writer, config)?,
                _ => writer.write_event(Event::End(elem))?,
            },
            Ok(Event::Eof) => break,
            Ok(elem) => writer.write_event(elem)?,
            Err(e) => anyhow::bail!("XML parse error at position {}: {:?}", reader.error_position(), e),
        }
    }

    if !matches!(config.build.typst.svg.extract_type, ExtractSvgType::Embedded) {
        let svgs: Vec<_> = svgs.into_iter().flatten().collect();
        compress_svgs(svgs, html_path, config)?;
    }

    Ok(writer.into_inner().into_inner())
}

fn process_svg_in_html(
    html_path: &Path,
    cnt: &mut i32,
    reader: &mut Reader<&[u8]>,
    writer: &mut Writer<Cursor<Vec<u8>>>,
    elem: BytesStart<'_>,
    config: &'static SiteConfig,
) -> Result<Option<Svg>> {
    if let ExtractSvgType::Embedded = config.build.typst.svg.extract_type {
        writer.write_event(Event::Start(elem))?;
        return Ok(None);
    }

    let attrs: Vec<_> = elem
        .attributes()
        .flatten()
        .filter_map(|attr| match attr.key.as_ref() {
            b"height" => process_height_attr(attr).ok(),
            b"viewBox" => process_viewbox_attr(attr).ok(),
            _ => Some(attr),
        })
        .collect();

    let mut svg_writer = Writer::new(Cursor::new(Vec::new()));
    svg_writer.write_event(Event::Start(BytesStart::new("svg").with_attributes(attrs)))?;

    loop {
        let event = reader.read_event()?;
        let is_end = matches!(&event, Event::End(e) if e.name().as_ref() == b"svg");
        svg_writer.write_event(event)?;
        if is_end {
            break;
        }
    }

    let svg_data = svg_writer.into_inner().into_inner();
    let inline_max_size = config.get_inline_max_size();
    // println!("{} {cnt} {} {}", html_path.display(), svg_data.len(), inline_max_size);
    let svg_filename = match (&config.build.typst.svg.extract_type, svg_data.len()) {
        (ExtractSvgType::JustSvg, _) => format!("svg-{cnt}.svg"),
        (_, size) if size < inline_max_size => format!("svg-{cnt}.svg"),
        _ => format!("svg-{cnt}.avif"),
    };
    let svg_path = html_path.parent().unwrap().join(svg_filename.as_str());
    *cnt += 1;

    let dpi = config.build.typst.svg.dpi;
    let opt = usvg::Options {
        dpi,
        ..Default::default()
    };
    let usvg_tree = usvg::Tree::from_data(&svg_data, &opt).unwrap();
    let write_opt = usvg::WriteOptions {
        indent: usvg::Indent::None,
        ..Default::default()
    };
    let usvg = usvg_tree.to_string(&write_opt);

    let (width, height) = extract_svg_size(&usvg).unwrap();
    let img_elem = {
        let svg_path = svg_path.strip_prefix(&config.build.output).unwrap();
        let svg_path = PathBuf::from("/").join(svg_path);
        let svg_path = svg_path.to_str().unwrap();
        let scale = config.get_scale();
        let attrs = [
            ("src", svg_path),
            ("style", &format!("width:{}px;height:{}px;", width / scale, height / scale)),
        ];
        BytesStart::new("img").with_attributes(attrs)
    };
    writer.write_event(Event::Start(img_elem)).unwrap();

    Ok(Some(Svg::new(usvg.into_bytes(), (width, height))))
}

fn process_height_attr(attr: Attribute<'_>) -> Result<Attribute<'_>> {
    let height = str::from_utf8(attr.value.as_ref())?.trim_end_matches("pt");
    let height = height.parse::<f32>()? + PADDING_TOP_FOR_SVG;
    Ok(Attribute {
        key: attr.key,
        value: format!("{height}pt").into_bytes().into(),
    })
}

fn process_viewbox_attr(attr: Attribute<'_>) -> Result<Attribute<'_>> {
    let viewbox_inner: Vec<_> = str::from_utf8(attr.value.as_ref())
        .unwrap()
        .split_whitespace()
        .map(|x| x.parse::<f32>().unwrap())
        .collect();
    let viewbox = format!(
        "{} {} {} {}",
        viewbox_inner[0],
        viewbox_inner[1] - PADDING_TOP_FOR_SVG,
        viewbox_inner[2],
        viewbox_inner[3] + PADDING_BOTTOM_FOR_SVG + PADDING_TOP_FOR_SVG
    );
    Ok(Attribute {
        key: attr.key,
        value: viewbox.as_bytes().to_vec().into(),
    })
}

fn extract_svg_size(svg_data: &str) -> Option<(f32, f32)> {
    let width_start = svg_data.find("width=\"")? + "width=\"".len();
    let width_end = svg_data[width_start..].find('"')? + width_start;
    let width_str = &svg_data[width_start..width_end];

    let height_start = svg_data[width_end..].find("height=\"")? + width_end + "height=\"".len();
    let height_end = svg_data[height_start..].find('"')? + height_start;
    let height_str = &svg_data[height_start..height_end];

    let width = width_str.parse::<f32>().unwrap();
    let height = height_str.parse::<f32>().unwrap();

    Some((width, height))
}

fn compress_svgs(svgs: Vec<Svg>, html_path: &Path, config: &'static SiteConfig) -> Result<()> {
    let scale = config.get_scale();
    let parent = html_path.parent().context("Invalid html path")?;
    let inline_max_size = config.get_inline_max_size();
    let relative_path = html_path
        .strip_prefix(&config.build.output)
        .map(|p| p.to_string_lossy())
        .unwrap_or_default();
    let relative_path = relative_path.trim_end_matches("index.html");

    for (cnt, svg) in svgs.iter().enumerate() {
        log!("svg"; "in {relative_path}: compress svg-{cnt}");

        let svg_data = svg.data.as_slice();
        let use_svg = matches!(config.build.typst.svg.extract_type, ExtractSvgType::JustSvg)
            || svg_data.len() < inline_max_size;

        let svg_filename = if use_svg {
            format!("svg-{cnt}.svg")
        } else {
            format!("svg-{cnt}.avif")
        };
        let svg_path = parent.join(&svg_filename);

        match &config.build.typst.svg.extract_type {
            ExtractSvgType::Embedded => continue,
            _ if use_svg => fs::write(&svg_path, svg_data)?,
            ExtractSvgType::Magick => compress_svg_with_magick(&svg_path, svg_data, scale)?,
            ExtractSvgType::Ffmpeg => compress_svg_with_ffmpeg(&svg_path, svg_data, scale)?,
            ExtractSvgType::Builtin => {
                compress_svg_with_builtin(&svg_path, svg_data, svg.size, scale)?
            }
            ExtractSvgType::JustSvg => unreachable!(),
        }

        log!("svg"; "in {relative_path}: finish compressing svg-{cnt}");
    }

    Ok(())
}

fn compress_svg_with_magick(svg_path: &Path, svg_data: &[u8], scale: f32) -> Result<()> {
    let density = (scale * 96.).to_string();
    let mut child_stdin = run_command_with_stdin!(
        ["magick"];
        "-background", "none", "-density", density, "-", &svg_path
    )?;
    child_stdin.write_all(svg_data)?;
    Ok(())
}

fn compress_svg_with_ffmpeg(svg_path: &Path, svg_data: &[u8], _scale: f32) -> Result<()> {
    let mut child_stdin = run_command_with_stdin!(
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
        "-y", &svg_path
    )?;
    child_stdin.write_all(svg_data)?;
    Ok(())
}

fn compress_svg_with_builtin(
    svg_path: &Path,
    svg_data: &[u8],
    size: (f32, f32),
    scale: f32,
) -> Result<()> {
    let (width, height) = (size.0 * scale, size.1 * scale);

    let pixmap: Vec<_> = svg_data
        .chunks(4)
        .map(|chunk| ravif::RGBA8::new(chunk[0], chunk[1], chunk[2], chunk[3]))
        .collect();

    let img = ravif::Encoder::new()
        .with_quality(90.)
        .with_speed(4)
        .encode_rgba(ravif::Img::new(&pixmap, width as usize, height as usize))?;

    fs::write(svg_path, img.avif_file)?;
    Ok(())
}

fn process_link_in_html(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    elem: BytesStart<'_>,
    config: &'static SiteConfig,
) -> Result<()> {
    let attrs: Result<Vec<Attribute>> = elem
        .attributes()
        .flatten()
        .map(|attr| {
            let key = attr.key;
            let value = attr.value;
            let attr = if key.as_ref() == b"href" || key.as_ref() == b"src" {
                let value = process_link_attribute(value, config)?;
                Attribute { key, value }
            } else {
                Attribute { key, value }
            };
            Ok(attr)
        })
        .collect();

    let elem = elem.to_owned().with_attributes(attrs?);
    writer.write_event(Event::Start(elem)).unwrap();
    Ok(())
}

fn process_link_attribute<'a>(
    value: Cow<'a, [u8]>,
    config: &'static SiteConfig,
) -> Result<Cow<'a, [u8]>> {
    let value_str = str::from_utf8(value.as_ref())?;
    let processed_value = match value_str.chars().next() {
        Some('/') => process_absolute_link(value_str, config)?,
        Some('#') => process_fragment_link(value_str, config)?,
        Some(_) => process_relative_or_external_link(value_str, config)?,
        None => anyhow::bail!("empty link URL found in typst file"),
    };
    Ok(processed_value.into_bytes().into())
}

fn process_absolute_link(value: &str, config: &'static SiteConfig) -> Result<String> {
    let base_path = PathBuf::from("/").join(config.build.base_path.as_path());
    let path = if is_asset_link(value, config) {
        base_path.join(value).to_string_lossy().into_owned()
    } else {
        let (path, fragment) = value.split_once('#').unwrap_or((value, ""));
        let slugified_path = slugify_path(path, config);
        let slugified_fragment = if !fragment.is_empty() {
            format!("#{}", slugify_fragment(fragment, config))
        } else {
            String::new()
        };
        format!(
            "{}{}",
            base_path.join(slugified_path).to_string_lossy(),
            slugified_fragment
        )
    };
    Ok(path)
}

fn process_fragment_link(value: &str, config: &'static SiteConfig) -> Result<String> {
    let fragment = &value[1..];
    Ok(format!("#{}", slugify_fragment(fragment, config)))
}

fn process_relative_or_external_link(value: &str, _config: &'static SiteConfig) -> Result<String> {
    let link = if is_external_link(value) {
        value.to_string()
    } else {
        format!("../{value}")
    };
    Ok(link)
}

fn process_head_in_html(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    config: &'static SiteConfig,
) -> Result<()> {
    let title = config.base.title.as_str();
    let description = config.base.description.as_str();

    if !title.is_empty() {
        writer.write_event(Event::Start(BytesStart::new("title")))?;
        writer.write_event(Event::Text(BytesText::new(title)))?;
        writer.write_event(Event::End(BytesEnd::new("title")))?;
    }

    if !description.is_empty() {
        let mut elem = BytesStart::new("meta");
        elem.push_attribute(("name", "description"));
        elem.push_attribute(("content", description));
        writer.write_event(Event::Start(elem))?;
        writer.write_event(Event::End(BytesEnd::new("meta")))?;
    }

    if config.build.tailwind.enable
        && let Some(input) = &config.build.tailwind.input
    {
        let href = compute_stylesheet_href(input, config)?;
        let mut elem = BytesStart::new("link");
        elem.push_attribute(("rel", "stylesheet"));
        elem.push_attribute(("href", href.as_str()));
        writer.write_event(Event::Start(elem))?;
    }

    writer.write_event(Event::End(BytesEnd::new("head")))?;
    Ok(())
}

fn compute_stylesheet_href(input: &Path, config: &'static SiteConfig) -> Result<String> {
    let base_path = &config.build.base_path;
    let assets = config.build.assets.canonicalize()?;
    let input = input.canonicalize()?;
    let relative = input.strip_prefix(&assets)?;
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

fn is_asset_link(path: impl AsRef<Path>, config: &'static SiteConfig) -> bool {
    let path = path.as_ref();
    let asset_top_levels = get_asset_top_levels(&config.build.assets);

    // println!("{:?}, {:?}", path, asset_top_levels);
    match path.components().nth(1) {
        Some(std::path::Component::Normal(first)) => {
            asset_top_levels.iter().any(|name| name == first)
        }
        _ => false,
    }
}

fn is_external_link(link: &str) -> bool {
    match link.find(':') {
        Some(colon_pos) => {
            let scheme = &link[..colon_pos];
            // scheme must be ASCII letters + digits + `+` / `-` / `.`
            // and must not contain `/` before the colon
            scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
        }
        None => false,
    }
}
