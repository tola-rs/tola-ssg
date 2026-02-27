//! Body content injector (Indexed -> Indexed).
//!
//! Injects scripts before `</body>` based on site configuration.
//! Currently handles SPA navigation script injection and recolor SVG filter.

use tola_vdom::prelude::*;

use crate::compiler::family::Indexed;
use crate::config::SiteConfig;
use crate::embed::build::{SPA_JS, SpaVars};

/// Injects body scripts based on site configuration
pub struct BodyInjector<'a> {
    config: &'a SiteConfig,
}

impl<'a> BodyInjector<'a> {
    pub fn new(config: &'a SiteConfig) -> Self {
        Self { config }
    }

    /// Recursively find and populate `<body>` element.
    fn inject_body(&self, element: &mut Element<Indexed>) {
        if element.tag == "body" {
            self.populate_body(element);
            return;
        }

        for child in &mut element.children {
            if let Node::Element(elem) = child {
                self.inject_body(elem);
            }
        }
    }

    /// Populate `<body>` with scripts.
    fn populate_body(&self, body: &mut Element<Indexed>) {
        let nav = &self.config.site.nav;
        let recolor = &self.config.theme.recolor;

        // Recolor SVG filter (inject at body start)
        if recolor.enable {
            use crate::config::section::theme::RecolorSource;
            use crate::embed::recolor::FILTER_SVG;
            use crate::image::recolor::generate_static_svg;

            let svg = match &recolor.source {
                RecolorSource::Static => generate_static_svg(&recolor.list),
                _ => FILTER_SVG.to_string(),
            };
            // Insert at beginning of body
            body.children.insert(0, Node::Text(Text::raw(svg)));
        }

        // SPA navigation script
        if nav.spa {
            let vars = SpaVars::from_config(self.config);

            // Inject as raw HTML text node (script tag)
            let script_tag = SPA_JS.external_tag_with_vars(&vars);
            body.push(Node::Text(Text::raw(script_tag)));
        }
    }
}

impl<'a> Transform<Indexed> for BodyInjector<'a> {
    type To = Indexed;

    fn transform(self, mut doc: Document<Indexed>) -> Document<Indexed> {
        self.inject_body(&mut doc.root);
        doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::compiler::family::{Raw, TolaSite};
    use crate::config::section::theme::RecolorSource;

    fn make_indexed_doc() -> Document<Indexed> {
        let mut html = TolaSite::element("html", Attrs::new());
        let head = TolaSite::element("head", Attrs::new());
        let body = TolaSite::element("body", Attrs::new());
        html.push_elem(head);
        html.push_elem(body);

        let raw: Document<Raw> = Document::new(html);
        Pipeline::new(raw).pipe(TolaSite::indexer()).into_inner()
    }

    fn body_text_content(doc: &Document<Indexed>) -> String {
        let body = doc
            .root
            .children
            .iter()
            .find_map(|n| match n {
                Node::Element(e) if e.tag == "body" => Some(e.as_ref()),
                _ => None,
            })
            .expect("document should have <body>");
        body.text_content()
    }

    #[test]
    fn test_dynamic_recolor_injects_filter_but_not_recolor_script() {
        let mut config = SiteConfig::default();
        config.theme.recolor.enable = true;
        config.theme.recolor.source = RecolorSource::Auto;

        let doc = BodyInjector::new(&config).transform(make_indexed_doc());
        let text = body_text_content(&doc);

        assert!(text.contains("filter id=\"tola-recolor\""));
        assert!(
            !text.contains("/.tola/recolor-"),
            "recolor.js should be injected in head, not body"
        );
    }

    #[test]
    fn test_static_recolor_injects_static_svg_only() {
        let mut config = SiteConfig::default();
        config.theme.recolor.enable = true;
        config.theme.recolor.source = RecolorSource::Static;
        config.theme.recolor.list = HashMap::from([
            ("light".to_string(), "#000000".to_string()),
            ("dark".to_string(), "#ffffff".to_string()),
        ]);

        let doc = BodyInjector::new(&config).transform(make_indexed_doc());
        let text = body_text_content(&doc);

        assert!(text.contains("tola-recolor-light"));
        assert!(text.contains("tola-recolor-dark"));
        assert!(!text.contains("/.tola/recolor-"));
    }
}
