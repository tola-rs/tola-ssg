//! Media processor (Indexed -> Indexed).
//!
//! Processes media elements (img, video, audio, etc.):
//! - URL processing for `src` attribute
//! - Auto-inject `.tola-recolor` class based on inheritance and config
//! - Remove background from images with `.tola-nobg` class

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use dashmap::DashSet;
use tola_vdom::prelude::*;

use crate::address::resolve_physical_path;
use crate::compiler::family::Indexed;
use crate::compiler::page::PageRoute;
use crate::config::SiteConfig;
use crate::config::section::theme::RecolorTarget;
use crate::core::LinkKind;
use crate::image::background;

// =============================================================================
// nobg reference tracking (minify mode only)
// =============================================================================

/// Original output paths of images referenced with nobg class
static NOBG_REFS: LazyLock<DashSet<PathBuf>> = LazyLock::new(DashSet::new);

/// Output paths of images referenced without nobg class
static NORMAL_REFS: LazyLock<DashSet<PathBuf>> = LazyLock::new(DashSet::new);

/// Clean up original images that are only referenced with nobg
///
/// Called after build completes. Removes original images that have no normal
/// references (only nobg references), keeping only the .nobg.png version
pub fn cleanup_nobg_originals() {
    for path in NOBG_REFS.iter() {
        if !NORMAL_REFS.contains(&*path) && path.exists() {
            let _ = std::fs::remove_file(&*path);
        }
    }
    NOBG_REFS.clear();
    NORMAL_REFS.clear();
}

const CLASS_RECOLOR: &str = "tola-recolor";
const CLASS_NO_RECOLOR: &str = "tola-no-recolor";
const CLASS_NOBG: &str = "tola-nobg";
const RECOLOR_TARGETS: &[&str] = &["img"];
const NOBG_FORMATS: &[&str] = &["png", "jpg", "jpeg", "webp"];

/// Processes media element src attributes in Indexed VDOM
pub struct MediaTransform<'a> {
    config: &'a SiteConfig,
    route: &'a PageRoute,
    /// Track references for cleanup (only in minify mode).
    track_refs: bool,
}

impl<'a> MediaTransform<'a> {
    pub fn new(config: &'a SiteConfig, route: &'a PageRoute) -> Self {
        Self {
            config,
            route,
            track_refs: config.build.minify,
        }
    }

    /// Process nobg for an img element (called when nobg is inherited or explicit).
    fn process_nobg_inherited(&self, elem: &mut Element<Indexed>) {
        let Some(src) = elem.get_attr("src").map(str::to_string) else {
            return;
        };

        // Skip external URLs
        if src.starts_with("http://") || src.starts_with("https://") || src.starts_with("//") {
            return;
        }

        // Resolve source file path
        let Some(source_path) = self.resolve_source_path(&src) else {
            return;
        };

        // Skip non-bitmap formats
        let ext = source_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if !NOBG_FORMATS.contains(&ext.to_lowercase().as_str()) {
            return;
        }

        // Generate output path and new src based on link type
        let (output_path, new_src, original_output) = self.generate_nobg_paths(&src, &source_path);

        // Track nobg reference for cleanup
        if self.track_refs {
            NOBG_REFS.insert(original_output);
        }

        // Process image (with caching)
        if let Err(e) = self.process_nobg_image(&source_path, &output_path) {
            eprintln!("nobg processing error: {}", e);
            return;
        }

        // Update src attribute to point to processed image
        set_media_src(elem, new_src);
    }

    /// Generate output path and new src for nobg image.
    ///
    /// - Site-root paths: output to same directory as original, src stays site-root
    /// - Relative paths: output to page's output_dir, src stays relative
    ///
    /// Returns (nobg_output_path, new_src, original_output_path).
    fn generate_nobg_paths(&self, src: &str, source: &Path) -> (PathBuf, String, PathBuf) {
        let stem = source
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("image");

        match LinkKind::parse(src) {
            LinkKind::SiteRoot(path) => {
                if let Some(route) = self.asset_route_for_src(src) {
                    let route_stem = route
                        .output
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(stem);
                    let output_path = route
                        .output
                        .with_file_name(format!("{route_stem}.nobg.png"));
                    let new_src = nobg_site_root_src(route.url.as_str(), route_stem);
                    return (output_path, new_src, route.output);
                }

                let trimmed = path.trim_start_matches('/');
                let output_path = self
                    .config
                    .paths()
                    .output_dir()
                    .join(Path::new(trimmed).parent().unwrap_or(Path::new("")))
                    .join(format!("{stem}.nobg.png"));
                let new_src = nobg_site_root_src(path, stem);
                let original_output = self.config.paths().output_dir().join(trimmed);
                (output_path, new_src, original_output)
            }
            _ => {
                // ./xxx.png -> {output_dir}/xxx.nobg.png, ./xxx.nobg.png
                let output_path = self.route.output_dir.join(format!("{}.nobg.png", stem));
                let new_src = format!("./{}.nobg.png", stem);
                // Use src filename for consistency with compute_output_path
                let filename = Path::new(src).file_name().unwrap_or_default();
                let original_output = self.route.output_dir.join(filename);
                (output_path, new_src, original_output)
            }
        }
    }

