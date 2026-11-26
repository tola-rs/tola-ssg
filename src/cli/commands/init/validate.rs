//! Initialization target checks.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use tola_diagnostic::{Diagnostic, DiagnosticError, Severity};

use crate::writes::{ExistingFile, FileWrites};

/// The scaffold files and directories `writes` cannot create, aggregated into one failure.
pub(super) fn scaffold(writes: &FileWrites, force: bool) -> Result<()> {
    let root = writes.root();
    checks([
        target(root, force),
        writes
            .check()
            .map_err(|error| match error.downcast_ref::<ExistingFile>() {
                Some(existing) => file_conflict_error(&existing.path, root),
                None => error.context("cannot create the site"),
            }),
    ])
}

/// The conflict a real run reports before it asks anything: the target policy alone, then the
/// first scaffold file that already exists.
///
/// The staged set is the built-in site.
pub(super) fn preflight(root: &Path, force: bool) -> Result<()> {
    let directory_refused = refuses(root, force)?;
    let existing = first_existing_file(root)?;
    let conflict = match (directory_refused, existing.as_deref()) {
        (false, None) => return Ok(()),
        (false, Some(existing)) => file_conflict(existing, root),
        (true, existing) => directory_conflict(root, existing),
    };
    Err(DiagnosticError::new("cannot create the site", vec![conflict]).into())
}

/// Whether the target policy refuses this scaffold: an occupied directory without `--force`.
fn refuses(root: &Path, force: bool) -> Result<bool> {
    let occupied = occupied(root)?;
    Ok(!force && occupied)
}

pub(super) fn target(root: &Path, force: bool) -> Result<()> {
    if refuses(root, force)? {
        return Err(DiagnosticError::new(
            "cannot create the site",
            vec![directory_conflict(root, None)],
        )
        .into());
    }
    Ok(())
}

/// Whether `root` already holds entries.
///
/// A missing root is empty; a path that is not a directory is refused here.
fn occupied(root: &Path) -> Result<bool> {
    let metadata = match fs::metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if fs::symlink_metadata(root).is_ok() {
                return Err(not_a_directory_error(root));
            }
            return Ok(false);
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "could not inspect the site directory `{}`",
                    crate::terminal::display_path(root)
                )
            });
        }
    };
    if !metadata.is_dir() {
        return Err(not_a_directory_error(root));
    }
    Ok(fs::read_dir(root)
        .with_context(|| {
            format!(
                "could not read the site directory `{}`",
                crate::terminal::display_path(root)
            )
        })?
        .next()
        .transpose()?
        .is_some())
}

/// The failure of a site path that exists as something other than a directory.
fn not_a_directory_error(root: &Path) -> anyhow::Error {
    DiagnosticError::new(
        "cannot create the site",
        vec![
            Diagnostic::new(
                crate::codes::init::CONFLICT,
                Severity::Error,
                format!(
                    "`{}` is not a directory",
                    crate::terminal::display_path(root)
                ),
            )
            .with_help("Choose a different path"),
        ],
    )
    .into()
}

/// The first scaffold file `root` already holds, if any.
fn first_existing_file(root: &Path) -> Result<Option<PathBuf>> {
    let writes = super::files::file_writes(root)?;
    Ok(writes
        .check()
        .err()
        .and_then(|error| error.downcast::<ExistingFile>().ok())
        .map(|existing| existing.path))
}

/// The conflict of an occupied directory, naming the scaffold file that already exists when one
/// does.
fn directory_conflict(root: &Path, existing: Option<&Path>) -> Diagnostic {
    let mut diagnostic = Diagnostic::new(
        crate::codes::init::CONFLICT,
        Severity::Error,
        format!(
            "directory `{}` is not empty",
            crate::terminal::display_path(root)
        ),
    )
    .with_help("Pass `--force` to add only the missing files");
    if let Some(existing) = existing {
        diagnostic = diagnostic.with_note(format!(
            "`{}` already exists; existing files are never overwritten",
            crate::terminal::display_path_within(existing, root)
        ));
    }
    diagnostic
}

