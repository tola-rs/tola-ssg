//! Site configuration management.
//!
//! Handles loading, parsing, and validating the `tola.toml` configuration file.

use crate::cli::{Cli, Commands};
use anyhow::{Context, Result, bail};
use educe::Educe;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};
use thiserror::Error;

/// Configuration-related errors
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("IO error when reading `{0}`")]
    Io(PathBuf, #[source] std::io::Error),

    #[error("Config file parsing error")]
    Toml(#[from] toml::de::Error),

    #[error("Config validation error: {0}")]
    Validation(String),
}

/// Default values for serde deserialization
pub mod config_defaults {
    pub fn r#true() -> bool {
        true
    }

    #[allow(unused)]
    pub fn r#false() -> bool {
        false
    }

    pub mod base {
        pub fn url() -> Option<String> {
            None
        }
        pub fn author() -> String {
            "<YOUR_NAME>".into()
        }
        pub fn email() -> String {
            "user@noreply.tola".into()
        }
    }

    pub mod build {
        use std::path::PathBuf;

        pub fn root() -> Option<PathBuf> {
            None
        }
        pub fn base_path() -> PathBuf {
            "".into()
        }
        pub fn language() -> String {
            "zh-Hans".into()
        }
        pub fn content() -> PathBuf {
            "content".into()
        }
        pub fn output() -> PathBuf {
            "public".into()
        }
        pub fn assets() -> PathBuf {
            "assets".into()
        }

        pub mod rss {
            use std::path::PathBuf;

            pub fn path() -> PathBuf {
                "feed.xml".into()
            }
        }

        #[allow(unused)]
        pub mod slug {
            use crate::config::SlugMode;

            pub fn default() -> SlugMode {
                SlugMode::default()
            }
            pub fn no() -> SlugMode {
                SlugMode::No
            }
            pub fn safe() -> SlugMode {
                SlugMode::Safe
            }
            pub fn on() -> SlugMode {
                SlugMode::On
            }
        }

        pub mod typst {
            pub fn command() -> Vec<String> {
                vec!["typst".into()]
            }
            pub mod svg {
                use crate::config::ExtractSvgType;

                pub fn extract_type() -> ExtractSvgType {
                    ExtractSvgType::default()
                }
                pub fn inline_max_size() -> String {
                    "20KB".into()
                }
                pub fn dpi() -> f32 {
                    96.
                }
            }
        }

        pub mod tailwind {
            use std::path::PathBuf;

            pub fn input() -> Option<PathBuf> {
                None
            }
            pub fn command() -> Vec<String> {
                vec!["tailwindcss".into()]
            }
        }
    }

    pub mod serve {
        pub fn interface() -> String {
            "127.0.0.1".into()
        }
        pub fn port() -> u16 {
            5277
        }
    }

    pub mod deploy {
        pub fn provider() -> String {
            "github".into()
        }

        pub mod github {
            use std::path::PathBuf;

            pub fn url() -> String {
                "https://github.com/alice/alice.github.io".into()
            }
            pub fn branch() -> String {
                "main".into()
            }
            pub fn token_path() -> Option<PathBuf> {
                None
            }
        }

        pub mod cloudflare {
            use std::path::PathBuf;

            pub fn _remote() -> String {
                "https://alice.com".into()
            }
            pub fn _branch() -> String {
                "main".into()
            }
            pub fn _token_path() -> PathBuf {
                "~/xxx/xxx/.github-token-in-this-file".into()
            }
        }

        pub mod vercal {
            use std::path::PathBuf;

            pub fn _remote() -> String {
                "https://alice.com".into()
            }
            pub fn _branch() -> String {
                "main".into()
            }
            pub fn _token_path() -> PathBuf {
                "~/xxx/xxx/.github-token-in-this-file".into()
            }
        }
    }
}

/// URL slug generation mode
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlugMode {
    /// Always slugify
    On,
    /// Only slugify non-ASCII characters (default)
    #[default]
    Safe,
    /// No slugification
    No,
}

