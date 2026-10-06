//! Compilation error type.

use super::NativeDiagnostic;
use thiserror::Error;
use typst::World;

use super::collection::Diagnostics;
use crate::world::font::FontLoadError;
use crate::world::{SnapshotError, WorldBuildError};

/// Typst compilation and export failures with structured diagnostics.
///
/// # Example
///
/// ```ignore
/// use tola_typst::prelude::*;
///
/// let cancellation = BundleCancellation::default();
/// let world = TypstWorld::builder(entry, root)
///     .with_local_cache()
///     .with_fonts(fonts)
///     .build(&cancellation)?;
/// match compile_world(&world) {
///     Ok(result) => { /* success */ }
///     Err(CompileError::Compilation { diagnostics, .. }) => {
///         for diag in diagnostics.errors() {
///             eprintln!("Error: {}", diag.message);
///         }
///     }
///     Err(CompileError::HtmlExport { message }) => {
///         eprintln!("{message}");
///     }
///     Err(e) => eprintln!("{e}"),
/// }
/// ```
#[derive(Debug, Error)]
pub enum CompileError {
    /// Typst compilation failed with diagnostics.
    #[error("{diagnostics}")]
    Compilation {
        /// The resolved diagnostics.
        diagnostics: Diagnostics,
    },

    /// HTML export failed.
    #[error("{message}")]
    HtmlExport {
        /// Error message from typst_html.
        message: String,
    },

    /// Bundle export failed with ordered raw diagnostics.
    #[error("{}", export_error_message(.diagnostics))]
    BundleExport {
        /// Diagnostics returned by Typst's Bundle exporter.
        diagnostics: Vec<NativeDiagnostic>,
    },

    /// Font preparation or a lazy font read failed.
    #[error("could not load the fonts")]
    Font(#[source] FontLoadError),

    /// The caller cancelled compilation, export, or input preparation.
    #[error("operation cancelled")]
    Cancelled,

    /// Input construction failed.
    #[error("{message}")]
    Input {
        /// Error message from input construction.
        message: String,
    },

    /// Snapshot build error.
    #[error("could not read the site's sources")]
    Snapshot(#[source] SnapshotError),

    /// Typst world configuration error.
    #[error("could not prepare the compiler")]
    World(#[source] WorldBuildError),
}

impl From<FontLoadError> for CompileError {
    fn from(error: FontLoadError) -> Self {
        match error {
            FontLoadError::Cancelled => Self::Cancelled,
            error => Self::Font(error),
        }
    }
}

/// An error type that reports cancellation in its own form.
///
/// Implementors let [`crate::BundleCancellation::ensure_active_as`]
/// deliver their cancellation error, so the "checked at adapter boundaries"
/// cancellation rule has one definition.
pub(crate) trait CancelledError: Sized {
    /// The cancellation error of this type.
    fn from_cancellation() -> Self;
}

impl CancelledError for CompileError {
    fn from_cancellation() -> Self {
        Self::Cancelled
    }
}

impl From<WorldBuildError> for CompileError {
    fn from(error: WorldBuildError) -> Self {
        match error {
            WorldBuildError::Font(error) => Self::from(error),
            WorldBuildError::Cancelled => Self::Cancelled,
            error => Self::World(error),
        }
    }
}

impl From<SnapshotError> for CompileError {
    fn from(error: SnapshotError) -> Self {
        match error {
            SnapshotError::Cancelled => Self::Cancelled,
            error => Self::Snapshot(error),
        }
    }
}

impl CompileError {
    /// Create a compilation error from typed native diagnostics.
    pub fn compilation<W: World>(world: &W, raw_diagnostics: Vec<NativeDiagnostic>) -> Self {
        let diagnostics = Diagnostics::resolve_owned(
            world,
            raw_diagnostics,
            super::SourceContextLimit::default(),
        );
        Self::Compilation { diagnostics }
    }

    /// Create a compilation error from diagnostics that are already resolved.
    pub fn from_resolved(diagnostics: Diagnostics) -> Self {
        Self::Compilation { diagnostics }
    }

    /// Create an HTML export error.
    pub fn html_export(message: impl Into<String>) -> Self {
        Self::HtmlExport {
            message: message.into(),
        }
    }

    /// Create a bundle export error from ordered typed native diagnostics.
    pub fn bundle_export(diagnostics: impl IntoIterator<Item = NativeDiagnostic>) -> Self {
        Self::BundleExport {
            diagnostics: diagnostics.into_iter().collect(),
        }
    }

    /// Create a cancellation error.
    pub const fn cancelled() -> Self {
        Self::Cancelled
    }

    /// Whether the caller cancelled the operation.
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    /// Create an input construction error.
    pub fn input(message: impl Into<String>) -> Self {
        Self::Input {
            message: message.into(),
        }
    }

    /// Return the ordered typed native diagnostics emitted by Typst's Bundle exporter.
    pub fn raw_diagnostics(&self) -> Option<&[NativeDiagnostic]> {
        match self {
            Self::BundleExport { diagnostics } => Some(diagnostics),
            _ => None,
        }
    }

    /// Whether this error holds any fatal diagnostic, as opposed to warnings alone.
    pub fn has_fatal_errors(&self) -> bool {
        match self {
            Self::Compilation { diagnostics } => diagnostics.has_errors(),
            _ => true,
        }
    }

    /// Get the diagnostics if this is a compilation error.
    pub fn diagnostics(&self) -> Option<&Diagnostics> {
        match self {
            Self::Compilation { diagnostics } => Some(diagnostics),
            _ => None,
        }
    }
}

/// The one sentence a failed Bundle export renders when the exporter supplied
/// no usable diagnostic. Hints reach the author as `help`, not inside a message.
pub(crate) fn export_error_message(diagnostics: &[NativeDiagnostic]) -> String {
    diagnostics
        .first()
        .map(|diagnostic| diagnostic.source().message.to_string())
        .unwrap_or_else(|| {
            "could not export the site; report this at \
             https://github.com/tola-rs/tola-ssg/issues"
                .to_owned()
        })
}
