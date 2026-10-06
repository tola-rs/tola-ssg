//! Prevent log paths from overlapping command inputs and outputs.

use std::path::Path;

use anyhow::{Context, Result, ensure};
use tola_build::cancellation::BuildCancellation;
use tola_build::config::{
    AssetsConfig, BuildSectionConfig, FontsConfig, IconsConfig, ResolvedSiteConfig,
    SiteConfigSchema, section::VendorConfig,
};
use tola_build::filesystem::normalize_existing_prefix;

use crate::cli::output::CommandOutput;
use crate::terminal::{display_path, display_path_within};
use crate::writes::FileWrites;

pub(crate) fn start(output: &CommandOutput, cancellation: &BuildCancellation) -> Result<()> {
    cancellation.ensure_active()?;
    let Some(log) = output.log() else {
        return Ok(());
    };
    match log.start() {
        Ok(true) => output.secondary(format!("Log: {}", display_path(log.path())))?,
        Ok(false) => {}
        // Tola names the session log itself, so one it cannot open must not stop the command;
        // a file the reader named with `--log-file` still fails the command.
        Err(error) if log.origin() == super::LogOrigin::Automatic => {
            let _ = output.diagnostic(&unavailable_diagnostic(log.declared_path()));
            trace_unavailable_cause(log.path(), &error);
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

/// Trace why an automatic session log is unavailable.
///
/// The warning the site author reads stands on its own, so the cause chain reaches only explicitly
/// requested verbose output.
pub(crate) fn trace_unavailable_cause(path: &Path, error: &anyhow::Error) {
    tracing::debug!(
        target: "tola::log",
        path = %display_path(path),
        error = %format!("{error:#}"),
        "session log could not be opened"
    );
}

/// The warning for a session log this command cannot use.
///
/// The reader never passed `--log-file` for the automatic session log Tola names, so the message
/// only reports the path; the help names the argument that can replace it.
pub(crate) fn unavailable_diagnostic(path: &Path) -> tola_build::diagnostic::Diagnostic {
    tola_build::diagnostic::Diagnostic::new(
        crate::codes::log::UNAVAILABLE,
        tola_build::diagnostic::Severity::Warning,
        format!("cannot open log `{}`", display_path(path)),
    )
    .with_note("logging is off for this session")
    .with_help("Make `.tola/logs` writable, or pass `--log-file` to record elsewhere")
}

/// The warning for an automatic session log that would sit inside the site's own inputs or
/// outputs.
fn conflicted_diagnostic(path: &Path) -> tola_build::diagnostic::Diagnostic {
    tola_build::diagnostic::Diagnostic::new(
        crate::codes::log::UNAVAILABLE,
        tola_build::diagnostic::Severity::Warning,
        format!(
            "cannot log to `{}`; it overlaps the site's inputs or outputs",
            display_path(path)
        ),
    )
    .with_note("logging is off for this session")
    .with_help("Move `.tola/logs` aside, or pass `--log-file` to record outside the site")
}

pub(crate) fn start_site(
    output: &CommandOutput,
    config: &ResolvedSiteConfig,
    cancellation: &BuildCancellation,
) -> Result<()> {
    let log = output.log();
    // A log that stopped or never opened stays out of the way: it is never reopened, revalidated,
    // or reported again.
    if log.is_some_and(super::LogFile::is_stopped) {
        return Ok(());
    }
    if let Err(error) = check_site(
        log.map(super::LogFile::path),
        config.get_root(),
        config.config_path(),
        SiteInputSections::from_config(config),
    ) {
        // Tola names the development session log itself, so a destination that cannot sit clear
        // of the site's inputs and outputs is refused rather than fatal: the log stops, or never
        // opens, and the site still runs. A `--log-file` the reader named stays an error.
        let Some(log) = log else {
            return Err(error);
        };
        if log.origin() != super::LogOrigin::Automatic {
            return Err(error);
        }
        disable_automatic_log(output, log, &conflicted_diagnostic(log.declared_path()));
        return Ok(());
    }
    start(output, cancellation)
}

/// Stop an automatic session log for good and tell the reader once why it is not recorded.
fn disable_automatic_log(
    output: &CommandOutput,
    log: &super::LogFile,
    diagnostic: &tola_build::diagnostic::Diagnostic,
) {
    if log.disable() {
        let _ = output.diagnostic(diagnostic);
    }
}

pub(crate) fn start_after_error(
    output: &CommandOutput,
    root: &Path,
    cancellation: &BuildCancellation,
) {
    let Some(log) = output.log() else {
        return;
    };
    if !log.is_pending() || cancellation.is_cancelled() {
        return;
    }
    let refused = check_unresolved_site(log.path(), root).is_err();
    if refused && log.origin() == super::LogOrigin::Automatic {
        // Tola's own log cannot sit clear of a site whose configuration is unresolved: keep it
        // closed so nothing is clobbered, and let the command's own failure reach the reader.
        disable_automatic_log(output, log, &conflicted_diagnostic(log.declared_path()));
        return;
    }
    // An automatic session log that cannot open reports itself, so this warning is left for a
    // refused destination and for a `--log-file` the reader named.
    if !refused && start(output, cancellation).is_ok() {
        return;
    }
    if cancellation.is_cancelled() {
        return;
    }
    // Tola's own log is reported by the path it declared; a `--log-file` stays as it does today.
    let reported = match log.origin() {
        super::LogOrigin::Automatic => log.declared_path(),
        super::LogOrigin::Explicit => log.path(),
    };
    let _ = output.diagnostic(
        &tola_build::diagnostic::Diagnostic::new(
            crate::codes::log::UNAVAILABLE,
            tola_build::diagnostic::Severity::Warning,
            format!("cannot open log `{}`", display_path_within(reported, root)),
        )
        .with_help("Use `.tola/logs/`, or a writable path outside the site"),
    );
}

fn check_unresolved_site(log: &Path, root: &Path) -> Result<()> {
    let root = normalize_existing_prefix(root);
    let directory = root.join(".tola/logs");
    let dedicated =
        normalize_existing_prefix(&directory) == directory && log.starts_with(&directory);
    ensure!(
        !log.starts_with(&root) || dedicated,
        "configuration is unavailable; log to `.tola/logs/` or a path outside the site"
    );
    Ok(())
}

/// Render `path` relative to `root`, otherwise to the current directory, as
/// [`crate::terminal::display_path_within`] does.
fn render_path(path: &Path, root: Option<&Path>) -> String {
    root.map_or_else(
        || display_path(path),
        |root| display_path_within(path, root),
    )
}

fn check_file_within(log: Option<&Path>, path: &Path, root: Option<&Path>) -> Result<()> {
    let Some(log) = log else {
        return Ok(());
    };
    let path = normalize_existing_prefix(path);
    let shown_log = render_path(log, root);
    let shown_path = render_path(&path, root);
    ensure!(
        !path.starts_with(log) && !log.starts_with(&path),
        "log `{shown_log}` conflicts with `{shown_path}`; choose a log path outside the site's inputs and outputs"
    );
    if path.is_file() && log.is_file() {
        let same = same_file::is_same_file(&path, log)
            .with_context(|| format!("cannot compare log `{shown_log}` with `{shown_path}`"))?;
        ensure!(
            !same,
            "log `{shown_log}` points to the input or output file `{shown_path}`; choose a separate file"
        );
    }
    Ok(())
}

fn check_tree_within(log: &Path, path: &Path, root: Option<&Path>) -> Result<()> {
    let path = normalize_existing_prefix(path);
    ensure!(
        !path.starts_with(log) && !log.starts_with(&path),
        "log `{}` conflicts with the site directory `{}`; choose a log path outside the site",
        render_path(log, root),
        render_path(&path, root)
    );
    Ok(())
}

/// Check one path against the log destination, rendering both relative to the current directory.
pub(crate) fn check_file(log: Option<&Path>, path: &Path) -> Result<()> {
    check_file_within(log, path, None)
}

/// The configuration sections whose declared paths a log destination must stay clear of.
pub(crate) struct SiteInputSections<'a> {
    build: &'a BuildSectionConfig,
    assets: &'a AssetsConfig,
    fonts: &'a FontsConfig,
    icons: &'a IconsConfig,
    vendor: &'a VendorConfig,
}

impl<'a> SiteInputSections<'a> {
    /// The sections of a configuration that has been resolved into a site.
    pub(crate) fn from_config(config: &'a ResolvedSiteConfig) -> Self {
        Self {
            build: config.build(),
            assets: config.assets(),
            fonts: config.fonts(),
            icons: config.icons(),
            vendor: &config.vendor,
        }
    }

    /// The sections of a configuration the command has not resolved yet.
    pub(crate) fn from_schema(schema: &'a SiteConfigSchema) -> Self {
        Self {
            build: &schema.build,
            assets: &schema.assets,
            fonts: &schema.typst.fonts,
            icons: &schema.icons,
            vendor: &schema.vendor,
        }
    }
}

/// Check site inputs and outputs, resolving relative paths against `root`.
pub(crate) fn check_site(
    log: Option<&Path>,
    root: &Path,
    config_path: &Path,
    inputs: SiteInputSections<'_>,
) -> Result<()> {
    let Some(log) = log else {
        return Ok(());
    };
    let log = normalize_existing_prefix(log);
    let log = log.as_path();
    let SiteInputSections {
        build,
        assets,
        fonts,
        icons,
        vendor,
    } = inputs;
    check_file_within(Some(log), config_path, Some(root))?;
    check_file_within(
        Some(log),
        &root.join(tola_build::filesystem::SITE_BUILD_LOCK_FILE),
        Some(root),
    )?;
    if let Some(workspace) =
        tola_build::filesystem::publication_workspace(&root.join(&build.publish_dir))
    {
        check_tree_within(log, &workspace, Some(root))?;
    }
    if let Some(workspace) = vendor.workspace_path() {
        check_tree_within(log, &root.join(workspace), Some(root))?;
    }
    for file in std::iter::once(build.entry.as_path()).chain(assets.file_sources()) {
        check_file_within(Some(log), &root.join(file), Some(root))?;
    }
    for collection in icons.collections.values() {
        use tola_build::config::section::IconCollectionSource;
        match collection {
            IconCollectionSource::LocalJson { path } => {
                check_file_within(Some(log), &root.join(path), Some(root))?
            }
            IconCollectionSource::LocalSvgDir { path } => {
                check_tree_within(log, &root.join(path), Some(root))?
            }
            _ => {}
        }
    }
    for tree in [&build.content_dir, &build.publish_dir]
        .into_iter()
        .map(std::path::PathBuf::as_path)
        .chain(fonts.paths.iter().map(std::path::PathBuf::as_path))
        .chain(assets.tree_sources())
    {
        check_tree_within(log, &root.join(tree), Some(root))?;
    }
    for hook in &build.hooks.before_build {
        for generated in &hook.generates {
            check_tree_within(log, &root.join(generated), Some(root))?;
        }
    }
    if let Some(path) = &vendor.path {
        check_tree_within(log, &root.join(path), Some(root))?;
    }
    Ok(())
}

pub(crate) fn check_writes(log: Option<&Path>, writes: &FileWrites, dry_run: bool) -> Result<()> {
    let Some(log) = log else {
        return Ok(());
    };
    let log = normalize_existing_prefix(log);
    let root = normalize_existing_prefix(writes.root());
    // The log path is the user's own argument, so it is shown relative to the current directory;
    // the scaffold paths below stay relative to the root the command writes into.
    let shown_log = display_path(&log);
    ensure!(
        !dry_run || !log.starts_with(&root),
        "dry-run log `{shown_log}` is inside the site directory; choose a log path outside it to keep the site unchanged"
    );
    for file in writes.file_paths() {
        check_file_within(Some(&log), file, Some(&root))?;
    }
    for directory in std::iter::once(writes.root()).chain(writes.directory_paths()) {
        ensure!(
            !normalize_existing_prefix(directory).starts_with(&log),
            "log file `{shown_log}` conflicts with the site directory `{}`; choose a log path outside it",
            render_path(directory, Some(&root))
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn logs_refuse_recovery_paths() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let mut schema = SiteConfigSchema::default();
        schema.vendor.path = Some("vendor".into());
        for relative in [
            ".tola-build.lock",
            ".public-publish/candidate/index.html",
            ".vendor-vendor/previous/icons/ui.json",
        ] {
            assert!(
                check_site(
                    Some(&root.join(relative)),
                    root,
                    &root.join("tola.toml"),
                    SiteInputSections::from_schema(&schema),
                )
                .is_err()
            );
        }
    }

    /// A log destination that resolves into the site's sources stays unopened: nothing is written
    /// into `content`, and the refusal is reported once however many rounds revalidate it.
    #[test]
    #[cfg(unix)]
    fn source_overlapping_logs_stay_out_of_content() {
        use std::fs;

        for is_automatic in [false, true] {
            let directory = TempDir::new().unwrap();
            let root = directory.path();
            let source = root.join("content");
            fs::create_dir(&source).unwrap();
            fs::create_dir(root.join(".tola")).unwrap();
            std::os::unix::fs::symlink(&source, root.join(".tola/logs")).unwrap();
            let name = if is_automatic {
                "dev-1.jsonl"
            } else {
                "session.jsonl"
            };
            let path = root.join(".tola/logs").join(name);
            let (sink, captured) = crate::terminal::OutputSink::buffered();
            let terminal = crate::terminal::Terminal::with_sink(sink, false, false);
            let log = if is_automatic {
                super::super::LogFile::prepare_session(&path, |_, error| {
                    panic!("log write failed: {error}")
                })
                .unwrap()
            } else {
                super::super::LogFile::prepare(&path, |_, error| {
                    panic!("log write failed: {error}")
                })
                .unwrap()
            };
            let output = CommandOutput::new(terminal, Some(log));

            start_after_error(&output, root, &BuildCancellation::default());
            if is_automatic {
                // A later round revalidates but must not warn or open again.
                start_after_error(&output, root, &BuildCancellation::default());
            }

            assert!(!source.join(name).exists());
            let shown = String::from_utf8(captured.bytes()).unwrap();
            assert_eq!(
                shown
                    .matches(crate::codes::log::UNAVAILABLE.as_str())
                    .count(),
                1,
                "{shown}"
            );
            if is_automatic {
                assert!(start(&output, &BuildCancellation::default()).is_ok());
                assert!(!source.join(name).exists());
            }
        }
    }

    /// Tola names the development session log, so a log it cannot open warns and the session
    /// continues without it.
    #[test]
    fn automatic_log_failure_warns_and_continues() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let log = super::super::LogFile::prepare_session(
            &root.join(".tola/logs/dev-1.jsonl"),
            |_, error| panic!("log write failed: {error}"),
        )
        .unwrap();
        // `.tola` becomes a file before the session opens its log.
        std::fs::write(root.join(".tola"), "").unwrap();
        let (sink, captured) = crate::terminal::OutputSink::buffered();
        let terminal = crate::terminal::Terminal::with_sink(sink, false, false);
        let output = CommandOutput::new(terminal, Some(log));

        start(&output, &BuildCancellation::default()).expect("the session continues");

        let shown = String::from_utf8(captured.bytes()).unwrap();
        assert!(
            shown.contains(crate::codes::log::UNAVAILABLE.as_str()),
            "{shown}"
        );
        assert!(shown.contains(".tola/logs/dev-1.jsonl"), "{shown}");
        assert!(!shown.contains("Log: "), "{shown}");
    }

    /// A file the reader named with `--log-file` must still fail the command.
    #[test]
    fn explicit_log_failure_stops_the_command() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let log =
            super::super::LogFile::prepare(&root.join(".tola/logs/session.jsonl"), |_, error| {
                panic!("log write failed: {error}")
            })
            .unwrap();
        std::fs::write(root.join(".tola"), "").unwrap();
        let (sink, _captured) = crate::terminal::OutputSink::buffered();
        let terminal = crate::terminal::Terminal::with_sink(sink, false, false);
        let output = CommandOutput::new(terminal, Some(log));

        assert!(start(&output, &BuildCancellation::default()).is_err());
    }

    #[test]
    fn logs_stay_out_of_the_scaffold() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("site");
        let mut writes = FileWrites::new(&root).unwrap();
        writes.create_file("tola.toml", "").unwrap();
        writes
            .create_file(".tola/builtin-packages/tola/site/0.0.0/lib.typ", "")
            .unwrap();
        writes.add_directory("content").unwrap();
        for relative in [
            "tola.toml",
            "tola.toml/log",
            ".tola/builtin-packages",
            "content",
        ] {
            assert!(check_writes(Some(&root.join(relative)), &writes, false).is_err());
        }
        let log = root.join(".tola/init.jsonl");
        assert!(check_writes(Some(&log), &writes, false).is_ok());
        assert!(check_writes(Some(&log), &writes, true).is_err());
        assert!(check_writes(Some(&temp.path().join("init.jsonl")), &writes, true).is_ok());
        assert!(!root.exists());
    }

    #[test]
    #[cfg(unix)]
    fn dry_run_follows_directory_aliases() {
        use std::fs;

        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("site");
        let alias = temporary.path().join("linked-site");
        fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let writes = FileWrites::new(&root).unwrap();

        assert!(check_writes(Some(&alias.join("init.jsonl")), &writes, true).is_err());
        assert!(fs::read_dir(root).unwrap().next().is_none());
    }
}
