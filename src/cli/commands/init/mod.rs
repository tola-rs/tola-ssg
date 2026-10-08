//! Site initialization.

mod config;
mod custom;
mod features;
mod files;
mod program;
mod prompt;
mod selection;
mod tree;
mod validate;

use crate::cancellation::Cancellation;
use crate::cli::args::InitArgs;
use crate::cli::log::{LogFile, destination};
use crate::cli::output::CommandOutput;
use crate::editor::Editor;
use crate::terminal::{Palette, code, display_path_toward_home};
use crate::writes::FileWrites;
use anyhow::Result;
use std::path::Path;
use tola_build::config::SiteConfigSchema;
use tola_build::diagnostic::{Diagnostic, DiagnosticError, Severity};

use features::Effects;
use selection::{SelectionIssue, selection_issues};

pub(in crate::cli) use features::{feature_values, preset_values};

/// Validate the scaffold and target paths before writing the site.
pub(in crate::cli) fn run(
    args: &InitArgs,
    scope: tola_build::InputScope,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.token().ensure_active()?;
    if args.interactive {
        prompt::ensure_terminal(output)?;
    }
    let cwd = std::env::current_dir()?;
    let root = tola_build::filesystem::normalize_path(&prompt::site_directory(
        args,
        &cwd,
        output,
        cancellation,
    )?);
    let packages = crate::cli::config::package_locations(&Default::default(), scope)?;
    if !args.dry_run
        && let Err(error) = validate::preflight(&root, args.force, &packages)
    {
        // The session log still records a failure that now precedes the questions.
        destination::start_after_error(output, &root, &cancellation.token());
        return Err(error);
    }
    let (choice, editors) = prompt::resolve_choices(args, &root, output, cancellation)?;
    let selected = choice.features();
    let issues = selection_issues(&selected);
    if !issues.is_empty() {
        return Err(selection_error(&issues));
    }
    if args.interactive
        && !args.dry_run
        && !prompt::confirm_scaffold(&root, &selected, &editors, output, cancellation)?
    {
        output.block("Nothing written.")?;
        return Ok(());
    }
    let effects = features::Effects::combine(&selected);
    let writes = stage_writes(output, &root, &editors, &packages, &effects, cancellation)?;
    if args.dry_run {
        show_dry_run(&root, &writes, output)?;
        let config = validate_configuration(&writes, scope);
        let log_check = destination::check_writes(output.log().map(LogFile::path), &writes, true);
        if log_check.is_ok() {
            cancellation.token().ensure_active()?;
            match &config {
                Ok(config) => start_log(output, &root, config, cancellation)?,
                Err(_) => destination::start(output, &cancellation.token())?,
            }
        }
        return validate::checks([
            validate::scaffold(&writes, args.force),
            config.map(|_| ()),
            log_check,
        ]);
    }

    validate::checks([
        validate::scaffold(&writes, args.force),
        destination::check_writes(output.log().map(LogFile::path), &writes, false),
    ])?;

    let config = validate_configuration(&writes, scope)?;
    let package_inputs =
        crate::editor::initial_package_inputs(&root, &packages, &cancellation.token())?;
    cancellation.token().ensure_active()?;
    start_log(output, &root, &config, cancellation)?;
    cancellation.token().ensure_active()?;
    writes.apply(&cancellation.token())?;
    package_inputs.apply(&cancellation.token())?;

    show_created_site(&writes, &editors, &packages, &effects, output)
}

fn selection_error(issues: &[SelectionIssue]) -> anyhow::Error {
    let diagnostics = issues
        .iter()
        .map(|issue| {
            let alternatives = features::providers(issue.slot)
                .iter()
                .map(|feature| format!("`{}`", features::feature_name(*feature)))
                .collect::<Vec<_>>();
            let requirement = match alternatives.as_slice() {
                [only] => only.clone(),
                many => format!("one of {}", many.join(", ")),
            };
            Diagnostic::new(
                crate::codes::init::SELECTION,
                Severity::Error,
                format!(
                    "`{}` requires {requirement}",
                    features::feature_name(issue.feature)
                ),
            )
            .with_help(format!("add {requirement} to `--features`"))
        })
        .collect();
    DiagnosticError::new(
        "cannot create the site with the selected features",
        diagnostics,
    )
    .into()
}

