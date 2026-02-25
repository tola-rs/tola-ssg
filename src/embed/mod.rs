//! Embedded static resources for Tola.
//!
//! # Module Structure
//!
//! - `template` - Template types for typed variable injection
//! - `asset` - Embedded asset types with content-hash filenames
//! - `build` - Build-time templates (redirect.html)
//! - `serve` - Dev server templates (welcome.html, hotreload.js)
//! - `css` - Embedded stylesheets (enhance.css)
//!
//! Typst virtual packages (@tola/*) are in `src/package/embed/`.
//!
//! # Usage
//!
//! ```ignore
//! use embed::build::{REDIRECT_HTML, RedirectVars};
//! use embed::serve::{HOTRELOAD_JS, HotreloadVars};
//! use embed::css::ENHANCE_CSS;
//!
//! // Render redirect template
//! let html = REDIRECT_HTML.render(&RedirectVars { canonical_url: "/new-url/" });
//!
//! // Render hotreload JS with port
//! let js = HOTRELOAD_JS.render(&HotreloadVars { ws_port: 35729 });
//! ```

mod asset;
mod template;

// Re-export core types
pub use asset::{AssetKind, EmbeddedAsset};
pub use template::{Template, TemplateVars};

pub mod build {
    use super::{AssetKind, EmbeddedAsset, Template, TemplateVars};
    use crate::config::SiteConfig;

    /// Variables for redirect.html template.
    pub struct RedirectVars<'a> {
        pub canonical_url: &'a str,
    }

    impl TemplateVars for RedirectVars<'_> {
        fn apply(&self, content: &str) -> String {
            content.replace("__CANONICAL_URL__", self.canonical_url)
        }
    }

    /// Redirect HTML template for alias pages.
    pub const REDIRECT_HTML: Template<RedirectVars<'static>> =
        Template::new(include_str!("build/redirect.html"));

    /// Variables for spa.js template.
    pub struct SpaVars {
        pub transition: bool,
        pub preload: bool,
        pub preload_delay: u32,
        pub path_prefix: String,
    }

    impl SpaVars {
        /// Build SPA runtime variables from site config.
        pub fn from_config(config: &SiteConfig) -> Self {
            let nav = &config.site.nav;
            Self {
                transition: nav.transition.is_enabled(),
                preload: nav.preload.enable,
                preload_delay: nav.preload.delay,
                path_prefix: normalize_path_prefix(&config.build.path_prefix),
            }
        }
    }

    /// Normalize config path_prefix to URL path format.
    ///
    /// Examples:
    /// - "" -> ""
    /// - "blog" -> "/blog"
    /// - "a/b" -> "/a/b"
    fn normalize_path_prefix(path: &std::path::Path) -> String {
        let parts: Vec<_> = path
            .iter()
            .filter_map(|c| c.to_str())
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            String::new()
        } else {
            format!("/{}", parts.join("/"))
        }
    }

    impl TemplateVars for SpaVars {
        fn apply(&self, content: &str) -> String {
            content
                .replace(
                    "__TOLA_TRANSITION__",
                    if self.transition { "true" } else { "false" },
                )
                .replace(
                    "__TOLA_PRELOAD__",
                    if self.preload { "true" } else { "false" },
                )
                .replace("__TOLA_PRELOAD_DELAY__", &self.preload_delay.to_string())
                .replace(
                    "__TOLA_PATH_PREFIX__",
                    &serde_json::to_string(&self.path_prefix).unwrap_or_else(|_| "\"\"".into()),
                )
        }

        fn hash_input(&self) -> String {
            format!(
                "{}{}{}{}",
                self.transition, self.preload, self.preload_delay, self.path_prefix
            )
        }
    }

    /// SPA navigation JavaScript with configuration injection.
    pub const SPA_JS: EmbeddedAsset<SpaVars> = EmbeddedAsset::new(
        AssetKind::JavaScript,
        "spa",
        include_str!(concat!(env!("OUT_DIR"), "/spa.min.js")),
    );
}

pub mod serve {
    use super::{AssetKind, EmbeddedAsset, Template, TemplateVars};

    /// Variables for hotreload.js.
    pub struct HotreloadVars {
        pub ws_port: u16,
    }