    /// Resolve source file path from src attribute.
    ///
    /// Supports:
    /// - File-relative paths: `./image.png` -> source file's parent directory
    /// - Site-root paths: `/images/xxx` -> configured asset route source
    fn resolve_source_path(&self, src: &str) -> Option<PathBuf> {
        match LinkKind::parse(src) {
            LinkKind::SiteRoot(path) => crate::asset::source_for_asset_url(path, self.config)
                .filter(|source| source.exists()),
            LinkKind::FileRelative(_) | LinkKind::Fragment(_) => {
                // Try relative to source file's directory
                if let Some(source_dir) = self.route.source.parent() {
                    let path = resolve_physical_path(source_dir, src);
                    if path.exists() {
                        return Some(path);
                    }
                }

                None
            }
            LinkKind::External(_) => None,
        }
    }

    /// Process image to remove background (with freshness check).
    fn process_nobg_image(&self, source: &Path, output: &Path) -> anyhow::Result<()> {
        // Skip if output is newer than source
        if output.exists()
            && let (Ok(src_meta), Ok(out_meta)) = (source.metadata(), output.metadata())
            && let (Ok(src_time), Ok(out_time)) = (src_meta.modified(), out_meta.modified())
            && out_time >= src_time
        {
            return Ok(());
        }

        // Ensure output directory exists
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }

        background::remove_background(source, output)
    }

    /// Compute output path for an image src.
    ///
    /// Uses same logic as `generate_nobg_paths` for consistency.
    fn compute_output_path(&self, src: &str) -> Option<PathBuf> {
        // Skip protocol-relative URLs (//example.com/...)
        if src.starts_with("//") {
            return None;
        }

        match LinkKind::parse(src) {
            LinkKind::SiteRoot(path) => self
                .asset_route_for_src(path)
                .map(|route| route.output)
                .or_else(|| {
                    let trimmed = path.trim_start_matches('/');
                    Some(self.config.paths().output_dir().join(trimmed))
                }),
            LinkKind::FileRelative(path) => {
                let filename = Path::new(path).file_name()?;
                Some(self.route.output_dir.join(filename))
            }
            _ => None,
        }
    }

    fn asset_route_for_src(&self, src: &str) -> Option<crate::asset::AssetRoute> {
        let source = crate::asset::source_for_asset_url(src, self.config)?;
        crate::asset::route_from_source(source, self.config).ok()
    }
}

fn nobg_site_root_src(path: &str, stem: &str) -> String {
    let path = path.trim_start_matches('/');
    let parent = Path::new(path).parent().unwrap_or(Path::new(""));
    let parent = parent.to_string_lossy().replace('\\', "/");
    if parent.is_empty() {
        format!("/{stem}.nobg.png")
    } else {
        format!("/{parent}/{stem}.nobg.png")
    }
}

impl Transform<Indexed> for MediaTransform<'_> {
    type To = Indexed;

    fn transform(self, mut doc: Document<Indexed>) -> Document<Indexed> {
        // Process recolor and nobg class inheritance + nobg image processing
        let auto_inject = self.config.theme.recolor.enable
            && self.config.theme.recolor.target == RecolorTarget::Auto;
        process_classes(&mut doc.root, &self, ClassState::default(), auto_inject);

        process_src_attrs(&mut doc.root, self.config, self.route);

        doc
    }
}

fn process_src_attrs(elem: &mut Element<Indexed>, config: &SiteConfig, route: &PageRoute) {
    if let Some(src) = elem.get_attr("src").map(|s| s.to_string()) {
        let processed = process_media_src(&src, config, route);
        set_media_src(elem, processed);
    }

    for child in &mut elem.children {
        if let Node::Element(child) = child {
            process_src_attrs(child, config, route);
        }
    }
}

fn process_media_src(value: &str, config: &SiteConfig, route: &PageRoute) -> String {
    match LinkKind::parse(value) {
        LinkKind::External(_) | LinkKind::Fragment(_) => value.to_string(),
        LinkKind::SiteRoot(path) => crate::asset::resolve_asset_href(path, config)
            .unwrap_or_else(|| site_root_file_href(path, config)),
        LinkKind::FileRelative(_) => resolve_media_relative(value, route),
    }
}

