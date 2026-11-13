//! `[typst.fonts]` section configuration.

use crate::config::ConfigDiagnostics;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tola_config::Config;

/// Fonts the Typst compiler may use.
///
/// Compiler fonts are not published: declare web fonts under `[assets]` instead.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "typst.fonts")]
pub struct FontsConfig {
    /// Directories to search for fonts, including subdirectories. Paths start at the site root.
    /// If omitted, search `static/typst-fonts/`.
    #[config(collection = inline)]
    pub paths: Vec<PathBuf>,
    /// Module aliases to search for compiler fonts, in the order listed. If omitted, skip module fonts.
    #[config(collection = inline)]
    pub modules: Vec<ModuleFonts>,
    /// Set `true` to search system fonts too. If omitted, skip system fonts.
    pub system: bool,
}

/// One module's compiler fonts, optionally registered under different family names.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ModuleFonts {
    /// Load a module's fonts using their original names.
    Alias(String),
    /// Load a module's fonts and rename selected families.
    Selection {
        /// Alias declared under `[modules]`.
        module: String,
        /// Original family name to compiler family name.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        rename: BTreeMap<String, String>,
    },
}

impl Default for FontsConfig {
    fn default() -> Self {
        Self {
            paths: vec![PathBuf::from("static/typst-fonts")],
            modules: Vec::new(),
            system: false,
        }
    }
}

impl FontsConfig {
    /// What `tola help "[typst.fonts]"` adds under its table.
    pub const HELP: &'static str = "\
Tola searches `static/typst-fonts/` by default. To use another directory, put the font
files there and set `paths` in `tola.toml`. For example, use `static/print-fonts/`:

```toml
[typst.fonts]
paths = [\"static/print-fonts\"]
system = false
```

Run `tola inspect fonts` from the site root to see the available families. Use the family
name in a Typst file, for example `#set text(font: \"IBM Plex Sans\")`.

Site Nu hooks use the same fonts to resize SVGs that contain text. Set the SVG's
`font-family` to a name shown by `tola inspect fonts`. To use site fonts from the
standalone image script, put `wordmark.svg` next to `tola.toml` and run:

```sh
tola nu --config tola.toml @tola-scripts/image resize wordmark.svg --width 240 --format png --output wordmark.png
```

The command saves a 240-pixel-wide PNG as `wordmark.png`. Choose a new output filename.

Use `modules` to select compiler fonts from modules already declared in the site, for example
`modules = [\"theme\", \"cjk\"]`. A module lists its directories as `fonts = [\"typst-fonts\"]` in
its `[typst]` table in `tola-module.toml`. Paths are relative to the module root and must ship
with it. Modules are searched after site paths, in the order listed. The default is an empty
list. Typst selects the face; search order breaks ties.

Use a table entry to rename a family from one module for this site's compiler:

```toml
[typst.fonts]
modules = [
  \"theme\",
  { module = \"cjk\", rename = { \"IBM Plex Sans\" = \"CJK Plex Sans\" } },
]
```

Matching ignores case and applies to every face of the family in that entry. Refer to
`CJK Plex Sans` in Typst. `tola inspect fonts --json` shows the resulting names and modules.

Embedded fonts stay available under `--pure`. Host fonts are never vendored, and `system = true`
combined with `--pure` is an error.

Use `--font-path DIR` to search an extra directory first for one command.
Use `--ignore-system-fonts` to skip system fonts for that run. `--pure` rejects `--font-path`.

### Load a font in the browser

Put a web font at `static/web-fonts/notes.woff2`. Save this as
`static/web-fonts/fonts.css`, next to the font.

```css
@font-face {
  font-family: Notes;
  src: url(\"./notes.woff2\") format(\"woff2\");
  font-display: swap;
}
body { font-family: Notes, sans-serif; }
```

Add both files to the site by putting this in `tola.toml`.

```toml
[assets]
trees = [{ source = \"static/web-fonts\", url-prefix = \"/fonts\" }]
```

In a site created with `tola init`, add this to `content/index.typ`.

```typst
#import \"@tola/address:0.0.0\": asset-url
#html.link(rel: \"stylesheet\", href: asset-url(\"/fonts/fonts.css\"))
= Font preview
```

Run `tola build` from the site root. With the default output directory, both files appear
under `public/fonts/`. Run `tola dev` and open the printed URL to see the font in use.
`asset-url` includes the site's deployment prefix. The font URL inside the CSS resolves
relative to the stylesheet.";

    /// Validate the paths before site-root normalization.
    pub fn validate_paths(&self, diag: &mut ConfigDiagnostics) {
        for (index, path) in self.paths.iter().enumerate() {
            diag.with_array_element(Self::FIELDS.paths.as_str(), index, |diag| {
                let key = format!("{}[{index}]", Self::FIELDS.paths.as_str());
                super::super::path::validate_site_relative_path(
                    path,
                    &key,
                    Self::FIELDS.paths,
                    diag,
                );
            });
        }
    }

    /// Normalize the paths relative to the site root.
    pub fn normalize(&mut self, root: &Path) {
        // `root` is already absolute. Preserve symlinks so the Typst host
        // can detect later retargets.
        for path in &mut self.paths {
            *path = root.join(&*path);
        }
    }
}
