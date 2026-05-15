//! Head content injector (Raw -> Raw).
//!
//! Injects site-wide `<head>` content from config into Raw VDOM before indexing.
//! Also sets `lang` attribute on `<html>` root if not present.
//!
//! Injected elements: title, description meta, icon link, stylesheets, scripts,
//! Atomic CSS output, auto-enhance CSS, and raw HTML elements.

use std::path::Path;

use tola_vdom::prelude::*;

use crate::asset::{href_for_route, route_from_config_source, version};
use crate::compiler::family::{Raw, TolaSite};
use crate::config::SiteConfig;
use crate::seo::feed;
use crate::utils::mime;

/// Injects site-wide `<head>` content into Raw VDOM
pub struct HeaderInjector<'a> {
    config: &'a SiteConfig,
    /// Whether to inject global header content (styles, scripts, elements).
    /// Default: `true`. Set to `false` for pages like 404 that need
    /// self-contained styles to avoid relative path issues.
    global_header: bool,
}

/// Compute versioned href for an asset (with ?v=hash for cache busting)
fn versioned_href(path: &Path, config: &SiteConfig) -> Option<String> {
    let route = route_from_config_source(path, config).ok()?;
    let href = href_for_route(&route, config);
    Some(version::versioned_url(&href, &route.source))
}

fn feed_title(site_title: &str, feed_label: &str) -> String {
    if site_title.is_empty() {
        feed_label.to_string()
    } else {
        format!("{site_title} ({feed_label})")
    }
}

impl<'a> HeaderInjector<'a> {
    pub fn new(config: &'a SiteConfig) -> Self {
        Self {
            config,
            global_header: true,
        }
    }

    /// Set whether to inject global header content.
    pub fn with_global_header(mut self, global_header: bool) -> Self {
        self.global_header = global_header;
        self
    }

    /// Recursively find and populate `<head>` element.
    fn inject_head(&self, element: &mut Element<Raw>) {
        if element.tag == "head" {
            self.populate_head(element);
            return;
        }

        for child in &mut element.children {
            if let Node::Element(elem) = child {
                self.inject_head(elem);
            }
        }
    }

    /// Populate `<head>` with site configuration content.
    fn populate_head(&self, head: &mut Element<Raw>) {
        let config = self.config;
        let head_config = &config.site.header;
        let existing_len = head.children.len();

        // Anti-FOUC dummy script (must be first to block rendering)
        if head_config.no_fouc {
            let mut script = TolaSite::element("script", Attrs::new());
            script.push_text(" ");
            head.push_elem(script);
        }

        // Title (skip if user already defined one)
        if !config.site.info.title.is_empty() && !Self::has_tag(head, "title") {
            let mut title = TolaSite::element("title", Attrs::new());
            title.push_text(&config.site.info.title);
            head.push_elem(title);
        }

        // Description meta (skip if user already defined one)
        if !config.site.info.description.is_empty() && !Self::has_meta_name(head, "description") {
            let mut attrs = Attrs::new();
            attrs.set("name", "description");
            attrs.set("content", &config.site.info.description);
            head.push_elem(TolaSite::element("meta", attrs));
        }

        for output in config.site.seo.feed_outputs() {
            let mut attrs = Attrs::new();
            attrs.set("rel", "alternate");
            attrs.set("type", output.format.mime_type());
            attrs.set("href", feed::feed_url(config, output));
            attrs.set(
                "title",
                feed_title(&config.site.info.title, output.format.label()),
            );
            head.push_elem(TolaSite::element("link", attrs));
        }

        // Icon
        if let Some(icon) = &head_config.icon
            && let Some(href) = versioned_href(icon, config)
        {
            let mut attrs = Attrs::new();
            attrs.set("rel", "shortcut icon");
            attrs.set("href", href);
            attrs.set("type", mime::for_icon(icon));
            head.push_elem(TolaSite::element("link", attrs));
        }

        // User-defined stylesheets
        for style in &head_config.styles {
            if let Some(href) = versioned_href(style, config) {
                let mut attrs = Attrs::new();
                attrs.set("rel", "stylesheet");
                attrs.set("href", href);
                head.push_elem(TolaSite::element("link", attrs));
            }
        }

        // Native Atomic CSS output.
        if let Some(asset) = crate::css::build::output_asset(config) {
            // The output is generated from all scanned sources, so cache busting
            // must follow the generated file rather than any single input file.
            let href = version::versioned_url(&asset.url, &asset.path);
            let mut attrs = Attrs::new();
            attrs.set("rel", "stylesheet");
            attrs.set("href", href);
            head.push_elem(TolaSite::element("link", attrs));
        }

        // Auto-enhance CSS (SVG theme adaptation + View Transitions)
        {
            use crate::embed::css::{ENHANCE_CSS, enhance_vars};
            let href =
                ENHANCE_CSS.url_path_with_vars(&config.build.path_prefix, &enhance_vars(config));
            let mut attrs = Attrs::new();
            attrs.set("rel", "stylesheet");
            attrs.set("href", href);
            head.push_elem(TolaSite::element("link", attrs));
        }

        // Recolor CSS + JS (if enabled)
        if config.theme.recolor.enable {
            use crate::config::section::theme::RecolorSource;
            use crate::embed::recolor;

            // CSS (always)
            let css_vars = recolor::css_vars(&config.theme.recolor);
            let href =
                recolor::RECOLOR_CSS.url_path_with_vars(&config.build.path_prefix, &css_vars);
            let mut attrs = Attrs::new();
            attrs.set("rel", "stylesheet");
            attrs.set("href", href);
            head.push_elem(TolaSite::element("link", attrs));

            // JS (only for dynamic mode: auto or css-var)
            if !matches!(config.theme.recolor.source, RecolorSource::Static) {
                let js_vars = recolor::js_vars(&config.theme.recolor);
                let src =
                    recolor::RECOLOR_JS.url_path_with_vars(&config.build.path_prefix, &js_vars);
                let mut attrs = Attrs::new();
                attrs.set("src", src);
                attrs.set("defer", "");
                head.push_elem(TolaSite::element("script", attrs));
            }
        }

        // Scripts
        for script in &head_config.scripts {
            if let Some(src) = versioned_href(script.path(), config) {
                let mut attrs = Attrs::new();
                attrs.set("src", src);
                if script.is_defer() {
                    attrs.set("defer", "");
                }
                if script.is_async() {
                    attrs.set("async", "");
                }
                head.push_elem(TolaSite::element("script", attrs));
            }
        }

        // Raw HTML elements (trusted input) - inject as raw text nodes
        for raw in &head_config.elements {
            // Raw HTML is injected as a Text node with TextKind::Raw
            // This allows custom script tags, meta tags, etc. to be rendered unescaped
            head.push(Node::Text(Text::raw(raw.as_str())));
        }

        // Open Graph / Twitter Cards (default injection if not user-defined)
        if !Self::has_og_tags(head) {
            self.inject_og_defaults(head);
        }

        // Keep all injected nodes ahead of user-defined head nodes.
        let injected_len = head.children.len().saturating_sub(existing_len);
        if existing_len > 0 && injected_len > 0 {
            head.children.rotate_left(existing_len);
        }
    }

