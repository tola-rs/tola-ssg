//! `[assets]` section configuration.
//!
//! `source` names a filesystem path; `url` and `url-prefix` are site-root URL
//! addresses and prefixes, not filesystem locations.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tola_config::Config;

use crate::config::{ConfigDiagnostics, FieldPath};
use tola_address::{OutputPath, OutputPathError, UrlPath, UrlPathError};

/// A configured asset URL could not establish a portable output identity.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AssetUrlError {
    #[error(transparent)]
    Url(#[from] UrlPathError),
    #[error(transparent)]
    Output(#[from] OutputPathError),
    #[error("asset output `{output}` is reserved for Tola")]
    Reserved { output: OutputPath },
}

/// A validated site-root prefix for an asset tree, with a canonical trailing slash.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(try_from = "String", into = "String")]
pub struct AssetUrlPrefix {
    url: UrlPath,
    output: Option<OutputPath>,
}

impl AssetUrlPrefix {
    pub fn parse(raw: &str) -> Result<Self, AssetUrlError> {
        let url = UrlPath::parse(raw)?;
        // A prefix names a directory, so `/assets` and `/assets/` are one prefix.
        let url = if url.as_str().trim_matches('/').is_empty() {
            url
        } else {
            let normalized = format!("/{}/", url.as_str().trim_matches('/'));
            UrlPath::from_decoded(&normalized)?
        };
        let output = if url.as_str() != "/" {
            let output = OutputPath::parse(url.as_str().trim_matches('/'))?;
            require_asset_output(&output)?;
            Some(output)
        } else {
            None
        };
        Ok(Self { url, output })
    }

    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    pub fn url_path(&self) -> &UrlPath {
        &self.url
    }

    /// The output path every member of this tree is published below, absent for the root prefix.
    pub(crate) fn output_root(&self) -> Option<&OutputPath> {
        self.output.as_ref()
    }

    pub(crate) fn output_for(&self, member: &OutputPath) -> Result<OutputPath, AssetUrlError> {
        match &self.output {
            Some(prefix) => Ok(prefix.join(member)),
            None => {
                require_asset_output(member)?;
                Ok(member.clone())
            }
        }
    }
}

impl TryFrom<String> for AssetUrlPrefix {
    type Error = AssetUrlError;
    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::parse(&raw)
    }
}

impl From<AssetUrlPrefix> for String {
    fn from(prefix: AssetUrlPrefix) -> Self {
        let encoded = prefix.url.to_encoded();
        if encoded == "/" {
            encoded
        } else {
            encoded.trim_end_matches('/').to_owned()
        }
    }
}

impl std::fmt::Display for AssetUrlPrefix {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One validated site-root asset URL and the logical output file it maps to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct AssetUrl {
    url: UrlPath,
    output: OutputPath,
}

impl AssetUrl {
    pub fn parse(raw: &str) -> Result<Self, AssetUrlError> {
        let url = UrlPath::parse(raw)?;
        let output = tola_address::asset_output_from_url(&url)?;
        require_asset_output(&output)?;
        Ok(Self { url, output })
    }

    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    pub fn url_path(&self) -> &UrlPath {
        &self.url
    }

    pub fn output_path(&self) -> &OutputPath {
        &self.output
    }
}

impl TryFrom<String> for AssetUrl {
    type Error = AssetUrlError;
    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::parse(&raw)
    }
}

impl From<AssetUrl> for String {
    fn from(url: AssetUrl) -> Self {
        url.url.to_encoded()
    }
}

impl std::fmt::Display for AssetUrl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn require_asset_output(output: &OutputPath) -> Result<(), AssetUrlError> {
    if output.is_reserved_for_non_system_output() {
        Err(AssetUrlError::Reserved {
            output: output.clone(),
        })
    } else {
        Ok(())
    }
}

