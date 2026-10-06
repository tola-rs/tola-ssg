//! Initialization target checks.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use tola_build::diagnostic::{Diagnostic, DiagnosticError, Severity};

use crate::writes::{ExistingFile, FileWrites};

use super::features::Effects;

/// The scaffold files and directories `writes` cannot create, aggregated into one failure.
pub(super) fn scaffold(writes: &FileWrites, force: bool) -> Result<()> {
    let root = writes.root();
    checks([
        target(root, force),
        crate::editor::check_initial_files(root),
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
/// The staged set is the minimal scaffold, a subset of every preset.
pub(super) fn preflight(
    root: &Path,
    force: bool,
    packages: &tola_typst::PackageLocations,
) -> Result<()> {
    let directory_refused = refuses(root, force)?;
    let existing = first_existing_file(root, packages)?;
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

fn target(root: &Path, force: bool) -> Result<()> {
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
            .with_help("choose a different path"),
        ],
    )
    .into()
}

/// The first scaffold file `root` already holds, if any.
fn first_existing_file(
    root: &Path,
    packages: &tola_typst::PackageLocations,
) -> Result<Option<PathBuf>> {
    let writes = super::files::file_writes(root, &[], packages, &Effects::default())?;
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
    .with_help("pass `--force` to add only the missing files");
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
    .with_help("remove it or choose another directory")
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
        Err(
            tola_build::diagnostic::DiagnosticError::new("cannot create the site", diagnostics)
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn init_target_refuses_occupied_paths() {
        let temp = TempDir::new().unwrap();
        let empty = TempDir::new().unwrap();
        let absent = temp.path().join("new_site");
        let occupied = temp.path().join("occupied");
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("file.txt"), "content").unwrap();
        let file = temp.path().join("site");
        fs::write(&file, "content").unwrap();

        for (path, force, accepted) in [
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
                accepted,
                "{} force={force}",
                path.display()
            );
        }
    }

    #[test]
    fn occupied_target_reports_conflict() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("file.txt"), "content").unwrap();

        let error = target(temp.path(), false).unwrap_err();
        let diagnostics = tola_build::diagnostic::attached(&error).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, crate::codes::init::CONFLICT);
        assert!(
            diagnostics[0].message.contains("is not empty"),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn file_target_reports_its_directory_requirement() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("site");
        fs::write(&file, "content").unwrap();
        let packages = tola_typst::PackageLocations::default();

        let diagnostics = preflight_diagnostics(&file, false, &packages);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, crate::codes::init::CONFLICT);
        assert!(
            diagnostics[0].message.contains("is not a directory"),
            "{diagnostics:?}"
        );
        assert_eq!(diagnostics[0].help[0].message, "choose a different path");
    }

    #[test]
    fn preflight_reports_the_target_conflict() {
        let temp = TempDir::new().unwrap();
        let packages = tola_typst::PackageLocations::default();

        let empty = temp.path().join("empty");
        fs::create_dir(&empty).unwrap();
        assert!(preflight(&empty, false, &packages).is_ok());

        let unrelated = temp.path().join("unrelated");
        fs::create_dir(&unrelated).unwrap();
        fs::write(unrelated.join("notes.txt"), "notes").unwrap();
        let diagnostics = preflight_diagnostics(&unrelated, false, &packages);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0].message.contains("is not empty"),
            "{diagnostics:?}"
        );
        assert!(diagnostics[0].notes.is_empty(), "{diagnostics:?}");
        assert_eq!(
            diagnostics[0].help[0].message,
            "pass `--force` to add only the missing files"
        );

        let existing = temp.path().join("existing");
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("site.typ"), "program").unwrap();
        let diagnostics = preflight_diagnostics(&existing, false, &packages);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0].message.contains("is not empty"),
            "{diagnostics:?}"
        );
        assert!(
            diagnostics[0]
                .notes
                .iter()
                .any(|note| note.contains("site.typ") && note.contains("never overwritten")),
            "{diagnostics:?}"
        );

        let diagnostics = preflight_diagnostics(&existing, true, &packages);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0].message.contains("site.typ")
                && diagnostics[0].message.contains("already exists"),
            "{diagnostics:?}"
        );
        assert!(diagnostics[0].notes.is_empty(), "{diagnostics:?}");
        assert_eq!(
            diagnostics[0].help[0].message,
            "remove it or choose another directory"
        );
        assert!(
            !diagnostics[0].message.contains("--force")
                && diagnostics[0]
                    .help
                    .iter()
                    .all(|help| !help.message.contains("--force")),
            "{diagnostics:?}"
        );

        assert!(preflight(&unrelated, true, &packages).is_ok());
    }

    /// The diagnostics a refused preflight has.
    fn preflight_diagnostics(
        root: &Path,
        force: bool,
        packages: &tola_typst::PackageLocations,
    ) -> Vec<Diagnostic> {
        let error = preflight(root, force, packages).unwrap_err();
        tola_build::diagnostic::attached(&error)
            .expect("the preflight has its diagnostics")
            .to_vec()
    }

    #[test]
    fn cancelled_validation_keeps_its_type() {
        let error = checks([Err(tola_build::cancellation::BuildCancelled.into())]).unwrap_err();

        assert!(error.is::<tola_build::cancellation::BuildCancelled>());
    }
}