    impl TemplateVars for HotreloadVars {
        fn apply(&self, content: &str) -> String {
            content.replace("__TOLA_WS_PORT__", &self.ws_port.to_string())
        }

        fn hash_input(&self) -> String {
            self.ws_port.to_string()
        }
    }

    /// Variables for welcome.html.
    pub struct WelcomeVars<'a> {
        pub title: &'a str,
        pub version: &'a str,
    }

    impl TemplateVars for WelcomeVars<'_> {
        fn apply(&self, content: &str) -> String {
            content
                .replace("__TITLE__", self.title)
                .replace("__VERSION__", self.version)
        }
    }

    /// Welcome page template.
    pub const WELCOME_HTML: Template<WelcomeVars<'static>> =
        Template::new(include_str!(concat!(env!("OUT_DIR"), "/welcome.html")));

    /// Hot reload JavaScript with WebSocket port injection.
    pub const HOTRELOAD_JS: EmbeddedAsset<HotreloadVars> = EmbeddedAsset::new(
        AssetKind::JavaScript,
        "hotreload",
        include_str!(concat!(env!("OUT_DIR"), "/hotreload.min.js")),
    );
}

pub mod css {
    use super::{AssetKind, EmbeddedAsset, TemplateVars};
    use crate::config::section::site::TransitionStyle;

    /// Typst CSS for SVG color adaptation and math/table layout.
    const TYPST_CSS: &str = include_str!("css/typst.css");

    /// Nav CSS template for View Transitions.
    const NAV_CSS_FADE: &str = include_str!("css/nav/fade.css");

    /// Variables for nav.css sub-template (from site.nav config).
    #[derive(Clone)]
    pub struct NavVars {
        /// Transition style.
        pub style: TransitionStyle,
        /// View Transitions duration in milliseconds (site.nav.transition.time).
        pub transition_time: u32,
    }

    impl NavVars {
        /// Generate nav CSS content (empty if disabled).
        pub fn render(&self) -> String {
            match self.style {
                TransitionStyle::None => String::new(),
                TransitionStyle::Fade => {
                    NAV_CSS_FADE.replace("__TRANSITION_TIME__", &self.transition_time.to_string())
                }
            }
        }

        /// Whether View Transitions are enabled.
        pub fn is_enabled(&self) -> bool {
            self.style != TransitionStyle::None
        }
    }

    /// Variables for enhance.css template.
    #[derive(Clone)]
    pub struct EnhanceVars {
        pub nav: NavVars,
    }

    impl TemplateVars for EnhanceVars {
        fn apply(&self, content: &str) -> String {
            content
                .replace("/*! TYPST_CSS */", TYPST_CSS)
                .replace("/*! NAV_CSS */", &self.nav.render())
        }

        fn hash_input(&self) -> String {
            format!("{:?}{}", self.nav.style, self.nav.transition_time)
        }
    }

    /// Build EnhanceVars from SiteConfig.
    pub fn enhance_vars(config: &crate::config::SiteConfig) -> EnhanceVars {
        EnhanceVars {
            nav: NavVars {
                style: config.site.nav.transition.style,
                transition_time: config.site.nav.transition.time,
            },
        }
    }

    /// Enhanced CSS for Typst SVG theme adaptation and View Transitions.
    pub const ENHANCE_CSS: EmbeddedAsset<EnhanceVars> =
        EmbeddedAsset::new(AssetKind::Css, "enhance", include_str!("css/enhance.css"));
}

pub mod typst {
    use super::{Template, TemplateVars};

    /// Variables for tola.typ templates.
    pub struct TolaTypstVars {
        pub version: &'static str,
    }

    impl Default for TolaTypstVars {
        fn default() -> Self {
            Self {
                version: env!("CARGO_PKG_VERSION"),
            }
        }
    }

    impl TemplateVars for TolaTypstVars {
        fn apply(&self, content: &str) -> String {
            content.replace("__VERSION__", self.version)
        }
    }

    /// Tola template for tola init to generate templates/tola.typ.
    pub const TOLA_TEMPLATE: Template<TolaTypstVars> =
        Template::new(include_str!("typst/templates/tola.typ"));
    /// Tola util for tola init to generate utils/tola.typ.
    pub const TOLA_UTIL: Template<TolaTypstVars> =
        Template::new(include_str!("typst/utils/tola.typ"));
}