/// Stage the scaffold once, starting the external log when its path stays clear of it.
fn stage_writes(
    output: &CommandOutput,
    root: &Path,
    editors: &[Editor],
    packages: &tola_typst::PackageLocations,
    effects: &Effects,
    cancellation: &Cancellation,
) -> Result<FileWrites> {
    let writes = files::file_writes(root, editors, packages, effects)?;
    let Some(log) = output.log().map(LogFile::path) else {
        return Ok(writes);
    };
    let physical_root = tola_build::filesystem::normalize_existing_prefix(root);
    if log.starts_with(&physical_root) {
        return Ok(writes);
    }
    let schema = config::schema(effects);
    // Start logging only when the log path stays clear of the scaffold and of the site
    // configuration's inputs and outputs.
    if destination::check_writes(Some(log), &writes, false).is_ok()
        && destination::check_site(
            Some(log),
            root,
            &root.join(config::CONFIG_PATH),
            destination::SiteInputSections::from_schema(&schema),
        )
        .is_ok()
    {
        destination::start(output, &cancellation.token())?;
    }
    Ok(writes)
}

/// The dry run's report: the file tree it would create, then the configuration inside it.
fn show_dry_run(root: &Path, writes: &FileWrites, output: &CommandOutput) -> Result<()> {
    output.block(format!(
        "Site files for {} (dry run)\n\n{}",
        display_path_toward_home(root),
        tree::render(writes)
    ))?;
    output.write_stdout_styled(&code::styled(config::config_source(writes), "toml"))?;
    Ok(())
}

fn show_created_site(
    writes: &FileWrites,
    editors: &[Editor],
    packages: &tola_typst::PackageLocations,
    effects: &Effects,
    output: &CommandOutput,
) -> Result<()> {
    let root = writes.root();
    let palette = output.terminal().palette();
    output.blank_line()?;
    output.summary(format!("Initialized {}", display_path_toward_home(root)))?;
    output.status(tree::render(writes))?;
    let steps = next_steps(root, effects);
    output.styled_block(&steps, &styled_report(&steps, palette))?;
    let configured = editors
        .iter()
        .copied()
        .filter(|editor| crate::editor::writes_settings(*editor))
        .map(Editor::name)
        .collect::<Vec<_>>();
    if !configured.is_empty() {
        output.status(format!(
            "Wrote editor settings for {}",
            configured.join(", ")
        ))?;
    }
    for editor in crate::editor::manual_editors(editors) {
        let settings = crate::editor::settings(editor, root, packages)?;
        output.block(crate::editor::manual_settings_instructions(
            editor, &settings,
        ))?;
    }
    output.status(crate::editor::startup_instructions(editors))?;
    if editors.is_empty() {
        let setup = "Editor setup:\n  `tola editor setup` chooses interactively\n  `tola editor setup --list` shows supported editors";
        output.styled_block(setup, &styled_report(setup, palette))?;
    }
    let help = "Help:\n  Site settings: `tola help config site`\n  Package reference: `tola help package web`\n  Function reference: `tola help package address slugify output-to-url`\n  Runnable examples: `tola help -i demo`\n  Authoring guide: `tola skill`";
    output.styled_block(help, &styled_report(help, palette))?;
    Ok(())
}

/// `block` with its header line (the first line, ending in `:`) drawn under `palette`.
fn styled_report(block: &str, palette: Palette) -> String {
    let Some((header, rest)) = block.split_once('\n') else {
        return palette.heading(block);
    };
    format!("{}\n{rest}", palette.heading(header))
}

/// The commands that take the author from an empty scaffold to a built site.
fn next_steps(root: &Path, effects: &Effects) -> String {
    let mut steps = vec![format!("Switch to `{}`.", display_path_toward_home(root))];
    steps.extend(effects.next_steps.iter().map(|step| (*step).to_owned()));
    steps.push("Create `content/index.typ` with your first page.".to_owned());
    steps.push("Run: `tola dev`".to_owned());
    format!("Next steps:\n  {}", steps.join("\n  "))
}

fn validate_configuration(
    writes: &FileWrites,
    scope: tola_build::InputScope,
) -> Result<tola_build::config::SiteConfigSchema> {
    crate::config::validate_settings(
        &writes.root().join(config::CONFIG_PATH),
        config::config_source(writes),
        scope,
    )
}

