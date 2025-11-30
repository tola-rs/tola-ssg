//! `[build]` section configuration.
//!
//! Contains build settings including paths, minification, typst, tailwind, etc.

use super::defaults;
use educe::Educe;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ============================================================================
// Enums
// ============================================================================

/// URL slug generation mode for paths and anchors.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlugMode {
    /// Always convert to ASCII slug (e.g., "你好" → "ni-hao").
    On,
    /// Only slugify non-ASCII; keep ASCII as-is (default).
    #[default]
    Safe,
    /// No slugification; preserve original text.
    No,
}

/// SVG image extraction method for embedded raster images.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtractSvgType {
    /// Use built-in Rust image libraries.
    Builtin,
    /// Use ImageMagick (`convert` command).
    Magick,
    /// Use FFmpeg for conversion.
    Ffmpeg,
    /// Keep as SVG without extracting images.
    JustSvg,
    /// Embed SVG directly in HTML (default).
    #[default]
    Embedded,
}

// ============================================================================
// Main BuildConfig
// ============================================================================

/// `[build]` section in tola.toml - build pipeline configuration.
///
/// # Example
/// ```toml
/// [build]
/// content = "content"      # Source directory
/// output = "public"        # Output directory
/// minify = true            # Minify HTML
///
/// [build.typst]
/// command = ["typst"]
///
/// [build.typst.svg]
/// dpi = 144.0
/// ```
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(default, deny_unknown_fields)]
pub struct BuildConfig {
    /// Project root directory (usually set via CLI `--root`).
    #[serde(default = "defaults::build::root")]
    #[educe(Default = defaults::build::root())]
    pub root: Option<PathBuf>,

    /// URL path prefix for subdirectory deployment (e.g., "blog" → `/blog/...`).
    #[serde(default = "defaults::build::path_prefix")]
    #[educe(Default = defaults::build::path_prefix())]
    pub path_prefix: PathBuf,

    /// Content source directory (Typst files).
    #[serde(default = "defaults::build::content")]
    #[educe(Default = defaults::build::content())]
    pub content: PathBuf,

    /// Build output directory.
    #[serde(default = "defaults::build::output")]
    #[educe(Default = defaults::build::output())]
    pub output: PathBuf,

    /// Static assets directory (images, CSS, JS).
    #[serde(default = "defaults::build::assets")]
    #[educe(Default = defaults::build::assets())]
    pub assets: PathBuf,

    /// HTML template directory.
    #[serde(default = "defaults::build::templates")]
    #[educe(Default = defaults::build::templates())]
    pub templates: PathBuf,

    /// Shared Typst utilities directory.
    #[serde(default = "defaults::build::utils")]
    #[educe(Default = defaults::build::utils())]
    pub utils: PathBuf,

    /// Minify HTML output (removes whitespace).
    #[serde(default = "defaults::r#true")]
    #[educe(Default = true)]
    pub minify: bool,

    /// Clear output directory before each build.
    #[serde(default = "defaults::r#false")]
    #[educe(Default = false)]
    pub clear: bool,

    /// RSS feed generation settings.
    #[serde(default)]
    pub rss: RssConfig,

    /// URL slugification settings.
    #[serde(default)]
    pub slug: SlugConfig,

    /// Typst compiler configuration.
    #[serde(default)]
    pub typst: TypstConfig,

    /// Tailwind CSS integration.
    #[serde(default)]
    pub tailwind: TailwindConfig,

    /// Custom `<head>` elements.
    #[serde(default)]
    pub head: HeadConfig,
}

// ============================================================================
// Sub-configurations
// ============================================================================

/// `[build.rss]` section - RSS feed generation configuration.
///
/// RSS generation is controlled by two factors:
/// - `enable`: this config option (user-controlled)
/// - Mode: `build`/`deploy` generate RSS, `serve` skips it for faster local preview
///
/// See `build_all` in `main.rs` and `build_rss` in `utils/rss.rs` for implementation.
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct RssConfig {
    /// Enable RSS feed generation (only effective in build/deploy mode).
    #[serde(default = "defaults::r#false")]
    #[educe(Default = defaults::r#false())]
    pub enable: bool,

    /// Output path for RSS feed file.
    #[serde(default = "defaults::build::rss::path")]
    #[educe(Default = defaults::build::rss::path())]
    pub path: PathBuf,
}

