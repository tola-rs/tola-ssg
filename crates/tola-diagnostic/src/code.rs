//! Stable diagnostic identifiers.

use std::fmt;

use serde::Serialize;

/// A stable diagnostic identifier, written `<surface>.<contract>`.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct DiagnosticCode(&'static str);

impl DiagnosticCode {
    /// Declare a diagnostic identifier; a malformed shape fails to compile.
    pub const fn new(code: &'static str) -> Self {
        assert!(
            well_formed(code),
            "a diagnostic code is `<surface>.<contract>`, lowercase, with one dot"
        );
        Self(code)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }

    pub fn surface(self) -> &'static str {
        self.0
            .split_once('.')
            .map_or(self.0, |(surface, _)| surface)
    }
}

/// `<surface>.<contract>`: lowercase, digits, `_`, one dot.
const fn well_formed(code: &str) -> bool {
    let bytes = code.as_bytes();
    let mut idx = 0;
    let mut dots = 0;
    let mut part_len = 0;
    while idx < bytes.len() {
        match bytes[idx] {
            b'a'..=b'z' | b'0'..=b'9' | b'_' => part_len += 1,
            b'.' => {
                if part_len == 0 {
                    return false;
                }
                dots += 1;
                part_len = 0;
            }
            _ => return false,
        }
        idx += 1;
    }
    dots == 1 && part_len > 0
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl fmt::Debug for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.0, f)
    }
}

impl PartialEq<&str> for DiagnosticCode {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_shape() {
        for ok in ["config.toml", "site.no_pages", "typst.bundle_export"] {
            assert!(well_formed(ok), "{ok}");
        }
        for bad in [
            "",
            "config",
            "config.",
            ".toml",
            "config..toml",
            "Config.toml",
            "config.Toml",
            "config-toml",
            "config.toml.x",
            "config toml",
        ] {
            assert!(!well_formed(bad), "{bad}");
        }
    }
}
