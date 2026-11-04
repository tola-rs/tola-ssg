//! One built-in starter or an explicitly declared module initializer.

mod config;
mod files;
mod program;
mod source;
mod tree;
mod validate;

use serde::Serialize;

use crate::cancellation::Cancellation;
use crate::cli::args::InitArgs;
use crate::cli::log::{LogFile, destination};
use crate::cli::output::CommandOutput;
use crate::terminal::{code, display_path_toward_home};
use crate::writes::FileWrites;
use anyhow::{Result, ensure};
use std::path::Path;
use tola_build::config::SiteConfigSchema;

pub(in crate::cli) fn run(
    args: &InitArgs,
    scope: tola_build::InputScope,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    cancellation.token().ensure_active()?;
    ensure!(
        !args.interactive || crate::cli::interaction::available(),
        "`--interactive` needs a terminal; omit it for non-interactive initialization"
    );
    let root = tola_build::filesystem::normalize_existing_prefix(
        &std::env::current_dir()?.join(args.path.as_deref().unwrap_or(Path::new("."))),
    );
    if args.from.is_some() {
        // Every initializer must provide a configuration, so this conflict is
        // known before running any source-owned questions or commands.
        let mut required = FileWrites::new(&root)?;
        required.create_file(config::CONFIG_PATH, Vec::new())?;
        validate::scaffold(&required, args.force)?;
    } else {
        validate::preflight(&root, args.force)?;
    }
    let writes = if let Some(module) = &args.from {
        source::generate(module, args, &root, scope, output, cancellation)?
    } else {
        files::file_writes(&root)?
    };
    let schema = validate_configuration(&writes, scope)?;
    validate::checks([
        validate::scaffold(&writes, args.force),
        destination::check_writes(output.log().map(LogFile::path), &writes, args.dry_run),
    ])?;
    if args.dry_run {
        show_dry_run(&root, &writes, output)?;
        return Ok(());
    }
    if args.interactive && args.from.is_none() && !confirm(&root, &writes, scope, cancellation)? {
        output.block("Nothing written")?;
        return Ok(());
    }
    start_log(output, &root, &schema, cancellation)?;
    writes.apply(&cancellation.token())?;
    output.summary(format!("Initialized {}", display_path_toward_home(&root)))?;
    output.status(tree::render(&writes))?;
    let edit = if args.from.is_some() {
        "Edit the site sources."
    } else {
        "Edit `content/index.typ`."
    };
    output.block(format!(
        "Next steps:\n  Switch to `{}`.\n  {edit}\n  Run `tola dev`.\n\n         To connect your editor, run `tola editor setup`.",
        display_path_toward_home(&root),
    ))?;
    Ok(())
}

#[derive(Serialize)]
struct Confirm<'a> {
    target: &'a Path,
    files: String,
}

fn confirm(
    root: &Path,
    writes: &FileWrites,
    scope: tola_build::InputScope,
    cancellation: &Cancellation,
) -> Result<bool> {
    // The interaction script owns presentation; Rust owns the exact write plan.
    let input = Confirm {
        target: root,
        files: tree::render(writes),
    };
    let result: Option<bool> =
        crate::cli::interaction::run(include_str!("confirm.nu"), &input, scope, cancellation)?;
    Ok(result.unwrap_or(false))
}

fn show_dry_run(root: &Path, writes: &FileWrites, output: &CommandOutput) -> Result<()> {
    output.block(format!(
        "Site files for {} (dry run)\n\n{}",
        display_path_toward_home(root),
        tree::render(writes),
    ))?;
    output.write_stdout_styled(&code::styled(config::config_source(writes), "toml"))?;
    Ok(())
}

fn validate_configuration(
    writes: &FileWrites,
    scope: tola_build::InputScope,
) -> Result<SiteConfigSchema> {
    let source = config::config_source(writes);
    let path = writes.root().join(config::CONFIG_PATH);
    let schema = crate::config::validate_settings(&path, source, scope)?;
    Ok(schema)
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
    output.apply_diagnostic_limits(&crate::config::DiagnosticsConfig::default());
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
        let writes = files::file_writes(root).unwrap();
        assert!(writes.check().is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("tola.toml")).unwrap(),
            "existing configuration"
        );
        assert!(!root.join(".tola").exists());
    }

    #[test]
    fn dry_follows_stdout_color_policy() {
        let directory = tempfile::tempdir().unwrap();
        let writes = files::file_writes(directory.path()).unwrap();
        for color in [false, true] {
            let (sink, stream) = crate::terminal::OutputSink::buffered();
            let output = CommandOutput::new(
                crate::terminal::Terminal::with_sink(sink, color, false),
                None,
            );
            show_dry_run(writes.root(), &writes, &output).unwrap();
            let text = String::from_utf8(stream.bytes()).unwrap();
            if color {
                assert!(text.ends_with(
                    &code::styled(config::config_source(&writes), "toml").paint_all(true)
                ));
            } else {
                assert!(!text.contains('\u{1b}'));
                assert!(text.ends_with(config::config_source(&writes)));
            }
        }
    }
}