/// `[build.slug]` section
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct SlugConfig {
    /// Slugify URL paths
    #[serde(default = "defaults::build::slug::default")]
    #[educe(Default = defaults::build::slug::default())]
    pub path: SlugMode,

    /// Slugify URL fragments (anchors)
    #[serde(default = "defaults::build::slug::on")]
    #[educe(Default = defaults::build::slug::on())]
    pub fragment: SlugMode,
}

/// `[build.typst]` section
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct TypstConfig {
    /// Typst command and arguments
    #[serde(default = "defaults::build::typst::command")]
    #[educe(Default = defaults::build::typst::command())]
    pub command: Vec<String>,

    /// SVG processing options
    #[serde(default)]
    pub svg: TypstSvgConfig,
}

/// `[build.typst.svg]` section
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct TypstSvgConfig {
    /// Method for extracting embedded SVG images
    #[serde(default = "defaults::build::typst::svg::extract_type")]
    #[educe(Default = defaults::build::typst::svg::extract_type())]
    pub extract_type: ExtractSvgType,

    /// Max size for inline SVG (e.g.: "20KB", "1MB")
    #[serde(default = "defaults::build::typst::svg::inline_max_size")]
    #[educe(Default = defaults::build::typst::svg::inline_max_size())]
    pub inline_max_size: String,

    /// DPI for SVG rendering
    #[serde(default = "defaults::build::typst::svg::dpi")]
    #[educe(Default = defaults::build::typst::svg::dpi())]
    pub dpi: f32,
}

/// `[build.tailwind]` section
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct TailwindConfig {
    /// Enable Tailwind CSS processing
    #[serde(default = "defaults::r#false")]
    #[educe(Default = false)]
    pub enable: bool,

    /// Input CSS file path
    #[serde(default = "defaults::build::tailwind::input")]
    #[educe(Default = defaults::build::tailwind::input())]
    pub input: Option<PathBuf>,

    /// Tailwind command and arguments
    #[serde(default = "defaults::build::tailwind::command")]
    #[educe(Default = defaults::build::tailwind::command())]
    pub command: Vec<String>,
}

/// `[build.head]` section for custom head elements
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct HeadConfig {
    /// Favicon path (relative to assets directory)
    #[serde(default)]
    pub icon: Option<PathBuf>,

    /// CSS stylesheet paths (relative to assets directory)
    #[serde(default)]
    pub styles: Vec<PathBuf>,

    /// Script entries (relative to assets directory)
    #[serde(default)]
    pub scripts: Vec<ScriptEntry>,

    /// Raw HTML elements to insert into head (e.g., `<meta name="darkreader-lock">`)
    #[serde(default)]
    pub elements: Vec<String>,
}

/// Script entry for `[build.head.scripts]`.
///
/// # Formats
/// ```toml
/// # Simple path
/// scripts = ["./assets/app.js"]
///
/// # With loading options
/// scripts = [
///     { path = "./assets/app.js", defer = true },
///     { path = "./assets/analytics.js", async = true },
/// ]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScriptEntry {
    /// Simple path string.
    Simple(PathBuf),
    /// Path with `defer`/`async` attributes.
    WithOptions {
        path: PathBuf,
        #[serde(default)]
        defer: bool,
        #[serde(default)]
        r#async: bool,
    },
}

impl ScriptEntry {
    /// Get the path for this script entry
    pub fn path(&self) -> &Path {
        match self {
            ScriptEntry::Simple(path) => path,
            ScriptEntry::WithOptions { path, .. } => path,
        }
    }

    /// Check if defer attribute should be added
    pub fn is_defer(&self) -> bool {
        match self {
            ScriptEntry::Simple(_) => false,
            ScriptEntry::WithOptions { defer, .. } => *defer,
        }
    }

