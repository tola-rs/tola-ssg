//! `[build.assets]` section configuration.
//!
//! Assets are explicit source lists:
//! - `nested`: copy every file under a source directory to a public URL prefix.
//! - `flatten`: copy one source file to one public URL at the output root.
//!
//! Path entries are the normal form. Route objects exist only when the default
//! basename-derived URL is not the desired public URL.

use rustc_hash::FxHashMap;
use std::path::{Component, Path, PathBuf};

use macros::Config;
use serde::{Deserialize, Serialize};

use crate::config::{ConfigDiagnostics, FieldPath, PublicUrl};

#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(section = "build.assets")]
pub struct AssetsConfig {
    /// Nested directory assets.
    pub nested: Vec<NestedEntry>,

    /// Flattened file assets.
    pub flatten: Vec<FlattenEntry>,

    /// Content-local assets.
    /// `false` disables implicit content asset copying.
    /// `true` allows every non-content file under `build.content`.
    pub colocated: bool,
}

impl Default for AssetsConfig {
    fn default() -> Self {
        Self {
            nested: vec![NestedEntry::Path("assets".into())],
            flatten: Vec::new(),
            colocated: false,
        }
    }
}

impl AssetsConfig {
    pub fn nested_sources(&self) -> impl Iterator<Item = &Path> {
        self.nested.iter().map(NestedEntry::source)
    }

    pub fn flatten_sources(&self) -> impl Iterator<Item = &Path> {
        self.flatten.iter().map(FlattenEntry::source)
    }

    pub fn sources(&self) -> impl Iterator<Item = &Path> {
        self.nested_sources().chain(self.flatten_sources())
    }

    #[allow(dead_code)]
    pub fn has_cname_in_flatten(&self) -> bool {
        self.flatten
            .iter()
            .any(|entry| entry.target().as_str() == "/CNAME")
    }

    pub fn normalize(&mut self, root: &Path) {
        for entry in &mut self.nested {
            entry.normalize(root);
        }
        for entry in &mut self.flatten {
            entry.normalize(root);
        }
    }

    pub fn validate_paths(&self, diag: &mut ConfigDiagnostics) {
        let nested_count = self.nested.len();
        for (i, entry) in self.nested.iter().enumerate() {
            validate_source_path(entry.source(), i, nested_count, Self::FIELDS.nested, diag);
            validate_target(entry.target(), i, nested_count, Self::FIELDS.nested, diag);
        }

        let flatten_count = self.flatten.len();
        for (i, entry) in self.flatten.iter().enumerate() {
            validate_source_path(entry.source(), i, flatten_count, Self::FIELDS.flatten, diag);
            validate_target(entry.target(), i, flatten_count, Self::FIELDS.flatten, diag);
        }
    }

    pub fn validate(&self, diag: &mut ConfigDiagnostics) {
        self.validate_sources(diag);
        self.validate_output_ownership(diag);
    }

    fn validate_sources(&self, diag: &mut ConfigDiagnostics) {
        for (idx, entry) in self.nested.iter().enumerate() {
            let source = entry.source();
            if source.exists() && !source.is_dir() {
                diag.error(
                    Self::FIELDS.nested,
                    format!("[{idx}] '{}' must be a directory", source.display()),
                );
            }
        }

        for (idx, entry) in self.flatten.iter().enumerate() {
            let source = entry.source();
            if source.exists() && !source.is_file() {
                diag.error(
                    Self::FIELDS.flatten,
                    format!("[{idx}] '{}' must be a file", source.display()),
                );
            }
        }

        for (i, current) in self.nested.iter().enumerate() {
            for (j, other) in self.nested.iter().enumerate().skip(i + 1) {
                report_source_overlap(
                    current.source(),
                    Self::FIELDS.nested,
                    i,
                    other.source(),
                    Self::FIELDS.nested,
                    j,
                    diag,
                );
            }
        }

        for (nested_idx, nested) in self.nested.iter().enumerate() {
            for (flatten_idx, flatten) in self.flatten.iter().enumerate() {
                report_source_overlap(
                    nested.source(),
                    Self::FIELDS.nested,
                    nested_idx,
                    flatten.source(),
                    Self::FIELDS.flatten,
                    flatten_idx,
                    diag,
                );
            }
        }

        for (i, current) in self.flatten.iter().enumerate() {
            for (j, other) in self.flatten.iter().enumerate().skip(i + 1) {
                if paths_equal(current.source(), other.source()) {
                    diag.error_with_hint(
                        Self::FIELDS.flatten,
                        format!(
                            "[{j}] '{}' uses the same source file as [{i}] '{}'",
                            other.source().display(),
                            current.source().display()
                        ),
                        "Each source file may be configured once.",
                    );
                }
            }
        }
    }

