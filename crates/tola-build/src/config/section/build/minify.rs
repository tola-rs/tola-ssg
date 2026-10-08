//! `[build.minify]` output settings.

use serde::{Deserialize, Serialize};
use tola_config::Config;

/// Minification applied to generated HTML, stylesheets, and scripts before they are published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build.minify")]
pub struct MinifyConfig {
    /// Minify the HTML of every published page.
    pub html: bool,

    /// Minify CSS sources declared under `[assets]` and the stylesheets pages write.
    pub css: bool,

    /// Minify `.js` and `.mjs` sources declared under `[assets]` and the scripts pages write.
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
Minification happens while building the outputs, so `check`, `dev`, and `preview` use it too.
Tola compacts the HTML it exports, CSS and `.js`/`.mjs` files declared in `[assets]`, and inline
stylesheets and scripts. Images, fonts, and `generate-outputs` hook files keep their own bytes.
All three switches default to `true`:

```toml
[build.minify]
html = true
css = true
javascript = true
```

Set a switch to `false` to skip that transformation. `--minify=false` disables all three for one
invocation. Configured asset URLs identify the bytes after minification, so cache busting tracks
what the browser receives. A configured CSS or JavaScript asset that cannot be minified produces
a warning and keeps its original bytes.";
}
