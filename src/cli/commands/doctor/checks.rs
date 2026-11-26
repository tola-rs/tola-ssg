//! Read-only site and local-tool checks for `tola doctor`.

use crate::cancellation::Cancellation;
use crate::cli::log::destination;
use crate::editor::{GeneratedDirectory, HeldFile};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use tola_build::config::SiteConfig;
use tola_build::hooks::{configured_hooks, expand_hook_argument, generated_input_producer};
use tola_diagnostic::{Diagnostic, Severity};

pub(super) fn diagnose(
    config_file: &crate::cli::ConfigFileArgs,
    packages: &crate::cli::TypstPackageArgs,
    scope: tola_input::InputScope,
    output: &crate::cli::output::CommandOutput,
    cancellation: &Cancellation,
) -> anyhow::Result<Vec<Diagnostic>> {
    let root = tola_build::config::loading::config_site_root(config_file.path.as_deref())?;
    let package_locations = match crate::cli::config::package_locations(packages, scope) {
        Ok(locations) => locations,
        Err(error) => {
            destination::start_after_error(output, &root, &cancellation.token());
            let mut diagnostics = vec![tola_diagnostic::fallback(
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
            let mut config = tola_build::modules::prepare_site(
                loaded.config(),
                tola_input::InputMode::new(scope, true),
                &cancellation.token(),
            )?;
            destination::start_site(output, &config, &cancellation.token())?;
            let mut diagnostics = tola_build::config::diagnostic::warning_diagnostics(&config);
            if let Err(error) = tola_build::build::install_hooks(
                &mut config,
                &tola_build::BuildResources::new()
                    .with_input_scope(scope)
                    .with_locked(true),
            ) {
                diagnostics.extend(crate::cli::output::attached_or_fallback(
                    &error,
                    crate::codes::doctor::CHECKS,
                ));
            }
            diagnostics.extend(diagnose_loaded(&config));
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

fn diagnose_loaded(config: &SiteConfig) -> Vec<Diagnostic> {
    let root = config.root();
    let mut diagnostics = Vec::new();
    diagnostics.extend(build_input_diagnostic(
        config,
        &config.build().entry,
        std::fs::Metadata::is_file,
        crate::codes::doctor::ENTRY_MISSING,
        "`build.entry` is not a file",
        "Create the entry file, or correct `build.entry`",
    ));
    diagnostics.extend(build_input_diagnostic(
        config,
        &config.build().content_dir,
        std::fs::Metadata::is_dir,
        crate::codes::doctor::CONTENT_ROOT_MISSING,
        "`build.content-dir` is not a directory",
        "Create the content directory, or correct `build.content-dir`",
    ));

    diagnostics.extend(editor_package_diagnostics(root));

    diagnostics.extend(hook_command_diagnostics(config));

    diagnostics.extend(environment_diagnostics(config.root()));

    diagnostics.extend(vendor_diagnostics(config));
    diagnostics.extend(font_diagnostics(config));
    diagnostics
}

fn build_input_diagnostic(
    config: &SiteConfig,
    path: &Path,
    is_expected: fn(&std::fs::Metadata) -> bool,
    code: tola_diagnostic::DiagnosticCode,
    message: &str,
    help: &str,
) -> Option<Diagnostic> {
    let root = config.root();
    let metadata = path.metadata();
    if metadata.as_ref().is_ok_and(is_expected) {
        return None;
    }
    if metadata.is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        && let Some(producer) = generated_input_producer(config.hooks(), root, path)
    {
        return Some(
            path_diagnostic(
                code,
                Severity::Warning,
                root,
                path,
                &format!("the missing input is declared by the before-build hook `{producer}`"),
            )
            .with_help("Run `tola check` to execute the before-build hook"),
        );
    }
    Some(path_diagnostic(code, Severity::Error, root, path, message).with_help(help))
}

fn hook_command_diagnostics(config: &SiteConfig) -> Vec<Diagnostic> {
    let root = config.root();
    let executable = std::env::current_exe();
    configured_hooks(config.hooks())
        .filter_map(|hook| {
            let program = hook.program()?;
            let expanded = executable
                .as_ref()
                .map_err(|_| "Tola could not locate its executable".to_owned())
                .and_then(|executable| {
                    expand_hook_argument(program, root, hook.origin, executable)
                        .map_err(|error| error.to_string())
                });
            let shown = tola_diagnostic::display_path(Path::new(program), root);
            let reason = match expanded {
                Ok(program) if command_exists(&program, root) => return None,
                Ok(_) => format!("command `{shown}` does not name an executable"),
                Err(reason) => reason,
            };
            Some(
                Diagnostic::new(
                    crate::codes::doctor::HOOK_COMMAND_MISSING,
                    Severity::Error,
                    format!(
                        "{} hook `{}` cannot run because {reason}",
                        hook.stage.as_str(),
                        hook.label
                    ),
                )
                .with_help("Make the command executable, install it on `PATH`, or correct `argv`"),
            )
        })
        .collect()
}

/// What `tola doctor` reports about the generated package directory: the paths a writing command
/// refuses, what differs from the sources this Tola ships, what is missing, and what another
/// version left behind.
fn editor_package_diagnostics(root: &Path) -> Vec<Diagnostic> {
    let directory = tola_input::layout::builtin_packages(root);
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
                .with_help("Run `tola editor packages` to create it"),
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
        diagnostics.push(diagnostic.with_help("Run `tola editor packages` to regenerate them"));
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
        diagnostic = diagnostic.with_note(crate::terminal::plural_count(
            extra,
            &format!("more {noun}"),
        ));
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

#[cfg(not(windows))]
fn command_exists(command: &OsStr, root: &Path) -> bool {
    if command.as_encoded_bytes().contains(&b'/') {
        return which::which(root.join(command)).is_ok();
    }
    which::which_in(command, std::env::var_os("PATH"), root).is_ok()
}

#[cfg(windows)]
fn command_exists(command: &OsStr, root: &Path) -> bool {
    let path = Path::new(command);
    let exe = |path: &Path| {
        let mut name = path.as_os_str().to_os_string();
        name.push(".exe");
        PathBuf::from(name)
    };
    let bytes = command.as_encoded_bytes();
    if bytes.is_empty() || bytes.ends_with(b"/") || bytes.ends_with(b"\\") {
        return false;
    }
    if bytes.contains(&b'/') || bytes.contains(&b'\\') {
        let path = root.join(path);
        return path.is_file()
            || (!bytes
                .get(bytes.len().saturating_sub(4)..)
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case(b".exe"))
                && exe(&path).is_file());
    }
    let program = if bytes.contains(&b'.') {
        path.to_owned()
    } else {
        exe(path)
    };
    let application = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_owned));
    let windows = std::env::var_os("SystemRoot").map(PathBuf::from);
    let path = std::env::var_os("PATH");
    let search = application
        .into_iter()
        .chain(windows.as_ref().map(|path| path.join("System32")))
        .chain(windows)
        .chain(path.iter().flat_map(std::env::split_paths));
    search
        .filter(|dir| !dir.as_os_str().is_empty())
        .any(|dir| dir.join(&program).is_file())
}

/// A vendored copy is portable only while it holds real files.
fn vendor_diagnostics(config: &tola_build::config::SiteConfig) -> Vec<Diagnostic> {
    let Some(vendor) = config.vendor().path.as_ref() else {
        return Vec::new();
    };
    let root = config.root();
    if !vendor.is_dir() {
        return vec![
            path_diagnostic(
                crate::codes::doctor::VENDOR_PATH_MISSING,
                Severity::Warning,
                root,
                vendor,
                "`vendor.path` is not a directory",
            )
            .with_help("Run `tola vendor` to write the site's external sources into it"),
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

/// Compiler font directories a build would find nothing in.
///
/// Only the paths the author declared are checked: an absent `typst.fonts.paths` falls back to
/// `static/typst-fonts`, which a site relying on the fonts Tola carries need not create. The
/// extension rule is the compiler's own, so a directory this reports empty provides no face to a
/// build either. A directory Tola cannot read is left to the compiler, which reports it there.
fn font_diagnostics(config: &tola_build::config::SiteConfig) -> Vec<Diagnostic> {
    if !config.declares_font_paths() {
        return Vec::new();
    }
    let root = config.root();
    let mut diagnostics = Vec::new();
    for (index, directory) in config.fonts().paths.iter().enumerate() {
        if !directory.is_dir() {
            diagnostics.push(
                path_diagnostic(
                    crate::codes::doctor::FONT_PATH_MISSING,
                    Severity::Error,
                    root,
                    directory,
                    &format!("`typst.fonts.paths[{index}]` is not a directory"),
                )
                .with_help("Create it, or correct `typst.fonts.paths`"),
            );
            continue;
        }
        if !holds_font_file(directory) {
            diagnostics.push(
                path_diagnostic(
                    crate::codes::doctor::FONT_DIRECTORY_EMPTY,
                    Severity::Warning,
                    root,
                    directory,
                    &format!("`typst.fonts.paths[{index}]` holds no font file"),
                )
                .with_help(
                    "Add a `.ttf`, `.otf`, `.ttc`, or `.otc` file, or correct `typst.fonts.paths`",
                ),
            );
        }
    }
    diagnostics
}

/// Whether any file below `directory` carries an extension the compiler scans fonts by.
fn holds_font_file(directory: &Path) -> bool {
    let mut pending = vec![directory.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            // A directory Tola cannot read is the compiler's to report, not this check's.
            return true;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = std::fs::metadata(entry.path()) else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if tola_typst::FontStore::accepts_path(&entry.path()) {
                return true;
            }
        }
    }
    false
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
    code: tola_diagnostic::DiagnosticCode,
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

    const MINIMAL_CONFIG: &str = "[build]\nentry = \"site.typ\"\ncontent-dir = \"content\"\n";

    #[test]
    fn pure_scope_reports_package_roots() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tola.toml");
        std::fs::write(&path, MINIMAL_CONFIG).unwrap();
        let reported = diagnose_config(
            &path,
            &crate::cli::TypstPackageArgs {
                package_path: Some(dir.path().join("host-packages")),
                ..crate::cli::TypstPackageArgs::default()
            },
            tola_input::InputScope::Pure,
        );

        assert!(
            reported
                .iter()
                .any(|d| d.code == crate::codes::doctor::PACKAGE_LOCATIONS)
        );
    }

    #[test]
    fn refused_package_paths_reported_once() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tola.toml");
        std::fs::write(&path, MINIMAL_CONFIG).unwrap();
        let (rel, _) = crate::editor::generated_package_files()
            .into_iter()
            .next()
            .expect("builtin packages provide editor files");
        std::fs::create_dir_all(dir.path().join(&rel)).unwrap();
        let displayed = crate::terminal::display_path_as_given(&rel);

        let reported = diagnose_at(&path);
        let refused = reported
            .iter()
            .find(|d| d.code == crate::codes::doctor::EDITOR_PACKAGES_UNUSABLE)
            .expect("a generated package path Tola refuses is reported");
        assert!(
            refused
                .notes
                .iter()
                .any(|n| n.contains(&displayed) && n.contains("is not a file")),
            "{:?}",
            refused.notes
        );
        let elsewhere: Vec<&str> = reported
            .iter()
            .filter(|d| d.code != crate::codes::doctor::EDITOR_PACKAGES_UNUSABLE)
            .flat_map(|d| d.notes.iter().map(String::as_str))
            .collect();
        assert!(
            !elsewhere.iter().any(|n| n.contains(&displayed)),
            "the refused path is reported once: {elsewhere:?}"
        );
    }

    fn diagnose_config(
        path: &Path,
        packages: &crate::cli::TypstPackageArgs,
        scope: tola_input::InputScope,
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
        diagnose_config(path, &Default::default(), tola_input::InputScope::Online)
    }

    #[test]
    fn missing_inputs_use_enabled_generators() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let path = root.join("tola.toml");
        std::fs::create_dir(root.join("theme")).unwrap();
        std::fs::write(
            root.join("theme/tola-module.toml"),
            r#"
[[hooks.before-build]]
name = "pages"
argv = ["{tola}"]
enabled-by = "generate-pages"
generates = ["./generated/pages"]
"#,
        )
        .unwrap();
        let cfg = r#"
[build]
entry = "generated/pages/site.typ"
content-dir = "generated/content"
[modules.theme]
path = "theme"
[[hooks.before-build]]
name = "content"
argv = ["{tola}"]
generates = ["generated/content"]
"#;
        for (enabled, severity) in [(true, Severity::Warning), (false, Severity::Error)] {
            std::fs::write(
                &path,
                format!("{cfg}\n[modules.theme.options]\ngenerate-pages = {enabled}\n"),
            )
            .unwrap();
            let reported = diagnose_at(&path);
            let entry = reported
                .iter()
                .find(|d| d.code == crate::codes::doctor::ENTRY_MISSING)
                .unwrap();
            assert_eq!(entry.severity, severity);
            assert_eq!(entry.message.contains("@module/theme/pages"), enabled);
            let content = reported
                .iter()
                .find(|d| d.code == crate::codes::doctor::CONTENT_ROOT_MISSING)
                .unwrap();
            assert_eq!(content.severity, Severity::Warning);
            assert!(content.message.contains("`content`"));
            assert!(!root.join("generated").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn hook_executables_expand_placeholders() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let root = dir.path();
        let path = root.join("tola.toml");
        std::fs::create_dir(root.join("theme")).unwrap();
        std::fs::write(
            &path,
            r#"
[modules.theme]
path = "theme"
[[hooks.before-build]]
name = "site-tool"
argv = ["./site-tool"]
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("theme/tola-module.toml"),
            r#"
[[hooks.before-build]]
name = "module-tool"
argv = ["{module}/tool"]
"#,
        )
        .unwrap();
        let script = root.join("site-tool");
        let mod_script = root.join("theme/tool");
        for file in [&script, &mod_script] {
            std::fs::write(file, "#!/bin/sh\nexit 1\n").unwrap();
        }
        for (site_mode, mod_mode, expected) in [
            (0o644, 0o755, "site-tool"),
            (0o755, 0o644, "@module/theme/module-tool"),
        ] {
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(site_mode)).unwrap();
            std::fs::set_permissions(&mod_script, std::fs::Permissions::from_mode(mod_mode))
                .unwrap();
            let reported = diagnose_at(&path);
            let errs: Vec<_> = reported
                .iter()
                .filter(|d| d.code == crate::codes::doctor::HOOK_COMMAND_MISSING)
                .collect();
            assert_eq!(errs.len(), 1, "{errs:?}");
            assert!(errs[0].message.contains(&format!("`{expected}`")));
        }
    }

    #[test]
    fn missing_vendor_reported() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tola.toml");
        std::fs::write(
            &path,
            "[build]\nentry = \"site.typ\"\ncontent-dir = \"content\"\n[vendor]\npath = \"vendor\"\n",
        )
        .unwrap();

        let reported = diagnose_at(&path);
        let missing = reported
            .iter()
            .find(|d| d.code == crate::codes::doctor::VENDOR_PATH_MISSING)
            .expect("a declared vendor directory that does not exist is reported");
        assert_eq!(
            missing.location.as_ref().map(|l| l.path.as_str()),
            Some("vendor")
        );
    }

    #[cfg(unix)]
    #[test]
    fn vendored_links_reported() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tola.toml");
        std::fs::write(
            &path,
            "[build]\nentry = \"site.typ\"\ncontent-dir = \"content\"\n[vendor]\npath = \"vendor\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("vendor/typst-packages")).unwrap();
        let link = dir.path().join("vendor/typst-packages/theme");
        std::os::unix::fs::symlink(dir.path().join("theme-source"), &link).unwrap();

        let reported = diagnose_at(&path);
        let linked = reported
            .iter()
            .find(|d| d.code == crate::codes::doctor::VENDOR_LINK)
            .expect("a link inside the vendor directory is reported");
        assert_eq!(
            linked.location.as_ref().map(|l| l.path.as_str()),
            Some("vendor/typst-packages/theme")
        );
    }
}
