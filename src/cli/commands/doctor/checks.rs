//! Read-only site and local-tool checks for `tola doctor`.

use crate::cancellation::Cancellation;
use crate::cli::log::destination;
use crate::editor::{GeneratedDirectory, HeldFile};
use std::path::{Path, PathBuf};

use tola_build::diagnostic::{Diagnostic, Severity};

pub(super) fn diagnose(
    config_file: &crate::cli::ConfigFileArgs,
    packages: &crate::cli::TypstPackageArgs,
    scope: tola_build::InputScope,
    output: &crate::cli::output::CommandOutput,
    cancellation: &Cancellation,
) -> anyhow::Result<Vec<Diagnostic>> {
    let root = tola_build::config::loading::config_site_root(config_file.path.as_deref())?;
    let package_locations = match crate::cli::config::package_locations(packages, scope) {
        Ok(locations) => locations,
        Err(error) => {
            destination::start_after_error(output, &root, &cancellation.token());
            let mut diagnostics = vec![tola_build::diagnostic::fallback(
                crate::codes::doctor::PACKAGE_LOCATIONS,
                &error,
            )];
            diagnostics.extend(environment_diagnostics(&root));
            return Ok(diagnostics);
        }
    };
    let loaded = crate::config::load(
        config_file.path.as_deref(),
        scope,
        package_locations,
        &crate::config::ConfigOverrides::default(),
    );

    match loaded {
        Ok(loaded) => {
            output.apply_diagnostic_limits(loaded.diagnostics());
            let config = loaded.config();
            destination::start_site(output, config, &cancellation.token())?;
            let mut diagnostics = tola_build::config::diagnostic::warning_diagnostics(config);
            diagnostics.extend(diagnose_loaded(loaded.into_config()));
            Ok(diagnostics)
        }
        Err(error) => {
            destination::start_after_error(output, &root, &cancellation.token());
            let mut diagnostics =
                crate::cli::output::attached_or_fallback(&error, crate::codes::config::LOAD);
            diagnostics.extend(environment_diagnostics(&root));
            Ok(diagnostics)
        }
    }
}

fn diagnose_loaded(config: tola_build::config::ResolvedSiteConfig) -> Vec<Diagnostic> {
    let root = config.get_root();
    let mut diagnostics = Vec::new();
    if !config.build().entry.is_file() {
        diagnostics.push(
            path_diagnostic(
                crate::codes::doctor::ENTRY_MISSING,
                Severity::Error,
                root,
                &config.build().entry,
                "`build.entry` is not a file",
            )
            .with_help("Create the entry file, or correct `build.entry`"),
        );
    }
    if !config.build().content_dir.is_dir() {
        diagnostics.push(
            path_diagnostic(
                crate::codes::doctor::CONTENT_ROOT_MISSING,
                Severity::Error,
                root,
                &config.build().content_dir,
                "`build.content-dir` is not a directory",
            )
            .with_help("Create the content directory, or correct `build.content-dir`"),
        );
    }

    diagnostics.extend(editor_package_diagnostics(root));

    for arguments in config.build().hooks.enabled_commands() {
        if let Some(command) = arguments.first()
            && !command_exists(command, config.get_root())
        {
            diagnostics.push(
                Diagnostic::new(
                    crate::codes::doctor::HOOK_COMMAND_MISSING,
                    Severity::Error,
                    format!("hook command `{command}` was not found"),
                )
                .with_help("Install the command on `PATH`, or correct its path in `build.hooks`"),
            );
        }
    }

    diagnostics.extend(environment_diagnostics(config.get_root()));

    diagnostics.extend(vendor_diagnostics(&config));
    diagnostics
}

