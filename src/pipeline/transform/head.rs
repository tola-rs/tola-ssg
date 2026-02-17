//! Head content injector (Raw → Raw).
//!
//! Injects site-wide `<head>` content from config into Raw VDOM before indexing.
//! Also sets `lang` attribute on `<html>` root if not present.
//!
//! Injected elements: title, description meta, icon link, stylesheets, scripts,
//! CSS processor output, auto-enhance CSS, and raw HTML elements.

use std::path::Path;

use tola_vdom::prelude::*;

use crate::asset::{compute_asset_href, version};
use crate::compiler::family::{Raw, TolaSite};
use crate::config::SiteConfig;
use crate::utils::mime;

/// Injects site-wide `<head>` content into Raw VDOM.
pub struct HeadInjector<'a> {
    config: &'a SiteConfig,
    /// Whether to inject global header content (styles, scripts, elements).
    /// Default: `true`. Set to `false` for pages like 404 that need
    /// self-contained styles to avoid relative path issues.
    global_header: bool,
}

/// Compute versioned href for an asset (with ?v=hash for cache busting).
fn versioned_href(path: &Path, config: &SiteConfig) -> Option<String> {
    let href = compute_asset_href(path, config).ok()?;
    let abs_path = config.get_root().join(path);
    Some(version::versioned_url(&href, &abs_path))
}

impl<'a> HeadInjector<'a> {
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
        let head_config = &config.build.header;

        // Title
        if !config.site.info.title.is_empty() {
            let mut title = TolaSite::element("title", Attrs::new());
            title.push_text(&config.site.info.title);
            head.push_elem(title);
        }

        // Description meta
        if !config.site.info.description.is_empty() {
            let mut meta = TolaSite::element("meta", Attrs::new());
            meta.set_attr("name", "description");
            meta.set_attr("content", &config.site.info.description);
            head.push_elem(meta);
        }

        // Icon
        if let Some(icon) = &head_config.icon
            && let Some(href) = versioned_href(icon, config)
        {
            let mut link = TolaSite::element("link", Attrs::new());
            link.set_attr("rel", "shortcut icon");
            link.set_attr("href", href);
            link.set_attr("type", mime::for_icon(icon));
            head.push_elem(link);
        }

        // User-defined stylesheets
        for style in &head_config.styles {
            if let Some(href) = versioned_href(style, config) {
                let mut link = TolaSite::element("link", Attrs::new());
                link.set_attr("rel", "stylesheet");
                link.set_attr("href", href);
                head.push_elem(link);
            }
        }

        // CSS Processor output
        if config.build.css.processor.enable
            && let Some(input) = &config.build.css.processor.input
            && let Ok(route) = crate::asset::route_from_source(input.clone(), config)
        {
            let mut link = TolaSite::element("link", Attrs::new());
            link.set_attr("rel", "stylesheet");
            // CSS processor output uses versioned URL based on OUTPUT file
            // (not input, since Tailwind generates different output based on scanned classes)
            let href = version::versioned_url(route.url.as_ref(), &route.output);
            link.set_attr("href", href);
            head.push_elem(link);
        }

        // Auto-enhance CSS (SVG theme adaptation + View Transitions)
        {
            use crate::embed::css::{ENHANCE_CSS, enhance_vars};
            let href = ENHANCE_CSS.url_path_with_vars(&enhance_vars(config));
            let mut link = TolaSite::element("link", Attrs::new());
            link.set_attr("rel", "stylesheet");
            link.set_attr("href", href);
            head.push_elem(link);
        }

        // Recolor CSS + JS (if enabled)
        if config.theme.recolor.enable {
            use crate::config::section::theme::RecolorSource;
            use crate::embed::recolor;

            // CSS (always)
            let css_vars = recolor::css_vars(&config.theme.recolor);
            let href = recolor::RECOLOR_CSS.url_path_with_vars(&css_vars);
            let mut link = TolaSite::element("link", Attrs::new());
            link.set_attr("rel", "stylesheet");
            link.set_attr("href", href);
            head.push_elem(link);

            // JS (only for dynamic mode: auto or css-var)
            if !matches!(config.theme.recolor.source, RecolorSource::Static) {
                let js_vars = recolor::js_vars(&config.theme.recolor);
                let src = recolor::RECOLOR_JS.url_path_with_vars(&js_vars);
                let mut script = TolaSite::element("script", Attrs::new());
                script.set_attr("src", src);
                script.set_attr("defer", "");
                head.push_elem(script);
            }
        }

        // Scripts
        for script in &head_config.scripts {
            if let Some(src) = versioned_href(script.path(), config) {
                let mut script_elem = TolaSite::element("script", Attrs::new());
                script_elem.set_attr("src", src);
                if script.is_defer() {
                    script_elem.set_attr("defer", "");
                }
                if script.is_async() {
                    script_elem.set_attr("async", "");
                }
                head.push_elem(script_elem);
            }
        }

        // Raw HTML elements (trusted input) - inject as raw text nodes
        for raw in &head_config.elements {
            // Raw HTML is injected as a Text node with TextKind::Raw
            // This allows custom script tags, meta tags, etc. to be rendered unescaped
            head.push(Node::Text(Text::raw(raw.as_str())));
        }
    }
}

impl<'a> Transform<Raw> for HeadInjector<'a> {
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

    fn make_html_doc() -> Document<Raw> {
        let mut html = TolaSite::element("html", Attrs::new());
        let head = TolaSite::element("head", Attrs::new());
        let body = TolaSite::element("body", Attrs::new());
        html.push_elem(head);
        html.push_elem(body);
        Document::new(html)
    }

    #[test]
    fn test_inject_title() {
        let mut config = SiteConfig::default();
        config.site.info.title = "Test Site".to_string();

        let doc = make_html_doc();
        let doc = HeadInjector::new(&config).transform(doc);

        // Find head
        let head = doc
            .root
            .children
            .iter()
            .find_map(|n| match n {
                Node::Element(e) if e.tag == "head" => Some(e.as_ref()),
                _ => None,
            })
            .expect("should have head");

        // Check for title element
        let has_title = head.children.iter().any(|n| match n {
            Node::Element(e) => e.tag == "title",
            _ => false,
        });

        assert!(has_title, "should have title element");
    }
}
