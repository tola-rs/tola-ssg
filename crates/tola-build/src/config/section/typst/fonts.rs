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
    /// What `tola help "[typst.fonts]"` adds under its table.
    pub const HELP: &'static str = "\
The compiler may use a font only to lay text out and export it; the file itself is never published.
A site lists the directories to search in `paths`, and decides with `system` whether the host's
own fonts count too. `static/typst-fonts`, the directory a new site scaffolds, is the default:

```toml
[typst.fonts]
paths = [\"static/typst-fonts\"]
system = false
```

A web font a page loads is an asset, not a compiler font, so declare it under `[assets]` and link
it from your stylesheet:

```toml
[typst.fonts]
paths = [\"static/typst-fonts\"]
system = false

[assets]
trees = [{ source = \"static/web-fonts\", url-prefix = \"/fonts\" }]
```

`paths` are searched recursively. A font `tola vendor` froze into `[vendor]` resolves from the site
before any of these directories. `system = true` adds the host's own fonts, so the result can
differ from one machine to the next — leave it off when a build must reproduce on another machine.";

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
