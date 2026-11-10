//! Process-wide command setup, cancellation, and exit reporting.

use std::process::ExitCode;

use serde_json::json;
use tola_diagnostic::{Diagnostic, Severity};

use super::Commands;
use super::log::{LogFile, LogOrigin};
use super::output::{CommandOutput, attached_or_fallback};
use super::parse::{Command, ParsedCommand};
use crate::cancellation::Cancellation;
use crate::terminal::{InputCancelled, PagerCommand, StdoutClosed, Terminal, display_path};

const CANCELLED_EXIT_CODE: u8 = 130;

/// The detailed session log this command records, and why Tola records it.
///
/// `--log-file` names the file and `--no-log-file` suppresses one. A development session
/// otherwise records one below the site's reserved `.tola` directory, because the terminal
/// region shows only the latest round while the log keeps every round. The origin decides who
/// answers for a log Tola cannot use.
fn session_log_path(parsed: &ParsedCommand) -> Option<(std::path::PathBuf, LogOrigin)> {
    if parsed.process.no_log_file {
        return None;
    }
    if let Some(path) = &parsed.process.log_file {
        return Some((path.clone(), LogOrigin::Explicit));
    }
    match &parsed.command {
        Command::Builtin(Commands::Dev { site, .. }) => {
            default_session_log_path(&site.config).map(|path| (path, LogOrigin::Automatic))
        }
        _ => None,
    }
}

/// The next development session log below the site's reserved directory.
fn default_session_log_path(config: &super::ConfigFileArgs) -> Option<std::path::PathBuf> {
    let root = super::config::source_root(config).ok()?;
    let directory = tola_input::layout::logs(&root);
    Some(directory.join(format!("dev-{}.jsonl", super::log::session_stamp())))
}

fn hook_nested_build(parsed: &ParsedCommand) -> Option<String> {
    let stage = std::env::var("TOLA_HOOK_STAGE").ok()?;
    let blocked = match &parsed.command {
        Command::Builtin(
            Commands::Build { .. } | Commands::Dev { .. } | Commands::Preview { .. },
        ) => true,
        Command::Builtin(Commands::Check { no_build, .. }) => !no_build,
        _ => false,
    };
    blocked.then_some(stage)
}

