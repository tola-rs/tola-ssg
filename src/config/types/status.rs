//! Field status types for config validation.
//!
//! Used to check if users set fields with special status
//! (experimental, not_implemented, deprecated).

use super::FieldPath;
use crate::config::ConfigDiagnostics;
use rustc_hash::FxHashSet;

/// Tracks which TOML paths were explicitly present in user config.
///
/// Paths are dot-separated (e.g. `deploy.cloudflare.provider`).
#[derive(Debug, Clone, Default)]
pub struct ConfigPresence {
    paths: FxHashSet<String>,
}

impl ConfigPresence {
    /// Build presence set from raw TOML content.
    pub fn from_toml(content: &str) -> Result<Self, toml::de::Error> {
        let value: toml::Value = toml::from_str(content)?;
        let mut presence = Self::default();
        presence.collect_value("", &value);
        Ok(presence)
    }

    /// Check whether a field or section path was explicitly present.
    #[inline]
    pub fn contains(&self, path: &str) -> bool {
        !path.is_empty() && self.paths.contains(path)
    }

    fn collect_value(&mut self, prefix: &str, value: &toml::Value) {
        match value {
            toml::Value::Table(table) => {
                if !prefix.is_empty() {
                    self.paths.insert(prefix.to_string());
                }
                for (key, child) in table {
                    let next = if prefix.is_empty() {
                        key.to_string()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    self.collect_value(&next, child);
                }
            }
            toml::Value::Array(items) => {
                if !prefix.is_empty() {
                    self.paths.insert(prefix.to_string());
                }
                // Keep traversing table items to capture nested keys in array-of-table cases.
                for item in items {
                    if matches!(item, toml::Value::Table(_)) {
                        self.collect_value(prefix, item);
                    }
                }
            }
            _ => {
                if !prefix.is_empty() {
                    self.paths.insert(prefix.to_string());
                }
            }
        }
    }
}

/// Field status for validation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldStatus {
    Experimental,
    NotImplemented,
    Deprecated,
}

impl FieldStatus {
    /// Get status label for display.
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Experimental => "experimental",
            Self::NotImplemented => "not implemented",
            Self::Deprecated => "deprecated",
        }
    }
}

/// Check field status and report diagnostics
///
/// Called by generated `validate_field_status` methods when a field
/// with special status differs from its default value
pub fn check_field_status(field_path: &str, status: FieldStatus, diag: &mut ConfigDiagnostics) {
    // Skip experimental hints if allowed
    if status == FieldStatus::Experimental && diag.allow_experimental {
        return;
    }

    let path = FieldPath::new(Box::leak(field_path.to_string().into_boxed_str()));

    match status {
        FieldStatus::NotImplemented => {
            diag.error_with_hint(
                path,
                "field is not implemented yet".to_string(),
                "remove this field or wait for future release",
            );
        }
        FieldStatus::Deprecated => {
            diag.warn(
                path,
                "field is deprecated and will be removed in a future version",
            );
        }
        FieldStatus::Experimental => {
            diag.experimental_hint(path);
        }
    }
}

/// Check section-level status and report diagnostics
///
/// Called when a section (struct) has experimental/deprecated/not_implemented status
/// and any of its fields are set to non-default values
pub fn check_section_status(section: &str, status: FieldStatus, diag: &mut ConfigDiagnostics) {
    // Skip experimental hints if allowed
    if status == FieldStatus::Experimental && diag.allow_experimental {
        return;
    }

    let path = FieldPath::new(Box::leak(format!("[{}]", section).into_boxed_str()));

    match status {
        FieldStatus::NotImplemented => {
            diag.error_with_hint(
                path,
                "this section is not implemented yet",
                "remove this section or wait for future release",
            );
        }
        FieldStatus::Deprecated => {
            diag.warn(
                path,
                "this section is deprecated and will be removed in a future version",
            );
        }
        FieldStatus::Experimental => {
            diag.experimental_hint(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ConfigPresence;

    #[test]
    fn collect_field_and_section_paths() {
        let toml = r#"
[deploy]
provider = "github"

[deploy.cloudflare]
provider = "cloudflare"
"#;
        let presence = ConfigPresence::from_toml(toml).unwrap();
        assert!(presence.contains("deploy"));
        assert!(presence.contains("deploy.provider"));
        assert!(presence.contains("deploy.cloudflare"));
        assert!(presence.contains("deploy.cloudflare.provider"));
    }

    #[test]
    fn collect_scalar_fields_without_table_header() {
        let toml = r#"title = "hello""#;
        let presence = ConfigPresence::from_toml(toml).unwrap();
        assert!(presence.contains("title"));
        assert!(!presence.contains("site"));
    }
}