    /// Check if async attribute should be added
    pub fn is_async(&self) -> bool {
        match self {
            ScriptEntry::Simple(_) => false,
            ScriptEntry::WithOptions { r#async, .. } => *r#async,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::super::SiteConfig;
    use super::*;

    #[test]
    fn test_build_config_defaults() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(config.build.content, PathBuf::from("content"));
        assert_eq!(config.build.output, PathBuf::from("public"));
        assert_eq!(config.build.assets, PathBuf::from("assets"));
        assert!(config.build.minify);
        assert!(!config.build.clear);
    }

    #[test]
    fn test_rss_config() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"

            [build.rss]
            enable = true
            path = "custom-feed.xml"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert!(config.build.rss.enable);
        assert_eq!(config.build.rss.path, PathBuf::from("custom-feed.xml"));
    }

    #[test]
    fn test_slug_config() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"

            [build.slug]
            path = "on"
            fragment = "no"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert!(matches!(config.build.slug.path, SlugMode::On));
        assert!(matches!(config.build.slug.fragment, SlugMode::No));
    }

    #[test]
    fn test_slug_mode_parsing() {
        // Test "on"
        let config: SiteConfig = toml::from_str(
            r#"
            [base]
            title = "Test"
            description = "Test"
            [build.slug]
            path = "on"
            fragment = "on"
        "#,
        )
        .unwrap();
        assert!(matches!(config.build.slug.path, SlugMode::On));
        assert!(matches!(config.build.slug.fragment, SlugMode::On));

        // Test "safe"
        let config: SiteConfig = toml::from_str(
            r#"
            [base]
            title = "Test"
            description = "Test"
            [build.slug]
            path = "safe"
            fragment = "safe"
        "#,
        )
        .unwrap();
        assert!(matches!(config.build.slug.path, SlugMode::Safe));
        assert!(matches!(config.build.slug.fragment, SlugMode::Safe));

        // Test "no"
        let config: SiteConfig = toml::from_str(
            r#"
            [base]
            title = "Test"
            description = "Test"
            [build.slug]
            path = "no"
            fragment = "no"
        "#,
        )
        .unwrap();
        assert!(matches!(config.build.slug.path, SlugMode::No));
        assert!(matches!(config.build.slug.fragment, SlugMode::No));
    }

    #[test]
    fn test_typst_config() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"

            [build.typst]
            command = ["typst-custom"]

