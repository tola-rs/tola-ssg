//! Failures while preparing or using a Nushell session.

use std::path::PathBuf;

use thiserror::Error;

/// One failure this crate reports.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NuError {
    /// The directory a session evaluates in is not an existing absolute directory.
    #[error("Nu invocation directory must be an existing absolute directory")]
    InvocationDirectory,
    /// A filesystem step failed without a name of its own.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// One named filesystem step failed. `action` names the step the message has always printed.
    #[error("{action}: {source}")]
    IoStep {
        action: &'static str,
        #[source]
        source: std::io::Error,
    },
    /// A module manifest is not readable TOML.
    #[error(transparent)]
    Manifest(#[from] toml::de::Error),
    /// A module manifest does not declare the names a session needs.
    #[error("{reason}")]
    ManifestShape { reason: &'static str },
    /// A configured module's root and manifest must be absolute.
    #[error("Nu module root and manifest must be absolute")]
    ModuleNotAbsolute,
    /// A module alias is empty or contains a path separator.
    #[error("`{alias}` is not a usable Nu module name")]
    ModuleAlias { alias: String },
    /// A module entry is not relative to the module that declares it.
    #[error("Nu module entry must be relative to its module")]
    ModuleEntryRelative,
    /// A module entry resolves outside its module, or is neither a file nor a supplied buffer.
    #[error("Nu module entry escapes its module or is not a file")]
    ModuleEntryEscapes,
    /// The module manifest no longer declares the entry the caller resolved.
    #[error(
        "the entry of Nu module `{alias}` changed while Tola was reading it; run the command again"
    )]
    ModuleEntryChanged { alias: String },
    /// A module or source path cannot be named as UTF-8.
    #[error("Nu {what} path is not UTF-8")]
    NotUtf8 { what: &'static str },
    /// A source identity is not a usable absolute path.
    #[error("{reason}")]
    SourceIdentity { reason: &'static str },
    /// A captured input could not be read or resolved as the session prepared.
    #[error("{action} {}: {source}", path.display())]
    Input {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The embedded engine did not load the module source the session parsed.
    #[error("Nu module source was not loaded")]
    ModuleSourceUnloaded,
    /// The embedded engine resolved no module for the source the session parsed.
    #[error("Nu module did not resolve")]
    ModuleUnresolved,
    /// The embedded engine refused the module source.
    #[error("{report}")]
    ModuleParse { report: String },
    /// The configured module set could not be loaded, with what each module reported.
    #[error("cannot load Nu modules:\n{failures}")]
    ModuleLoad { failures: String },
    /// Two configured modules use one alias.
    #[error("two Nu modules use the name `{alias}`")]
    DuplicateAlias { alias: String },
    /// Supplied buffers did not converge into a stable import graph.
    #[error(
        "Nu buffer import identities did not converge; cyclic or unresolved imports require correction"
    )]
    ImportsUnstable,
    /// The engine read a source from disk rather than the supplied buffer.
    #[error("Nu source did not use the supplied buffer snapshot: {}", path.display())]
    BufferNotUsed { path: PathBuf },
    /// Two supplied buffers name one file with different text or versions.
    #[error("conflicting Nu buffers for {}", path.display())]
    OverlayConflict { path: PathBuf },
    /// A supplied buffer names a source outside every configured module.
    #[error("new Nu buffer source is outside configured modules")]
    BufferOutsideModules,
    /// A source the session parsed disappeared during setup.
    #[error("Nu source disappeared during setup: {}", path.display())]
    SourceDisappeared { path: PathBuf },
    /// Nushell itself refused the module set.
    #[error(transparent)]
    Shell(#[from] nu_protocol::ShellError),
    /// A caller asked for a module locator no configured entry publishes.
    #[error(
        "no configured Nu entry is named `{locator}`; use a configured public Nu entry, or `./` for a file beginning with `@`"
    )]
    UnknownLocator { locator: String },
    /// A source supplied only as a buffer now exists on disk.
    #[error("Nu buffer source appeared on disk; rebuild the session: {}", path.display())]
    BufferAppeared { path: PathBuf },
    /// A captured input's target changed since the session was prepared.
    #[error("Nu input target changed; rebuild the session: {}", path.display())]
    InputTargetChanged { path: PathBuf },
    /// A captured input disappeared.
    #[error("Nu input disappeared: {}: {source}", path.display())]
    InputDisappeared {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A captured input's bytes changed.
    #[error("Nu input changed; rebuild the session: {}", path.display())]
    InputChanged { path: PathBuf },
    /// A captured input changed again while the session was being prepared.
    #[error("Nu input changed during setup: {}", path.display())]
    InputChangedDuringSetup { path: PathBuf },
    /// A walked path is not below the root it was read from.
    #[error(transparent)]
    StripPrefix(#[from] std::path::StripPrefixError),
    /// One step failed while locating what it names; `action` names the step.
    #[error("{action}: {source}")]
    Context {
        action: &'static str,
        #[source]
        source: Box<NuError>,
    },
    /// The embedded Nushell engine could not be initialized.
    #[error("initialize embedded Nushell")]
    Engine,
}

/// The error one public function reports.
pub(crate) type Result<T> = std::result::Result<T, NuError>;