/// One site directory published below one site-root URL prefix.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AssetTreeDeclaration {
    /// Logical source directory. TOML supplies a site-relative path; resolved runtime
    /// configuration holds its logical absolute path without following symlinks.
    pub source: PathBuf,

    /// Site-root URL prefix for descendants of `source`.
    #[serde(rename = "url-prefix")]
    pub url_prefix: AssetUrlPrefix,
}

impl AssetTreeDeclaration {
    /// Declare a source tree with an already validated URL prefix.
    pub fn new(source: impl Into<PathBuf>, url_prefix: AssetUrlPrefix) -> Self {
        Self {
            source: source.into(),
            url_prefix,
        }
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn url_prefix(&self) -> &AssetUrlPrefix {
        &self.url_prefix
    }

    fn resolve_source(&mut self, root: &Path) {
        self.source = crate::filesystem::lexical_path_identity(&root.join(&self.source));
    }
}

/// One site file published at one exact site-root URL.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AssetFileDeclaration {
    /// Logical source file. TOML supplies a site-relative path; resolved runtime configuration
    /// holds its logical absolute path without following symlinks.
    pub source: PathBuf,

    /// Exact site-root URL of this file. The URL is an address only: its extension
    /// does not transcode the source bytes.
    pub url: AssetUrl,
}

impl AssetFileDeclaration {
    /// Declare a source file with an already validated URL and output identity.
    pub fn new(source: impl Into<PathBuf>, url: AssetUrl) -> Self {
        Self {
            source: source.into(),
            url,
        }
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn url(&self) -> &AssetUrl {
        &self.url
    }

    fn resolve_source(&mut self, root: &Path) {
        self.source = crate::filesystem::lexical_path_identity(&root.join(&self.source));
    }
}

/// Publish declared files and directories at the site-root URLs they name.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "assets")]
pub struct AssetsConfig {
    /// Give changed published bytes a new `?h=...` in URLs returned by `asset-url`.
    /// Published paths are not renamed. Off by default.
    #[serde(rename = "cache-busting")]
    #[config(name = "cache-busting")]
    pub cache_busting: bool,

    /// Source directories published below a URL prefix, each written as
    /// `{ source = "static/icons", url-prefix = "/icons" }`.
    #[config(collection = inline)]
    pub trees: Vec<AssetTreeDeclaration>,

    /// Single source files published at the exact URL they name, each written as
    /// `{ source = "static/robots.txt", url = "/robots.txt" }`. A file inside a
    /// declared tree is published here instead.
    #[config(collection = inline)]
    pub files: Vec<AssetFileDeclaration>,
}

impl AssetsConfig {
    /// What `tola help config assets` adds under its table.
    pub const HELP: &'static str = "\
A declared source is published whether or not a document references or reads it. A directory
publishes its members below a URL prefix; an exact file declaration gives one source its own URL:

```toml
[assets]
cache-busting = true
trees = [{ source = \"static/web-assets\", url-prefix = \"/assets\" }]
files = [{ source = \"static/web-assets/tailwind-output/site.css\", url = \"/assets/css/site.css\" }]
```

Here `files` takes over the generated stylesheet from the tree: it publishes at `/assets/css/site.css`,
not `/assets/tailwind-output/site.css`. The tree still publishes its other members. `source` is a
path relative to the site root; `url` and `url-prefix` are site-root URL paths, before `site.base-path`.
A `before-build` hook can write that source; a `generate-outputs` hook adds its outputs directly
and needs no `[assets]` entry.

Link a published declaration with `asset-url`, passing its declared URL rather than its source
path. With `cache-busting = true`, the result carries `?h=...` identifying the published bytes;
changed bytes give a new URL while the output filename stays the same. The result already includes
`site.base-path`, so use it directly:

```typst
#import \"@tola/address:0.0.0\": asset-url
#let styles = asset-url(\"/assets/css/site.css\")
#let script = asset-url(\"/assets/app.js\")
```

Each output path has one owner. Give tree declarations disjoint source paths and URL prefixes;
an exact file may take over a tree member, but two producers claiming the same output fail the
build. Tola checks configured assets together with Bundle and hook outputs before publishing.";