pub mod recolor {
    use super::{AssetKind, EmbeddedAsset, TemplateVars};
    use crate::config::section::theme::{RecolorConfig, RecolorSource};
    use std::collections::HashMap;

    /// SVG filter template for dynamic mode.
    pub const FILTER_SVG: &str = include_str!("recolor/filter.svg");

    /// CSS template (unified for both modes).
    const RECOLOR_CSS_TEMPLATE: &str = include_str!("recolor/recolor.css");

    /// JS template for dynamic mode.
    const RECOLOR_JS_TEMPLATE: &str = include_str!("recolor/recolor.js");

    /// Variables for recolor.js (dynamic mode).
    #[derive(Clone)]
    pub struct RecolorJsVars {
        /// Source mode: "auto" or CSS variable name like "--text-color".
        pub source: String,
    }

    impl TemplateVars for RecolorJsVars {
        fn apply(&self, content: &str) -> String {
            let quoted = format!("\"{}\"", self.source);
            content.replace("__TOLA_RECOLOR_SOURCE__", &quoted)
        }

        fn hash_input(&self) -> String {
            self.source.clone()
        }
    }

    /// Recolor JS for dynamic mode.
    pub const RECOLOR_JS: EmbeddedAsset<RecolorJsVars> =
        EmbeddedAsset::new(AssetKind::JavaScript, "recolor", RECOLOR_JS_TEMPLATE);

    /// Variables for recolor.css.
    #[derive(Clone)]
    pub struct RecolorCssVars {
        /// Static mode variables (empty for dynamic mode).
        pub static_vars: String,
        /// Filter value: `url(#tola-recolor)` or `var(--tola-recolor-filter)`.
        pub filter_value: String,
    }

    impl TemplateVars for RecolorCssVars {
        fn apply(&self, content: &str) -> String {
            content
                .replace("__STATIC_VARS__", &self.static_vars)
                .replace("__FILTER_VALUE__", &self.filter_value)
        }

        fn hash_input(&self) -> String {
            format!("{}{}", self.static_vars, self.filter_value)
        }
    }

    /// Recolor CSS.
    pub const RECOLOR_CSS: EmbeddedAsset<RecolorCssVars> =
        EmbeddedAsset::new(AssetKind::Css, "recolor", RECOLOR_CSS_TEMPLATE);

    /// Parse hex color to RGB values (0.0-1.0).
    pub fn parse_hex_color(hex: &str) -> Option<(f32, f32, f32)> {
        let hex = hex.trim_start_matches('#');
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()? as f32 / 255.0;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()? as f32 / 255.0;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()? as f32 / 255.0;
        Some((r, g, b))
    }

    /// Generate SVG filter for a specific color.
    /// Uses luminance-based switching: black->target, white->black or white.
    pub fn generate_filter(id: &str, hex: &str) -> Option<String> {
        let (r, g, b) = parse_hex_color(hex)?;

        // Luminance-based B value: light target -> white becomes black
        let target_lum = 0.299 * r + 0.587 * g + 0.114 * b;
        let b_val = if target_lum > 0.5 { 0.0 } else { 1.0 };

        Some(format!(
            r#"<filter id="{id}" color-interpolation-filters="sRGB">
      <feColorMatrix type="matrix" values=".33 .33 .33 0 0
                                           .33 .33 .33 0 0
                                           .33 .33 .33 0 0
                                            0   0   0  1 0"/>
      <feComponentTransfer>
        <feFuncR type="table" tableValues="{r:.3} {b_val:.3}"/>
        <feFuncG type="table" tableValues="{g:.3} {b_val:.3}"/>
        <feFuncB type="table" tableValues="{b:.3} {b_val:.3}"/>
      </feComponentTransfer>
    </filter>"#
        ))
    }

    /// Generate static mode SVG with all theme filters.
    pub fn generate_static_svg(list: &HashMap<String, String>) -> String {
        let mut filters = Vec::new();
        for (name, color) in list {
            if let Some(filter) = generate_filter(&format!("tola-recolor-{name}"), color) {
                filters.push(filter);
            }
        }
        format!(
            r#"<svg style="display:none" aria-hidden="true">
  <defs>
    {}
  </defs>
</svg>"#,
            filters.join("\n    ")
        )
    }

