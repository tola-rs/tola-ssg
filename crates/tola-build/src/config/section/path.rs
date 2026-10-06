//! Rules for paths configured relative to the site root.

use std::path::{Component, Path};

use crate::config::{ConfigDiagnostics, FieldPath};

/// Why `component` may not appear in a path configured relative to the site root.
///
/// Absolute roots, Windows prefixes, and `..` would escape the site root once normalized.
fn forbidden_component_reason(component: Component<'_>) -> Option<&'static str> {
    match component {
        Component::ParentDir => Some("it contains `..`"),
        Component::RootDir | Component::Prefix(_) => Some("it is an absolute path"),
        _ => None,
    }
}

/// Whether `path` enters the reserved `.tola` directory at its first component.
pub(super) fn enters_internal_directory(path: &Path) -> bool {
    path.components()
        .find_map(|component| match component {
            Component::Normal(part) => Some(part),
            _ => None,
        })
        .is_some_and(is_internal_directory_name)
}

/// Whether the resolved `path` reaches the reserved `.tola` directory.
///
/// The written path and the path the filesystem resolves are both compared, because a linked
/// directory reaches its target physically.
pub(super) fn reaches_internal_directory(root: &Path, path: &Path) -> bool {
    let reserved = root.join(crate::filesystem::INTERNAL_DIR);
    portable_path_is_below(path, &reserved)
        || portable_path_is_below(
            &crate::filesystem::normalize_existing_prefix(path),
            &crate::filesystem::normalize_existing_prefix(&reserved),
        )
}

/// Whether `name` names the reserved `.tola` directory under portable filesystem identity.
fn is_internal_directory_name(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        tola_address::portable_collision_key(name)
            == tola_address::portable_collision_key(crate::filesystem::INTERNAL_DIR)
    })
}

/// Whether the absolute, normalized `path` names `prefix` or a path below it.
///
/// The component keys fold case and normalization the way the filesystems Tola supports alias
/// directory names, so `.TOLA` is refused everywhere rather than only where the filesystem folds
/// it: a path that reaches Tola's own directory on one platform is not accepted on another.
fn portable_path_is_below(path: &Path, prefix: &Path) -> bool {
    portable_path_key(path)
        .zip(portable_path_key(prefix))
        .is_some_and(|(path, prefix)| path.starts_with(prefix.as_slice()))
}

fn portable_path_key(path: &Path) -> Option<Vec<String>> {
    path.components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .map(tola_address::portable_collision_key)
        })
        .collect()
}

/// Record why `path` cannot be a path configured relative to the site root.
///
/// `key` is the written key, without backticks and with any array index, so one message
/// names the line the author has to change. The declaration is not resolved against the site
/// root yet, so the path renders as the author spelled it, through the one path renderer.
/// Returns whether the path is a usable declaration, so a caller with a further check about
/// the same path can stop here.
pub(super) fn validate_site_relative_path(
    path: &Path,
    key: &str,
    field: FieldPath,
    diagnostics: &mut ConfigDiagnostics,
) -> bool {
    if path.as_os_str().is_empty() {
        diagnostics.error_with_help(
            field,
            format!("`{key}` is empty"),
            "write a path relative to the site root",
        );
        return false;
    }
    if let Some(reason) = path
        .components()
        .filter_map(forbidden_component_reason)
        .next()
    {
        diagnostics.error_with_help(
            field,
            format!("`{key}` is `{}`: {reason}", declared_path(path)),
            "write it relative to the site root",
        );
        return false;
    }
    true
}

/// Render one declared path the way every message spells a path inside the site.
///
/// The declaration is unresolved, so `display_path` is asked for the path's own spelling: an
/// empty root has no prefix to strip, and the renderer still gives `/` separators everywhere.
pub(super) fn declared_path(path: &Path) -> String {
    crate::filesystem::display_path(path, Path::new(""))
}