    pub fn tree_sources(&self) -> impl Iterator<Item = &Path> {
        self.trees.iter().map(AssetTreeDeclaration::source)
    }

    pub fn file_sources(&self) -> impl Iterator<Item = &Path> {
        self.files.iter().map(AssetFileDeclaration::source)
    }

    pub fn normalize(&mut self, root: &Path) {
        for declaration in &mut self.trees {
            declaration.resolve_source(root);
        }
        for declaration in &mut self.files {
            declaration.resolve_source(root);
        }
    }

    pub fn validate_paths(&self, diag: &mut ConfigDiagnostics) {
        for (index, declaration) in self.trees.iter().enumerate() {
            validate_source_path(declaration.source(), index, Self::FIELDS.trees, diag);
        }

        for (index, declaration) in self.files.iter().enumerate() {
            validate_source_path(declaration.source(), index, Self::FIELDS.files, diag);
        }
    }

    pub fn validate(&self, diag: &mut ConfigDiagnostics) {
        self.validate_source_ownership(diag);
        self.validate_output_ownership(diag);
    }

    fn validate_source_ownership(&self, diag: &mut ConfigDiagnostics) {
        // A `assets.files` declaration owns the output of the source it
        // names, so a tree that also covers that source publishes its other
        // members and skips this one. Two trees have no such owner.
        for (index, current) in self.trees.iter().enumerate() {
            for (other_index, other) in self.trees.iter().enumerate().skip(index + 1) {
                let (parent, child) = if logical_path_is_inside(current.source(), other.source()) {
                    (other_index, index)
                } else if logical_path_is_inside(other.source(), current.source()) {
                    (index, other_index)
                } else {
                    continue;
                };
                diag.error_with_help(
                    Self::FIELDS.trees,
                    format!(
                        "`assets.trees[{child}]` source is inside `assets.trees[{parent}]`"
                    ),
                    "Keep the parent source alone, or replace it with non-overlapping child sources",
                );
            }
        }
    }