/// What `tola doctor` reports about the generated package directory: the paths a writing command
/// refuses, what differs from the sources this Tola ships, what is missing, and what another
/// version left behind.
fn editor_package_diagnostics(root: &Path) -> Vec<Diagnostic> {
    let directory = root.join(tola_build::filesystem::PACKAGE_MIRROR_DIRECTORY);
    let mut diagnostics = Vec::new();
    match crate::editor::generated_directory(root) {
        GeneratedDirectory::Missing => {
            diagnostics.push(
                path_diagnostic(
                    crate::codes::doctor::EDITOR_PACKAGES_MISSING,
                    Severity::Warning,
                    root,
                    &directory,
                    "the generated package directory is missing",
                )
                .with_help("run `tola editor packages` to create it"),
            );
            return diagnostics;
        }
        GeneratedDirectory::Refused(refused) => {
            diagnostics.push(
                path_diagnostic(
                    crate::codes::doctor::EDITOR_PACKAGES_UNUSABLE,
                    Severity::Warning,
                    root,
                    &directory,
                    "Tola will not write the generated package directory",
                )
                .with_note(format!("{} {}", refused.displayed(), refused.condition()))
                .with_help("Move it aside, then run `tola editor packages`"),
            );
            return diagnostics;
        }
        GeneratedDirectory::Usable => {}
    }

    let generated = crate::editor::generated_package_files();
    let found = crate::editor::read_package_directory(root);
    let mut differ = Vec::new();
    let mut missing = Vec::new();
    // Every file below a refused path reports that one path, so the map holds each refusal once.
    let mut refused = std::collections::BTreeMap::new();
    for (relative, held) in found.files {
        let path = root.join(relative);
        match held {
            HeldFile::Differing => differ.push(path),
            HeldFile::Missing => missing.push(path),
            HeldFile::Refused(refusal) => {
                refused.insert(refusal.displayed(), refusal);
            }
            HeldFile::Current => {}
        }
    }
    for refusal in found.refused {
        refused.insert(refusal.displayed(), refusal);
    }
    if !refused.is_empty() {
        let mut diagnostic = path_diagnostic(
            crate::codes::doctor::EDITOR_PACKAGES_UNUSABLE,
            Severity::Warning,
            root,
            &directory,
            "Tola will not write some paths in the generated package directory",
        )
        .with_note(format!(
            "{} Tola will not write",
            crate::terminal::plural_count(refused.len(), "path")
        ));
        for (displayed, refusal) in refused.iter().take(SAMPLED) {
            diagnostic = diagnostic.with_note(format!("{displayed} {}", refusal.condition()));
        }
        if let Some(extra) = refused
            .len()
            .checked_sub(SAMPLED)
            .filter(|count| *count > 0)
        {
            diagnostic = diagnostic.with_note(crate::terminal::plural_count(extra, "more path"));
        }
        diagnostics.push(diagnostic.with_help("Move them aside, then run `tola editor packages`"));
    }

    // A stable order, so two runs of the same directory name the same files.
    differ.sort();
    missing.sort();
    if !differ.is_empty() || !missing.is_empty() || !found.obsolete.is_empty() {
        let mut diagnostic = path_diagnostic(
            crate::codes::doctor::EDITOR_PACKAGES_STALE,
            Severity::Warning,
            root,
            &directory,
            "editor packages do not match the sources this Tola ships",
        );
        diagnostic = sampled(
            diagnostic,
            root,
            &differ,
            format!("{} of {} files differ", differ.len(), generated.len()),
            "file",
        );
        diagnostic = sampled(
            diagnostic,
            root,
            &missing,
            format!(
                "{} missing",
                crate::terminal::plural_count(missing.len(), "file")
            ),
            "file",
        );
        diagnostic = sampled(
            diagnostic,
            root,
            &found.obsolete,
            format!(
                "{} left over from another version",
                crate::terminal::plural_count(found.obsolete.len(), "path")
            ),
            "path",
        );
        if !differ.is_empty() {
            diagnostic = diagnostic
                .with_note("the editor opens these files for `@tola/*` definitions")
                .with_note(
                    "while they differ, the editor uses the copies inside Tola; a build always does",
                );
        }
        diagnostics.push(diagnostic.with_help("run `tola editor packages` to regenerate them"));
    }
    diagnostics
}

/// How many paths of one class a report names before it counts the rest.
const SAMPLED: usize = 3;

/// Add one class of the report: its count, at most [`SAMPLED`] of its paths, and the rest counted.
fn sampled(
    mut diagnostic: Diagnostic,
    root: &Path,
    paths: &[PathBuf],
    count: String,
    noun: &str,
) -> Diagnostic {
    if paths.is_empty() {
        return diagnostic;
    }
    diagnostic = diagnostic.with_note(count);
    for path in paths.iter().take(SAMPLED) {
        diagnostic = diagnostic.with_note(crate::terminal::display_path_within(path, root));
    }
    if let Some(extra) = paths.len().checked_sub(SAMPLED).filter(|count| *count > 0) {
        diagnostic =
            diagnostic.with_note(crate::terminal::plural_count(extra, &format!("more {noun}")));
    }
    diagnostic
}

