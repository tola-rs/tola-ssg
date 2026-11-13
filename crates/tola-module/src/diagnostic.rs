//! Diagnostics for acquiring a declared module.

use tola_diagnostic::{Diagnostic, Severity, codes, collapse_whitespace, render_relative};
use tola_input::TarballLocation;

use crate::RemoteRevision;
use crate::inputs::{ArchiveStage, ModuleRootError};
use crate::manifest::ManifestError;
use crate::remote::FetchError;

/// Explain a module source failure using its declaration and the operation that failed.
pub fn source_diagnostic(error: &anyhow::Error) -> Option<Diagnostic> {
    let error = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ModuleRootError>())?;
    let report = |message: String| Diagnostic::new(codes::module::SOURCE, Severity::Error, message);
    Some(match error {
        ModuleRootError::Fetch {
            alias,
            revision,
            source,
        } => {
            let selector = match revision {
                Some(RemoteRevision::Tag(tag)) => format!("tag `{tag}`"),
                Some(RemoteRevision::Branch(branch)) => format!("branch `{branch}`"),
                Some(RemoteRevision::Rev(rev)) => format!("commit `{rev}`"),
                None => "the default branch".into(),
            };
            let declaration = |field: &str| format!("`modules.{alias}.{field}`");
            let (reason, help) = match source {
                FetchError::Address { .. } => (
                    "the repository address is invalid",
                    format!("Use an HTTPS or SSH address in {}", declaration("git.url")),
                ),
                FetchError::Authentication { .. } => (
                    "the repository rejected or requires credentials",
                    "Check the Git credential helper for HTTPS or the SSH key for SSH".into(),
                ),
                FetchError::NotVisible { .. } => (
                    "the repository is unavailable or private",
                    format!(
                        "Check {} and your access to the repository",
                        declaration("git.url")
                    ),
                ),
                FetchError::Network { .. } => (
                    "the repository could not be reached",
                    "Check the connection or proxy configuration".into(),
                ),
                FetchError::MissingSsh { .. } => (
                    "the system SSH client could not be started",
                    format!(
                        "Check that `ssh` is available, or use HTTPS in {}",
                        declaration("git.url")
                    ),
                ),
                FetchError::RevisionUnavailable { .. } => (
                    "the repository did not provide that revision",
                    format!(
                        "Check that {selector} is available from the repository in {}",
                        declaration("git.url")
                    ),
                ),
                FetchError::Other { .. } => (
                    "the fetch failed",
                    format!("Check {} and {}", declaration("git.url"), selector),
                ),
            };
            let message = format!("could not fetch {selector} for module `{alias}`");
            report(message).with_note(reason).with_help(help)
        }
        ModuleRootError::Subdir {
            alias,
            subdir,
            commit,
            ..
        } => {
            let directory = if subdir.as_os_str().is_empty() {
                ".".into()
            } else {
                render_relative(subdir)
            };
            report(format!(
                "module `{alias}` cannot read `{directory}` as a directory in commit `{commit}`"
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
        } => {
            let (field, read) = match location {
                TarballLocation::Url { .. } => ("tarball.url", "download"),
                TarballLocation::File { .. } => ("tarball.path", "read"),
            };
            let key = format!("`modules.{alias}.{field}`");
            let (message, help) = match stage {
                ArchiveStage::Read => (
                    format!("could not {read} the archive for module `{alias}`"),
                    format!("Check {key} and confirm the archive is accessible"),
                ),
                ArchiveStage::Extract => (
                    format!("could not extract the archive for module `{alias}`"),
                    format!(
                        "Check the archive at {key} or ask the publisher for a corrected release"
                    ),
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
            let reason = match source.downcast_ref::<ManifestError>() {
                Some(ManifestError::ManifestParse { source, .. }) => source.message().to_owned(),
                _ if source
                    .downcast_ref::<reqwest::Error>()
                    .is_some_and(reqwest::Error::is_timeout) =>
                {
                    "the download timed out".into()
                }
                _ => collapse_whitespace(&source.to_string()),
            };
            report(message).with_note(reason).with_help(help)
        }
        ModuleRootError::TarballDigestMismatch {
            alias,
            declared,
            expected,
            found,
        } => {
            let (expected_from, help) = if *declared {
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
                "expected {expected} from {expected_from}, found {found}"
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
