//! The site construction mode one attempt runs in.

/// Output policy and hook execution for one site build.
///
/// The mode is chosen by the caller and read by the layers that decide what a build publishes and
/// which hooks participate, so it sits below the build that owns the attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildMode {
    Production,
    Development,
}

impl BuildMode {
    /// The mode's name, as a hook command receives it in `TOLA_BUILD_MODE`.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Production => "prod",
            Self::Development => "dev",
        }
    }
}