fn environment_diagnostics(root: &Path) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for relative in [".vscode/settings.json", ".helix/languages.toml"] {
        let path = root.join(relative);
        match std::fs::read_to_string(&path) {
            // An absent settings file configures nothing; an unreadable one hides which
            // language service the editor runs.
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => diagnostics.push(
                path_diagnostic(
                    crate::codes::doctor::EDITOR_FILE_UNREADABLE,
                    Severity::Warning,
                    root,
                    &path,
                    "Tola could not read this editor settings file",
                )
                .with_help("Make the file readable, then rerun `tola doctor`"),
            ),
        }
    }
    diagnostics
}

fn command_exists(command: &str, root: &Path) -> bool {
    let path = Path::new(command);
    if path.components().count() > 1 {
        return root.join(path).is_file();
    }
    which::which(command).is_ok()
}

/// A vendored copy is portable only while it holds real files.
fn vendor_diagnostics(config: &tola_build::config::ResolvedSiteConfig) -> Vec<Diagnostic> {
    let Some(vendor) = config.vendor().path.as_ref() else {
        return Vec::new();
    };
    let root = config.get_root();
    if !vendor.is_dir() {
        return vec![
            path_diagnostic(
                crate::codes::doctor::VENDOR_PATH_MISSING,
                Severity::Warning,
                root,
                vendor,
                "`vendor.path` is not a directory",
            )
            .with_help("run `tola vendor` to write the site's external sources into it"),
        ];
    }
    let links = links_inside(vendor);
    let Some(first) = links.first() else {
        return Vec::new();
    };
    let mut diagnostic = path_diagnostic(
        crate::codes::doctor::VENDOR_LINK,
        Severity::Warning,
        root,
        first,
        "the vendored copy contains a symbolic link",
    )
    .with_note("the copy cannot be built elsewhere")
    .with_help("Run `tola vendor --refresh` to resolve fresh copies");
    if let Some(extra) = links.len().checked_sub(1).filter(|count| *count > 0) {
        diagnostic = diagnostic.with_note(format!("{extra} more links in the same directory"));
    }
    vec![diagnostic]
}

/// Every symbolic link below a directory, in a stable order.
fn links_inside(directory: &Path) -> Vec<PathBuf> {
    let mut links = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if metadata.is_symlink() {
                links.push(entry.path());
            } else if metadata.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    links.sort();
    links
}

