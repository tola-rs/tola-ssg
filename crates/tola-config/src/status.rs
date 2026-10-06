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

/// Check lifecycle status for an explicitly configured field or section.
///
/// Experimental fields share one warning, suppressed when allowed.
/// Deprecated fields always warn; not-implemented fields always fail validation.
pub fn check_field_status(
    field_path: &'static str,
    status: FieldStatus,
    diagnostics: &mut ConfigDiagnostics,
) {
    let field = FieldPath::new(field_path);
    match status {
        FieldStatus::Experimental => {
            if !diagnostics.allows_experimental() {
                diagnostics.record_experimental_field(field);
            }
        }
        FieldStatus::Deprecated => {
            diagnostics.deprecated(field, "deprecated; still accepted");
        }
        FieldStatus::NotImplemented => {
            diagnostics.error_with_help(
                field,
                "this setting is not implemented yet",
                "Remove it from `tola.toml`",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigDiagnostics, FieldPath};
    use super::{FieldStatus, check_field_status};

    #[test]
    fn experimental_warns_unless_allowed() {
        let mut diagnostics = ConfigDiagnostics::new();
        check_field_status(
            "build.experimental",
            FieldStatus::Experimental,
            &mut diagnostics,
        );
        assert_eq!(
            diagnostics.experimental_fields(),
            &[FieldPath::new("build.experimental")]
        );

        let mut diagnostics = ConfigDiagnostics::with_allow_experimental(true);
        check_field_status(
            "build.experimental",
            FieldStatus::Experimental,
            &mut diagnostics,
        );
        assert!(diagnostics.experimental_fields().is_empty());
    }

    #[test]
    fn deprecated_warns_without_failing() {
        let mut diagnostics = ConfigDiagnostics::new();
        check_field_status("site.old-title", FieldStatus::Deprecated, &mut diagnostics);
        assert_eq!(diagnostics.warnings().len(), 1);
        assert_eq!(
            diagnostics.warnings()[0].tag(),
            Some(crate::ConfigDiagnosticTag::Deprecated)
        );

        assert!(diagnostics.into_result().is_ok());
    }

    #[test]
    fn not_implemented_fails_validation() {
        let mut diagnostics = ConfigDiagnostics::new();
        check_field_status(
            "build.future",
            FieldStatus::NotImplemented,
            &mut diagnostics,
        );

        assert!(diagnostics.into_result().is_err());
    }
}
