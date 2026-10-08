//! `[build.minify]` output settings.

use serde::{Deserialize, Serialize};
use tola_config::Config;

/// Minification, before publication, of the HTML Typst compiles and of the CSS and JavaScript
/// files in `[assets]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build.minify")]
pub struct MinifyConfig {
    /// Whether to minify the HTML of every published page.
    pub html: bool,

    /// Whether to minify CSS sources declared under `[assets]` and the `<style>` elements a page
    /// embeds.
    pub css: bool,

    /// Whether to minify `.js` and `.mjs` sources declared under `[assets]` and the `<script>`
    /// elements a page embeds, modules included.
    pub javascript: bool,
}

impl Default for MinifyConfig {
    fn default() -> Self {
        Self {
            html: true,
            css: true,
            javascript: true,
        }
    }
}

impl MinifyConfig {
    /// What `tola help config build.minify` adds under its table.
    pub const HELP: &'static str = "\
Minification happens while building the outputs, and `check`, `dev`, and `preview` all decide
whether to minify from `[build.minify]`. Because minification is fast, all three switches default
to `true`.

`--minify=false` disables all three for one invocation. With `cache-busting = true` under
`[assets]`, `asset-url` (from `@tola/address`) returns a URL with a `?h=` appended, identifying
the bytes after minification — the content the browser actually receives; the URL changes when
that content changes. If CSS or JavaScript cannot be minified, it produces a warning and keeps
its original content.";
}
