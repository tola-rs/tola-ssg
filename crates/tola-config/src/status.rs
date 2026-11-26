//! Configuration field lifecycle status and diagnostics.

use super::{ConfigDiagnostics, FieldPath};

/// A schema field's lifecycle status.
///
/// Diagnostics apply only to paths explicitly present in the source, not defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldStatus {
    /// Available, but its contract may change.
    Experimental,
    /// Still accepted; avoid in new configuration.
    Deprecated,
    /// Declared in the schema, but not yet usable.
    NotImplemented,
}

pub fn check_field_status(
    field_path: &'static str,
    status: FieldStatus,
    diag: &mut ConfigDiagnostics,
) {
    let field = FieldPath::new(field_path);
    match status {
        FieldStatus::Experimental => {
            if !diag.allows_experimental() {
                diag.record_experimental_field(field);
            }
        }
        FieldStatus::Deprecated => {
            diag.deprecated(field, "deprecated; still accepted");
        }
        FieldStatus::NotImplemented => {
            diag.error_with_help(
                field,
                "this setting is not implemented yet",
                "Remove it from `tola.toml`",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigDiagnostics, FieldPath, FieldStatus, check_field_status};

    #[test]
    fn experimental_status() {
        let mut diag = ConfigDiagnostics::new();
        check_field_status("build.experimental", FieldStatus::Experimental, &mut diag);
        assert_eq!(
            diag.experimental_fields(),
            &[FieldPath::new("build.experimental")]
        );

        let mut diag = ConfigDiagnostics::with_allow_experimental(true);
        check_field_status("build.experimental", FieldStatus::Experimental, &mut diag);
        assert!(diag.experimental_fields().is_empty());
    }

    #[test]
    fn deprecated_status() {
        let mut diag = ConfigDiagnostics::new();
        check_field_status("site.old-title", FieldStatus::Deprecated, &mut diag);
        assert_eq!(diag.warnings().len(), 1);
        assert_eq!(
            diag.warnings()[0].tag(),
            Some(crate::ConfigDiagnosticTag::Deprecated)
        );
        assert!(diag.into_result().is_ok());
    }

    #[test]
    fn not_implemented_status() {
        let mut diag = ConfigDiagnostics::new();
        check_field_status("build.future", FieldStatus::NotImplemented, &mut diag);
        assert!(diag.into_result().is_err());
    }
}
