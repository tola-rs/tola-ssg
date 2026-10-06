//! Portable output filename rules shared by URLs and bundle paths.

use caseless::Caseless;
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

pub(crate) const MAX_PORTABLE_SEGMENT_BYTES: usize = 255;

/// Why a logical output component cannot be represented portably on supported filesystems.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PortablePathError {
    #[error("the path must not start with a drive letter or filesystem prefix")]
    FilesystemPrefix,
    #[error("the path must not contain `<`, `>`, `:`, `\"`, `|`, `?`, or `*`")]
    UnsafeCharacter,
    #[error("a path segment must not end with a dot or a space")]
    TrailingDotOrSpace,
    #[error("the path must not use a reserved Windows device name such as NUL")]
    WindowsDeviceName,
    #[error("this path segment is {byte_len} bytes; the limit is {MAX_PORTABLE_SEGMENT_BYTES}")]
    SegmentTooLong { byte_len: usize },
}

pub(crate) fn validate_portable_segment(
    segment: &str,
    first: bool,
) -> Result<(), PortablePathError> {
    if first && is_windows_prefix(segment) {
        return Err(PortablePathError::FilesystemPrefix);
    }
    if segment.contains(['<', '>', ':', '"', '|', '?', '*']) {
        return Err(PortablePathError::UnsafeCharacter);
    }
    if segment.ends_with([' ', '.']) {
        return Err(PortablePathError::TrailingDotOrSpace);
    }
    if is_windows_device_name(segment) {
        return Err(PortablePathError::WindowsDeviceName);
    }
    if segment.len() > MAX_PORTABLE_SEGMENT_BYTES {
        return Err(PortablePathError::SegmentTooLong {
            byte_len: segment.len(),
        });
    }
    Ok(())
}

/// Conservative key for case-insensitive and normalization-aware filesystems.
pub fn portable_collision_key(component: &str) -> String {
    component.chars().nfd().default_case_fold().nfd().collect()
}

/// Whether `key` names the same path as `prefix` or a path below it.
pub fn portable_key_is_below(key: &[String], prefix: &[String]) -> bool {
    prefix.len() <= key.len() && key.starts_with(prefix)
}

/// Whether two portable keys name the same path, or one names a path below the other.
///
/// Two keys that overlap cannot be owned separately: one of them names a directory the other
/// needs, or the same file twice.
pub fn portable_keys_overlap(left: &[String], right: &[String]) -> bool {
    portable_key_is_below(left, right) || portable_key_is_below(right, left)
}

fn is_windows_prefix(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn is_windows_device_name(segment: &str) -> bool {
    let stem = segment.split('.').next().unwrap_or(segment);
    let name = stem.trim_end_matches([' ', '.']);

    name.eq_ignore_ascii_case("con")
        || name.eq_ignore_ascii_case("prn")
        || name.eq_ignore_ascii_case("aux")
        || name.eq_ignore_ascii_case("nul")
        || name.eq_ignore_ascii_case("conin$")
        || name.eq_ignore_ascii_case("conout$")
        || is_numbered_windows_device(name, "com")
        || is_numbered_windows_device(name, "lpt")
}

fn is_numbered_windows_device(name: &str, prefix: &str) -> bool {
    let Some(suffix) = name
        .get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .and_then(|_| name.get(prefix.len()..))
    else {
        return false;
    };

    matches!(
        suffix,
        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_variants_share_one_key() {
        assert_eq!(
            portable_collision_key("CAFÉ"),
            portable_collision_key("cafe\u{301}")
        );
        assert_eq!(
            portable_collision_key("straße"),
            portable_collision_key("STRASSE")
        );
    }

    #[test]
    fn cross_platform_hazards_are_refused() {
        for (segment, first) in [
            ("C:", true),
            ("NUL.txt", false),
            ("name.", false),
            ("bad*name", false),
        ] {
            assert!(validate_portable_segment(segment, first).is_err());
        }
    }

    fn key(path: &str) -> Vec<String> {
        path.split('/').map(portable_collision_key).collect()
    }

    #[test]
    fn overlap_follows_component_boundaries() {
        for (left, right) in [
            ("a", "a"),
            ("a", "a/b"),
            ("a/b", "a"),
            ("a/CAFÉ", "a/cafe\u{301}"),
        ] {
            assert!(
                portable_keys_overlap(&key(left), &key(right)),
                "{left} {right}"
            );
        }
        for (left, right) in [("a", "ab"), ("a/b", "a/c"), ("a/b", "ab/c"), ("a", "b")] {
            assert!(
                !portable_keys_overlap(&key(left), &key(right)),
                "{left} {right}"
            );
        }
        assert!(portable_key_is_below(&key("a"), &key("a")));
        assert!(portable_key_is_below(&key("a/b"), &key("a")));
        assert!(!portable_key_is_below(&key("a"), &key("a/b")));
        assert!(!portable_key_is_below(&key("ab"), &key("a")));
    }
}