    /// Generate static mode CSS variables.
    fn generate_static_vars(list: &HashMap<String, String>) -> String {
        let theme_overrides: Vec<_> = list
            .keys()
            .map(|name| {
                format!(
                    "[data-theme=\"{name}\"] {{ --tola-recolor-filter: url(#tola-recolor-{name}); }}"
                )
            })
            .collect();

        format!(
            r#":root {{
  --tola-recolor-filter: url(#tola-recolor-light);
}}

@media (prefers-color-scheme: dark) {{
  :root {{
    --tola-recolor-filter: url(#tola-recolor-dark);
  }}
}}

{}"#,
            theme_overrides.join("\n")
        )
    }

    /// Build RecolorCssVars from config.
    pub fn css_vars(config: &RecolorConfig) -> RecolorCssVars {
        match &config.source {
            RecolorSource::Static => RecolorCssVars {
                static_vars: generate_static_vars(&config.list),
                filter_value: "var(--tola-recolor-filter)".to_string(),
            },
            _ => RecolorCssVars {
                static_vars: String::new(),
                filter_value: "url(#tola-recolor)".to_string(),
            },
        }
    }

    /// Build RecolorJsVars from config (for dynamic mode).
    pub fn js_vars(config: &RecolorConfig) -> RecolorJsVars {
        let source = match &config.source {
            RecolorSource::Auto => "auto".to_string(),
            RecolorSource::CssVar(var) => var.clone(),
            RecolorSource::Static => "auto".to_string(), // Should not happen
        };
        RecolorJsVars { source }
    }
}

// =============================================================================
// Embedded Assets Writer
// =============================================================================

use crate::config::SiteConfig;
use anyhow::Result;
use std::path::Path;

