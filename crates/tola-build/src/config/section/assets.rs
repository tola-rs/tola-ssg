//! `[assets]` section configuration.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tola_address::UrlPath;
use tola_config::Config;

use crate::config::{ConfigDiagnostics, FieldPath};
pub use tola_module::assets::{
    AssetFileDeclaration, AssetTreeDeclaration, AssetUrl, AssetUrlError, AssetUrlPrefix,
};

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

    /// Selected members of source directories published below a URL prefix, each written as
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
    /// What `tola help "[assets]"` adds under its table.
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
Set `include` and `exclude` on a tree to select members using case-sensitive paths and globs
relative to the declaring site or module root. For the tree above,
`include = [\"static/web-assets/**/*.svg\"]` selects its SVG files at any depth and
`exclude = [\"static/web-assets/drafts\"]` skips its `drafts` directory. A matched directory
selects its descendants within the tree. Omitting `include` selects the whole tree, while
`include = []` selects no files. `exclude` defaults to an empty list and applies only to that tree.
Use `enable = false` to disable a declaration or `enabled-by` to require a boolean site option.
A disabled exact file still takes over its tree member. Set `minify = false` to preserve bytes.
A `before-build` hook can write that source; a `generate-outputs` hook adds its outputs directly
and needs no `[assets]` entry.

Link a published declaration with `asset-url`, passing its declared URL rather than its source
path. With `cache-busting = true`, the result carries `?h=...` identifying the published bytes;
changed bytes give a new URL while the output filename stays the same. The result already includes
`site.base-path`, so use it directly:

```typst
#import \"@tola/address:0.0.0\": asset-url

#let style = asset-url(\"/assets/css/site.css\")   // /assets/css/site.css (or /assets/css/site.css?h=... with cache-busting)
#let script = asset-url(\"/assets/app.js\")        // /assets/app.js (or /assets/app.js?h=... with cache-busting)

```

One producer per output path: the Bundle's compiled result, an `[assets]` declaration, or a hook.
One hard rule governs declarations: **no two declarations may claim the same output path.**

Two `trees` sources cannot nest. A second tree inside the first fails the build:

```toml
[assets]
trees = [
  { source = \"static/assets\", url-prefix = \"/assets\" },
  { source = \"static/assets/css\", url-prefix = \"/css\" },
]
```

Two declarations claiming one output path fail the same way — two `files` writing to one `url`:

```toml
files = [
  { source = \"static/a.css\", url = \"/app.css\" },
  { source = \"static/b.css\", url = \"/app.css\" },
]
```

One exception: a `files` declaration may take a member over from a tree (the stylesheet above) —
that member is then published by the declaration, and the tree publishes the rest. Before
publishing, Tola checks these rules across `[assets]`, the Bundle, and hook outputs at once.";

    pub fn tree_sources(&self) -> impl Iterator<Item = &Path> {
        self.trees.iter().map(AssetTreeDeclaration::source)
    }

    pub fn file_sources(&self) -> impl Iterator<Item = &Path> {
        self.files.iter().map(AssetFileDeclaration::source)
    }

    pub fn normalize(&mut self, root: &Path) {
        for declaration in &mut self.trees {
            declaration.read_source(root);
        }
        for declaration in &mut self.files {
            declaration.read_source(root);
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
            if !tree.enable || tree.enabled_by.is_some() {
                continue;
            }
            let url_prefix = tree.url_prefix().url_path();
            for (other_index, other) in self.trees.iter().enumerate().skip(index + 1) {
                if !other.enable || other.enabled_by.is_some() {
                    continue;
                }
                let other_url_prefix = other.url_prefix().url_path();
                report_url_prefix_overlap(url_prefix, index, other_url_prefix, other_index, diag);
            }
        }

        for (index, file) in self.files.iter().enumerate() {
            if !file.enable || file.enabled_by.is_some() {
                continue;
            }
            let url = file.url().url_path();
            let url_key = url.url_collision_key();
            for (other_index, other) in self.files.iter().enumerate().skip(index + 1) {
                if !other.enable || other.enabled_by.is_some() {
                    continue;
                }
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
            if !tree.enable || tree.enabled_by.is_some() {
                continue;
            }
            let url_prefix = tree.url_prefix().url_path();
            let prefix_key = url_prefix.url_collision_key();
            for (file_index, file) in self.files.iter().enumerate() {
                if !file.enable || file.enabled_by.is_some() {
                    continue;
                }
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
    left_idx: usize,
    right: &UrlPath,
    right_idx: usize,
    diag: &mut ConfigDiagnostics,
) {
    let field = AssetsConfig::FIELDS.trees;
    let left_key = left.url_collision_key();
    let right_key = right.url_collision_key();
    if left_key == right_key {
        diag.error(
            field,
            format!(
                "`{}[{right_idx}]` URL `{}` duplicates `{}[{left_idx}]` URL `{}`",
                field.as_str(),
                right,
                field.as_str(),
                left
            ),
        );
    } else if tola_address::portable_key_is_below(&left_key, &right_key) {
        report_contained_url(right, right_idx, left, left_idx, diag);
    } else if tola_address::portable_key_is_below(&right_key, &left_key) {
        report_contained_url(left, left_idx, right, right_idx, diag);
    }
}

fn report_contained_url(
    parent: &UrlPath,
    parent_idx: usize,
    child: &UrlPath,
    child_idx: usize,
    diag: &mut ConfigDiagnostics,
) {
    let field = AssetsConfig::FIELDS.trees;
    diag.error_with_help(
        field,
        format!(
            "`{}[{child_idx}]` URL `{}` is inside `{}[{parent_idx}]` URL `{}`",
            field.as_str(),
            child,
            field.as_str(),
            parent
        ),
        "Choose URL prefixes that do not contain one another",
    );
}

fn logical_path_is_inside(path: &Path, parent: &Path) -> bool {
    let path = tola_input::lexical_path_identity(path);
    let parent = tola_input::lexical_path_identity(parent);
    path != parent && path.starts_with(parent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::tests::file_declaration;
    use std::path::PathBuf;

    fn tree_declaration(
        source: impl Into<PathBuf>,
        prefix: impl AsRef<str>,
    ) -> AssetTreeDeclaration {
        AssetTreeDeclaration::new(source, AssetUrlPrefix::parse(prefix.as_ref()).unwrap())
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
    fn sources_cannot_overlap_exclusive_roots() {
        let directory = tempfile::tempdir().unwrap();
        let site = directory.path();
        let content = site.join("content");
        let output = site.join("public");
        let internal = site.join(tola_input::INTERNAL_DIR);
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
