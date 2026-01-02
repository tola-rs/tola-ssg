//! Field status types for config validation.
//!
//! Used to check if users set fields with special status
//! (experimental, not_implemented, deprecated).

use super::FieldPath;
use crate::config::ConfigDiagnostics;

/// Field status for validation.
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

/// Check field status and report diagnostics.
///
/// Called by generated `validate_field_status` methods when a field
/// with special status differs from its default value.
pub fn check_field_status(
    field_path: &str,
    status: FieldStatus,
    diag: &mut ConfigDiagnostics,
) {
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

/// Check section-level status and report diagnostics.
///
/// Called when a section (struct) has experimental/deprecated/not_implemented status
/// and any of its fields are set to non-default values.
pub fn check_section_status(
    section: &str,
    status: FieldStatus,
    diag: &mut ConfigDiagnostics,
) {
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