/// Write all embedded assets to output directory
///
/// This centralizes the logic for writing config-dependent embedded assets:
/// - enhance.css (always)
/// - spa.js (if site.nav.spa)
/// - recolor.css + recolor.js (if theme.recolor.enable)
pub fn write_embedded_assets(config: &SiteConfig, output_dir: &Path) -> Result<()> {
    // Ensure output directory exists
    std::fs::create_dir_all(output_dir)?;

    // enhance.css (always written)
    {
        use css::{ENHANCE_CSS, enhance_vars};
        let vars = enhance_vars(config);
        ENHANCE_CSS.cleanup_old(output_dir)?;
        ENHANCE_CSS.write_with_vars(output_dir, &vars)?;
    }

    // spa.js (if spa enabled)
    if config.site.nav.spa {
        use build::{SPA_JS, SpaVars};

        let vars = SpaVars::from_config(config);

        SPA_JS.cleanup_old(output_dir)?;
        SPA_JS.write_with_vars(output_dir, &vars)?;
    }

    // recolor assets (if enabled)
    if config.theme.recolor.enable {
        use recolor::{RECOLOR_CSS, RECOLOR_JS, css_vars, js_vars};
        let recolor_config = &config.theme.recolor;

        RECOLOR_CSS.cleanup_old(output_dir)?;
        RECOLOR_CSS.write_with_vars(output_dir, &css_vars(recolor_config))?;

        // JS only needed for dynamic mode (auto or css-var)
        if !matches!(
            recolor_config.source,
            crate::config::section::theme::RecolorSource::Static
        ) {
            RECOLOR_JS.cleanup_old(output_dir)?;
            RECOLOR_JS.write_with_vars(output_dir, &js_vars(recolor_config))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_css_asset() {
        use crate::config::section::site::TransitionStyle;
        let vars = css::EnhanceVars {
            nav: css::NavVars {
                style: TransitionStyle::Fade,
                transition_time: 200,
            },
        };
        let rendered = css::ENHANCE_CSS.render(&vars);
        assert!(rendered.contains("200ms"));
        assert!(!rendered.contains("__NAV_CSS__"));
        assert!(!rendered.contains("__TRANSITION_TIME__"));
        // Has variables -> has hash
        let url = css::ENHANCE_CSS.url_path_with_vars(&vars);
        assert!(url.starts_with("/.tola/enhance-"));
        assert!(url.ends_with(".css"));
    }

    #[test]
    fn test_css_asset_nav_disabled() {
        use crate::config::section::site::TransitionStyle;
        let vars = css::EnhanceVars {
            nav: css::NavVars {
                style: TransitionStyle::None,
                transition_time: 200,
            },
        };
        let rendered = css::ENHANCE_CSS.render(&vars);
        // Nav disabled -> no view-transition CSS
        assert!(!rendered.contains("view-transition"));
        assert!(!rendered.contains("__NAV_CSS__"));
    }

    #[test]
    fn test_hotreload_js_with_vars() {
        let vars = serve::HotreloadVars { ws_port: 35729 };
        let rendered = serve::HOTRELOAD_JS.render(&vars);
        assert!(rendered.contains("35729"));
        assert!(!rendered.contains("__TOLA_WS_PORT__"));
        // Has variables -> has hash
        let url = serve::HOTRELOAD_JS.url_path_with_vars(&vars);
        assert!(url.starts_with("/.tola/hotreload-"));
        assert!(url.ends_with(".js"));
    }

    #[test]
    fn test_spa_js_with_vars() {
        let vars = build::SpaVars {
            transition: true,
            preload: true,
            preload_delay: 150,
            path_prefix: "/blog".to_string(),
        };
        let rendered = build::SPA_JS.render(&vars);
        assert!(rendered.contains("150"));
        assert!(rendered.contains("/blog"));
        assert!(!rendered.contains("__TOLA_TRANSITION__"));
        assert!(!rendered.contains("__TOLA_PRELOAD__"));
        assert!(!rendered.contains("__TOLA_PRELOAD_DELAY__"));
        assert!(!rendered.contains("__TOLA_PATH_PREFIX__"));
        // Has variables -> has hash
        let url = build::SPA_JS.url_path_with_vars(&vars);
        assert!(url.starts_with("/.tola/spa-"));
        assert!(url.ends_with(".js"));
    }

    #[test]
    fn test_spa_vars_from_config_path_prefix() {
        let mut config = crate::config::SiteConfig::default();
        config.build.path_prefix = std::path::PathBuf::from("docs/blog");
        let vars = build::SpaVars::from_config(&config);
        assert_eq!(vars.path_prefix, "/docs/blog");
    }

    #[test]
    fn test_redirect_template() {
        let vars = build::RedirectVars {
            canonical_url: "/new-page/",
        };
        let html = build::REDIRECT_HTML.render(&vars);
        assert!(html.contains("/new-page/"));
        assert!(html.contains("canonical"));
    }

    #[test]
    fn test_recolor_css_dynamic() {
        let vars = recolor::RecolorCssVars {
            static_vars: String::new(),
            filter_value: "url(#tola-recolor)".to_string(),
        };
        let rendered = recolor::RECOLOR_CSS.render(&vars);
        assert!(rendered.contains("url(#tola-recolor)"));
        assert!(!rendered.contains("__FILTER_VALUE__"));
        assert!(!rendered.contains("__STATIC_VARS__"));
    }

    #[test]
    fn test_recolor_css_static() {
        let vars = recolor::RecolorCssVars {
            static_vars: ":root { --tola-recolor-filter: url(#test); }".to_string(),
            filter_value: "var(--tola-recolor-filter)".to_string(),
        };
        let rendered = recolor::RECOLOR_CSS.render(&vars);
        assert!(rendered.contains("var(--tola-recolor-filter)"));
        assert!(rendered.contains(":root"));
        assert!(!rendered.contains("__FILTER_VALUE__"));
    }

    #[test]
    fn test_recolor_parse_hex() {
        let (r, g, b) = recolor::parse_hex_color("#88c0d0").unwrap();
        assert!((r - 0.533).abs() < 0.01);
        assert!((g - 0.753).abs() < 0.01);
        assert!((b - 0.816).abs() < 0.01);
    }

    #[test]
    fn test_recolor_js_vars() {
        let vars = recolor::RecolorJsVars {
            source: "auto".to_string(),
        };
        let rendered = recolor::RECOLOR_JS.render(&vars);
        assert!(rendered.contains("\"auto\""));
        assert!(!rendered.contains("__TOLA_RECOLOR_SOURCE__"));
    }
}