    /// Check if head already contains a specific tag.
    fn has_tag(head: &Element<Raw>, tag: &str) -> bool {
        head.children
            .iter()
            .any(|n| matches!(n, Node::Element(e) if e.tag == tag))
    }

    /// Check if head already contains a meta tag with specific name.
    fn has_meta_name(head: &Element<Raw>, name: &str) -> bool {
        head.children.iter().any(|n| {
            matches!(n, Node::Element(e) if e.tag == "meta" && e.get_attr("name").is_some_and(|v| v == name))
        })
    }

    /// Check if head already contains OG tags (user-defined via Typst head parameter).
    fn has_og_tags(head: &Element<Raw>) -> bool {
        head.children.iter().any(|n| {
            matches!(n, Node::Element(e) if e.tag == "meta" && e.get_attr("property").is_some_and(|v| v.starts_with("og:")))
        })
    }

    /// Inject default Open Graph and Twitter Card meta tags.
    fn inject_og_defaults(&self, head: &mut Element<Raw>) {
        use crate::seo::og::OgDefaults;

        let og = OgDefaults::from_config(self.config);

        // og:type
        head.push_elem(Self::meta_property("og:type", og.og_type));

        // og:site_name
        if !og.site_name.is_empty() {
            head.push_elem(Self::meta_property("og:site_name", og.site_name));
        }

        // og:locale
        if !og.locale.is_empty() {
            head.push_elem(Self::meta_property("og:locale", og.locale));
        }

        // og:description
        if !og.description.is_empty() {
            head.push_elem(Self::meta_property("og:description", og.description));
        }

        // twitter:card
        head.push_elem(Self::meta_name("twitter:card", og.twitter_card));
    }

    /// Create a meta element with property attribute.
    fn meta_property(property: &str, content: &str) -> Element<Raw> {
        let mut attrs = Attrs::new();
        attrs.set("property", property);
        attrs.set("content", content);
        TolaSite::element("meta", attrs)
    }

    /// Create a meta element with name attribute.
    fn meta_name(name: &str, content: &str) -> Element<Raw> {
        let mut attrs = Attrs::new();
        attrs.set("name", name);
        attrs.set("content", content);
        TolaSite::element("meta", attrs)
    }
}

impl<'a> Transform<Raw> for HeaderInjector<'a> {
    type To = Raw;