    fn validate_output_ownership(&self, diag: &mut ConfigDiagnostics) {
        for (index, tree) in self.trees.iter().enumerate() {
            let url_prefix = tree.url_prefix().url_path();
            for (other_index, other) in self.trees.iter().enumerate().skip(index + 1) {
                let other_url_prefix = other.url_prefix().url_path();
                report_url_prefix_overlap(
                    url_prefix,
                    Self::FIELDS.trees,
                    index,
                    other_url_prefix,
                    Self::FIELDS.trees,
                    other_index,
                    diag,
                );
            }
        }

        for (index, file) in self.files.iter().enumerate() {
            let url = file.url().url_path();
            let url_key = url.url_collision_key();
            for (other_index, other) in self.files.iter().enumerate().skip(index + 1) {
                let other_url = other.url().url_path();
                let other_key = other_url.url_collision_key();
                if tola_address::portable_keys_overlap(&url_key, &other_key) {
                    diag.error_with_help(
                        Self::FIELDS.files,
                        format!(
                            "`assets.files[{other_index}]` URL `{}` overlaps `assets.files[{index}]` URL `{}`",
                            other_url, url
                        ),
                        "Give each file a distinct URL",
                    );
                }
            }
        }

        for (tree_index, tree) in self.trees.iter().enumerate() {
            let url_prefix = tree.url_prefix().url_path();
            let prefix_key = url_prefix.url_collision_key();
            for (file_index, file) in self.files.iter().enumerate() {
                let url = file.url().url_path();
                let url_key = url.url_collision_key();
                // A file URL inside a tree's prefix is how an exact declaration
                // takes over one source the tree covers: the tree publishes its
                // other members, and the declaration keeps this address. A file
                // URL at or above the tree's prefix cannot work: the tree's
                // members need the file's own path to be a directory.
                if url_key == prefix_key {
                    diag.error_with_help(
                        Self::FIELDS.trees,
                        format!(
                            "`assets.trees[{tree_index}]` URL prefix `{}` duplicates `assets.files[{file_index}]` URL `{}`",
                            url_prefix, url
                        ),
                        "Change the file URL or the tree prefix",
                    );
                } else if tola_address::portable_key_is_below(&prefix_key, &url_key) {
                    diag.error_with_help(
                        Self::FIELDS.trees,
                        format!(
                            "`assets.trees[{tree_index}]` URL prefix `{}` is below `assets.files[{file_index}]` URL `{}`",
                            url_prefix, url
                        ),
                        "Change the file URL or tree prefix so the tree is not below a file URL",
                    );
                }
            }
        }
    }
}

fn validate_source_path(path: &Path, idx: usize, field: FieldPath, diag: &mut ConfigDiagnostics) {
    let key = format!("{}[{idx}].source", field.as_str());
    super::path::validate_site_relative_path(path, &key, field, diag);
}

fn report_url_prefix_overlap(
    left: &UrlPath,
    left_field: FieldPath,
    left_idx: usize,
    right: &UrlPath,
    right_field: FieldPath,
    right_idx: usize,
    diag: &mut ConfigDiagnostics,
) {
    let left_key = left.url_collision_key();
    let right_key = right.url_collision_key();
    if left_key == right_key {
        diag.error(
            right_field,
            format!(
                "`{}[{right_idx}]` URL `{}` duplicates `{}[{left_idx}]` URL `{}`",
                right_field.as_str(),
                right,
                left_field.as_str(),
                left
            ),
        );
    } else if tola_address::portable_key_is_below(&left_key, &right_key) {
        report_contained_url(
            right,
            right_field,
            right_idx,
            left,
            left_field,
            left_idx,
            diag,
        );
    } else if tola_address::portable_key_is_below(&right_key, &left_key) {
        report_contained_url(
            left,
            left_field,
            left_idx,
            right,
            right_field,
            right_idx,
            diag,
        );
    }
}

fn report_contained_url(
    parent: &UrlPath,
    parent_field: FieldPath,
    parent_idx: usize,
    child: &UrlPath,
    child_field: FieldPath,
    child_idx: usize,
    diag: &mut ConfigDiagnostics,
) {
    diag.error_with_help(
        child_field,
        format!(
            "`{}[{child_idx}]` URL `{}` is inside `{}[{parent_idx}]` URL `{}`",
            child_field.as_str(),
            child,
            parent_field.as_str(),
            parent
        ),
        "Choose URL prefixes that do not contain one another",
    );
}

fn logical_path_is_inside(path: &Path, parent: &Path) -> bool {
    let path = crate::filesystem::lexical_path_identity(path);
    let parent = crate::filesystem::lexical_path_identity(parent);
    path != parent && path.starts_with(parent)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_declaration(
        source: impl Into<PathBuf>,
        prefix: impl AsRef<str>,
    ) -> AssetTreeDeclaration {
        AssetTreeDeclaration::new(source, AssetUrlPrefix::parse(prefix.as_ref()).unwrap())
    }

    fn file_declaration(source: impl Into<PathBuf>, url: impl AsRef<str>) -> AssetFileDeclaration {
        AssetFileDeclaration::new(source, AssetUrl::parse(url.as_ref()).unwrap())
    }

    #[test]
    fn incomplete_declarations_are_refused() {
        for source in [
            "trees = [\"assets\"]",
            "trees = [{ source = \"assets\" }]",
            "trees = [{ url-prefix = \"/assets\" }]",
            "files = [\"favicon.ico\"]",
            "files = [{ source = \"favicon.ico\" }]",
            "files = [{ url = \"/favicon.ico\" }]",
        ] {
            assert!(
                toml::from_str::<AssetsConfig>(source).is_err(),
                "accepted {source}"
            );
        }
    }

    #[test]
    fn source_paths_must_not_be_empty() {
        let config: AssetsConfig = toml::from_str(
            r#"
trees = [{ source = "", url-prefix = "/assets" }]
files = [{ source = "", url = "/favicon.ico" }]
"#,
        )
        .unwrap();
        let mut diagnostics = ConfigDiagnostics::new();

        config.validate_paths(&mut diagnostics);

        assert_eq!(
            diagnostics
                .errors()
                .iter()
                .map(|error| error.field)
                .collect::<Vec<_>>(),
            [AssetsConfig::FIELDS.trees, AssetsConfig::FIELDS.files]
        );
    }

    #[test]
    fn sources_resolve_below_the_root() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let mut config: AssetsConfig = toml::from_str(
            r#"
trees = [{ source = "assets/images", url-prefix = "/images" }]
files = [{ source = "assets/styles/base.css", url = "/styles/base.css" }]
"#,
        )
        .unwrap();

        config.normalize(root);

        assert_eq!(config.trees[0].source(), root.join("assets/images"));
        assert_eq!(config.trees[0].url_prefix().as_str(), "/images/");
        assert_eq!(
            config.files[0].source(),
            root.join("assets/styles/base.css")
        );
        assert_eq!(config.files[0].url().as_str(), "/styles/base.css");
    }