fn path_diagnostic(
    code: tola_build::diagnostic::DiagnosticCode,
    severity: Severity,
    root: &Path,
    path: &Path,
    message: &str,
) -> Diagnostic {
    Diagnostic::at_path(
        code,
        severity,
        crate::terminal::display_path_within(path, root),
        message,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn pure_scope_reports_selected_package_roots() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(
            &path,
            "[build]\nentry = \"site.typ\"\ncontent-dir = \"content\"\n",
        )
        .unwrap();
        let reported = diagnose_config(
            &path,
            &crate::cli::TypstPackageArgs {
                package_path: Some(directory.path().join("host-packages")),
                ..crate::cli::TypstPackageArgs::default()
            },
            tola_build::InputScope::Pure,
        );

        assert!(
            reported
                .iter()
                .any(|diagnostic| diagnostic.code == crate::codes::doctor::PACKAGE_LOCATIONS)
        );
    }

    #[test]
    fn missing_config_is_reported() {
        let directory = tempdir().unwrap();
        let missing = directory.path().join("missing.toml");
        let diagnostics = diagnose_at(&missing);

        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == tola_build::codes::config::IO)
        );
    }

    #[test]
    fn refused_package_paths_are_reported_once() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(
            &path,
            "[build]\nentry = \"site.typ\"\ncontent-dir = \"content\"\n",
        )
        .unwrap();
        let (relative, _) = crate::editor::generated_package_files()
            .into_iter()
            .next()
            .expect("builtin packages provide editor files");
        // A directory where one generated file belongs: a path Tola will not write, which is not a
        // file that differs from the sources this Tola ships.
        std::fs::create_dir_all(directory.path().join(&relative)).unwrap();
        let displayed = relative.to_string_lossy().into_owned();

        let reported = diagnose_at(&path);
        let refused = reported
            .iter()
            .find(|diagnostic| diagnostic.code == crate::codes::doctor::EDITOR_PACKAGES_UNUSABLE)
            .expect("a generated package path Tola refuses is reported");
        assert!(
            refused
                .notes
                .iter()
                .any(|note| note.contains(&displayed) && note.contains("is not a file")),
            "{:?}",
            refused.notes
        );
        let reported_elsewhere: Vec<&str> = reported
            .iter()
            .filter(|diagnostic| diagnostic.code != crate::codes::doctor::EDITOR_PACKAGES_UNUSABLE)
            .flat_map(|diagnostic| diagnostic.notes.iter().map(String::as_str))
            .collect();
        assert!(
            !reported_elsewhere
                .iter()
                .any(|note| note.contains(&displayed)),
            "the refused path is reported once: {reported_elsewhere:?}"
        );
    }

    #[test]
    fn unreadable_editor_settings_file_is_reported() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(
            &path,
            "[build]\nentry = \"site.typ\"\ncontent-dir = \"content\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(directory.path().join(".vscode/settings.json")).unwrap();

        let reported = diagnose_at(&path);
        reported
            .iter()
            .find(|diagnostic| {
                diagnostic.code == crate::codes::doctor::EDITOR_FILE_UNREADABLE
                    && diagnostic
                        .location
                        .as_ref()
                        .is_some_and(|location| location.path.ends_with(".vscode/settings.json"))
            })
            .expect("an unreadable settings file is reported");
    }

    /// The diagnostics `tola doctor` reports for `path`, with `packages` and `scope`, against a
    /// plain terminal and a fresh cancellation.
    fn diagnose_config(
        path: &Path,
        packages: &crate::cli::TypstPackageArgs,
        scope: tola_build::InputScope,
    ) -> Vec<Diagnostic> {
        diagnose(
            &crate::cli::ConfigFileArgs {
                path: Some(path.to_path_buf()),
            },
            packages,
            scope,
            &crate::cli::output::CommandOutput::new(
                crate::terminal::Terminal::new(clap::ColorChoice::Never, false, None),
                None,
            ),
            &Cancellation::default(),
        )
        .unwrap()
    }

    fn diagnose_at(path: &Path) -> Vec<Diagnostic> {
        diagnose_config(path, &Default::default(), tola_build::InputScope::Online)
    }

    #[test]
    fn malformed_config_stops_site_checks() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(&path, "[build\n").unwrap();

        let diagnostics = diagnose_at(&path);

        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "config.toml")
        );
        assert!(
            !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == crate::codes::doctor::ENTRY_MISSING)
        );
    }

    #[test]
    fn missing_vendor_directory_is_reported() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(
            &path,
            "[build]\nentry = \"site.typ\"\ncontent-dir = \"content\"\n[vendor]\npath = \"vendor\"\n",
        )
        .unwrap();

        let reported = diagnose_at(&path);
        let missing = reported
            .iter()
            .find(|diagnostic| diagnostic.code == crate::codes::doctor::VENDOR_PATH_MISSING)
            .expect("a declared vendor directory that does not exist is reported");

        assert_eq!(
            missing
                .location
                .as_ref()
                .map(|location| location.path.as_str()),
            Some("vendor")
        );
    }

    #[cfg(unix)]
    #[test]
    fn vendored_links_are_reported() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(
            &path,
            "[build]\nentry = \"site.typ\"\ncontent-dir = \"content\"\n[vendor]\npath = \"vendor\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(directory.path().join("vendor/typst-packages")).unwrap();
        let link = directory.path().join("vendor/typst-packages/theme");
        std::os::unix::fs::symlink(directory.path().join("theme-source"), &link).unwrap();

        let reported = diagnose_at(&path);
        let linked = reported
            .iter()
            .find(|diagnostic| diagnostic.code == crate::codes::doctor::VENDOR_LINK)
            .expect("a link inside the vendor directory is reported");

        assert_eq!(
            linked
                .location
                .as_ref()
                .map(|location| location.path.as_str()),
            Some("vendor/typst-packages/theme")
        );
    }
}