pub(crate) fn run() -> ExitCode {
    let parsed = match ParsedCommand::parse() {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    super::color::install(parsed.process.color);
    if let Some(stage) = hook_nested_build(&parsed) {
        let diagnostic = Diagnostic::new(
            crate::codes::hook::NESTED_BUILD,
            Severity::Error,
            format!("the `{stage}` hook cannot start another Tola build"),
        )
        .with_help(
            "Run the build outside the hook; a nested build waits on the same site lock and can re-run this hook.",
        );
        let terminal = Terminal::new(parsed.process.color, parsed.process.quiet, None);
        let _ = terminal.diagnostic(&diagnostic);
        return ExitCode::FAILURE;
    }
    if let Command::Builtin(Commands::Nu(args)) = &parsed.command {
        let scope = match args.input.scope() {
            tola_input::InputScope::Pure => Some("pure"),
            tola_input::InputScope::Offline => Some("offline"),
            tola_input::InputScope::Online => None,
        };
        return super::commands::nu::run(args, scope);
    }
    let events = crate::dev::events::Events(
        matches!(&parsed.command, Command::Builtin(Commands::Dev { development, .. }) if development.json),
    );
    let pager = match &parsed.command {
        Command::Builtin(Commands::Help(args))
            if args.format == super::args::HelpFormat::Text && !args.no_pager =>
        {
            PagerCommand::for_process()
        }
        _ => Ok(None),
    };
    let pager = match pager {
        Ok(pager) => pager,
        Err(error) => {
            let terminal = Terminal::new(parsed.process.color, parsed.process.quiet, None);
            let _ = terminal.diagnostic(&tola_diagnostic::fallback(
                crate::codes::terminal::PAGER,
                &error,
            ));
            return ExitCode::FAILURE;
        }
    };
    let terminal = Terminal::new(parsed.process.color, parsed.process.quiet, pager);
    let cancellation = match Cancellation::install() {
        Ok(cancellation) => cancellation,
        Err(error) => {
            let fallback = tola_diagnostic::fallback(crate::codes::command::SIGNAL, &error);
            let _ = terminal.diagnostic(&fallback);
            let _ = events.stopped("error", &[fallback]);
            return ExitCode::FAILURE;
        }
    };
    terminal.sink().set_cancellation(cancellation.token());
    let failure = super::dispatch::FailureReport::of(&parsed);
    let (output, mut outcome) = match prepare_output(&parsed, terminal.clone()) {
        Ok(output) => {
            let outcome =
                super::dispatch::dispatch(parsed.command, &output, &cancellation, failure);
            (output, outcome)
        }
        Err(error) => {
            let output = CommandOutput::new(terminal, None);
            let outcome = super::dispatch::report_failure(failure, &output, error);
            (output, outcome)
        }
    };
    if events.0 {
        let failure = outcome
            .as_ref()
            .err()
            .filter(|error| !is_command_cancellation(error));
        let stopped_reason = if failure.is_some() {
            "error"
        } else {
            "requested"
        };
        let stopped_diagnostics = failure
            .map(|error| attached_or_fallback(error, crate::codes::command::FAILED))
            .unwrap_or_default();
        if let Err(error) = events.stopped(stopped_reason, &stopped_diagnostics)
            && outcome.is_ok()
        {
            outcome = Err(error);
        }
    }
    let code = match outcome {
        Ok(()) if !cancellation.was_interrupted() => 0,
        Err(error) if error.is::<StdoutClosed>() && !cancellation.was_interrupted() => 0,
        outcome
            if cancellation.was_interrupted()
                || outcome.as_ref().err().is_some_and(is_command_cancellation) =>
        {
            let _ = output.status("Cancelled");
            CANCELLED_EXIT_CODE
        }
        Err(error) if error.is::<clap::Error>() => {
            let clap_error = error.downcast_ref::<clap::Error>().unwrap();
            let _ = clap_error.print();
            let code = clap_error.exit_code() as u8;
            if code != 0 {
                output.record_failure(&error);
            }
            code
        }
        Err(error) if error.is::<super::external::CommandExit>() => {
            if let Some(diagnostics) = tola_diagnostic::attached(&error) {
                let _ = output.diagnostics(diagnostics);
                output.record_failure(&error);
            }
            let status = error
                .downcast_ref::<super::external::CommandExit>()
                .unwrap()
                .0;
            status
                .code()
                .and_then(|code| u8::try_from(code).ok())
                .unwrap_or_else(|| {
                    tola_subprocess::terminating_signal(&status)
                        .and_then(|signal| u8::try_from(128 + signal).ok())
                        .unwrap_or(1)
                })
        }
        Err(error) => {
            let diagnostics = attached_or_fallback(&error, crate::codes::command::FAILED);
            let _ = output.diagnostics(&diagnostics);
            output.record_failure(&error);
            1
        }
        Ok(()) => unreachable!("successful non-interrupted commands return above"),
    };
    if let Some(log) = output.log() {
        log.record(
            "INFO",
            "tola::terminal",
            json!({
                "kind": "command_finished", "success": code == 0, "exit_code": code,
                "message": "command finished",
            }),
        );
    }
    ExitCode::from(code)
}

fn is_command_cancellation(error: &anyhow::Error) -> bool {
    error.is::<InputCancelled>()
        || error.chain().any(|cause| {
            cause.is::<tola_build::cancellation::BuildCancelled>()
                || cause
                    .downcast_ref::<std::io::Error>()
                    .and_then(std::io::Error::get_ref)
                    .is_some_and(|inner| inner.is::<tola_build::cancellation::BuildCancelled>())
        })
}

fn prepare_output(parsed: &ParsedCommand, terminal: Terminal) -> anyhow::Result<CommandOutput> {
    let log_target = session_log_path(parsed);
    if let Some((log_path, _)) = &log_target
        && let Some(config) = parsed.command.config_file()
    {
        let cwd = std::env::current_dir()?;
        let config_path = config.path.clone().unwrap_or_else(|| {
            tola_build::config::loading::find_default_config(&cwd)
                .unwrap_or_else(|| cwd.join("tola.toml"))
        });
        super::log::destination::check_file(Some(log_path), &config_path)?;
    }
    // An automatic log that cannot be prepared keeps its cause here until the subscriber that
    // shows verbose output is installed below.
    let mut unavailable = None;
    let log = match log_target {
        Some((path, origin)) => match prepare_log(&path, origin, &terminal) {
            Ok(log) => Some(log),
            // Tola named this path for a development session, so an unusable log is a warning
            // rather than a reason to stop; a file the reader named still fails the command.
            Err(error) if origin == LogOrigin::Automatic => {
                report_unavailable_log(&terminal, &path);
                unavailable = Some((path, error));
                None
            }
            Err(error) => return Err(error),
        },
        None => None,
    };
    if let Some(log) = &log {
        log.record(
            "INFO",
            "tola::terminal",
            json!({
                "kind": "command_started", "version": env!("CARGO_PKG_VERSION"),
                "typst": typst::utils::version().raw(), "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH, "command": parsed.name,
                "message": "command started",
            }),
        );
    }
    super::log::tracing::install(parsed.process.verbose, terminal.clone(), log.clone())?;
    // The warning the site author reads is already out; the cause chain belongs to explicitly requested
    // verbose output, and this is the subscriber that prints it.
    if let Some((path, error)) = &unavailable {
        super::log::destination::trace_unavailable_cause(path, error);
    }
    install_panic_report(log.clone(), terminal.clone());
    Ok(CommandOutput::new(terminal, log))
}

/// Prepare the session log, wiring the write-failure report the recorder calls later.
fn prepare_log(
    path: &std::path::Path,
    origin: LogOrigin,
    terminal: &Terminal,
) -> anyhow::Result<LogFile> {
    let failures = terminal.clone();
    // A write failure reports the path the log was declared with: Tola's own session log must not
    // read as the target a symlinked prefix resolves to, and a path the reader named stays as
    // given.
    let declared = path.to_owned();
    let on_write_failure = move |path: &std::path::Path, _: &std::io::Error| {
        let reported = match origin {
            LogOrigin::Automatic => declared.as_path(),
            LogOrigin::Explicit => path,
        };
        report_log_failure(&failures, origin, reported);
    };
    match origin {
        LogOrigin::Automatic => LogFile::prepare_session(path, on_write_failure),
        LogOrigin::Explicit => LogFile::prepare(path, on_write_failure),
    }
}

/// Replace the standard panic report with the diagnostic the site author reads.
///
/// A panic means a Tola defect, not a site mistake, so the reader gets a plain sentence and,
/// when the log recorded the panic, the path to attach to the report. The payload, location, and
/// backtrace stay in the log.
fn install_panic_report(log: Option<LogFile>, terminal: Terminal) {
    std::panic::set_hook(Box::new(move |panic| {
        crate::terminal::restore();
        // Only a log that really holds the record may be named: a log that never started, or one
        // whose writes stopped, has no panic to attach.
        let mut stored = None;
        if let Some(log) = &log
            && log.record_panic(panic)
        {
            stored = Some(log.path());
        }
        let diagnostic = explained_panic_diagnostic(panic.payload())
            .unwrap_or_else(|| internal_error_diagnostic(stored));
        let mut rendered =
            crate::terminal::render_diagnostic(&diagnostic, None, terminal.uses_color());
        rendered.push('\n');
        // This thread may already hold the stream lock when it panicked, so this write never waits.
        let _ = terminal
            .sink()
            .write_stderr_nonblocking(rendered.as_bytes());
    }));
}

/// The diagnostic the site author reads for an upstream panic Tola recognizes.
///
/// A recognized panic is a mistake in the site's own documents, not a Tola defect, so it
/// carries no report request. The hook is what renders it: no step catches a panic, so the
/// process unwinds out of the command's error path and a panic on the main thread exits as 101.
fn explained_panic_diagnostic(
    payload: &(dyn std::any::Any + Send),
) -> Option<tola_diagnostic::Diagnostic> {
    let explained = tola_typst::diagnostic::explained_panic(payload)?;
    let mut diagnostic = tola_diagnostic::Diagnostic::new(
        tola_diagnostic::codes::typst::HTML_EXPORT,
        tola_diagnostic::Severity::Error,
        explained.message,
    );
    if let Some(help) = explained.help {
        diagnostic = diagnostic.with_help(help);
    }
    if let Some(note) = explained.note {
        diagnostic = diagnostic.with_note(note);
    }
    Some(diagnostic)
}

/// Where a failure no author can act on is reported.
pub(crate) const REPORT_URL: &str = "https://github.com/tola-rs/tola-ssg/issues";

fn internal_error_diagnostic(log_path: Option<&std::path::Path>) -> tola_diagnostic::Diagnostic {
    let help = match log_path {
        Some(path) => format!(
            "Report this at {REPORT_URL} and attach the log written to `{}`",
            display_path(path)
        ),
        None => format!("Rerun with `--log-file tola.log` and report this at {REPORT_URL}"),
    };
    tola_diagnostic::Diagnostic::new(
        crate::codes::internal::ERROR,
        tola_diagnostic::Severity::Error,
        "Tola could not finish this command",
    )
    .with_help(help)
}

/// Report an automatic session log this session cannot use.
fn report_unavailable_log(terminal: &Terminal, path: &std::path::Path) {
    report_diagnostic(
        terminal,
        &super::log::destination::unavailable_diagnostic(path),
    );
}

/// Write one diagnostic directly to the terminal; the command output does not exist yet.
fn report_diagnostic(terminal: &Terminal, diagnostic: &tola_diagnostic::Diagnostic) {
    let mut rendered = crate::terminal::render_diagnostic(diagnostic, None, terminal.uses_color());
    rendered.push('\n');
    let _ = terminal.sink().write_stderr(rendered.as_bytes());
}

fn report_log_failure(terminal: &Terminal, origin: LogOrigin, path: &std::path::Path) {
    // Recording is unavailable, so report directly: no recursion, no site display limits.
    // The io failure itself is not rendered; the reader needs the path and the consequence.
    let diagnostic = tola_diagnostic::Diagnostic::new(
        crate::codes::log::WRITE,
        tola_diagnostic::Severity::Warning,
        format!("cannot write log `{}`", display_path(path)),
    )
    .with_note("logging has stopped")
    .with_help(match origin {
        // The session log is Tola's choice, and a session is not rerun to regain it.
        LogOrigin::Automatic => {
            "Make `.tola/state/logs` writable, or pass `--log-file` to record elsewhere"
        }
        LogOrigin::Explicit => {
            "Choose a writable location with free space for `--log-file`, then rerun"
        }
    });
    report_diagnostic(terminal, &diagnostic);
}