            [build.typst.svg]
            extract_type = "magick"
            inline_max_size = "50KB"
            dpi = 144.0
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(config.build.typst.command, vec!["typst-custom".to_string()]);
        assert!(matches!(
            config.build.typst.svg.extract_type,
            ExtractSvgType::Magick
        ));
        assert_eq!(config.build.typst.svg.inline_max_size, "50KB");
        assert_eq!(config.build.typst.svg.dpi, 144.0);
    }

    #[test]
    fn test_extract_svg_type_parsing() {
        let types = [
            ("builtin", ExtractSvgType::Builtin),
            ("magick", ExtractSvgType::Magick),
            ("ffmpeg", ExtractSvgType::Ffmpeg),
            ("justsvg", ExtractSvgType::JustSvg),
            ("embedded", ExtractSvgType::Embedded),
        ];

        for (str_type, expected) in types {
            let config = format!(
                r#"
                [base]
                title = "Test"
                description = "Test"
                [build.typst.svg]
                extract_type = "{str_type}"
            "#
            );
            let config: SiteConfig = toml::from_str(&config).unwrap();

            assert!(
                std::mem::discriminant(&config.build.typst.svg.extract_type)
                    == std::mem::discriminant(&expected),
                "Failed for type: {str_type}"
            );
        }
    }

    #[test]
    fn test_tailwind_config() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"

            [build.tailwind]
            enable = true
            input = "assets/styles/main.css"
            command = ["tailwindcss-v4"]
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert!(config.build.tailwind.enable);
        assert_eq!(
            config.build.tailwind.input,
            Some(PathBuf::from("assets/styles/main.css"))
        );
        assert_eq!(
            config.build.tailwind.command,
            vec!["tailwindcss-v4".to_string()]
        );
    }

    #[test]
    fn test_head_config_icon() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"

            [build.head]
            icon = "./assets/images/favicon.avif"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(
            config.build.head.icon,
            Some(PathBuf::from("./assets/images/favicon.avif"))
        );
    }

    #[test]
    fn test_head_config_styles() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"

            [build.head]
            styles = [
                "./assets/fonts/custom/font.css",
                "./assets/styles/highlight.min.css"
            ]
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(config.build.head.styles.len(), 2);
        assert_eq!(
            config.build.head.styles[0],
            PathBuf::from("./assets/fonts/custom/font.css")
        );
        assert_eq!(
            config.build.head.styles[1],
            PathBuf::from("./assets/styles/highlight.min.css")
        );
    }

    #[test]
    fn test_head_config_scripts_simple() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"

            [build.head]
            scripts = [
                "./assets/scripts/a.js",
                "./assets/scripts/b.js"
            ]
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(config.build.head.scripts.len(), 2);
        assert_eq!(
            config.build.head.scripts[0].path(),
            Path::new("./assets/scripts/a.js")
        );
        assert!(!config.build.head.scripts[0].is_defer());
        assert!(!config.build.head.scripts[0].is_async());
    }

    #[test]
    fn test_head_config_scripts_with_options() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"

            [build.head]
            scripts = [
                { path = "./assets/scripts/a.js", defer = true },
                "./assets/scripts/b.js",
                { path = "./assets/scripts/c.js", async = true }
            ]
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(config.build.head.scripts.len(), 3);

        // First script with defer
        assert_eq!(
            config.build.head.scripts[0].path(),
            Path::new("./assets/scripts/a.js")
        );
        assert!(config.build.head.scripts[0].is_defer());
        assert!(!config.build.head.scripts[0].is_async());

        // Second script - simple path
        assert_eq!(
            config.build.head.scripts[1].path(),
            Path::new("./assets/scripts/b.js")
        );
        assert!(!config.build.head.scripts[1].is_defer());
        assert!(!config.build.head.scripts[1].is_async());

        // Third script with async
        assert_eq!(
            config.build.head.scripts[2].path(),
            Path::new("./assets/scripts/c.js")
        );
        assert!(!config.build.head.scripts[2].is_defer());
        assert!(config.build.head.scripts[2].is_async());
    }

    #[test]
    fn test_head_config_elements() {
        let config = r##"
            [base]
            title = "Test"
            description = "Test blog"

            [build.head]
            elements = [
                '<meta name="darkreader-lock">',
                '<meta name="theme-color" content="#ffffff">'
            ]
        "##;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(config.build.head.elements.len(), 2);
        assert_eq!(
            config.build.head.elements[0],
            "<meta name=\"darkreader-lock\">"
        );
        assert_eq!(
            config.build.head.elements[1],
            "<meta name=\"theme-color\" content=\"#ffffff\">"
        );
    }

    #[test]
    fn test_head_config_full() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"

            [build.head]
            icon = "./assets/images/blog/avatar.avif"
            styles = [
                "./assets/fonts/MapleMono-NF-CN-Regular/result.css",
                "./assets/styles/highlight.min.css"
            ]
            scripts = [
                { path = "./assets/scripts/a.js", defer = true },
                "./assets/scripts/b.js",
                { path = "./assets/scripts/c.js", async = true }
            ]
            elements = [
                '<meta name="darkreader-lock">'
            ]
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert!(config.build.head.icon.is_some());
        assert_eq!(config.build.head.styles.len(), 2);
        assert_eq!(config.build.head.scripts.len(), 3);
        assert_eq!(config.build.head.elements.len(), 1);
    }

    #[test]
    fn test_head_config_defaults() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert!(config.build.head.icon.is_none());
        assert!(config.build.head.styles.is_empty());
        assert!(config.build.head.scripts.is_empty());
        assert!(config.build.head.elements.is_empty());
    }

    #[test]
    fn test_unknown_field_rejection() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test blog"

            [build]
            unknown_field = "should_fail"
        "#;
        let result: Result<SiteConfig, _> = toml::from_str(config);

        assert!(result.is_err());
    }

    #[test]
    fn test_build_paths_custom() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [build]
            content = "posts"
            output = "dist"
            assets = "static"
            templates = "layouts"
            utils = "lib"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();

        assert_eq!(config.build.content, PathBuf::from("posts"));
        assert_eq!(config.build.output, PathBuf::from("dist"));
        assert_eq!(config.build.assets, PathBuf::from("static"));
        assert_eq!(config.build.templates, PathBuf::from("layouts"));
        assert_eq!(config.build.utils, PathBuf::from("lib"));
    }

    #[test]
    fn test_build_path_prefix() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [build]
            path_prefix = "blog"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert_eq!(config.build.path_prefix, PathBuf::from("blog"));
    }

    #[test]
    fn test_build_minify_disabled() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [build]
            minify = false
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert!(!config.build.minify);
    }

    #[test]
    fn test_build_clear_enabled() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [build]
            clear = true
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert!(config.build.clear);
    }

    #[test]
    fn test_rss_config_defaults() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert!(!config.build.rss.enable);
        assert_eq!(config.build.rss.path, PathBuf::from("feed.xml"));
    }

    #[test]
    fn test_rss_unknown_field_rejection() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [build.rss]
            unknown = "field"
        "#;
        let result: Result<SiteConfig, _> = toml::from_str(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_slug_config_defaults() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert!(matches!(config.build.slug.path, SlugMode::Safe));
        assert!(matches!(config.build.slug.fragment, SlugMode::On));
    }

    #[test]
    fn test_slug_unknown_field_rejection() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [build.slug]
            unknown = "field"
        "#;
        let result: Result<SiteConfig, _> = toml::from_str(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_typst_config_defaults() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert_eq!(config.build.typst.command, vec!["typst".to_string()]);
        assert!(matches!(
            config.build.typst.svg.extract_type,
            ExtractSvgType::Embedded
        ));
        assert_eq!(config.build.typst.svg.inline_max_size, "20KB");
        assert_eq!(config.build.typst.svg.dpi, 96.0);
    }

    #[test]
    fn test_typst_unknown_field_rejection() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [build.typst]
            unknown = "field"
        "#;
        let result: Result<SiteConfig, _> = toml::from_str(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_typst_svg_unknown_field_rejection() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [build.typst.svg]
            unknown = "field"
        "#;
        let result: Result<SiteConfig, _> = toml::from_str(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_tailwind_config_defaults() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert!(!config.build.tailwind.enable);
        assert!(config.build.tailwind.input.is_none());
        assert_eq!(
            config.build.tailwind.command,
            vec!["tailwindcss".to_string()]
        );
    }

    #[test]
    fn test_tailwind_unknown_field_rejection() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [build.tailwind]
            unknown = "field"
        "#;
        let result: Result<SiteConfig, _> = toml::from_str(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_head_unknown_field_rejection() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [build.head]
            unknown = "field"
        "#;
        let result: Result<SiteConfig, _> = toml::from_str(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_script_entry_methods() {
        // Simple script
        let simple = ScriptEntry::Simple(PathBuf::from("script.js"));
        assert_eq!(simple.path(), Path::new("script.js"));
        assert!(!simple.is_defer());
        assert!(!simple.is_async());

        // Script with defer
        let deferred = ScriptEntry::WithOptions {
            path: PathBuf::from("deferred.js"),
            defer: true,
            r#async: false,
        };
        assert_eq!(deferred.path(), Path::new("deferred.js"));
        assert!(deferred.is_defer());
        assert!(!deferred.is_async());

        // Script with async
        let async_script = ScriptEntry::WithOptions {
            path: PathBuf::from("async.js"),
            defer: false,
            r#async: true,
        };
        assert_eq!(async_script.path(), Path::new("async.js"));
        assert!(!async_script.is_defer());
        assert!(async_script.is_async());

        // Script with both defer and async
        let both = ScriptEntry::WithOptions {
            path: PathBuf::from("both.js"),
            defer: true,
            r#async: true,
        };
        assert!(both.is_defer());
        assert!(both.is_async());
    }

    #[test]
    fn test_typst_command_multiple_args() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [build.typst]
            command = ["typst", "--root", "/path"]
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert_eq!(config.build.typst.command, vec!["typst", "--root", "/path"]);
    }

    #[test]
    fn test_tailwind_command_multiple_args() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [build.tailwind]
            command = ["npx", "tailwindcss"]
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert_eq!(config.build.tailwind.command, vec!["npx", "tailwindcss"]);
    }

    #[test]
    fn test_typst_svg_dpi_float() {
        let config = r#"
            [base]
            title = "Test"
            description = "Test"
            [build.typst.svg]
            dpi = 72.5
        "#;
        let config: SiteConfig = toml::from_str(config).unwrap();
        assert_eq!(config.build.typst.svg.dpi, 72.5);
    }
}
