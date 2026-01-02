//! Compilation phase for virtual package behavior.

/// Compilation phase, determines virtual package behavior.
///
/// - `Filter`: Internal filtering (draft detection). Deferred packages return empty data.
/// - `Visible`: User-visible operations (build/query/validate). Deferred packages panic if misused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Filter,
    Visible,
}

impl Phase {
    /// Phase value as string (for sys.inputs).
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Filter => "filter",
            Self::Visible => "visible",
        }
    }

    /// sys.inputs key for phase.
    pub const fn input_key() -> &'static str {
        "__tola_phase"
    }
}
