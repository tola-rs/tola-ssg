//! Literal path substitutions for declared command arguments.

use std::ffi::{OsStr, OsString};

/// Substitute declared markers once, preserving native paths and argument boundaries.
pub fn expand_argument(argument: &str, bindings: &[(&str, &OsStr)]) -> OsString {
    let mut buf = OsString::new();
    let mut rest = argument;
    loop {
        let next = bindings
            .iter()
            .filter_map(|(marker, val)| rest.find(*marker).map(|off| (off, marker, val)))
            .min_by_key(|(off, _, _)| *off);
        let Some((off, marker, val)) = next else {
            buf.push(rest);
            return buf;
        };
        buf.push(&rest[..off]);
        buf.push(val);
        rest = &rest[off + marker.len()..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_skips_inserted_markers() {
        let bindings = [
            ("{module}", OsStr::new("/themes/{project}/notes")),
            ("{project}", OsStr::new("/my site")),
        ];
        assert_eq!(
            expand_argument("{module}/build --site={project}", &bindings),
            "/themes/{project}/notes/build --site=/my site",
        );
    }
}
