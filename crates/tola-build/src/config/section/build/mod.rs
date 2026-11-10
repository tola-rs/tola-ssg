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

/// How the site is built: its Typst entry, content directory, and published output.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build")]
pub struct BuildSectionConfig {
    /// Project Typst file relative to the site root, or a table with the selected module alias.
    pub entry: BuildEntry,

    /// Directory the site's sources are written in, relative to the site root. Tola reads the
    /// `.typ` files below it.
    #[config(name = "content-dir")]
    #[serde(rename = "content-dir")]
    pub content_dir: PathBuf,

    /// Directory the built site is published to, relative to the site root. Tola owns everything
    /// below it: a non-empty directory Tola did not write fails the build, and every successful
    /// build replaces the whole directory. Keep nothing else there.
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
The site is one Typst Bundle, and `entry` is its starting program. Its imports — templates,
helpers, and page programs — join the same compilation. The `tola init` scaffold sets `entry` to
the site root's `site.typ`. Set `entry` to a different project file, or use
`entry = { module = \"notes\" }` to compile the configured module's `build.entry`.

`content-dir` defines source identity: `content/guide/install.typ` arrives in `all-sources()` as
a source whose `path` field is `guide/install.typ`. The file layout does not affect `permalink`;
the route segments it suggests are just the default route's segments, which you can replace with
your own. Moving or renaming it changes that identity and those segments. The entry file is never
a source; keep shared helpers and templates outside this directory so discovery does not treat
them as content.

Discovery does not publish a page. The entry chooses which sources become documents and which
output paths they use. For example, this converts the discovered segments to default output paths:

```typst
#import \"@tola/address:0.0.0\": join-route, output-of
#import \"@tola/source:0.0.0\": all-sources
#let outputs = all-sources().map(source => output-of(join-route(source.route-segments)))
```

A site that validates metadata can drop drafts and slugify each source's `route-segments` in its
entry; these policies are yours to define and handle. A schema parses `permalink` with
`url-path`, so the selected metadata already holds a decoded route. That route overrides the
file layout, keeping the URL unchanged when the source moves.

```typst
#let source-route(source) = {
  if source.meta.permalink == none {
    join-route(source.route-segments.map(segment => slugify(segment, language: site.language.lang)))
  } else {
    source.meta.permalink
  }
}

#let select-pages(sources) = {
  // Every source is validated here, drafts included, before choosing which to publish.
  parse-sources(sources, page-schema)
    .filter(source => not source.meta.draft)
    .map(source => (
      source: source,
      output: output-of(source-route(source)),
    ))
}
```

`publish-dir` is the directory `tola build` replaces as a whole, removing stale outputs. Choose an
empty directory or one Tola already owns for this site; a non-empty directory Tola did not write
fails the build. Hand-maintained files belong in `[assets]` or in outputs declared under
`[[hooks.generate-outputs]]`. Errors or cancellation before publication leave the previous
output intact. `check`, `dev`, and `preview` do not replace this directory.

`[build.minify]` compacts supported output bytes, and `[build.references]` chooses `error`
or `warn` for broken references. The top-level `[hooks]` table runs the site's commands before
compilation, during output generation, or after publication. Conflicts and references are
checked across all outputs before publication.";

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
    use std::path::Path;

    #[test]
    fn build_template_round_trips_through_toml() {
        let source = format!(
            "[site]\ntitle = \"Test\"\ndescription = \"Test\"\n{}",
            BuildSectionConfig::try_template_with_header().unwrap()
        );

        let parsed: crate::config::SiteConfigSchema = toml::from_str(&source).unwrap();

        assert_eq!(
            parsed.build.entry.project_path(),
            Some(Path::new("site.typ"))
        );
        assert!(parsed.assets.trees.is_empty());
        assert!(parsed.assets.files.is_empty());
        assert!(parsed.icons.collections.is_empty());
        assert!(parsed.hooks.before_build.is_empty());
    }

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

    #[test]
    fn default_paths_validate() {
        let config = BuildSectionConfig::default();
        let mut diagnostics = crate::config::ConfigDiagnostics::new();

        config.validate_paths(&mut diagnostics);

        assert!(diagnostics.into_result().is_ok());
        assert_eq!(config.entry.project_path(), Some(Path::new("site.typ")));
        assert_eq!(config.content_dir, Path::new("content"));
    }
}
