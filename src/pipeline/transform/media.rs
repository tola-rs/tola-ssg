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

use super::link::process_link_value;
use crate::address::resolve_physical_path;
use crate::compiler::family::{Indexed, TolaSite::FamilyKind};
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
        let Some(src) = elem.get_attr("src") else {
            return;
        };

        // Skip external URLs
        if src.starts_with("http://") || src.starts_with("https://") || src.starts_with("//") {
            return;
        }

        // Resolve source file path
        let Some(source_path) = self.resolve_source_path(src) else {
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
        let (output_path, new_src, original_output) = self.generate_nobg_paths(src, &source_path);

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
        elem.set_attr("src", new_src);
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
                // /images/xxx.png -> public/images/xxx.nobg.png, /images/xxx.nobg.png
                let trimmed = path.trim_start_matches('/');
                let parent = Path::new(trimmed).parent().unwrap_or(Path::new(""));
                let output_path = self
                    .config
                    .build
                    .output
                    .join(parent)
                    .join(format!("{}.nobg.png", stem));
                let new_src = format!("/{}/{}.nobg.png", parent.display(), stem);
                // Use src path for consistency with compute_output_path
                let original_output = self.config.build.output.join(trimmed);
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
    /// - File-relative paths: `./image.png` -> colocated_dir or source parent
    /// - Site-root paths: `/images/xxx` -> config.build.assets.nested mapping
    fn resolve_source_path(&self, src: &str) -> Option<PathBuf> {
        match LinkKind::parse(src) {
            LinkKind::SiteRoot(path) => {
                // /images/xxx -> find nested asset entry with output_name "images"
                let trimmed = path.trim_start_matches('/');
                for entry in &self.config.build.assets.nested {
                    let output_name = entry.output_name();
                    // Exact match: /assets -> output_name "assets"
                    if trimmed == output_name {
                        let source_path = self.config.root.join(entry.source());
                        if source_path.exists() {
                            return Some(source_path);
                        }
                    }
                    // Prefix with slash: /assets/xxx -> output_name "assets", rest "xxx"
                    if let Some(rest) = trimmed.strip_prefix(output_name)
                        && let Some(file_path) = rest.strip_prefix('/')
                    {
                        let source_path = self.config.root.join(entry.source()).join(file_path);
                        if source_path.exists() {
                            return Some(source_path);
                        }
                    }
                }
                None
            }
            LinkKind::FileRelative(_) | LinkKind::Fragment(_) => {
                // Try colocated directory first (for ./image.png style paths)
                if let Some(colocated) = &self.route.colocated_dir {
                    let path = resolve_physical_path(colocated, src);
                    if path.exists() {
                        return Some(path);
                    }
                }

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
            LinkKind::SiteRoot(path) => {
                let trimmed = path.trim_start_matches('/');
                Some(self.config.build.output.join(trimmed))
            }
            LinkKind::FileRelative(path) => {
                let filename = Path::new(path).file_name()?;
                Some(self.route.output_dir.join(filename))
            }
            _ => None,
        }
    }
}

impl Transform<Indexed> for MediaTransform<'_> {
    type To = Indexed;

    fn transform(self, mut doc: Document<Indexed>) -> Document<Indexed> {
        // Process recolor and nobg class inheritance + nobg image processing
        let auto_inject = self.config.theme.recolor.enable
            && self.config.theme.recolor.target == RecolorTarget::Auto;
        process_classes(&mut doc.root, &self, ClassState::default(), auto_inject);

        // Process src attributes (URL resolution)
        doc.modify_by::<FamilyKind::Media, _>(|elem| {
            if let Some(src) = elem.get_attr("src").map(|s| s.to_string())
                && let Ok(processed) = process_link_value(&src, self.config, self.route)
            {
                elem.set_attr("src", processed);
            }
        });

        doc
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
