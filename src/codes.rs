//! Every diagnostic identifier the application publishes, grouped by the surface that produces it.
//!
//! Surfaces the site construction library owns are re-exported, so a call site always names one
//! module per surface and the text of a code is written exactly once in the workspace. The
//! `<surface>.<contract>` shape is checked while the code is constructed.

/// Build attempts the development server schedules.
pub mod build {
    use tola_build::diagnostic::DiagnosticCode;

    /// A rebuild task could not run.
    pub const TASK: DiagnosticCode = DiagnosticCode::new("build.task");
}

/// The command boundary.
pub mod command {
    use tola_build::diagnostic::DiagnosticCode;

    /// A command failed without classifying the failure.
    pub const FAILED: DiagnosticCode = DiagnosticCode::new("command.failed");
    /// A command ended because Tola stopped it.
    pub const SIGNAL: DiagnosticCode = DiagnosticCode::new("command.signal");
}

/// Configuration loading and reloading.
pub mod config {
    pub use tola_build::codes::config::{LOAD, RELOAD};

    use tola_build::diagnostic::DiagnosticCode;

    /// A configuration change requires restarting the development server.
    pub const SERVE_RESTART: DiagnosticCode = DiagnosticCode::new("config.serve_restart");
}

/// Read-only site and local-tool checks.
pub mod doctor {
    use tola_build::diagnostic::DiagnosticCode;

    /// A site check could not run.
    pub const CHECKS: DiagnosticCode = DiagnosticCode::new("doctor.checks");
    /// The configured content root is not a directory.
    pub const CONTENT_ROOT_MISSING: DiagnosticCode =
        DiagnosticCode::new("doctor.content_root_missing");
    /// A file an editor check reads could not be read.
    pub const EDITOR_FILE_UNREADABLE: DiagnosticCode =
        DiagnosticCode::new("doctor.editor_file_unreadable");
    /// Generated editor packages are missing.
    pub const EDITOR_PACKAGES_MISSING: DiagnosticCode =
        DiagnosticCode::new("doctor.editor_packages_missing");
    /// Generated editor packages do not match this release.
    pub const EDITOR_PACKAGES_STALE: DiagnosticCode =
        DiagnosticCode::new("doctor.editor_packages_stale");
    /// The configured entry is not a file.
    pub const ENTRY_MISSING: DiagnosticCode = DiagnosticCode::new("doctor.entry_missing");
    /// A hook command is not installed.
    pub const HOOK_COMMAND_MISSING: DiagnosticCode =
        DiagnosticCode::new("doctor.hook_command_missing");
    /// The host package roots could not be discovered.
    pub const PACKAGE_LOCATIONS: DiagnosticCode = DiagnosticCode::new("doctor.package_locations");
    /// A declared vendor directory does not exist yet.
    pub const VENDOR_PATH_MISSING: DiagnosticCode =
        DiagnosticCode::new("doctor.vendor_path_missing");
    /// A link inside the vendor directory makes the vendored copy depend on this machine.
    pub const VENDOR_LINK: DiagnosticCode = DiagnosticCode::new("doctor.vendor_link");
}

/// Editor integration: the language service's diagnostics and the setup command's.
pub mod editor {
    pub use tola_lsp::codes::editor::CONFIGURATION;

    use tola_build::diagnostic::DiagnosticCode;

    /// Editor setup configured a directory that is not a Tola site.
    pub const NO_SITE: DiagnosticCode = DiagnosticCode::new("editor.no_site");
    /// A selected editor or desktop application could not be opened.
    pub const LAUNCH: DiagnosticCode = DiagnosticCode::new("editor.launch");
}

/// Built-in demo projects.
pub mod demo {
    use tola_build::diagnostic::DiagnosticCode;

    pub const EXPORT: DiagnosticCode = DiagnosticCode::new("demo.export");
    pub const PREVIEW: DiagnosticCode = DiagnosticCode::new("demo.preview");
}

/// Hook execution around publication.
pub mod hook {
    pub use tola_build::codes::hook::AFTER_PUBLISH;
}

/// Site scaffolding.
pub mod init {
    use tola_build::diagnostic::DiagnosticCode;

    /// The target directory holds files Tola will not overwrite.
    pub const CONFLICT: DiagnosticCode = DiagnosticCode::new("init.conflict");
    /// The selected features cannot be scaffolded as they stand.
    pub const SELECTION: DiagnosticCode = DiagnosticCode::new("init.selection");
}

/// Documentation selectors.
pub mod help {
    use tola_build::diagnostic::DiagnosticCode;

    /// A selector does not name a supported documentation target.
    pub const TARGET: DiagnosticCode = DiagnosticCode::new("help.target");
    /// A requested export is absent from the selected bundled package.
    pub const MEMBER: DiagnosticCode = DiagnosticCode::new("help.member");
}

/// Failures with no classified owner.
pub mod internal {
    use tola_build::diagnostic::DiagnosticCode;

