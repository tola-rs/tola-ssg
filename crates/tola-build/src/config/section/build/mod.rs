//! `[build]` section configuration.
//!
//! Build paths and output settings.
//!
//! # Example
//!
//! ```toml
//! [build]
//! entry = "site.typ"
//! content-dir = "content"
//! publish-dir = "public"
//!
//! [build.minify]
//! html = true
//! css = true
//! javascript = true
//!
//! [assets]
//! trees = [{ source = "static/web-assets", url-prefix = "/assets" }]
//! files = []
//! ```
//!
//! See [`super::assets::AssetsConfig`] and [`HooksConfig`] for detailed options.

pub mod hooks;
mod minify;
mod references;

pub use hooks::{AfterPublishHookConfig, BeforeBuildHookConfig, DevParticipation, HooksConfig};
pub use minify::MinifyConfig;
pub use references::{ReferenceLevel, ReferencesConfig};

use crate::config::ConfigDiagnostics;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tola_config::Config;

/// The Typst entry, content directory, and publish directory. These are the defaults in
/// `tola.toml`.
///
/// ```toml
/// [build]
/// entry = "site.typ"
/// content-dir = "content"
/// publish-dir = "public"
/// ```
///
/// With the `site.typ` created by `tola init`, write `= Hello` in `content/index.typ`. Run
/// `tola build` from the site directory to write `public/index.html`.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build")]
pub struct BuildSectionConfig {
    /// Typst file to run, relative to the site root. If omitted, use `site.typ`.
    /// Set `{ module = "notes" }` to use the `notes` module's entry.
    pub entry: BuildEntry,

    /// Directory to search for content files, relative to the site root. If omitted, use `content/`.
    #[config(name = "content-dir")]
    #[serde(rename = "content-dir")]
    pub content_dir: PathBuf,

    /// Where to write the site, relative to the site root. If omitted, use `public/`.
    /// Choose an empty directory or the one where Tola published this site before.
    /// A successful `tola build` replaces the directory contents.
    #[config(name = "publish-dir")]
    #[serde(rename = "publish-dir")]
    pub publish_dir: PathBuf,

    #[config(sub)]
    pub minify: MinifyConfig,

    #[config(sub)]
    pub references: ReferencesConfig,
}

/// The source of the site's Typst entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum BuildEntry {
    Project(PathBuf),
    Module { module: String },
}

impl From<&str> for BuildEntry {
    fn from(path: &str) -> Self {
        Self::Project(path.into())
    }
}

impl BuildEntry {
    /// The site-relative file spelling for a project entry.
    pub fn project_path(&self) -> Option<&std::path::Path> {
        match self {
            Self::Project(path) => Some(path),
            Self::Module { .. } => None,
        }
    }
}

/// Build settings after path resolution.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedBuildConfig {
    pub entry: PathBuf,
    #[serde(rename = "content-dir")]
    pub content_dir: PathBuf,
    #[serde(rename = "publish-dir")]
    pub publish_dir: PathBuf,
    pub minify: MinifyConfig,
    pub references: ReferencesConfig,
}

impl Default for BuildSectionConfig {
    fn default() -> Self {
        Self {
            entry: "site.typ".into(),
            content_dir: "content".into(),
            publish_dir: "public".into(),
            minify: MinifyConfig::default(),
            references: ReferencesConfig::default(),
        }
    }
}

impl BuildSectionConfig {
    /// What `tola help "[build]"` adds under its table.
    pub const HELP: &'static str = "\
The entry builds one Typst Bundle. It decides which sources to publish and passes an output path
to `#document` for each page.

`all-sources()` reports `content/guide/install.typ` as `guide/install.typ`. Moving the file
changes its `path` and default `route-segments`. The entry file is excluded from discovery.
Keep shared helpers and templates outside `content-dir`.

The template from `tola init` uses this loop in `site.typ`. With a new `content/guide/index.typ`,
`tola build` writes `public/guide/index.html`.

```typst
#import \"@tola/address:0.0.0\": join-route, output-of
#import \"@tola/source:0.0.0\": all-sources
#for source in all-sources() {
  document(output-of(join-route(source.route-segments)), format: \"html\")[
    #include source.file
  ]
}
```

To skip drafts, validate metadata with `parse-sources` before filtering. To keep a page URL
when the source moves, parse its `permalink` with `@tola/schema`'s `url-path` and use that route
in `output-of`.

Put static files in `[assets]` and declare generated files in `[[hooks.generate-outputs]]`.
Tola checks all output paths and references before publication. A failure or cancellation
before publication leaves the previous files in place.

`par-hooks` defaults to true, allowing hooks with the same order to run together. Set it to false
to run one hook at a time. `hook-jobs` defaults to `\"auto\"`, using the available parallelism for
the site session; a positive integer sets an explicit shared limit across modules and stages.

`[build.minify]` controls minification. `[build.references]` sets the severity of broken references.";

    pub(crate) fn validate_paths(&self, diag: &mut ConfigDiagnostics) {
        if let Some(entry) = self.entry.project_path() {
            super::path::validate_site_relative_path(
                entry,
                Self::FIELDS.entry.as_str(),
                Self::FIELDS.entry,
                diag,
            );
        }
        let usable = super::path::validate_site_relative_path(
            &self.content_dir,
            Self::FIELDS.content_dir.as_str(),
            Self::FIELDS.content_dir,
            diag,
        );
        if usable && super::path::enters_internal_directory(&self.content_dir) {
            diag.error_with_help(
                Self::FIELDS.content_dir,
                format!(
                    "`build.content-dir` is `{}`, inside `.tola`",
                    super::path::declared_path(&self.content_dir)
                ),
                "keep it outside `.tola`",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BuildSectionConfig;

    #[test]
    fn declared_paths_stay_site_relative() {
        for (entry, content, expected) in [
            (
                "/outside/site.typ",
                "content",
                BuildSectionConfig::FIELDS.entry,
            ),
            (
                "../outside/site.typ",
                "content",
                BuildSectionConfig::FIELDS.entry,
            ),
            (
                "site.typ",
                "/outside/content",
                BuildSectionConfig::FIELDS.content_dir,
            ),
            (
                "site.typ",
                "../outside/content",
                BuildSectionConfig::FIELDS.content_dir,
            ),
            ("", "content", BuildSectionConfig::FIELDS.entry),
            ("site.typ", "", BuildSectionConfig::FIELDS.content_dir),
        ] {
            let config = BuildSectionConfig {
                entry: entry.into(),
                content_dir: content.into(),
                ..BuildSectionConfig::default()
            };
            let mut diagnostics = crate::config::ConfigDiagnostics::new();

            config.validate_paths(&mut diagnostics);

            assert!(
                diagnostics
                    .errors()
                    .iter()
                    .any(|diagnostic| diagnostic.field == expected),
                "accepted {entry:?}, {content:?}"
            );
        }
    }

    #[test]
    fn content_root_rejects_internal_directory() {
        for content in [".tola", "./.tola/nested"] {
            let config = BuildSectionConfig {
                content_dir: content.into(),
                ..BuildSectionConfig::default()
            };
            let mut diagnostics = crate::config::ConfigDiagnostics::new();
            config.validate_paths(&mut diagnostics);
            assert!(
                diagnostics.errors().iter().any(|diagnostic| {
                    diagnostic.field == BuildSectionConfig::FIELDS.content_dir
                }),
                "accepted content root {content:?}"
            );
        }
    }
}
