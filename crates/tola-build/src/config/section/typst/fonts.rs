//! `[typst.fonts]` section configuration.

use crate::config::ConfigDiagnostics;
use serde::{Deserialize, Serialize};
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
    /// Use system fonts too. Enabling this can change the result on another machine.
    pub system: bool,
}

impl Default for FontsConfig {
    fn default() -> Self {
        Self {
            paths: vec![PathBuf::from("static/typst-fonts")],
            system: false,
        }
    }
}

impl FontsConfig {
    /// What `tola help config typst.fonts` adds under its table.
    pub const HELP: &'static str = "\
Compiler fonts lay out and export Typst content; their files are not published. `paths` lists
site-relative directories to search recursively. The scaffold's directory is the default:

```toml
[typst.fonts]
paths = [\"static/typst-fonts\"]
system = false
```

Vendored fonts join these directories before system-font discovery. Tola also carries embedded
fonts, which remain available with `--pure`. `system = true` adds host fonts when the invocation
permits them; `--pure` excludes them even if this setting is on. Keep it off when another machine
must build with the same font inputs.

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
