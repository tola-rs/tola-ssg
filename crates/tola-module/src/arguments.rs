//! Literal path substitutions for declared command arguments.

use std::ffi::{OsStr, OsString};

/// Substitute declared markers once, preserving native paths and argument boundaries.
pub fn expand_argument(argument: &str, bindings: &[(&str, &OsStr)]) -> OsString {
    let mut expanded = OsString::new();
    let mut rest = argument;
    loop {
        let next = bindings
            .iter()
            .filter_map(|(marker, value)| rest.find(*marker).map(|offset| (offset, marker, value)))
            .min_by_key(|(offset, _, _)| *offset);
        let Some((offset, marker, value)) = next else {
            expanded.push(rest);
            return expanded;
        };
        expanded.push(&rest[..offset]);
        expanded.push(value);
        rest = &rest[offset + marker.len()..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserted_not_interpreted_more_markers() {
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