    #[test]
    fn cache_busting_defaults_to_off() {
        let default: AssetsConfig = toml::from_str(
            r#"
trees = [{ source = "assets", url-prefix = "/assets" }]
files = [{ source = "assets/app.js", url = "/app.js" }]
"#,
        )
        .unwrap();
        assert!(!default.cache_busting);

        let enabled: AssetsConfig = toml::from_str("cache-busting = true").unwrap();
        assert!(enabled.cache_busting);
        assert!(
            toml::from_str::<AssetsConfig>("cache-busting = \"yes\"").is_err(),
            "the switch is a boolean"
        );
    }

    #[test]
    fn portable_urls_have_one_owner() {
        let config = AssetsConfig {
            trees: vec![
                tree_declaration("first-tree", "/Assets"),
                tree_declaration("second-tree", "/%61ssets/icons"),
            ],
            files: vec![
                file_declaration("first.svg", "/Logo.svg"),
                file_declaration("second.svg", "/%4cogo.svg"),
                file_declaration("third.svg", "/logo.svg"),
            ],
            ..AssetsConfig::default()
        };
        let mut diagnostics = ConfigDiagnostics::new();

        config.validate(&mut diagnostics);

        let errors = diagnostics.errors();
        assert_eq!(errors.len(), 4);
        assert!(
            errors[0].message.contains("`assets.trees[1]`")
                && errors[0].message.contains("`assets.trees[0]`"),
            "{}",
            errors[0].message
        );
        assert!(
            errors[1..]
                .iter()
                .all(|error| error.field == AssetsConfig::FIELDS.files)
        );
    }

