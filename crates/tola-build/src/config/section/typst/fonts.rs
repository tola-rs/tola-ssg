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
    /// Font directories, relative to the site root; searched recursively.
    #[config(collection = inline)]
    pub paths: Vec<PathBuf>,
    /// Modules whose declared compiler fonts are searched, in order.
    #[config(collection = inline)]
    pub modules: Vec<ModuleFonts>,
    /// Use system fonts too. Enabling this can change the result on another machine.
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
Compiler fonts lay out and export Typst content; their files are not published. `paths` lists
site-relative directories to search recursively. The default is `static/typst-fonts`:

```toml
[typst.fonts]
paths = [\"static/typst-fonts\"]
system = false
```

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

`--font-path` is invocation-only and refused under `--pure`; `--ignore-system-fonts` narrows one
invocation.

Browser fonts are separate: publish them through `[assets]` and load them with CSS `@font-face`.
A Typst font setting does not install a browser font:

```toml
[assets]
trees = [{ source = \"static/web-fonts\", url-prefix = \"/fonts\" }]
```";

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
