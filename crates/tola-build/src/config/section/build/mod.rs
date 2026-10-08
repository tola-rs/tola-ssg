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

/// How the site is built: its Typst entry, pages directory, and published output.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build")]
pub struct BuildSectionConfig {
    /// Typst program that builds the site, relative to the site root.
    pub entry: PathBuf,

    /// Directory the site's pages are written in, relative to the site root. Tola reads the
    /// `.typ` files below it.
    #[config(name = "content-dir")]
    #[serde(rename = "content-dir")]
    pub content_dir: PathBuf,

    /// Directory the built site is published to, relative to the site root. Tola owns everything
    /// below it, so keep nothing else there.
    #[config(name = "publish-dir")]
    #[serde(rename = "publish-dir")]
    pub publish_dir: PathBuf,

    #[config(sub)]
    pub minify: MinifyConfig,

    #[config(sub)]
    pub references: ReferencesConfig,

    #[config(sub)]
    pub hooks: HooksConfig,
}

impl Default for BuildSectionConfig {
    fn default() -> Self {
        Self {
            entry: "site.typ".into(),
            content_dir: "content".into(),
            publish_dir: "public".into(),
            minify: MinifyConfig::default(),
            hooks: HooksConfig::default(),
            references: ReferencesConfig::default(),
        }
    }
}

impl BuildSectionConfig {
    /// What `tola help config build` adds under its table.
    pub const HELP: &'static str = "\
The site is one Typst Bundle, and `entry` is its starting program. Its imports — templates,
helpers, and page programs — join the same compilation. The scaffold keeps `site.typ` at the
site root; an entry elsewhere in the site works too.

`content-dir` defines source identity: `content/guide/install.typ` arrives in `all-sources()` as
`guide/install.typ`, with the route segments its file layout suggests. Moving or renaming it
changes that identity and those segments. The entry file is never a source; keep shared helpers
and templates outside this directory so discovery does not treat them as content.

Discovery does not publish a page. The entry chooses which sources become documents and which
output paths they use. For example, this converts the discovered segments to default output paths:

```typst
#import \"@tola/address:0.0.0\": route, route-to-output
#import \"@tola/source:0.0.0\": all-sources
#let outputs = all-sources().map(source => route-to-output(route(source.route-segments)))
```

The scaffold's `select-pages` also validates metadata, filters drafts, and slugs default route
segments for the site's language. A source's `permalink` overrides its default route, so its URL
can stay unchanged when the source moves. These policies live in editable site code.

`publish-dir` is the directory `tola build` replaces as a whole, removing stale outputs. Choose an
empty directory or one Tola already owns for this site; hand-maintained files belong in `[assets]`
or a hook's declared outputs. Errors or cancellation before publication leave the previous output intact.
`check`, `dev`, and `preview` do not replace this directory.

Three child tables tune the build: `[build.minify]` compacts supported output bytes;
`[build.references]` chooses `error` or `warn` for broken references; `[build.hooks]` runs the
site's own commands before compilation, during output generation, or after publication. Their
pages explain each contract. All outputs meet in one complete set: conflicts and references are
checked before the site is published, so one producer cannot silently replace another's file.";

    pub(crate) fn validate_paths(&self, diag: &mut ConfigDiagnostics) {
        super::path::validate_site_relative_path(
            &self.entry,
            Self::FIELDS.entry.as_str(),
            Self::FIELDS.entry,
            diag,
        );
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
    use std::path::{Path, PathBuf};

    #[test]
    fn build_template_round_trips_through_toml() {
        let source = format!(
            "[site]\ntitle = \"Test\"\ndescription = \"Test\"\n{}",
            BuildSectionConfig::try_template_with_header().unwrap()
        );

        let parsed: crate::config::SiteConfigSchema = toml::from_str(&source).unwrap();

        assert_eq!(parsed.build.entry, PathBuf::from("site.typ"));
        assert!(parsed.assets.trees.is_empty());
        assert!(parsed.assets.files.is_empty());
        assert!(parsed.icons.collections.is_empty());
        assert!(parsed.build.hooks.before_build.is_empty());
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
        assert_eq!(config.entry, Path::new("site.typ"));
        assert_eq!(config.content_dir, Path::new("content"));
    }
}