    fn transform(self, mut doc: Document<Raw>) -> Document<Raw> {
        // Add lang to html root (always, regardless of global_header)
        if doc.root.tag == "html" && !doc.root.has_attr("lang") {
            doc.root
                .set_attr("lang", self.config.site.info.language.as_str());
        }

        // Skip head injection if global_header is false
        // (e.g., 404 pages that need self-contained styles)
        if self.global_header {
            self.inject_head(&mut doc.root);
        }

        doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::section::build::assets::NestedEntry;
    use crate::config::{FeedConfig, FeedFormat};
    use std::fs;
    use tempfile::TempDir;

    fn make_html_doc() -> Document<Raw> {
        let mut html = TolaSite::element("html", Attrs::new());
        let head = TolaSite::element("head", Attrs::new());
        let body = TolaSite::element("body", Attrs::new());
        html.push_elem(head);
        html.push_elem(body);
        Document::new(html)
    }

    fn head(doc: &Document<Raw>) -> &Element<Raw> {
        doc.root
            .children
            .iter()
            .find_map(|n| match n {
                Node::Element(e) if e.tag == "head" => Some(e.as_ref()),
                _ => None,
            })
            .expect("should have head")
    }

    #[test]
    fn test_inject_title() {
        let mut config = SiteConfig::default();
        config.site.info.title = "Test Site".to_string();

        let doc = make_html_doc();
        let doc = HeaderInjector::new(&config).transform(doc);

        let head = head(&doc);

        // Check for title element
        let has_title = head.children.iter().any(|n| match n {
            Node::Element(e) => e.tag == "title",
            _ => false,
        });

        assert!(has_title, "should have title element");
    }

    #[test]
    fn injects_feed_discovery_links() {
        let mut config = SiteConfig::default();
        config.site.info.title = "Test Site".to_string();
        config.site.info.url = Some("https://example.com/blog".to_string());
        config.site.seo.feeds = vec![
            FeedConfig {
                format: FeedFormat::Rss,
                output: "feed.xml".into(),
                features: vec![],
            },
            FeedConfig {
                format: FeedFormat::Json,
                output: "feed.json".into(),
                features: vec![],
            },
        ];

        let doc = HeaderInjector::new(&config).transform(make_html_doc());
        let links: Vec<_> = head(&doc)
            .children
            .iter()
            .filter_map(|node| match node {
                Node::Element(elem) if elem.tag == "link" => Some(elem.as_ref()),
                _ => None,
            })
            .filter(|elem| elem.get_attr("rel") == Some("alternate"))
            .collect();

        assert_eq!(links.len(), 2);
        assert!(links.iter().any(|link| {
            link.get_attr("type") == Some("application/rss+xml")
                && link.get_attr("href") == Some("https://example.com/blog/feed.xml")
                && link.get_attr("title") == Some("Test Site (RSS)")
        }));
        assert!(links.iter().any(|link| {
            link.get_attr("type") == Some("application/feed+json")
                && link.get_attr("href") == Some("https://example.com/blog/feed.json")
                && link.get_attr("title") == Some("Test Site (JSON Feed)")
        }));
    }

    #[test]
    fn injected_href_links_have_link_family_payloads() {
        let dir = TempDir::new().unwrap();
        let assets_dir = dir.path().join("assets");
        fs::create_dir_all(&assets_dir).unwrap();
        let style_path = assets_dir.join("site.css");
        fs::write(&style_path, "body{}").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.assets.nested = vec![NestedEntry::Simple(assets_dir)];
        config.site.header.no_fouc = false;
        config.site.header.styles = vec![style_path];

        let doc = HeaderInjector::new(&config).transform(make_html_doc());
        let indexed = TolaSite::indexer().transform(doc);
        let links = indexed.find_all(|elem| elem.is_tag("link") && elem.has_attr("href"));

        assert!(!links.is_empty());
        for link in links {
            let data = ExtractFamily::<LinkFamily>::get(&link.ext).unwrap();
            assert_eq!(data.href.as_deref(), link.get_attr("href"));
        }
    }

    #[test]
    fn injects_atomic_css_output_stylesheet() {
        crate::asset::version::clear();

        let dir = TempDir::new().unwrap();
        let output_dir = dir.path().join("public");
        let css_path = output_dir.join("assets/site.css");
        fs::create_dir_all(css_path.parent().unwrap()).unwrap();
        fs::write(&css_path, ".flex{display:flex}").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.output = output_dir;
        config.build.atomic_css.enable = true;
        config.build.atomic_css.output = Some("assets/site.css".into());
        config.site.header.no_fouc = false;

        let doc = HeaderInjector::new(&config).transform(make_html_doc());
        let links: Vec<_> = head(&doc)
            .children
            .iter()
            .filter_map(|node| match node {
                Node::Element(elem) if elem.tag == "link" => Some(elem.as_ref()),
                _ => None,
            })
            .collect();

        assert!(links.iter().any(|link| {
            link.get_attr("rel") == Some("stylesheet")
                && link
                    .get_attr("href")
                    .is_some_and(|href| href.starts_with("/assets/site.css?v="))
        }));

        crate::asset::version::clear();
    }
}
