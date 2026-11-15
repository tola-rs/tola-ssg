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

use crate::config::{ConfigDiagnostics, fragment};
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use tola_config::{
    Config, Fragment, FragmentContext, ValidateFragment, validate_value, wrong_type,
};

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
    /// Allow hooks sharing a stage, site or module instance, and order to run together.
    /// Defaults to true. False runs one hook at a time without changing the batch order.
    #[serde(rename = "par-hooks")]
    #[config(name = "par-hooks")]
    pub par_hooks: bool,

    /// Maximum concurrent hooks shared by all modules and stages in this site session.
    /// Defaults to "auto": the available parallelism sampled when the session starts.
    /// A positive integer sets an explicit limit; this does not control a script's own threads.
    #[serde(rename = "hook-jobs")]
    #[config(name = "hook-jobs", fragment_with = hook_jobs_fragment)]
    pub hook_jobs: HookJobs,
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

/// The site session's maximum number of concurrent hooks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HookJobs {
    /// Use the session's sampled available parallelism, falling back to one if unavailable.
    #[default]
    Auto,
    /// Use the author's limit, even when it exceeds the automatic value.
    Limit(NonZeroUsize),
}

impl HookJobs {
    /// Return the explicit limit or the host's automatic limit.
    pub fn resolved(self, auto_limit: usize) -> usize {
        match self {
            Self::Auto => auto_limit.max(1),
            Self::Limit(limit) => limit.get(),
        }
    }
}

impl Serialize for HookJobs {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::Limit(limit) => serializer.serialize_u64(limit.get() as u64),
        }
    }
}

impl<'de> Deserialize<'de> for HookJobs {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            Name(String),
            Limit(NonZeroUsize),
        }
        match Written::deserialize(deserializer)? {
            Written::Name(name) if name == "auto" => Ok(Self::Auto),
            Written::Limit(limit) => Ok(Self::Limit(limit)),
            Written::Name(_) => Err(serde::de::Error::custom("use `auto` or a positive integer")),
        }
    }
}

fn hook_jobs_fragment(value: &toml::Value, context: &mut FragmentContext<'_>) {
    match value {
        toml::Value::String(_) | toml::Value::Integer(_) => {
            validate_value::<HookJobs>(value, context)
        }
        _ => wrong_type(value, "a positive integer or \"auto\"", context),
    }
}

/// The source of the site's Typst entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum BuildEntry {
    Project(PathBuf),
    Module { module: String },
}

#[derive(Deserialize, Fragment)]
#[serde(deny_unknown_fields)]
struct ModuleEntryFields {
    module: String,
}

impl<'de> Deserialize<'de> for BuildEntry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            Project(PathBuf),
            Module(ModuleEntryFields),
        }
        Ok(match Written::deserialize(deserializer)? {
            Written::Project(path) => Self::Project(path),
            Written::Module(fields) => Self::Module {
                module: fields.module,
            },
        })
    }
}

impl ValidateFragment for BuildEntry {
    fn validate_fragment(value: &toml::Value, context: &mut FragmentContext<'_>) {
        fragment::string_or_table::<ModuleEntryFields>(value, context);
    }
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
    #[serde(rename = "par-hooks")]
    pub par_hooks: bool,
    #[serde(rename = "hook-jobs")]
    pub hook_jobs: HookJobs,
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
            par_hooks: true,
            hook_jobs: HookJobs::Auto,
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
    fn hook_jobs_accepts_auto_or_positive_integer() {
        for (value, expected) in [("\"auto\"", 8), ("3", 3)] {
            let config: BuildSectionConfig =
                toml::from_str(&format!("hook-jobs = {value}")).unwrap();
            assert!(config.par_hooks);
            assert_eq!(config.hook_jobs.resolved(8), expected);
        }
        for value in ["0", "-1", "1.5", "true", "\"3\"", "\"default\""] {
            assert!(toml::from_str::<BuildSectionConfig>(&format!("hook-jobs = {value}")).is_err());
        }
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
}