/// The conflict of one scaffold file that already exists; `--force` cannot resolve it.
fn file_conflict(existing: &Path, root: &Path) -> Diagnostic {
    Diagnostic::new(
        crate::codes::init::CONFLICT,
        Severity::Error,
        format!(
            "`{}` already exists",
            crate::terminal::display_path_within(existing, root)
        ),
    )
    .with_help("Remove it or choose another directory")
}

fn file_conflict_error(existing: &Path, root: &Path) -> anyhow::Error {
    DiagnosticError::new(
        "cannot create the site",
        vec![file_conflict(existing, root)],
    )
    .into()
}

pub(super) fn checks(checks: impl IntoIterator<Item = Result<()>>) -> Result<()> {
    let mut diagnostics = Vec::new();
    for error in checks.into_iter().filter_map(Result::err) {
        if error.is::<tola_build::cancellation::BuildCancelled>() {
            return Err(error);
        }
        diagnostics.extend(crate::cli::output::attached_or_fallback(
            &error,
            crate::codes::init::CONFLICT,
        ));
    }
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(tola_diagnostic::DiagnosticError::new("cannot create the site", diagnostics).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn preflight_diags(root: &Path, force: bool) -> Vec<Diagnostic> {
        let err = preflight(root, force).unwrap_err();
        tola_diagnostic::attached(&err)
            .expect("the preflight has its diagnostics")
            .to_vec()
    }

    #[test]
    fn target_rejects_occupied_paths() {
        let dir = TempDir::new().unwrap();
        let empty = TempDir::new().unwrap();
        let absent = dir.path().join("new_site");
        let occupied = dir.path().join("occupied");
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("file.txt"), "content").unwrap();
        let file = dir.path().join("site");
        fs::write(&file, "content").unwrap();

        for (path, force, ok) in [
            (empty.path().to_path_buf(), false, true),
            (empty.path().join("."), false, true),
            (absent, false, true),
            (occupied.clone(), false, false),
            (occupied, true, true),
            (file.clone(), false, false),
            (file, true, false),
        ] {
            assert_eq!(
                target(&path, force).is_ok(),
                ok,
                "{} force={force}",
                path.display()
            );
        }
    }

    #[test]
    fn file_target_rejected() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("site");
        fs::write(&file, "content").unwrap();

        let diags = preflight_diags(&file, false);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, crate::codes::init::CONFLICT);
    }

    #[test]
    fn preflight_reports_conflict() {
        let dir = TempDir::new().unwrap();

        let empty = dir.path().join("empty");
        fs::create_dir(&empty).unwrap();
        assert!(preflight(&empty, false).is_ok());

        let unrelated = dir.path().join("unrelated");
        fs::create_dir(&unrelated).unwrap();
        fs::write(unrelated.join("notes.txt"), "notes").unwrap();
        let diags = preflight_diags(&unrelated, false);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, crate::codes::init::CONFLICT);
        assert!(diags[0].notes.is_empty(), "{diags:?}");
        assert!(
            diags[0].help.iter().any(|h| h.message.contains("--force")),
            "{diags:?}"
        );

        let existing = dir.path().join("existing");
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("site.typ"), "program").unwrap();
        let diags = preflight_diags(&existing, false);
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].notes.iter().any(|n| n.contains("site.typ")),
            "{diags:?}"
        );

        let diags = preflight_diags(&existing, true);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("site.typ"), "{diags:?}");
        assert!(diags[0].notes.is_empty(), "{diags:?}");
        assert!(
            !diags[0].message.contains("--force")
                && diags[0].help.iter().all(|h| !h.message.contains("--force")),
            "{diags:?}"
        );

        assert!(preflight(&unrelated, true).is_ok());
    }

    #[test]
    fn cancelled_validation_keeps_type() {
        let err = checks([Err(tola_build::cancellation::BuildCancelled.into())]).unwrap_err();
        assert!(err.is::<tola_build::cancellation::BuildCancelled>());
    }
}
