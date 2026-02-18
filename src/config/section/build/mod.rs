//! `[build]` section configuration.
//!
//! Contains build settings including paths, minification, and sub-configurations.
//!
//! # Example
//!
//! ```toml
//! [build]
//! content = "content"         # Source directory for .typ files (relative to site root)
//! output = "public"           # Output directory for generated HTML (relative to site root)
//! assets = "assets"           # Static assets directory (relative to site root)
//! deps = ["templates"]        # Dependency dirs (relative to site root)
//! minify = true               # Minify HTML output
//! not_found = "404.html"      # Custom 404 page (relative to site root, supports .typ/.html)
//!
//! [build.feed]
//! enable = true               # Generate RSS/Atom feed
//! path = "feed.xml"           # Feed output path
//!
//! [build.sitemap]
//! enable = true               # Generate sitemap.xml
//!
//! [build.slug]
//! path = "safe"               # URL path slugification: full | safe | ascii
//! fragment = "full"           # Anchor slugification
//!
//! [build.svg]
//! external = true             # Extract to separate files (false = embed in HTML)
//! converter = "builtin"       # Conversion tool: builtin | magick | ffmpeg | none
//! format = "avif"             # Output format: avif | png | jpg | webp
//! dpi = 144.0                 # Rendering DPI (default: 96.0)
//!
//! [build.header]
//! icon = "favicon.ico"        # Favicon path (relative to `[build.assets]`)
//! styles = ["styles/custom.css"]     # CSS files (relative to `[build.assets]`)
//! scripts = ["scripts/app.js"]        # JS files (relative to `[build.assets]`)
//! ```
//!
//! See submodules for detailed options: [`feed`], [`sitemap`], [`slug`], [`svg`], [`css`], [`header`].

pub mod assets;
mod css;
mod diagnostics;
mod feed;
mod header;
mod hooks;
mod meta;
mod sitemap;
mod slug;
mod svg;

pub use assets::AssetsConfig;
pub use css::CssConfig;
pub use diagnostics::DiagnosticsConfig;
pub use feed::{FeedConfig, FeedFormat};
pub use header::HeaderConfig;
pub use hooks::{HookConfig, HooksConfig, WatchMode};
pub use meta::MetaConfig;
pub use sitemap::SitemapConfig;
pub use slug::{SlugCase, SlugConfig, SlugMode};
pub use svg::{SvgConfig, SvgConverter, SvgFormat};

use crate::config::{ConfigDiagnostics, FieldPath};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildSectionConfig {
    /// URL path prefix for subdirectory deployment.
    /// Automatically extracted from `[base].url` path component.
    #[serde(skip)]
    pub path_prefix: PathBuf,

    /// Content source directory (Typst files).
    pub content: PathBuf,

    /// Build output directory.
    pub output: PathBuf,

    /// Static assets configuration.
    pub assets: AssetsConfig,

    /// Dependency directories (templates/, utilities/, etc.).
    pub deps: Vec<PathBuf>,

    /// Virtual data files directory (relative to output).
    pub data: PathBuf,

    /// Minify HTML output.
    pub minify: bool,

    /// Clean output directory before building (CLI only).
    #[serde(skip)]
    pub clean: bool,

    /// Skip draft pages during build (CLI only).
    #[serde(skip)]
    pub skip_drafts: bool,

    /// Custom 404 page source file.
    pub not_found: Option<PathBuf>,

    /// Feed generation settings.
    pub feed: FeedConfig,

    /// Sitemap generation settings.
    pub sitemap: SitemapConfig,

    /// URL slugification settings.
    pub slug: SlugConfig,

    /// SVG processing settings.
    pub svg: SvgConfig,

    /// CSS processing settings.
    pub css: CssConfig,

    /// Build hooks (pre/post commands).
    pub hooks: HooksConfig,

    /// Metadata extraction settings.
    pub meta: MetaConfig,

    /// Custom `<head>` elements.
    pub header: HeaderConfig,

    /// Diagnostics display settings (warnings/errors).
    pub diagnostics: DiagnosticsConfig,

    /// Allow experimental features without warnings.
    #[serde(default)]
    pub allow_experimental: bool,
}

impl Default for BuildSectionConfig {
    fn default() -> Self {
        Self {
            path_prefix: PathBuf::new(),
            content: "content".into(),
            output: "public".into(),
            assets: AssetsConfig {
                nested: vec![assets::NestedEntry::Simple("assets".into())],
                flatten: vec![],
            },
            deps: vec!["templates".into(), "utils".into()],
            data: "_data".into(),
            minify: true,
            clean: false,
            skip_drafts: false,
            not_found: None,
            feed: FeedConfig::default(),
            sitemap: SitemapConfig::default(),
            slug: SlugConfig::default(),
            svg: SvgConfig::default(),
            css: CssConfig::default(),
            hooks: HooksConfig::default(),
            meta: MetaConfig::default(),
            header: HeaderConfig::default(),
            diagnostics: DiagnosticsConfig::default(),
            allow_experimental: false,
        }
    }
}

impl BuildSectionConfig {
    /// Validate build configuration.
    ///
    /// Checks deps paths exist and warns about missing ones.
    pub fn validate(&self, diag: &mut ConfigDiagnostics) {
        // Warn about missing deps directories
        for dep in &self.deps {
            if !dep.exists() {
                let rel_path = dep
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| dep.display().to_string());
                diag.hint(
                    FieldPath::new("build.deps"),
                    format!("directory '{}' not found, skipping", rel_path),
                );
            }
        }
    }

    /// Filter deps to only existing directories.
    ///
    /// Call after validate() to remove missing paths from watch list.
    pub fn filter_existing_deps(&mut self) {
        self.deps.retain(|p| p.exists());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_parse_config;
    use std::path::Path;

    #[test]
    fn test_defaults() {
        let config = test_parse_config("");
        assert_eq!(config.build.content, PathBuf::from("content"));
        assert_eq!(config.build.output, PathBuf::from("public"));
        // assets is now AssetsConfig with default nested = ["assets"]
        assert_eq!(config.build.assets.nested.len(), 1);
        assert_eq!(config.build.assets.nested[0].source(), Path::new("assets"));
        assert_eq!(
            config.build.deps,
            vec![PathBuf::from("templates"), PathBuf::from("utils")]
        );
        assert_eq!(config.build.data, PathBuf::from("_data"));
        assert!(config.build.minify);
        assert!(config.build.path_prefix.as_os_str().is_empty());
    }

    #[test]
    fn test_custom_assets() {
        // New format: [build.assets] section
        let config = test_parse_config(
            r#"
[build.assets]
nested = ["static", { dir = "vendor", as = "lib" }]
flatten = ["CNAME"]
"#,
        );
        assert_eq!(config.build.assets.nested.len(), 2);
        assert_eq!(config.build.assets.nested[0].source(), Path::new("static"));
        assert_eq!(config.build.assets.nested[1].output_name(), "lib");
        assert_eq!(config.build.assets.flatten.len(), 1);
        // minify defaults to true, only test assets config here
    }
}
