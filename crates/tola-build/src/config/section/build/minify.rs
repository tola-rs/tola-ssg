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
    /// What `tola help "[build.minify]"` adds under its table.
    pub const HELP: &'static str = "\
Minification reaches only the bytes Tola publishes itself: the HTML it writes for every page, and
the CSS and scripts it publishes from `[assets]` declarations or from the pages that write them.
Images, fonts, and anything a `generate-outputs` hook writes are published byte for byte. It runs
during publication and costs little, so all three keys are on by default:

```toml
[build.minify]
html = true
css = true
javascript = true
```

Set one to `false` to publish that kind of file exactly as written. One caveat: minified CSS may
drop the quotes from attribute selectors — `[data-width=\"fixed\"]` becomes `[data-width=fixed]` —
so write selectors that read the same either way.";
}