    fn validate_output_ownership(&self, diag: &mut ConfigDiagnostics) {
        let mut exact_files: FxHashMap<String, (FieldPath, usize, &'static str)> =
            FxHashMap::default();

        for (idx, nested) in self.nested.iter().enumerate() {
            let target = nested.target();
            validate_reserved_target(&target, Self::FIELDS.nested, idx, diag);
            for (other_idx, other) in self.nested.iter().enumerate().skip(idx + 1) {
                let other_target = other.target();
                report_url_prefix_overlap(
                    &target,
                    Self::FIELDS.nested,
                    idx,
                    &other_target,
                    Self::FIELDS.nested,
                    other_idx,
                    diag,
                );
            }
        }

        for (idx, flatten) in self.flatten.iter().enumerate() {
            let target = flatten.target();
            validate_reserved_target(&target, Self::FIELDS.flatten, idx, diag);
            if let Some((prev_field, prev_idx, prev_kind)) = exact_files.insert(
                target.as_str().to_string(),
                (Self::FIELDS.flatten, idx, "flatten"),
            ) {
                diag.error(
                    Self::FIELDS.flatten,
                    format!(
                        "[{idx}] URL '{}' conflicts with {prev_kind}[{prev_idx}]",
                        target
                    ),
                );
                if prev_field != Self::FIELDS.flatten {
                    diag.error(prev_field, "conflicting asset URL declared here");
                }
            }
        }

        for (nested_idx, nested) in self.nested.iter().enumerate() {
            let nested_target = nested.target();
            for (flatten_idx, flatten) in self.flatten.iter().enumerate() {
                let flatten_target = flatten.target();
                if flatten_target.as_str() == nested_target.as_str() {
                    diag.error_with_hint(
                        Self::FIELDS.flatten,
                        format!(
                            "[{flatten_idx}] URL '{}' duplicates nested[{nested_idx}] URL '{}'",
                            flatten_target, nested_target
                        ),
                        "Choose either the directory owner, or a different flattened output name.",
                    );
                } else if url_is_under_prefix(flatten_target.as_str(), nested_target.as_str()) {
                    diag.error_with_hint(
                        Self::FIELDS.flatten,
                        format!(
                            "[{flatten_idx}] URL '{}' is inside nested[{nested_idx}] URL '{}'",
                            flatten_target,
                            nested_target
                        ),
                        "Choose either the directory owner, or split assets into explicit non-overlapping routes.",
                    );
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum NestedEntry {
    Path(PathBuf),
    Route {
        dir: PathBuf,
        #[serde(rename = "as")]
        output_as: Option<PublicUrl>,
    },
}

impl NestedEntry {
    pub fn source(&self) -> &Path {
        match self {
            Self::Path(path) => path,
            Self::Route { dir, .. } => dir,
        }
    }

    pub fn target(&self) -> PublicUrl {
        match self {
            Self::Path(path) => default_target(path, "assets"),
            Self::Route { dir, output_as } => output_as
                .clone()
                .unwrap_or_else(|| default_target(dir, "assets")),
        }
    }

    #[cfg(test)]
    pub fn new(dir: impl Into<PathBuf>, output_as: impl Into<String>) -> Self {
        Self::with_as(dir, output_as)
    }

    #[cfg(test)]
    pub fn with_as(dir: impl Into<PathBuf>, output_as: impl Into<String>) -> Self {
        Self::Route {
            dir: dir.into(),
            output_as: Some(output_as.into().into()),
        }
    }

    fn normalize(&mut self, root: &Path) {
        match self {
            Self::Path(path) => {
                let output_as = Some(default_target(path, "assets"));
                let dir = crate::utils::path::normalize_path(&root.join(&*path));
                *self = Self::Route { dir, output_as };
            }
            Self::Route { dir, output_as } => {
                if output_as.is_none() {
                    *output_as = Some(default_target(dir, "assets"));
                }
                *dir = crate::utils::path::normalize_path(&root.join(&*dir));
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum FlattenEntry {
    Path(PathBuf),
    Route {
        file: PathBuf,
        #[serde(rename = "as")]
        output_as: Option<PublicUrl>,
    },
}

impl FlattenEntry {
    pub fn source(&self) -> &Path {
        match self {
            Self::Path(path) => path,
            Self::Route { file, .. } => file,
        }
    }

    pub fn target(&self) -> PublicUrl {
        match self {
            Self::Path(path) => default_target(path, "asset"),
            Self::Route { file, output_as } => output_as
                .clone()
                .unwrap_or_else(|| default_target(file, "asset")),
        }
    }

    #[cfg(test)]
    pub fn new(file: impl Into<PathBuf>, output_as: impl Into<String>) -> Self {
        Self::with_as(file, output_as)
    }

    #[cfg(test)]
    pub fn with_as(file: impl Into<PathBuf>, output_as: impl Into<String>) -> Self {
        Self::Route {
            file: file.into(),
            output_as: Some(output_as.into().into()),
        }
    }

    fn normalize(&mut self, root: &Path) {
        match self {
            Self::Path(path) => {
                let output_as = Some(default_target(path, "asset"));
                let file = crate::utils::path::normalize_path(&root.join(&*path));
                *self = Self::Route { file, output_as };
            }
            Self::Route { file, output_as } => {
                if output_as.is_none() {
                    *output_as = Some(default_target(file, "asset"));
                }
                *file = crate::utils::path::normalize_path(&root.join(&*file));
            }
        }
    }
}

fn validate_source_path(
    path: &Path,
    idx: usize,
    total: usize,
    field: FieldPath,
    diag: &mut ConfigDiagnostics,
) {
    for comp in path.components() {
        let reason = match comp {
            Component::ParentDir => Some("parent directory '..' not allowed"),
            Component::Prefix(_) | Component::RootDir => Some("absolute paths not allowed"),
            _ => None,
        };
        if let Some(reason) = reason {
            let prefix = if total > 1 {
                format!("[{idx}] ")
            } else {
                String::new()
            };
            diag.error(
                field,
                format!("{prefix}path '{}': {reason}", path.display()),
            );
        }
    }
}

fn default_target(source: &Path, fallback: &str) -> PublicUrl {
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback);
    PublicUrl::new(format!("/{name}"))
}

fn validate_target(
    target: PublicUrl,
    idx: usize,
    total: usize,
    field: FieldPath,
    diag: &mut ConfigDiagnostics,
) {
    target.validate_indexed(field, idx, total, diag);

    let label = if total > 1 {
        format!("[{idx}] ")
    } else {
        String::new()
    };
    let logical = target.logical_path();
    if logical.contains('/') {
        diag.error(
            field,
            format!(
                "{label}URL '{}' must be a root-level asset name",
                target.as_str()
            ),
        );
    }
}

fn validate_reserved_target(
    target: &PublicUrl,
    field: FieldPath,
    idx: usize,
    diag: &mut ConfigDiagnostics,
) {
    let logical = target.as_str().trim_start_matches('/');
    if logical == crate::asset::SYSTEM_ASSET_DIR
        || logical
            .strip_prefix(crate::asset::SYSTEM_ASSET_DIR)
            .is_some_and(|rest| rest.starts_with('/'))
    {
        diag.error(
            field,
            format!(
                "[{idx}] URL '{}' is reserved for generated assets",
                target.as_str()
            ),
        );
    }
}

fn report_source_overlap(
    left: &Path,
    left_field: FieldPath,
    left_idx: usize,
    right: &Path,
    right_field: FieldPath,
    right_idx: usize,
    diag: &mut ConfigDiagnostics,
) {
    if paths_equal(left, right) {
        diag.error_with_hint(
            right_field,
            format!(
                "[{right_idx}] source '{}' duplicates [{left_idx}] '{}'",
                right.display(),
                left.display()
            ),
            "Each physical asset source may be owned by one route.",
        );
    } else if path_is_inside(left, right) {
        report_contained_source(
            right,
            right_field,
            right_idx,
            left,
            left_field,
            left_idx,
            diag,
        );
    } else if path_is_inside(right, left) {
        report_contained_source(
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

fn report_contained_source(
    parent: &Path,
    parent_field: FieldPath,
    parent_idx: usize,
    child: &Path,
    child_field: FieldPath,
    child_idx: usize,
    diag: &mut ConfigDiagnostics,
) {
    diag.error_with_hint(
        child_field,
        format!(
            "[{child_idx}] source '{}' is inside [{parent_idx}] '{}'",
            child.display(),
            parent.display()
        ),
        "Choose either the parent source, or list explicit non-overlapping child sources.",
    );
    if parent_field != child_field {
        diag.hint(
            parent_field,
            format!("overlapping asset source starts at '{}'", parent.display()),
        );
    }
}

fn report_url_prefix_overlap(
    left: &PublicUrl,
    left_field: FieldPath,
    left_idx: usize,
    right: &PublicUrl,
    right_field: FieldPath,
    right_idx: usize,
    diag: &mut ConfigDiagnostics,
) {
    if left.as_str() == right.as_str() {
        diag.error(
            right_field,
            format!(
                "[{right_idx}] URL '{}' duplicates [{left_idx}] '{}'",
                right, left
            ),
        );
    } else if url_is_under_prefix(left.as_str(), right.as_str()) {
        report_contained_url(
            right,
            right_field,
            right_idx,
            left,
            left_field,
            left_idx,
            diag,
        );
    } else if url_is_under_prefix(right.as_str(), left.as_str()) {
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
    parent: &PublicUrl,
    _parent_field: FieldPath,
    parent_idx: usize,
    child: &PublicUrl,
    child_field: FieldPath,
    child_idx: usize,
    diag: &mut ConfigDiagnostics,
) {
    diag.error_with_hint(
        child_field,
        format!(
            "[{child_idx}] URL '{}' is inside [{parent_idx}] '{}'",
            child, parent
        ),
        "Public URL ownership must be explicit and non-overlapping.",
    );
}

fn path_is_inside(path: &Path, parent: &Path) -> bool {
    let path = crate::utils::path::normalize_existing_prefix(path);
    let parent = crate::utils::path::normalize_existing_prefix(parent);
    path != parent && path.starts_with(parent)
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    crate::utils::path::normalize_existing_prefix(left)
        == crate::utils::path::normalize_existing_prefix(right)
}

fn url_is_under_prefix(path: &str, prefix: &str) -> bool {
    let path = path.trim_end_matches('/');
    let prefix = prefix.trim_end_matches('/');
    path != prefix
        && path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colocated_default_false() {
        let config = AssetsConfig::default();

        assert_eq!(config.nested.len(), 1);
        assert_eq!(config.nested[0].source(), Path::new("assets"));
        assert_eq!(config.nested[0].target().as_str(), "/assets");
        assert!(config.flatten.is_empty());
        assert!(!config.colocated);
    }

    #[test]
    fn parses_path_assets_and_route_overrides() {
        let config: AssetsConfig = toml::from_str(
            r#"
nested = ["assets/images", { dir = "vendor/static", as = "/lib" }]
flatten = ["assets/styles/base.css", { file = "assets/CNAME", as = "/CNAME" }]
"#,
        )
        .unwrap();

        assert_eq!(config.nested[0].source(), Path::new("assets/images"));
        assert_eq!(config.nested[0].target().as_str(), "/images");
        assert_eq!(config.nested[1].source(), Path::new("vendor/static"));
        assert_eq!(config.nested[1].target().as_str(), "/lib");
        assert_eq!(
            config.flatten[0].source(),
            Path::new("assets/styles/base.css")
        );
        assert_eq!(config.flatten[0].target().as_str(), "/base.css");
        assert_eq!(config.flatten[1].source(), Path::new("assets/CNAME"));
        assert_eq!(config.flatten[1].target().as_str(), "/CNAME");
    }

    #[test]
    fn path_entry_targets_survive_normalization() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let mut config: AssetsConfig = toml::from_str(
            r#"
nested = ["assets/images"]
flatten = ["assets/styles/base.css"]
"#,
        )
        .unwrap();

        config.normalize(root);

        assert_eq!(config.nested[0].target().as_str(), "/images");
        assert_eq!(config.flatten[0].target().as_str(), "/base.css");
    }

    #[test]
    fn rejects_overlapping_nested_sources() {
        let config: AssetsConfig = toml::from_str(
            r#"
nested = ["assets", "assets/icons"]
"#,
        )
        .unwrap();
        let mut diag = ConfigDiagnostics::new();

        config.validate(&mut diag);

        assert!(
            diag.errors()
                .iter()
                .any(|error| error.message.contains("is inside"))
        );
    }

    #[test]
    fn rejects_nested_as_with_nested_url_path() {
        let config: AssetsConfig = toml::from_str(
            r#"
nested = [{ dir = "assets/styles", as = "/assets/styles" }]
"#,
        )
        .unwrap();
        let mut diag = ConfigDiagnostics::new();

        config.validate_paths(&mut diag);

        assert!(
            diag.errors()
                .iter()
                .any(|error| error.message.contains("root-level asset name"))
        );
    }

    #[test]
    fn rejects_flatten_as_with_nested_url_path() {
        let config: AssetsConfig = toml::from_str(
            r#"
flatten = [{ file = "assets/tailwind.css", as = "/styles/tailwind.css" }]
"#,
        )
        .unwrap();
        let mut diag = ConfigDiagnostics::new();

        config.validate_paths(&mut diag);

        assert!(
            diag.errors()
                .iter()
                .any(|error| error.message.contains("root-level asset name"))
        );
    }

    #[test]
    fn rejects_as_without_leading_slash() {
        let config: AssetsConfig = toml::from_str(
            r#"
nested = [{ dir = "assets/images", as = "images" }]
flatten = [{ file = "assets/favicon.ico", as = "favicon.ico" }]
"#,
        )
        .unwrap();
        let mut diag = ConfigDiagnostics::new();

        config.validate_paths(&mut diag);

        let messages: Vec<_> = diag
            .errors()
            .iter()
            .map(|error| error.message.as_str())
            .collect();
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.contains("must start with `/`"))
                .count(),
            2
        );
    }

    #[test]
    fn rejects_colocated_list_syntax() {
        assert!(toml::from_str::<AssetsConfig>(r#"colocated = ["posts"]"#).is_err());
    }
}