fn resolve_media_relative(value: &str, route: &PageRoute) -> String {
    if route.is_index {
        value.to_string()
    } else {
        format!("../{value}")
    }
}

fn site_root_file_href(value: &str, config: &SiteConfig) -> String {
    let idx = value.find(['?', '#']).unwrap_or(value.len());
    let path = value[..idx].trim_start_matches('/');
    let suffix = &value[idx..];
    format!("{}{}", config.paths().url_for_site_path(path), suffix)
}

fn set_media_src(elem: &mut Element<Indexed>, src: String) {
    elem.set_attr("src", src.clone());
    if let Some(data) = ExtractFamily::<MediaFamily>::get_mut(&mut elem.ext) {
        data.set_src(Some(src));
    }
}

/// Inherited class state for recursive processing
#[derive(Default, Clone, Copy)]
struct ClassState {
    /// Inherited recolor state: None = no inheritance, Some(true) = recolor, Some(false) = no-recolor
    recolor: Option<bool>,
    /// Inherited nobg state
    nobg: bool,
}

/// Recursively process recolor/nobg classes with inheritance
fn process_classes(
    elem: &mut Element<Indexed>,
    transform: &MediaTransform<'_>,
    inherited: ClassState,
    auto_inject: bool,
) {
    // Check current element's explicit classes
    let has_recolor = elem.has_class(CLASS_RECOLOR);
    let has_no_recolor = elem.has_class(CLASS_NO_RECOLOR);
    let has_nobg = elem.has_class(CLASS_NOBG);

    // Update inherited state
    let current = ClassState {
        recolor: if has_recolor {
            Some(true)
        } else if has_no_recolor {
            Some(false)
        } else {
            inherited.recolor
        },
        nobg: has_nobg || inherited.nobg,
    };

    // Process img/svg elements
    if RECOLOR_TARGETS.contains(&elem.tag.as_str()) {
        apply_recolor_class(
            elem,
            has_recolor,
            has_no_recolor,
            has_nobg,
            current.nobg,
            current.recolor,
            auto_inject,
        );
        apply_nobg_processing(elem, has_nobg, inherited.nobg, transform);
    }

    // Recurse into children
    for child in &mut elem.children {
        if let Node::Element(child_elem) = child {
            process_classes(child_elem, transform, current, auto_inject);
        }
    }
}

/// Apply recolor class based on inheritance rules
fn apply_recolor_class(
    elem: &mut Element<Indexed>,
    has_recolor: bool,
    has_no_recolor: bool,
    has_nobg: bool,
    inherited_nobg: bool,
    inherited_recolor: Option<bool>,
    auto_inject: bool,
) {
    // Skip if element already has explicit class or nobg
    if has_recolor || has_no_recolor || has_nobg || inherited_nobg {
        return;
    }

    match inherited_recolor {
        Some(true) => elem.add_class(CLASS_RECOLOR),
        Some(false) => elem.add_class(CLASS_NO_RECOLOR),
        None if auto_inject => elem.add_class(CLASS_RECOLOR),
        None => {}
    }
}

