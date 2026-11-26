//! Diagnostics for acquiring a declared module.

use tola_diagnostic::{Diagnostic, Severity, codes, collapse_whitespace, render_relative};
use tola_input::TarballLocation;

use crate::RemoteRevision;
use crate::inputs::{ArchiveStage, ModuleRootError};
use crate::manifest::ManifestError;
use crate::remote::FetchError;

pub(crate) fn manifest_diagnostic(err: &ManifestError) -> Option<Diagnostic> {
    let (path, file, msg, help) = match err {
        ManifestError::Rejected { diagnostic, .. } => return Some(diagnostic.as_ref().clone()),
        ManifestError::ManifestPath { path, file, source }
        | ManifestError::ManifestRead { path, file, source } => {
            let (msg, help) = match source.kind() {
                std::io::ErrorKind::NotFound => (
                    format!("`{path}` is missing"),
                    "Create the file at that path or choose a different module directory",
                ),
                std::io::ErrorKind::PermissionDenied => (
                    format!("could not read `{path}` because access was denied"),
                    "Make the file readable, then try again",
                ),
                std::io::ErrorKind::InvalidData => (
                    format!("`{path}` must be a regular file"),
                    "Replace it with a TOML file",
                ),
                _ => (
                    err.to_string(),
                    "Check that the file is accessible, then try again",
                ),
            };
            (path, file, msg, help)
        }
        ManifestError::ManifestEncoding { path, file, .. } => {
            (path, file, err.to_string(), "Save the file as UTF-8 text")
        }
        ManifestError::ManifestChanged { path, file }
        | ManifestError::ManifestMoved { path, file } => (
            path,
            file,
            err.to_string(),
            "Wait for the file to finish saving, then try again",
        ),
        ManifestError::ManifestEscapesRoot { path, file, .. } => (
            path,
            file,
            err.to_string(),
            "Place `tola-module.toml` inside the module directory",
        ),
        _ => return None,
    };
    Some(
        Diagnostic::at_path(codes::module::MANIFEST, Severity::Error, path, msg)
            .with_file(file.clone())
            .with_help(help),
    )
}

/// Explain a module acquisition or manifest failure.
pub fn error_diagnostic(error: &anyhow::Error) -> Option<Diagnostic> {
    source_diagnostic(error).or_else(|| {
        error.chain().find_map(|cause| {
            cause
                .downcast_ref::<ManifestError>()
                .and_then(manifest_diagnostic)
        })
    })
}

fn source_diagnostic(err: &anyhow::Error) -> Option<Diagnostic> {
    let root_err = err
        .chain()
        .find_map(|c| c.downcast_ref::<ModuleRootError>())?;
    let report = |msg: String| Diagnostic::new(codes::module::SOURCE, Severity::Error, msg);
    Some(match root_err {
        ModuleRootError::Fetch {
            alias,
            revision,
            source,
        } => fetch_diagnostic(alias, revision.as_ref(), source),
        ModuleRootError::Subdir {
            alias,
            subdir,
            commit,
            ..
        } => {
            let dir = if subdir.as_os_str().is_empty() {
                ".".into()
            } else {
                render_relative(subdir)
            };
            report(format!(
                "module `{alias}` cannot read `{dir}` as a directory in commit `{commit}`"
            ))
            .with_help(format!(
                "Check that `modules.{alias}.git.subdir` names a directory inside that revision"
            ))
        }
        ModuleRootError::Tarball {
            alias,
            location,
            stage,
            source,
        } => tarball_diagnostic(alias, location, *stage, source),
        ModuleRootError::TarballDigestMismatch {
            alias,
            declared,
            expected,
            found,
        } => {
            let (src_label, help) = if *declared {
                (
                    format!("`modules.{alias}.tarball.sha256`"),
                    "Check the archive against the publisher's checksum before changing the declaration".into(),
                )
            } else {
                (
                    "`tola.lock`".into(),
                    format!(
                        "Restore the locked archive, or run `tola update {alias}` if you intend to accept the new archive"
                    ),
                )
            };
            report(format!(
                "the archive for module `{alias}` does not match its checksum"
            ))
            .with_note(format!(
                "expected {expected} from {src_label}, found {found}"
            ))
            .with_help(help)
        }
        ModuleRootError::NotLocked { alias } => {
            report(format!("module `{alias}` has no entry in `tola.lock`"))
                .with_help(format!("Run `tola update {alias}`"))
        }
        ModuleRootError::DeclarationChanged { alias } => report(format!(
            "the declaration of module `{alias}` differs from `tola.lock`"
        ))
        .with_help(format!(
            "Check `modules.{alias}`, then run `tola update {alias}`"
        )),
        ModuleRootError::NetworkDenied { alias, scope } => report(format!(
            "module `{alias}` needs a download, which `--{scope}` does not allow"
        ))
        .with_help("Run the command online or provide the site's vendor copy"),
        _ => return None,
    })
}

