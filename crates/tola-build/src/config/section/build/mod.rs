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
    /// What `tola help "[build]"` adds under its table.
    pub const HELP: &'static str = "\
The site is one Typst Bundle, and `entry` is the program it starts from: Typst compiles the
Bundle beginning at that file, so every import it reaches — templates, helpers, page programs —
joins the same compilation. Keep it at the site root; the scaffold's `site.typ` lists the pages
with `select-pages(all-sources())`, emits each one with `page-template`, then closes with
`not-found-template()`. The defaults below are what a site gets when it writes nothing:

```toml
[build]
entry = \"site.typ\"
content-dir = \"content\"
publish-dir = \"public\"
```

`content-dir` is the identity root for content: a `.typ` file below it is one source, and its
path below that directory is the source's identity — the `id` and `path` `all-sources()` reports,
and the layout its default route segments follow. `content/guide/install.typ` therefore arrives
as `guide/install.typ`, and renaming or moving the file changes its address. The entry file is
never a source, and helpers and templates belong outside this directory so discovery does not
pick them up.

```typst
#import \"@tola/address:0.0.0\": route, route-to-output
#import \"@tola/source:0.0.0\": all-sources
#let pages = all-sources().map(source => route-to-output(route(source.route-segments)))
```

`publish-dir` belongs to Tola: each build replaces everything it owns there, recording what that
is in `_tola/owner`, so hand-written files go through `[assets]` or a hook instead.

Four tables tune and extend the result. `minify` compacts the bytes Tola publishes, and
`references` decides how a published page pointing at nothing is reported — `error` fails the
build there, so that severity belongs to `[build.references]`, not to `[diagnostics]`, which
limits only how many diagnostics the terminal shows. `hooks` runs the site's own commands around
the build, in three stages: one produces inputs before discovery, one produces final outputs
after the site compiles, and one consumes the published site. Each of the four has its rules and
worked examples on its own page.";

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