/// Apply nobg processing (server-side background removal)
fn apply_nobg_processing(
    elem: &mut Element<Indexed>,
    has_nobg: bool,
    inherited_nobg: bool,
    transform: &MediaTransform<'_>,
) {
    // Only process img elements
    if !elem.is_tag("img") {
        return;
    }

    if has_nobg || inherited_nobg {
        transform.process_nobg_inherited(elem);
    } else if transform.track_refs {
        // Track normal reference for cleanup decision
        if let Some(src) = elem.get_attr("src")
            && let Some(output_path) = transform.compute_output_path(src)
        {
            NORMAL_REFS.insert(output_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::family::TolaSite;
    use crate::config::section::build::assets::FlattenEntry;
    use tola_vdom::core::ExtractFamily;
    use tola_vdom::families::MediaFamily;

    #[test]
    fn transform_keeps_media_payload_src_in_sync_with_attr() {
        let config = SiteConfig::default();
        let route = PageRoute {
            source: PathBuf::from("content/post.typ"),
            is_index: false,
            is_404: false,
            permalink: crate::core::UrlPath::from_page("/post/"),
            output_file: PathBuf::from("public/post/index.html"),
            output_dir: PathBuf::from("public/post"),
            full_url: "https://example.com/post/".to_string(),
        };
        let root = TolaSite::element("main", Attrs::new()).child(TolaSite::element(
            "img",
            Attrs::from([("src", "./photo.png")]),
        ));
        let indexed = TolaSite::indexer().transform(Document::new(root));

        let transformed = MediaTransform::new(&config, &route).transform(indexed);

        let image = transformed.find(|elem| elem.is_tag("img")).unwrap();
        let media = ExtractFamily::<MediaFamily>::get(&image.ext).unwrap();
        assert_eq!(image.get_attr("src"), Some(".././photo.png"));
        assert_eq!(media.src.as_deref(), Some(".././photo.png"));
    }

    #[test]
    fn site_root_media_src_is_not_pageified_when_asset_is_missing() {
        let mut config = SiteConfig::default();
        config.build.path_prefix = PathBuf::from("docs/blog");
        let route = PageRoute {
            source: PathBuf::from("content/index.typ"),
            is_index: true,
            is_404: false,
            permalink: crate::core::UrlPath::from_page("/"),
            output_file: PathBuf::from("public/docs/blog/index.html"),
            output_dir: PathBuf::from("public/docs/blog"),
            full_url: "https://example.com/docs/blog/".to_string(),
        };
        let root = TolaSite::element("main", Attrs::new()).child(TolaSite::element(
            "img",
            Attrs::from([("src", "/posts/hello/missing.png?v=1")]),
        ));
        let indexed = TolaSite::indexer().transform(Document::new(root));

        let transformed = MediaTransform::new(&config, &route).transform(indexed);

        let image = transformed.find(|elem| elem.is_tag("img")).unwrap();
        let media = ExtractFamily::<MediaFamily>::get(&image.ext).unwrap();
        assert_eq!(
            image.get_attr("src"),
            Some("/docs/blog/posts/hello/missing.png?v=1")
        );
        assert_eq!(
            media.src.as_deref(),
            Some("/docs/blog/posts/hello/missing.png?v=1")
        );
    }

    #[test]
    fn src_attrs_without_media_family_are_processed_as_assets() {
        let mut config = SiteConfig::default();
        config.build.path_prefix = PathBuf::from("docs/blog");
        let route = PageRoute {
            source: PathBuf::from("content/index.typ"),
            is_index: true,
            is_404: false,
            permalink: crate::core::UrlPath::from_page("/"),
            output_file: PathBuf::from("public/docs/blog/index.html"),
            output_dir: PathBuf::from("public/docs/blog"),
            full_url: "https://example.com/docs/blog/".to_string(),
        };
        let root = TolaSite::element("main", Attrs::new()).child(TolaSite::element(
            "script",
            Attrs::from([("src", "/scripts/app.js")]),
        ));
        let indexed = TolaSite::indexer().transform(Document::new(root));

        let transformed = MediaTransform::new(&config, &route).transform(indexed);

        let script = transformed.find(|elem| elem.is_tag("script")).unwrap();
        assert_eq!(script.get_attr("src"), Some("/docs/blog/scripts/app.js"));
    }

    #[test]
    fn nobg_site_root_image_at_output_root_keeps_single_leading_slash() {
        let config = SiteConfig::default();
        let route = PageRoute {
            source: PathBuf::from("content/index.typ"),
            is_index: true,
            is_404: false,
            permalink: crate::core::UrlPath::from_page("/"),
            output_file: PathBuf::from("public/index.html"),
            output_dir: PathBuf::from("public"),
            full_url: "https://example.com/".to_string(),
        };
        let transform = MediaTransform::new(&config, &route);

        let (_, new_src, _) = transform.generate_nobg_paths("/hero.png", Path::new("hero.png"));

        assert_eq!(new_src, "/hero.nobg.png");
    }

    #[test]
    fn nobg_site_root_source_resolution_uses_flatten_asset_route() {
        let dir = tempfile::TempDir::new().unwrap();
        let source_raw = dir.path().join("assets/hero.png");
        std::fs::create_dir_all(source_raw.parent().unwrap()).unwrap();
        std::fs::write(&source_raw, "image").unwrap();
        let source = crate::utils::path::normalize_path(&source_raw);

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.output = dir.path().join("public");
        config.build.assets.flatten = vec![FlattenEntry::Simple(source.clone())];

        let route = PageRoute {
            source: dir.path().join("content/index.typ"),
            is_index: true,
            is_404: false,
            permalink: crate::core::UrlPath::from_page("/"),
            output_file: dir.path().join("public/index.html"),
            output_dir: dir.path().join("public"),
            full_url: "https://example.com/".to_string(),
        };
        let transform = MediaTransform::new(&config, &route);

        assert_eq!(transform.resolve_source_path("/hero.png"), Some(source));
    }
}