fn fetch_diagnostic(alias: &str, rev: Option<&RemoteRevision>, err: &FetchError) -> Diagnostic {
    let sel = match rev {
        Some(RemoteRevision::Tag(tag)) => format!("tag `{tag}`"),
        Some(RemoteRevision::Branch(branch)) => format!("branch `{branch}`"),
        Some(RemoteRevision::Rev(rev)) => format!("commit `{rev}`"),
        None => "the default branch".into(),
    };
    let key = |field: &str| format!("`modules.{alias}.{field}`");
    let (reason, help) = match err {
        FetchError::Address { .. } => (
            "the repository address is invalid",
            format!("Use an HTTPS or SSH address in {}", key("git.url")),
        ),
        FetchError::Authentication { .. } => (
            "the repository rejected or requires credentials",
            "Check the Git credential helper for HTTPS or the SSH key for SSH".into(),
        ),
        FetchError::NotVisible { .. } => (
            "the repository is unavailable or private",
            format!("Check {} and your access to the repository", key("git.url")),
        ),
        FetchError::Network { .. } => (
            "the repository could not be reached",
            "Check the connection or proxy configuration".into(),
        ),
        FetchError::MissingSsh { .. } => (
            "the system SSH client could not be started",
            format!(
                "Check that `ssh` is available, or use HTTPS in {}",
                key("git.url")
            ),
        ),
        FetchError::RevisionUnavailable { .. } => (
            "the repository did not provide that revision",
            format!(
                "Check that {sel} is available from the repository in {}",
                key("git.url")
            ),
        ),
        FetchError::Other { .. } => (
            "the fetch failed",
            format!("Check {} and {sel}", key("git.url")),
        ),
    };
    Diagnostic::new(
        codes::module::SOURCE,
        Severity::Error,
        format!("could not fetch {sel} for module `{alias}`"),
    )
    .with_note(reason)
    .with_help(help)
}

fn tarball_diagnostic(
    alias: &str,
    loc: &TarballLocation,
    stage: ArchiveStage,
    err: &anyhow::Error,
) -> Diagnostic {
    let (field, verb) = match loc {
        TarballLocation::Url { .. } => ("tarball.url", "download"),
        TarballLocation::File { .. } => ("tarball.path", "read"),
    };
    let key = format!("`modules.{alias}.{field}`");
    let (msg, help) = match stage {
        ArchiveStage::Read => (
            format!("could not {verb} the archive for module `{alias}`"),
            format!("Check {key} and confirm the archive is accessible"),
        ),
        ArchiveStage::Extract => (
            format!("could not extract the archive for module `{alias}`"),
            format!("Check the archive at {key} or ask the publisher for a corrected release"),
        ),
        ArchiveStage::Manifest => (
            format!("the archive for module `{alias}` has an invalid `tola-module.toml`"),
            format!("Check the release selected by {key}"),
        ),
        ArchiveStage::Cache => (
            format!("could not save the archive contents for module `{alias}`"),
            "Check that Tola can write to the site's `.tola` directory".into(),
        ),
    };
    let reason = match err.downcast_ref::<ManifestError>() {
        Some(ManifestError::Rejected { diagnostic, .. }) => diagnostic.message.clone(),
        _ if err
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_timeout) =>
        {
            "the download timed out".into()
        }
        _ => collapse_whitespace(&err.to_string()),
    };
    Diagnostic::new(codes::module::SOURCE, Severity::Error, msg)
        .with_note(reason)
        .with_help(help)
}
