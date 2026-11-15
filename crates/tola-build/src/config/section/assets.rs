//! `[assets]` section configuration.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tola_address::UrlPath;
use tola_config::Config;

use crate::config::{ConfigDiagnostics, FieldPath};
pub use tola_module::assets::{
    AssetFileDeclaration, AssetTreeDeclaration, AssetUrl, AssetUrlError, AssetUrlPrefix,
};

/// Publish images, stylesheets, and other static files at the URLs you choose.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "assets")]
pub struct AssetsConfig {
    /// Set `true` to add `?h=...` to URLs from `asset-url`, so a changed file gets a new URL.
    /// The published filename stays the same. Omit this setting to leave the query off.
    #[serde(rename = "cache-busting")]
    #[config(name = "cache-busting")]
    pub cache_busting: bool,

    /// Publish files from a directory below a URL prefix. For example,
    /// `{ source = "static/icons", url-prefix = "/icons" }`. Use `include` and `exclude`
    /// to choose which files to publish. Set `follow-file-symlinks = true` on a tree
    /// to publish the files its selected symbolic links point to.
    #[config(collection = inline, fragment_with = crate::config::fragment::asset_trees)]
    pub trees: Vec<AssetTreeDeclaration>,

    /// Set the URL for each source file, for example
    /// `{ source = "static/robots.txt", url = "/robots.txt" }`. `source` is relative to the
    /// site root. `url` starts with `/`. For a file also matched by `trees`, `files` takes priority.
    #[config(collection = inline, fragment_with = crate::config::fragment::asset_files)]
    pub files: Vec<AssetFileDeclaration>,
}

impl AssetsConfig {
    /// What `tola help "[assets]"` adds under its table.
    pub const HELP: &'static str = r#"
### Publish a logo

Put a PNG at `static/logo.png` and add this to `tola.toml`.

```toml
[assets]
files = [{ source = "static/logo.png", url = "/assets/logo.png" }]
```

Add this to a page such as `content/index.typ` in a site created with `tola init`.

```typst
#import "@tola/address:0.0.0": asset-url
#html.img(src: asset-url("/assets/logo.png"), alt: "Site logo")
```

Run `tola build` from the site directory. With the defaults, the file is written to
`public/assets/logo.png` and the page refers to `/assets/logo.png`. With
`site.base-path = "/docs/"`, the link becomes `/docs/assets/logo.png` and the file path stays
the same. Turn on `cache-busting` to add `?h=...` to the link. The hash changes when the
published bytes change.

### Publish a directory

When you split site configuration with [`include`](tola-help://config/include), asset
declarations from the files are combined. Use different output paths for each declaration.

Use `trees = [{ source = "static/icons", url-prefix = "/icons" }]` to publish the files
under `static/icons/` at `/icons/`. Paths under `source` are preserved.
By default, all files in the directory are included. For example,
`include = ["static/icons/**/*.svg"]` selects SVGs and `exclude = ["static/icons/drafts"]`
omits that directory. Patterns are case-sensitive and relative to the site root, or to the
module root for a module declaration. `include = []` selects nothing.

An exact `files` declaration takes precedence over `trees` for the same source, even if
that file declaration is disabled. Keep tree source directories from nesting and use distinct,
non-overlapping URL prefixes. Each output path must be unique across pages, assets, and hooks.

Use `enable = false` to disable a declaration or `enabled-by = "images.enable"` to read a
boolean option. Quote segments that contain a literal dot. Use `minify = false` to preserve
the source bytes.

Tola publishes enabled assets even when no page links to them. A `before-build` hook can
create asset sources. Declare files from a `generate-outputs` hook in its own `outputs` list.

### Publish a file through a symbolic link

From the root of a site created with `tola init`, run these commands to create a file and
link to it from `static/downloads/`.

```sh
mkdir -p shared static/downloads
printf '%s\n' 'Project notes' > shared/notes.txt
ln -s ../../shared/notes.txt static/downloads/guide.txt
```

Add this tree to the site's `tola.toml`.

```toml
[[assets.trees]]
source = "static/downloads"
url-prefix = "/downloads"
follow-file-symlinks = true
```

Run `tola build` from the site root. With the default publish directory, Tola writes
`public/downloads/guide.txt` as a regular file containing `Project notes`. The published
path uses the link's name, and the content comes from `shared/notes.txt`. To link to the file
from a page, add this to `content/index.typ` and rebuild. The link points to
`/downloads/guide.txt` with the default `site.base-path`.

```typst
#import "@tola/address:0.0.0": asset-url
#link(asset-url("/downloads/guide.txt"))[Download the notes]
```

`follow-file-symlinks` defaults to `false` and applies only to members of this tree.
Tola checks `include` and `exclude` against the link's path. A selected symbolic link
fails the build if the setting is off. With it on, the link must point to a readable
regular file. A broken link or a link to a directory still fails the build. Use
`exclude` to skip such a link. To publish the contents of a linked directory, exclude
the link and declare the target directory as a separate tree.

The target may be outside `source`, as in the example. A module's links must stay
inside that module. With `--pure`, the target must also be inside the site. As with
other assets, keep the target outside `build.content-dir` and `build.publish-dir`."#;

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
            crate::config::boundaries::validate_asset_source_boundaries(
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

        crate::config::boundaries::validate_asset_source_boundaries(
            &config,
            &site,
            &content,
            &output,
            &mut diagnostics,
        );

        assert!(diagnostics.to_string().contains("content root"));
    }
}
