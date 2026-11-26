//! Portable filename rules shared by published paths and module archives.

use caseless::Caseless;
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

pub(crate) const MAX_PORTABLE_SEGMENT_BYTES: usize = 255;

/// Why a filename component cannot be represented portably across supported filesystems.
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

/// Validate a filename component; callers check separators and dot segments.
pub fn validate_portable_segment(seg: &str, first: bool) -> Result<(), PortablePathError> {
    if first && is_windows_prefix(seg) {
        return Err(PortablePathError::FilesystemPrefix);
    }
    if seg.contains(['<', '>', ':', '"', '|', '?', '*']) {
        return Err(PortablePathError::UnsafeCharacter);
    }
    if seg.ends_with([' ', '.']) {
        return Err(PortablePathError::TrailingDotOrSpace);
    }
    if is_windows_device_name(seg) {
        return Err(PortablePathError::WindowsDeviceName);
    }
    if seg.len() > MAX_PORTABLE_SEGMENT_BYTES {
        return Err(PortablePathError::SegmentTooLong {
            byte_len: seg.len(),
        });
    }
    Ok(())
}

/// Return whether `ch` is a C0 or C1 control character.
pub(crate) fn is_control_char(ch: char) -> bool {
    matches!(ch as u32, 0x00..=0x1f | 0x7f..=0x9f)
}

/// Conservative key for case-insensitive and normalization-aware filesystems.
pub fn portable_collision_key(comp: &str) -> String {
    comp.chars().nfd().default_case_fold().nfd().collect()
}

/// Return whether `key` names `prefix` or a descendant below it.
pub fn portable_key_is_below(key: &[String], prefix: &[String]) -> bool {
    prefix.len() <= key.len() && key.starts_with(prefix)
}

/// Return whether two portable keys name the same path or one sits below the other.
pub fn portable_keys_overlap(lhs: &[String], rhs: &[String]) -> bool {
    portable_key_is_below(lhs, rhs) || portable_key_is_below(rhs, lhs)
}

fn is_windows_prefix(seg: &str) -> bool {
    let bytes = seg.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn is_windows_device_name(seg: &str) -> bool {
    let stem = seg.split('.').next().unwrap_or(seg);
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

fn is_numbered_windows_device(name: &str, pfx: &str) -> bool {
    let Some(sfx) = name
        .get(..pfx.len())
        .filter(|head| head.eq_ignore_ascii_case(pfx))
        .and_then(|_| name.get(pfx.len()..))
    else {
        return false;
    };

    matches!(
        sfx,
        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(path: &str) -> Vec<String> {
        path.split('/').map(portable_collision_key).collect()
    }

    #[test]
    fn collision_key_variants() {
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
    fn invalid_segments() {
        for (seg, first) in [
            ("C:", true),
            ("NUL.txt", false),
            ("name.", false),
            ("bad*name", false),
        ] {
            assert!(validate_portable_segment(seg, first).is_err());
        }
    }

    #[test]
    fn key_overlap() {
        for (lhs, rhs) in [
            ("a", "a"),
            ("a", "a/b"),
            ("a/b", "a"),
            ("a/CAFÉ", "a/cafe\u{301}"),
        ] {
            assert!(portable_keys_overlap(&key(lhs), &key(rhs)), "{lhs} {rhs}");
        }
        for (lhs, rhs) in [("a", "ab"), ("a/b", "a/c"), ("a/b", "ab/c"), ("a", "b")] {
            assert!(!portable_keys_overlap(&key(lhs), &key(rhs)), "{lhs} {rhs}");
        }
        assert!(portable_key_is_below(&key("a"), &key("a")));
        assert!(portable_key_is_below(&key("a/b"), &key("a")));
        assert!(!portable_key_is_below(&key("a"), &key("a/b")));
        assert!(!portable_key_is_below(&key("ab"), &key("a")));
    }
}