fn start_log(
    output: &CommandOutput,
    root: &Path,
    schema: &SiteConfigSchema,
    cancellation: &Cancellation,
) -> Result<()> {
    destination::check_site(
        output.log().map(LogFile::path),
        root,
        &root.join(config::CONFIG_PATH),
        destination::SiteInputSections::from_schema(schema),
    )?;
    let diagnostics = crate::config::DiagnosticsConfig::default();
    output.apply_diagnostic_limits(&diagnostics);
    destination::start(output, &cancellation.token())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_leaves_existing_files_alone() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(root.join("tola.toml"), "existing configuration").unwrap();

        let writes = files::file_writes(
            root,
            &[Editor::Vscode],
            &Default::default(),
            &Effects::default(),
        )
        .unwrap();

        assert!(tree::render(&writes).contains(".vscode"));
        assert!(writes.check().is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("tola.toml")).unwrap(),
            "existing configuration",
        );
        assert!(!root.join(".tola").exists());
    }

    #[test]
    fn rich_scaffold_includes_tailwind_and_deno() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();

        let rich = features::Effects::combine(&features::features("rich"));
        let writes = files::file_writes(root, &[], &Default::default(), &rich).unwrap();
        assert!(writes.file_contents(Path::new("deno.json")).is_some());
        assert!(
            writes
                .file_contents(Path::new("static/tailwind-sources/site.css"))
                .is_some()
        );
        let report = next_steps(root, &rich);
        assert!(report.contains("`just setup`"), "{report}");
        assert!(report.contains("`just css`"), "{report}");

        let medium = features::Effects::combine(&features::features("medium"));
        let writes = files::file_writes(root, &[], &Default::default(), &medium).unwrap();
        assert!(writes.file_contents(Path::new("deno.json")).is_none());
        assert!(
            writes
                .file_contents(Path::new("static/tailwind-sources/site.css"))
                .is_none()
        );
        let report = next_steps(root, &medium);
        assert!(!report.contains("`just setup`"), "{report}");
        assert!(!report.contains("`just css`"), "{report}");
    }

    #[test]
    fn interactive_init_needs_terminal() {
        let (sink, _output) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );
        let args = InitArgs {
            path: None,
            dry_run: false,
            force: false,
            preset: None,
            features: Vec::new(),
            interactive: true,
            editor: Vec::new(),
        };

        let error = run(
            &args,
            tola_build::InputScope::Online,
            &output,
            &Cancellation::default(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("--preset"), "{error}");
    }

    #[test]
    fn feature_selection_reports_its_issue() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("site");
        let args = InitArgs {
            path: Some(root.clone()),
            dry_run: true,
            force: false,
            preset: None,
            features: vec!["tailwind-css".to_owned()],
            interactive: false,
            editor: Vec::new(),
        };
        let (sink, _output) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, false, false),
            None,
        );

        let error = run(
            &args,
            tola_build::InputScope::Online,
            &output,
            &Cancellation::default(),
        )
        .unwrap_err();

        let diagnostics = tola_build::diagnostic::attached(&error).unwrap();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].code, crate::codes::init::SELECTION);
        assert_eq!(
            diagnostics[0].message,
            "`tailwind-css` requires `deno-toolchain`"
        );
        assert_eq!(
            diagnostics[0].help[0].message,
            "add `deno-toolchain` to `--features`"
        );
        assert!(!root.exists());
    }

    /// The bytes one dry run writes into a buffered terminal under `color`.
    fn dry_run_output(writes: &FileWrites, color: bool) -> String {
        let (sink, stream) = crate::terminal::OutputSink::buffered();
        let output = CommandOutput::new(
            crate::terminal::Terminal::with_sink(sink, color, false),
            None,
        );
        show_dry_run(writes.root(), writes, &output).unwrap();
        String::from_utf8(stream.bytes()).unwrap()
    }

    #[test]
    fn dry_run_configuration_follows_the_stdout_color_policy() {
        let directory = tempfile::tempdir().unwrap();
        let writes = files::file_writes(
            directory.path(),
            &[],
            &Default::default(),
            &Effects::default(),
        )
        .unwrap();
        let source = config::config_source(&writes);

        let plain = dry_run_output(&writes, false);
        assert!(!plain.contains('\u{1b}'), "{plain:?}");
        assert!(plain.ends_with(source), "{plain:?}");

        let colored = dry_run_output(&writes, true);
        let tree = &plain[..plain.len() - source.len()];
        assert_eq!(
            colored.strip_prefix(tree).expect("the tree reads the same"),
            code::styled(source, "toml").paint_all(true),
        );
    }
}
