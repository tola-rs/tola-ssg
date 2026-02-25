//! Body content injector (Indexed -> Indexed).
//!
//! Injects scripts before `</body>` based on site configuration.
//! Currently handles SPA navigation script injection and recolor filter/JS.

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
            use crate::embed::recolor;

            let svg = match &recolor.source {
                RecolorSource::Static => recolor::generate_static_svg(&recolor.list),
                _ => recolor::FILTER_SVG.to_string(),
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

        // Recolor JS (dynamic mode only, inject at body end)
        if recolor.enable {
            use crate::config::section::theme::RecolorSource;
            use crate::embed::recolor;

            match &recolor.source {
                RecolorSource::Static => {} // No JS needed for static mode
                _ => {
                    let vars = recolor::js_vars(recolor);
                    let script_tag = recolor::RECOLOR_JS.external_tag_with_vars(&vars);
                    body.push(Node::Text(Text::raw(script_tag)));
                }
            }
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