    #[test]
    fn file_may_own_source_inside_tree() {
        let config = AssetsConfig {
            trees: vec![tree_declaration("assets", "/assets")],
            files: vec![
                file_declaration("assets/app.js", "/assets/app.js"),
                file_declaration("assets/theme.css", "/js/theme.css"),
            ],
            ..AssetsConfig::default()
        };
        let mut diagnostics = ConfigDiagnostics::new();

        config.validate(&mut diagnostics);

        let result = diagnostics.into_result();
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn url_prefixes_cannot_contain_each_other() {
        let files = AssetsConfig {
            trees: Vec::new(),
            files: vec![
                file_declaration("download", "/download"),
                file_declaration("readme", "/download/readme.txt"),
            ],
            ..AssetsConfig::default()
        };
        let mut file_diagnostics = ConfigDiagnostics::new();
        files.validate(&mut file_diagnostics);
        assert!(file_diagnostics.into_result().is_err());

        for (tree_prefix, file_url) in [("/downloads/icons", "/downloads"), ("/assets", "/assets")]
        {
            let config = AssetsConfig {
                trees: vec![tree_declaration("tree", tree_prefix)],
                files: vec![file_declaration("file", file_url)],
                ..AssetsConfig::default()
            };
            let mut diagnostics = ConfigDiagnostics::new();
            config.validate(&mut diagnostics);
            assert!(
                diagnostics.into_result().is_err(),
                "{tree_prefix} {file_url}"
            );
        }
    }

    #[test]
    fn url_parse_derives_output_identities() {
        assert_eq!(AssetUrlPrefix::parse("/").unwrap().as_str(), "/");
        assert!(AssetUrl::parse("/").is_err());
        let prefix = AssetUrlPrefix::parse("/%61ssets/icons").unwrap();
        assert_eq!(prefix.url_path().as_str(), "/assets/icons/");
        let file = AssetUrl::parse("/assets/%4cogo.svg").unwrap();
        assert_eq!(file.url_path().as_str(), "/assets/Logo.svg");
        assert_eq!(file.output_path().as_str(), "assets/Logo.svg");
    }

    #[test]
    fn url_parse_refuses_reserved_outputs() {
        for raw in [
            "images",
            "//images",
            " /images",
            "/images?query",
            "/images#part",
        ] {
            assert!(AssetUrlPrefix::parse(raw).is_err(), "{raw}");
            assert!(AssetUrl::parse(raw).is_err(), "{raw}");
        }
        let raw = "/_tola/private";
        assert!(matches!(
            AssetUrlPrefix::parse(raw),
            Err(AssetUrlError::Reserved { .. })
        ));
        assert!(matches!(
            AssetUrl::parse(raw),
            Err(AssetUrlError::Reserved { .. })
        ));
        assert_eq!(
            AssetUrlPrefix::parse("/assets").unwrap(),
            AssetUrlPrefix::parse("/assets/").unwrap()
        );
    }

    #[test]
    fn typed_urls_round_trip_through_toml() {
        let config = AssetsConfig {
            trees: vec![tree_declaration("assets", "/图")],
            files: vec![file_declaration("logo.svg", "/图/logo.svg")],
            ..AssetsConfig::default()
        };
        let encoded = toml::to_string(&config).unwrap();
        let decoded: AssetsConfig = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.trees, config.trees);
        assert_eq!(decoded.files, config.files);
        assert_eq!(
            String::from(config.trees[0].url_prefix().clone()),
            "/%E5%9B%BE"
        );
    }

    #[test]
    fn sources_cannot_overlap_exclusive_roots() {
        let directory = tempfile::tempdir().unwrap();
        let site = directory.path();
        let content = site.join("content");
        let output = site.join("public");
        let internal = site.join(crate::filesystem::INTERNAL_DIR);
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(&output).unwrap();
        std::fs::create_dir_all(&internal).unwrap();

        for (source, tree) in [
            (content.join("assets"), true),
            (content.join("download.bin"), false),
            (output.join("assets"), true),
            (internal.join("cache.bin"), false),
        ] {
            let config = if tree {
                AssetsConfig {
                    trees: vec![tree_declaration(source, "/assets")],
                    ..AssetsConfig::default()
                }
            } else {
                AssetsConfig {
                    files: vec![file_declaration(source, "/download.bin")],
                    ..AssetsConfig::default()
                }
            };
            let mut diagnostics = ConfigDiagnostics::new();
            crate::config::loading::validate_asset_source_boundaries(
                &config,
                site,
                &content,
                &output,
                &mut diagnostics,
            );
            assert!(!diagnostics.errors().is_empty());
        }
    }

    #[cfg(unix)]
    #[test]
    fn source_symlink_cannot_alias_owned_roots() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let site = directory.path().join("site");
        let content = site.join("content");
        let output = site.join("public");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::create_dir_all(&output).unwrap();
        let alias = site.join("external-looking");
        symlink(&content, &alias).unwrap();
        let config = AssetsConfig {
            trees: vec![tree_declaration(alias, "/assets")],
            ..AssetsConfig::default()
        };
        let mut diagnostics = ConfigDiagnostics::new();

        crate::config::loading::validate_asset_source_boundaries(
            &config,
            &site,
            &content,
            &output,
            &mut diagnostics,
        );

        assert!(diagnostics.to_string().contains("content root"));
    }
}
