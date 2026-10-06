use std::path::Path;

/// Whether a path's name is shaped like the file an editor leaves behind mid-write.
///
/// Ordinary dotfiles are inputs, and the shape alone never excludes one: the subscriptions
/// reaching the path decide, in `EventBoundary::admits_editor_artifact`.
pub(super) fn is_editor_temp(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let extension = path.extension().and_then(|extension| extension.to_str());

    matches!(
        extension,
        Some("bck" | "bak" | "backup" | "swp" | "swo" | "tmp")
    ) || matches!(name, ".swp" | ".swo" | ".tmp")
        || name.ends_with('~')
        || name.starts_with(".#")
        || name.starts_with("~$")
        || (name.starts_with('#') && name.ends_with('#'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_dotfiles_are_inputs() {
        for name in [".gitignore", ".env", ".well-known", ".hidden.typ"] {
            assert!(!is_editor_temp(Path::new(name)), "rejected {name}");
        }
        for name in ["page.typ.swp", ".#page.typ", "#page.typ#", "page.typ~"] {
            assert!(is_editor_temp(Path::new(name)), "accepted {name}");
        }
    }
}