/// SVG extraction method for embedded images
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtractSvgType {
    /// Use built-in Rust libraries
    Builtin,
    /// Use ImageMagick
    Magick,
    /// Use FFmpeg
    Ffmpeg,
    /// Keep as SVG without conversion
    JustSvg,
    /// Embed directly in HTML (default)
    #[default]
    Embedded,
}

/// `[base]` section in tola.toml
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct BaseConfig {
    /// Site title
    pub title: String,

    /// Author name, e.g.: "Bob"
    #[serde(default = "config_defaults::base::author")]
    #[educe(Default = config_defaults::base::author())]
    pub author: String,

    /// Author email, e.g.: "bob@example.com"
    #[serde(default = "config_defaults::base::email")]
    #[educe(Default = config_defaults::base::email())]
    pub email: String,

    /// Site description
    pub description: String,

    /// Base URL for RSS/sitemap generation, e.g.: "https://example.com"
    #[serde(default = "config_defaults::base::url")]
    #[educe(Default = config_defaults::base::url())]
    pub url: Option<String>,

    /// Language code, e.g.: "zh-Hans", "en_US"
    #[serde(default = "config_defaults::build::language")]
    #[educe(Default = config_defaults::build::language())]
    pub language: String,

    /// Copyright notice
    #[serde(default)]
    pub copyright: String,
}

#[test]
fn validate_base_config() {
    let config = r#"
        [base]
        title = "KawaYww"
        description = "KawaYww's Blog"
        url = "https://kawayww.com"
        language = "zh_Hans"
        copyright = "2025 KawaYww"
    "#;
    let config: SiteConfig = toml::from_str(config).unwrap();

    assert_eq!(config.base.title, "KawaYww");
    assert_eq!(config.base.description, "KawaYww's Blog");
    assert_eq!(config.base.url, Some("https://kawayww.com".to_string()));
    assert_eq!(config.base.language, "zh_Hans");
    assert_eq!(config.base.copyright, "2025 KawaYww");
}