    /// Tola failed in a way no producer classified.
    pub const ERROR: DiagnosticCode = DiagnosticCode::new("internal.error");
}

/// The session log.
pub mod log {
    use tola_build::diagnostic::DiagnosticCode;

    /// The configured log file is not usable.
    pub const LOCATION: DiagnosticCode = DiagnosticCode::new("log.location");
    /// The session log is not available.
    pub const UNAVAILABLE: DiagnosticCode = DiagnosticCode::new("log.unavailable");
    /// A log record could not be written.
    pub const WRITE: DiagnosticCode = DiagnosticCode::new("log.write");
}

/// Browser delivery of a new revision.
pub mod reload {
    use tola_build::diagnostic::DiagnosticCode;

    /// The preferred reload port is already in use.
    pub const PORT_IN_USE: DiagnosticCode = DiagnosticCode::new("reload.port_in_use");
}

/// The development HTTP server.
pub mod server {
    use tola_build::diagnostic::DiagnosticCode;

    /// The server is reachable beyond this machine.
    pub const NETWORK_EXPOSED: DiagnosticCode = DiagnosticCode::new("server.network_exposed");
    /// The requested port is already in use.
    pub const PORT_IN_USE: DiagnosticCode = DiagnosticCode::new("server.port_in_use");
}

/// Terminal interaction.
pub mod terminal {
    use tola_build::diagnostic::DiagnosticCode;

    /// Terminal output could not be written.
    pub const WRITE: DiagnosticCode = DiagnosticCode::new("terminal.write");
    pub const PAGER: DiagnosticCode = DiagnosticCode::new("terminal.pager");
}

/// Filesystem observation.
pub mod watch {
    use tola_build::diagnostic::DiagnosticCode;

    /// A filesystem watch could not be attached.
    pub const ATTACH: DiagnosticCode = DiagnosticCode::new("watch.attach");
    /// A watched root could not be created.
    pub const CREATE: DiagnosticCode = DiagnosticCode::new("watch.create");
    /// A filesystem event could not be read.
    pub const EVENT: DiagnosticCode = DiagnosticCode::new("watch.event");
    /// Declared hook inputs could not be observed.
    pub const HOOK_INPUTS: DiagnosticCode = DiagnosticCode::new("watch.hook_inputs");
    /// The watcher runtime stopped.
    pub const RUNTIME: DiagnosticCode = DiagnosticCode::new("watch.runtime");
    /// Observation stopped before the build finished.
    pub const STOPPED: DiagnosticCode = DiagnosticCode::new("watch.stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use tola_build::diagnostic::DiagnosticCode;

    /// Every code this module declares or re-exports, keyed by the surface that owns it.
    const DECLARED_CODES_BY_SURFACE: [(&str, &[DiagnosticCode]); 15] = [
        ("build", &[build::TASK]),
        ("command", &[command::FAILED, command::SIGNAL]),
        (
            "config",
            &[config::LOAD, config::RELOAD, config::SERVE_RESTART],
        ),
        (
            "doctor",
            &[
                doctor::CHECKS,
                doctor::CONTENT_ROOT_MISSING,
                doctor::EDITOR_FILE_UNREADABLE,
                doctor::EDITOR_PACKAGES_MISSING,
                doctor::EDITOR_PACKAGES_STALE,
                doctor::ENTRY_MISSING,
                doctor::HOOK_COMMAND_MISSING,
                doctor::PACKAGE_LOCATIONS,
                doctor::VENDOR_LINK,
                doctor::VENDOR_PATH_MISSING,
            ],
        ),
        ("demo", &[demo::EXPORT, demo::PREVIEW]),
        (
            "editor",
            &[editor::CONFIGURATION, editor::NO_SITE, editor::LAUNCH],
        ),
        ("help", &[help::TARGET, help::MEMBER]),
        ("hook", &[hook::AFTER_PUBLISH]),
        ("init", &[init::CONFLICT, init::SELECTION]),
        ("internal", &[internal::ERROR]),
        ("log", &[log::LOCATION, log::UNAVAILABLE, log::WRITE]),
        ("reload", &[reload::PORT_IN_USE]),
        ("server", &[server::NETWORK_EXPOSED, server::PORT_IN_USE]),
        ("terminal", &[terminal::WRITE, terminal::PAGER]),
        (
            "watch",
            &[
                watch::ATTACH,
                watch::CREATE,
                watch::EVENT,
                watch::HOOK_INPUTS,
                watch::RUNTIME,
                watch::STOPPED,
            ],
        ),
    ];

    #[test]
    fn declared_codes_are_unique() {
        let mut seen = BTreeSet::new();
        for (_, codes) in DECLARED_CODES_BY_SURFACE {
            for code in codes {
                assert!(seen.insert(*code), "{code} is declared twice");
            }
        }
    }

    #[test]
    fn each_code_names_its_surface() {
        for (surface, codes) in DECLARED_CODES_BY_SURFACE {
            for code in codes {
                assert_eq!(code.surface(), surface, "{code}");
            }
        }
    }
}
