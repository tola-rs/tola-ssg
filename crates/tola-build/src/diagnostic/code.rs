//! Stable diagnostic identifiers.

use std::fmt;

use serde::Serialize;

/// A stable diagnostic identifier, written `<surface>.<contract>`.
///
/// The surface names where the failure was found and the contract names what failed, so a reader
/// can group diagnostics and a tool can recognize one without matching message text. Codes are
/// declared once, in [`crate::codes`], and the shape is checked while that declaration compiles.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct DiagnosticCode(&'static str);

impl DiagnosticCode {
    /// Declare a diagnostic identifier.
    ///
    /// # Panics
    ///
    /// Fails to compile when `code` has no `<surface>.<contract>` shape.
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

    /// The part before the dot.
    pub fn surface(self) -> &'static str {
        self.0
            .split_once('.')
            .map_or(self.0, |(surface, _)| surface)
    }
}

/// The `<surface>.<contract>` shape: lowercase `a`-`z`, digits and `_`, with exactly one dot
/// between two non-empty parts.
const fn well_formed(code: &str) -> bool {
    let bytes = code.as_bytes();
    let mut index = 0;
    let mut dots = 0;
    let mut part_len = 0;
    while index < bytes.len() {
        match bytes[index] {
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
        index += 1;
    }
    dots == 1 && part_len > 0
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl fmt::Debug for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.0, formatter)
    }
}

impl PartialEq<&str> for DiagnosticCode {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl From<DiagnosticCode> for String {
    fn from(code: DiagnosticCode) -> Self {
        code.0.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_code_keeps_its_text() {
        let code = DiagnosticCode::new("config.toml");

        assert_eq!(code.as_str(), "config.toml");
        assert_eq!(code, "config.toml");
        assert_eq!(code.to_string(), "config.toml");
        assert_eq!(serde_json::json!(code), serde_json::json!("config.toml"));
    }

    #[test]
    fn shape_rule_rejects_malformed_codes() {
        for accepted in ["config.toml", "site.no_pages", "typst.bundle_export"] {
            assert!(well_formed(accepted), "{accepted}");
        }
        for refused in [
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
            assert!(!well_formed(refused), "{refused}");
        }
    }
}