#[test]
fn test_base_config_defaults() {
    let config = r#"
        [base]
        title = "Test"
        description = "Test blog"
    "#;
    let config: SiteConfig = toml::from_str(config).unwrap();

    assert_eq!(config.base.author, "<YOUR_NAME>");
    assert_eq!(config.base.email, "user@noreply.tola");
    assert_eq!(config.base.language, "zh-Hans");
    assert_eq!(config.base.url, None);
    assert_eq!(config.base.copyright, "");
}

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
fn test_build_rss_config() {
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
fn test_build_slug_config() {
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
fn test_build_typst_config() {
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
fn test_build_tailwind_config() {
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
fn test_build_head_config_icon() {
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
fn test_build_head_config_styles() {
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
fn test_build_head_config_scripts_simple() {
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
fn test_build_head_config_scripts_with_options() {
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
fn test_build_head_config_elements() {
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
fn test_build_head_config_full() {
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
fn test_build_head_config_defaults() {
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
fn test_serve_config() {
    let config = r#"
        [base]
        title = "Test"
        description = "Test blog"

        [serve]
        interface = "0.0.0.0"
        port = 8080
        watch = false
    "#;
    let config: SiteConfig = toml::from_str(config).unwrap();

    assert_eq!(config.serve.interface, "0.0.0.0");
    assert_eq!(config.serve.port, 8080);
    assert!(!config.serve.watch);
}

#[test]
fn test_serve_config_defaults() {
    let config = r#"
        [base]
        title = "Test"
        description = "Test blog"
    "#;
    let config: SiteConfig = toml::from_str(config).unwrap();

    assert_eq!(config.serve.interface, "127.0.0.1");
    assert_eq!(config.serve.port, 5277);
    assert!(config.serve.watch);
}

#[test]
fn test_deploy_config() {
    let config = r#"
        [base]
        title = "Test"
        description = "Test blog"

        [deploy]
        provider = "github"
        force = true

        [deploy.github]
        url = "https://github.com/user/user.github.io"
        branch = "gh-pages"
        token_path = "~/.github-token"
    "#;
    let config: SiteConfig = toml::from_str(config).unwrap();

    assert_eq!(config.deploy.provider, "github");
    assert!(config.deploy.force);
    assert_eq!(
        config.deploy.github_provider.url,
        "https://github.com/user/user.github.io"
    );
    assert_eq!(config.deploy.github_provider.branch, "gh-pages");
    assert_eq!(
        config.deploy.github_provider.token_path,
        Some(PathBuf::from("~/.github-token"))
    );
}

#[test]
fn test_extra_fields() {
    let config = r#"
        [base]
        title = "Test"
        description = "Test blog"

        [extra]
        custom_field = "custom_value"
        number_field = 42
        nested = { key = "value" }
    "#;
    let config: SiteConfig = toml::from_str(config).unwrap();

    assert_eq!(
        config.extra.get("custom_field").and_then(|v| v.as_str()),
        Some("custom_value")
    );
    assert_eq!(
        config.extra.get("number_field").and_then(|v| v.as_integer()),
        Some(42)
    );
}

#[test]
fn test_unknown_field_rejection_in_base() {
    let config = r#"
        [base]
        title = "Test"
        description = "Test blog"
        unknown_field = "should_fail"
    "#;
    let result: Result<SiteConfig, _> = toml::from_str(config);

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("unknown field"));
}

#[test]
fn test_unknown_field_rejection_in_build() {
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
fn test_unknown_field_rejection_in_serve() {
    let config = r#"
        [base]
        title = "Test"
        description = "Test blog"

        [serve]
        unknown_field = "should_fail"
    "#;
    let result: Result<SiteConfig, _> = toml::from_str(config);

    assert!(result.is_err());
}

#[test]
fn test_get_inline_max_size_kb() {
    let config = r#"
        [base]
        title = "Test"
        description = "Test blog"

        [build.typst.svg]
        inline_max_size = "20KB"
    "#;
    let config: SiteConfig = toml::from_str(config).unwrap();

    assert_eq!(config.get_inline_max_size(), 20 * 1024);
}

#[test]
fn test_get_inline_max_size_mb() {
    let config = r#"
        [base]
        title = "Test"
        description = "Test blog"

        [build.typst.svg]
        inline_max_size = "5MB"
    "#;
    let config: SiteConfig = toml::from_str(config).unwrap();

    assert_eq!(config.get_inline_max_size(), 5 * 1024 * 1024);
}

#[test]
fn test_get_inline_max_size_bytes() {
    let config = r#"
        [base]
        title = "Test"
        description = "Test blog"

        [build.typst.svg]
        inline_max_size = "100B"
    "#;
    let config: SiteConfig = toml::from_str(config).unwrap();

    assert_eq!(config.get_inline_max_size(), 100);
}

#[test]
fn test_get_scale_default_dpi() {
    let config = r#"
        [base]
        title = "Test"
        description = "Test blog"
    "#;
    let config: SiteConfig = toml::from_str(config).unwrap();

    // Default DPI is 96, so scale should be 1.0
    assert_eq!(config.get_scale(), 1.0);
}

#[test]
fn test_get_scale_custom_dpi() {
    let config = r#"
        [base]
        title = "Test"
        description = "Test blog"

        [build.typst.svg]
        dpi = 192.0
    "#;
    let config: SiteConfig = toml::from_str(config).unwrap();

    // 192 / 96 = 2.0
    assert_eq!(config.get_scale(), 2.0);
}

#[test]
fn test_slug_mode_parsing() {
    let config_on = r#"
        [base]
        title = "Test"
        description = "Test blog"

        [build.slug]
        path = "on"
        fragment = "on"
    "#;
    let config: SiteConfig = toml::from_str(config_on).unwrap();
    assert!(matches!(config.build.slug.path, SlugMode::On));
    assert!(matches!(config.build.slug.fragment, SlugMode::On));

    let config_safe = r#"
        [base]
        title = "Test"
        description = "Test blog"

        [build.slug]
        path = "safe"
        fragment = "safe"
    "#;
    let config: SiteConfig = toml::from_str(config_safe).unwrap();
    assert!(matches!(config.build.slug.path, SlugMode::Safe));
    assert!(matches!(config.build.slug.fragment, SlugMode::Safe));

    let config_no = r#"
        [base]
        title = "Test"
        description = "Test blog"

        [build.slug]
        path = "no"
        fragment = "no"
    "#;
    let config: SiteConfig = toml::from_str(config_no).unwrap();
    assert!(matches!(config.build.slug.path, SlugMode::No));
    assert!(matches!(config.build.slug.fragment, SlugMode::No));
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
            description = "Test blog"

            [build.typst.svg]
            extract_type = "{}"
            "#,
            str_type
        );
        let config: SiteConfig = toml::from_str(&config).unwrap();

        assert!(
            std::mem::discriminant(&config.build.typst.svg.extract_type)
                == std::mem::discriminant(&expected),
            "Failed for type: {}",
            str_type
        );
    }
}

#[test]
fn test_from_str() {
    let config_str = r#"
        [base]
        title = "My Blog"
        description = "A test blog"
        author = "Test Author"
    "#;
    let result = SiteConfig::from_str(config_str);

    assert!(result.is_ok());
    let config = result.unwrap();
    assert_eq!(config.base.title, "My Blog");
    assert_eq!(config.base.author, "Test Author");
}

#[test]
fn test_from_str_invalid_toml() {
    let invalid_config = r#"
        [base
        title = "My Blog"
    "#;
    let result = SiteConfig::from_str(invalid_config);

    assert!(result.is_err());
}

#[test]
fn test_get_root_default() {
    let config = SiteConfig::default();
    assert_eq!(config.get_root(), Path::new("./"));
}

#[test]
fn test_set_root() {
    let mut config = SiteConfig::default();
    config.set_root(Path::new("/custom/path"));
    assert_eq!(config.get_root(), Path::new("/custom/path"));
}

#[test]
fn test_config_error_display() {
    let io_err = ConfigError::Io(
        PathBuf::from("test.toml"),
        std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"),
    );
    let display = format!("{}", io_err);
    assert!(display.contains("IO error"));
    assert!(display.contains("test.toml"));

    let validation_err = ConfigError::Validation("Test validation error".to_string());
    let display = format!("{}", validation_err);
    assert!(display.contains("Test validation error"));
}

/// `[build]` section in tola.toml
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(default, deny_unknown_fields)]
pub struct BuildConfig {
    /// Root directory path
    #[serde(default = "config_defaults::build::root")]
    #[educe(Default = config_defaults::build::root())]
    pub root: Option<PathBuf>,

    /// Base path for URLs, e.g.: "myblog"
    #[serde(default = "config_defaults::build::base_path")]
    #[educe(Default = config_defaults::build::base_path())]
    pub base_path: PathBuf,

    /// Content directory path (relative to root)
    #[serde(default = "config_defaults::build::content")]
    #[educe(Default = config_defaults::build::content())]
    pub content: PathBuf,

    /// Output directory path (relative to root)
    #[serde(default = "config_defaults::build::output")]
    #[educe(Default = config_defaults::build::output())]
    pub output: PathBuf,

    /// Assets directory path (relative to root)
    #[serde(default = "config_defaults::build::assets")]
    #[educe(Default = config_defaults::build::assets())]
    pub assets: PathBuf,

    /// Minify HTML output
    #[serde(default = "config_defaults::r#true")]
    #[educe(Default = true)]
    pub minify: bool,

    /// Clear output directory before building
    #[serde(default = "config_defaults::r#false")]
    #[educe(Default = false)]
    pub clear: bool,

    /// RSS feed configuration
    #[serde(default)]
    pub rss: RssConfig,

    /// URL slugification settings
    #[serde(default)]
    pub slug: SlugConfig,

    /// Typst compiler configuration
    #[serde(default)]
    pub typst: TypstConfig,

    /// Tailwind CSS configuration
    #[serde(default)]
    pub tailwind: TailwindConfig,

    /// Custom head elements configuration
    #[serde(default)]
    pub head: HeadConfig,
}

/// `[build.rss]` section
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct RssConfig {
    /// Enable RSS feed generation
    #[serde(default = "config_defaults::r#false")]
    #[educe(Default = config_defaults::r#false())]
    pub enable: bool,

    /// Output path for RSS feed file
    #[serde(default = "config_defaults::build::rss::path")]
    #[educe(Default = config_defaults::build::rss::path())]
    pub path: PathBuf,
}

/// `[build.slug]` section
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct SlugConfig {
    /// Slugify URL paths
    #[serde(default = "config_defaults::build::slug::default")]
    #[educe(Default = config_defaults::build::slug::default())]
    pub path: SlugMode,

    /// Slugify URL fragments (anchors)
    #[serde(default = "config_defaults::build::slug::on")]
    #[educe(Default = config_defaults::build::slug::on())]
    pub fragment: SlugMode,
}

/// `[build.typst]` section
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct TypstConfig {
    /// Typst command and arguments
    #[serde(default = "config_defaults::build::typst::command")]
    #[educe(Default = config_defaults::build::typst::command())]
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
    #[serde(default = "config_defaults::build::typst::svg::extract_type")]
    #[educe(Default = config_defaults::build::typst::svg::extract_type())]
    pub extract_type: ExtractSvgType,

    /// Max size for inline SVG (e.g.: "20KB", "1MB")
    #[serde(default = "config_defaults::build::typst::svg::inline_max_size")]
    #[educe(Default = config_defaults::build::typst::svg::inline_max_size())]
    pub inline_max_size: String,

    /// DPI for SVG rendering
    #[serde(default = "config_defaults::build::typst::svg::dpi")]
    #[educe(Default = config_defaults::build::typst::svg::dpi())]
    pub dpi: f32,
}

/// `[build.tailwind]` section
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct TailwindConfig {
    /// Enable Tailwind CSS processing
    #[serde(default = "config_defaults::r#false")]
    #[educe(Default = false)]
    pub enable: bool,

    /// Input CSS file path
    #[serde(default = "config_defaults::build::tailwind::input")]
    #[educe(Default = config_defaults::build::tailwind::input())]
    pub input: Option<PathBuf>,

    /// Tailwind command and arguments
    #[serde(default = "config_defaults::build::tailwind::command")]
    #[educe(Default = config_defaults::build::tailwind::command())]
    pub command: Vec<String>,
}

/// Script entry for `[build.head.scripts]`
/// Can be either a simple path string or an object with path and defer/async options
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScriptEntry {
    /// Simple path string
    Simple(PathBuf),
    /// Object with path and optional defer/async
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

/// `[serve]` section in tola.toml
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct ServeConfig {
    /// Network interface to bind (e.g.: "127.0.0.1", "0.0.0.0")
    #[serde(default = "config_defaults::serve::interface")]
    #[educe(Default = config_defaults::serve::interface())]
    pub interface: String,

    /// Port number to listen on
    #[serde(default = "config_defaults::serve::port")]
    #[educe(Default = config_defaults::serve::port())]
    pub port: u16,

    /// Enable file watching for live reload
    #[serde(default = "config_defaults::r#true")]
    #[educe(Default = true)]
    pub watch: bool,
}

/// `[deploy]` section in tola.toml
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct DeployConfig {
    /// Deployment provider (e.g.: "github")
    #[serde(default = "config_defaults::deploy::provider")]
    #[educe(Default = config_defaults::deploy::provider())]
    pub provider: String,

    /// Force push to remote
    #[serde(default = "config_defaults::r#false")]
    #[educe(Default = config_defaults::r#false())]
    pub force: bool,

    /// GitHub Pages configuration
    #[serde(rename = "github", default)]
    pub github_provider: GithubProvider,

    /// Cloudflare Pages configuration
    #[serde(rename = "cloudflare", default)]
    pub cloudflare_provider: CloudflareProvider,

    /// Vercel configuration
    #[serde(rename = "vercal", default)]
    pub vercal_provider: VercalProvider,
}

/// `[deploy.github]` section
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct GithubProvider {
    /// Repository URL
    #[serde(default = "config_defaults::deploy::github::url")]
    #[educe(Default = config_defaults::deploy::github::url())]
    pub url: String,

    /// Branch to push to
    #[serde(default = "config_defaults::deploy::github::branch")]
    #[educe(Default = config_defaults::deploy::github::branch())]
    pub branch: String,

    /// Path to file containing GitHub token
    /// WARNING: Never commit this token to a public repository!
    #[serde(default = "config_defaults::deploy::github::token_path")]
    #[educe(Default = config_defaults::deploy::github::token_path())]
    pub token_path: Option<PathBuf>,
}

/// `[deploy.cloudflare]` section (placeholder)
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct CloudflareProvider {
    /// Provider identifier
    #[serde(default = "config_defaults::deploy::provider")]
    #[educe(Default = config_defaults::deploy::provider())]
    pub provider: String,
}

/// `[deploy.vercal]` section (placeholder)
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct VercalProvider {
    /// Provider identifier
    #[serde(default = "config_defaults::deploy::provider")]
    #[educe(Default = config_defaults::deploy::provider())]
    pub provider: String,
}

/// Root configuration structure representing tola.toml
#[derive(Debug, Clone, Educe, Serialize, Deserialize)]
#[educe(Default)]
#[serde(deny_unknown_fields)]
pub struct SiteConfig {
    /// CLI arguments reference
    #[serde(skip)]
    pub cli: Option<&'static Cli>,

    /// Basic site information
    #[serde(default)]
    pub base: BaseConfig,

    /// Build settings
    #[serde(default)]
    pub build: BuildConfig,

    /// Development server settings
    #[serde(default)]
    pub serve: ServeConfig,

    /// Deployment settings
    #[serde(default)]
    pub deploy: DeployConfig,

    /// User-defined extra fields
    #[serde(default)]
    pub extra: HashMap<String, toml::Value>,
}

impl SiteConfig {
    /// Parse configuration from TOML string
    pub fn from_str(content: &str) -> Result<Self> {
        let config: SiteConfig = toml::from_str(content)?;
        Ok(config)
    }

    /// Load configuration from file path
    pub fn from_path(path: &Path) -> Result<Self> {
        let content =
            fs::read_to_string(path).map_err(|err| ConfigError::Io(path.to_path_buf(), err))?;
        Self::from_str(&content)
    }

    /// Get the root directory path
    pub fn get_root(&self) -> &Path {
        self.build.root.as_deref().unwrap_or(Path::new("./"))
    }

    /// Set the root directory path
    pub fn set_root(&mut self, path: &Path) {
        self.build.root = Some(path.to_path_buf())
    }

    /// Get CLI arguments reference
    pub fn get_cli(&self) -> &'static Cli {
        self.cli.unwrap()
    }

    /// Parse inline_max_size string (e.g., "20KB") to bytes
    pub fn get_inline_max_size(&self) -> usize {
        let size_str = &self.build.typst.svg.inline_max_size;
        let multiplier = if size_str.ends_with("MB") {
            1024 * 1024
        } else if size_str.ends_with("KB") {
            1024
        } else {
            1
        };
        let value: usize = size_str
            .trim_end_matches(|c: char| c.is_ascii_uppercase())
            .parse()
            .unwrap_or(0);
        multiplier * value
    }

    /// Get DPI scale factor (relative to 96 DPI)
    pub fn get_scale(&self) -> f32 {
        self.build.typst.svg.dpi / 96.0
    }

    /// Update configuration with CLI arguments
    pub fn update_with_cli(&mut self, cli: &'static Cli) {
        self.cli = Some(cli);

        let root = cli.root.as_ref().cloned().unwrap_or_else(|| self.get_root().to_owned());
        self.set_root(&root);
        self.update_path_with_root(&root);

        Self::update_option(&mut self.build.minify, cli.minify.as_ref());
        Self::update_option(&mut self.build.tailwind.enable, cli.tailwind.as_ref());

        self.build.typst.svg.inline_max_size = self.build.typst.svg.inline_max_size.to_uppercase();

        match &cli.command {
            Commands::Init { name: Some(name) } => {
                let new_root = self.build.root.as_ref().map_or_else(
                    || name.clone(),
                    |r| r.join(name),
                );
                self.update_path_with_root(&new_root);
            }
            Commands::Serve { interface, port, watch } => {
                Self::update_option(&mut self.serve.interface, interface.as_ref());
                Self::update_option(&mut self.serve.port, port.as_ref());
                Self::update_option(&mut self.serve.watch, watch.as_ref());
                self.base.url = Some(format!("http://{}:{}", self.serve.interface, self.serve.port));
            }
            Commands::Deploy { force } => {
                Self::update_option(&mut self.deploy.force, force.as_ref());
            }
            _ => {}
        }
    }

    /// Update config option if CLI value is provided
    fn update_option<T: Clone>(config_option: &mut T, cli_option: Option<&T>) {
        if let Some(option) = cli_option {
            *config_option = option.clone();
        }
    }

    /// Update all paths relative to root directory
    fn update_path_with_root(&mut self, root: &Path) {
        let cli = self.get_cli();

        self.set_root(root);
        Self::update_option(&mut self.build.content, cli.content.as_ref());
        Self::update_option(&mut self.build.assets, cli.assets.as_ref());
        Self::update_option(&mut self.build.output, cli.output.as_ref());

        self.build.content = root.join(&self.build.content);
        self.build.assets = root.join(&self.build.assets);
        self.build.output = root.join(&self.build.output);
        self.build.rss.path = self.build.output.join(&self.build.rss.path);

        if self.build.tailwind.enable
            && let Some(input) = self.build.tailwind.input.as_ref()
        {
            self.build.tailwind.input.replace(root.join(input));
        }

        if let Some(token_path) = &self.deploy.github_provider.token_path {
            let path = shellexpand::tilde(token_path.to_str().unwrap()).into_owned();
            let path = PathBuf::from(path);
            self.deploy.github_provider.token_path = if path.is_relative() {
                Some(root.join(path))
            } else {
                Some(path.to_owned())
            };
        }
    }

    /// Validate configuration for the current command
    #[allow(unused)]
    pub fn validate(&self) -> Result<()> {
        let cli = self.get_cli();

        if !self.get_root().join(&cli.config).exists() {
            bail!("Config file not found");
        }

        if self.build.rss.enable && self.base.url.is_none() {
            bail!("[base.url] is required for RSS generation");
        }

        Self::check_command_installed("[build.typst.command]", &self.build.typst.command)?;

        if let Some(base_url) = &self.base.url
            && !base_url.starts_with("http")
        {
            bail!(ConfigError::Validation(
                "[base.url] must start with http:// or https://".into()
            ));
        }

        if self.build.tailwind.enable {
            Self::check_command_installed("[build.tailwind.command]", &self.build.tailwind.command)?;

            match &self.build.tailwind.input {
                None => bail!(
                    "[build.tailwind.enable] = true requires [build.tailwind.input] to be set"
                ),
                Some(path) if !path.exists() => {
                    bail!(ConfigError::Validation("[build.tailwind.input] not found".into()))
                }
                Some(path) if !path.is_file() => {
                    bail!(ConfigError::Validation("[build.tailwind.input] is not a file".into()))
                }
                _ => {}
            }
        }

        let valid_size_suffixes = ["B", "KB", "MB"];
        if !valid_size_suffixes.iter().any(|s| self.build.typst.svg.inline_max_size.ends_with(s)) {
            bail!(ConfigError::Validation(
                "[build.typst.svg.inline_max_size] must end with B, KB, or MB".into()
            ));
        }

        match &cli.command {
            Commands::Init { .. } if self.get_root().exists() => {
                bail!("Path already exists");
            }
            Commands::Deploy { .. } => {
                if let Some(path) = &self.deploy.github_provider.token_path {
                    if !path.exists() {
                        bail!(ConfigError::Validation("[deploy.github.token_path] not found".into()));
                    }
                    if !path.is_file() {
                        bail!(ConfigError::Validation("[deploy.github.token_path] is not a file".into()));
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Check if a command is installed and available
    fn check_command_installed(field: &str, command: &[String]) -> Result<()> {
        if command.is_empty() {
            bail!(ConfigError::Validation(format!(
                "{field} must have at least one element"
            )));
        }

        let cmd = &command[0];
        which::which(cmd)
            .with_context(|| format!("`{cmd}` not found. Please install it first."))?;

        Ok(())
    }
}
