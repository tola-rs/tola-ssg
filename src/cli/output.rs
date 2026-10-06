//! Command messages delivered to terminal presentation and optional file recording.

use std::io;

use serde_json::json;
use tola_build::diagnostic::{Diagnostic, Severity};

use crate::cli::log::LogFile;
use crate::config::DiagnosticsConfig;
use crate::terminal::{Palette, Terminal};

/// Kinds of log record this boundary writes; each names one record shape.
const STATUS_KIND: &str = "status";
const SUMMARY_KIND: &str = "summary";
const TEXT_KIND: &str = "text";
const DIAGNOSTIC_KIND: &str = "diagnostic";
const RESOLVED_KIND: &str = "resolved";
const FAILURE_KIND: &str = "failure";

/// Log targets this boundary writes under; each names the producer answering for a record.
const TERMINAL_TARGET: &str = "tola::terminal";
const DIAGNOSTIC_TARGET: &str = "tola::diagnostic";

/// Diagnostics attached to `error`, or one fallback diagnostic carrying `code`.
pub(crate) fn attached_or_fallback(
    error: &anyhow::Error,
    code: tola_build::diagnostic::DiagnosticCode,
) -> Vec<Diagnostic> {
    tola_build::diagnostic::attached(error)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| vec![tola_build::diagnostic::fallback(code, error)])
}

#[derive(Clone, Copy)]
pub(crate) struct RoundIdentity<'a> {
    number: u64,
    revision: Option<&'a str>,
}

#[derive(Clone, Copy)]
pub(crate) struct CompletedRound<'a> {
    identity: Option<RoundIdentity<'a>>,
    completion: crate::terminal::development::Completion<'a>,
}

impl<'a> CompletedRound<'a> {
    pub(crate) fn published(number: u64, revision: &'a str, summary: &'a str) -> Self {
        Self {
            identity: Some(RoundIdentity {
                number,
                revision: Some(revision),
            }),
            completion: crate::terminal::development::Completion::Published(summary),
        }
    }

    pub(crate) fn initial(summary: &'a str) -> Self {
        Self {
            identity: None,
            completion: crate::terminal::development::Completion::Published(summary),
        }
    }

    pub(crate) fn failed(number: Option<u64>, message: &'a str) -> Self {
        Self {
            identity: number.map(|number| RoundIdentity {
                number,
                revision: None,
            }),
            completion: crate::terminal::development::Completion::Failed(message),
        }
    }

    pub(crate) fn identity(self) -> Option<RoundIdentity<'a>> {
        self.identity
    }
}

fn round_fields(round: Option<RoundIdentity<'_>>) -> serde_json::Map<String, serde_json::Value> {
    let mut fields = serde_json::Map::new();
    if let Some(round) = round {
        fields.insert("round".to_owned(), json!(round.number));
        if let Some(revision) = round.revision {
            fields.insert("revision".to_owned(), json!(revision));
        }
    }
    fields
}

/// Output destinations shared by one command and its workers.
#[derive(Clone)]
pub(crate) struct CommandOutput {
    terminal: Terminal,
    log: Option<LogFile>,
}

impl CommandOutput {
    pub(crate) fn new(terminal: Terminal, log: Option<LogFile>) -> Self {
        Self { terminal, log }
    }

    pub(crate) fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    pub(crate) fn log(&self) -> Option<&LogFile> {
        self.log.as_ref()
    }

    /// Apply the diagnostic display limits of the configuration this command loaded.
    pub(crate) fn apply_diagnostic_limits(&self, limits: &DiagnosticsConfig) {
        self.terminal
            .set_diagnostic_limits(limits.max_errors, limits.max_warnings);
    }

    /// Install the source texts this command's diagnostics render snippets from.
    pub(crate) fn install_source_files(
        &self,
        root: std::path::PathBuf,
        packages: tola_typst::PackageLocations,
    ) {
        self.terminal
            .set_source_files(crate::terminal::SourceFiles::new(root, packages));
    }

    pub(crate) fn status(&self, text: impl AsRef<str>) -> io::Result<()> {
        self.record_text(STATUS_KIND, text.as_ref(), None);
        self.terminal.status(text)
    }

    pub(crate) fn activity(&self, text: impl AsRef<str>) -> io::Result<()> {
        self.record_text(STATUS_KIND, text.as_ref(), None);
        self.terminal.activity(text.as_ref())
    }

    /// Start the development view.
    ///
    /// `None` means the run keeps the plain output path: this terminal cannot draw a frame, or the
    /// reader asked for warnings and errors alone with `--quiet`.
    pub(crate) fn begin_dev_view(
        &self,
        cancelled: crate::cancellation::Cancellation,
    ) -> io::Result<Option<crate::terminal::development::DevView>> {
        if self.terminal.is_quiet() {
            return Ok(None);
        }
        let log = self
            .log
            .as_ref()
            .filter(|log| !log.is_pending() && !log.is_stopped())
            .map(|log| {
                self.terminal.palette().secondary(&format!(
                    "Log: {}",
                    crate::terminal::display_path(log.path()),
                ))
            });
        crate::terminal::development::DevView::start(
            &self.terminal.sink(),
            self.terminal.palette(),
            log,
            cancelled,
        )
    }

    /// Write a transient or secondary status line.
    pub(crate) fn secondary(&self, text: impl AsRef<str>) -> io::Result<()> {
        self.record_text(STATUS_KIND, text.as_ref(), None);
        self.terminal.secondary(text)
    }

    /// Report the address the development server serves.
    pub(crate) fn serving(&self, url: &str) -> io::Result<()> {
        self.record_text(
            STATUS_KIND,
            &crate::terminal::serving_line(url, Palette::new(false)),
            None,
        );
        self.terminal.serving(url)
    }

    pub(crate) fn waiting_for_build(&self) -> io::Result<()> {
        self.activity("Waiting for another build of this site")
    }

    pub(crate) fn summary(&self, text: impl AsRef<str>) -> io::Result<()> {
        self.record_text(SUMMARY_KIND, text.as_ref(), None);
        self.terminal.summary(text)
    }

    pub(crate) fn block(&self, text: impl AsRef<str>) -> io::Result<()> {
        self.record_text(TEXT_KIND, text.as_ref(), None);
        self.terminal.block(text)
    }

    /// Write a complete block whose styling the caller already applied, recording its plain text.
    pub(crate) fn styled_block(&self, plain: &str, rendered: &str) -> io::Result<()> {
        self.record_text(TEXT_KIND, plain, None);
        self.terminal.styled_block(rendered)
    }

    /// Announce the hooks this command executes, grouped by the stage that owns them.
    ///
    /// A command that schedules no stage prints nothing; a hook the build mode leaves
    /// out stays listed with the reason, so the author sees what this run will do.
    pub(crate) fn hook_overview(
        &self,
        hooks: &tola_build::config::section::build::HooksConfig,
        mode: tola_build::build::BuildMode,
        stages: &[tola_build::config::section::build::hooks::HookStage],
    ) -> io::Result<()> {
        let configured = tola_build::hooks::configured_hooks(hooks).collect::<Vec<_>>();
        let rendered = crate::terminal::hook_overview(
            &configured,
            mode,
            stages,
            self.terminal.columns(),
            self.terminal.palette(),
        );
        if rendered.is_empty() {
            return Ok(());
        }
        // Two deliberate renders of the one overview: the log records it at the fixed redirected
        // width without styling, the terminal at its own width with styled names.
        self.record_text(
            TEXT_KIND,
            &crate::terminal::hook_overview(&configured, mode, stages, None, Palette::new(false)),
            None,
        );
        self.terminal.hooks(&rendered)
    }

    pub(crate) fn blank_line(&self) -> io::Result<()> {
        self.terminal.blank_line()
    }

    pub(crate) fn diagnostic(&self, diagnostic: &Diagnostic) -> io::Result<()> {
        self.diagnostics(std::slice::from_ref(diagnostic))
    }

    pub(crate) fn diagnostics(&self, diagnostics: &[Diagnostic]) -> io::Result<()> {
        self.record_diagnostics(None, diagnostics);
        self.terminal.diagnostics(diagnostics)
    }

    pub(crate) fn report_round(
        &self,
        round: CompletedRound<'_>,
        diagnostics: &[Diagnostic],
    ) -> io::Result<()> {
        self.record_diagnostics(round.identity, diagnostics);
        match round.completion {
            crate::terminal::development::Completion::Failed(message) => {
                self.record_text(STATUS_KIND, message, round.identity);
            }
            crate::terminal::development::Completion::Published(summary) => {
                self.record_text(SUMMARY_KIND, summary, round.identity);
            }
        }
        self.terminal.report_round(round.completion, diagnostics)
    }

    pub(crate) fn record_diagnostics(
        &self,
        round: Option<RoundIdentity<'_>>,
        diagnostics: &[Diagnostic],
    ) {
        for diagnostic in diagnostics {
            let level = match diagnostic.severity {
                Severity::Error => "ERROR",
                Severity::Warning => "WARN",
            };
            self.record_diagnostic(level, DIAGNOSTIC_TARGET, DIAGNOSTIC_KIND, round, diagnostic);
        }
    }

    /// Transparent diagnostic and read-evidence wrappers repeat their source's display text.
    /// Their typed chain stays intact; only this log presentation skips those wrappers.
    pub(crate) fn record_failure(&self, error: &anyhow::Error) {
        if let Some(log) = &self.log {
            log.record(
                "ERROR",
                DIAGNOSTIC_TARGET,
                json!({ "kind": FAILURE_KIND, "message": failure_chain(error) }),
            );
        }
    }

    pub(crate) fn write_stdout(&self, bytes: impl AsRef<[u8]>) -> anyhow::Result<()> {
        self.terminal.write_stdout(bytes)
    }

    /// Write styled text to stdout, under this invocation's stdout colour decision.
    pub(crate) fn write_stdout_styled(
        &self,
        text: &crate::terminal::documentation::StyledText,
    ) -> anyhow::Result<()> {
        self.terminal.write_stdout_styled(text)
    }

    /// Write documentation to stdout, through this invocation's pager when it has one.
    pub(crate) fn write_documentation(&self, bytes: impl AsRef<[u8]>) -> anyhow::Result<()> {
        self.terminal.write_documentation(bytes)
    }

    pub(crate) fn write_stdout_line(&self, bytes: impl AsRef<[u8]>) -> anyhow::Result<()> {
        self.terminal.write_stdout_line(bytes)
    }

    pub(crate) fn is_interactive(&self) -> bool {
        self.terminal.is_interactive()
    }

    pub(crate) fn select_many(
        &self,
        text: &str,
        choices: &[&str],
        cancelled: &dyn Fn() -> bool,
    ) -> anyhow::Result<Vec<usize>> {
        self.terminal.select_many(text, choices, cancelled)
    }

    pub(crate) fn select_one(
        &self,
        text: &str,
        choices: &[&str],
        initial: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> anyhow::Result<usize> {
        self.terminal.select_one(text, choices, initial, cancelled)
    }

    pub(crate) fn read_line(
        &self,
        text: &str,
        default: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> anyhow::Result<String> {
        self.terminal.read_line(text, default, cancelled)
    }

    pub(crate) fn confirm(
        &self,
        text: &str,
        default: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> anyhow::Result<bool> {
        self.terminal.confirm(text, default, cancelled)
    }

    pub(crate) fn record_resolved(
        &self,
        round: Option<RoundIdentity<'_>>,
        diagnostic: &Diagnostic,
    ) {
        self.record_diagnostic("INFO", TERMINAL_TARGET, RESOLVED_KIND, round, diagnostic);
    }

    /// One diagnostic-shaped record, under the level, target, and kind its channel uses.
    fn record_diagnostic(
        &self,
        level: &str,
        target: &str,
        kind: &str,
        round: Option<RoundIdentity<'_>>,
        diagnostic: &Diagnostic,
    ) {
        let Some(log) = &self.log else {
            return;
        };
        let mut fields = round_fields(round);
        fields.insert("kind".to_owned(), json!(kind));
        fields.insert("code".to_owned(), json!(diagnostic.code));
        fields.insert("message".to_owned(), json!(diagnostic.message));
        fields.insert("diagnostic".to_owned(), json!(diagnostic));
        log.record(level, target, serde_json::Value::Object(fields));
    }

    fn record_text(&self, kind: &str, text: &str, round: Option<RoundIdentity<'_>>) {
        let Some(log) = &self.log else {
            return;
        };
        if text.trim_matches('\n').is_empty() {
            return;
        }
        let mut fields = round_fields(round);
        fields.insert("kind".to_owned(), json!(kind));
        fields.insert("message".to_owned(), json!(text));
        log.record("INFO", TERMINAL_TARGET, serde_json::Value::Object(fields));
    }
}

fn failure_chain(error: &anyhow::Error) -> String {
    let mut message = String::new();
    for cause in error.chain() {
        if (cause.is::<tola_build::diagnostic::DiagnosticError>() && cause.source().is_some())
            || cause.is::<tola_typst::BundleCompileFailure>()
            || cause.is::<tola_typst::CompileFailure>()
            || cause.is::<tola_typst::ScanFailure>()
        {
            continue;
        }
        if !message.is_empty() {
            message.push_str(": ");
        }
        use std::fmt::Write as _;
        write!(message, "{cause}").expect("writing a failure into memory cannot fail");
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::{EVERY_STAGE, OutputSink, Terminal};
    use tola_build::build::BuildMode;
    use tola_build::config::section::build::{BeforeBuildHookConfig, HooksConfig};

    fn buffered_command_output(
        directory: &tempfile::TempDir,
        use_color: bool,
    ) -> (CommandOutput, impl Fn() -> Vec<u8>) {
        let log = LogFile::prepare(&directory.path().join("session.jsonl"), |_, error| {
            panic!("log write failed: {error}")
        })
        .unwrap();
        log.start().unwrap();
        let (sink, stream) = OutputSink::buffered();
        (
            CommandOutput::new(Terminal::with_sink(sink, use_color, false), Some(log)),
            move || stream.bytes(),
        )
    }

    #[test]
    fn round_records_name_their_round() {
        let directory = tempfile::tempdir().unwrap();
        let (output, _) = buffered_command_output(&directory, false);
        let warning = Diagnostic::new(
            tola_build::codes::typst::COMPILE,
            Severity::Warning,
            "layout was ignored during HTML export",
        );
        let failure = Diagnostic::new(
            tola_build::codes::typst::COMPILE,
            Severity::Error,
            "expected block",
        );

        output
            .report_round(
                CompletedRound::published(4, "rev-4", "Rebuilt site in 620 ms"),
                std::slice::from_ref(&warning),
            )
            .unwrap();
        output
            .report_round(CompletedRound::failed(Some(5), "Build failed"), &[failure])
            .unwrap();

        let source = std::fs::read_to_string(output.log().unwrap().path()).unwrap();
        let records = source
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        let round = |kind: &str, number: u64| {
            records
                .iter()
                .find(|record| {
                    record["fields"]["kind"] == kind && record["fields"]["round"] == number
                })
                .unwrap_or_else(|| panic!("no `{kind}` record for round {number}"))
                .clone()
        };

        assert_eq!(round("diagnostic", 4)["fields"]["revision"], "rev-4");
        assert_eq!(round("summary", 4)["fields"]["revision"], "rev-4");
        assert!(round("diagnostic", 5)["fields"].get("revision").is_none());
        assert_eq!(
            round("diagnostic", 5)["fields"]["message"],
            "expected block"
        );
        assert!(
            records
                .iter()
                .all(|record| record["fields"]["round"].is_u64())
        );
    }

    #[test]
    fn log_records_omit_terminal_styles() {
        let directory = tempfile::tempdir().unwrap();
        let (output, terminal_bytes) = buffered_command_output(&directory, true);
        let mut hooks = HooksConfig::default();
        hooks.before_build.push(BeforeBuildHookConfig {
            name: "alpha".to_owned(),
            command: vec!["tool".into()],
            ..BeforeBuildHookConfig::default()
        });
        output.serving("http://127.0.0.1:5277").unwrap();
        output
            .hook_overview(&hooks, BuildMode::Production, EVERY_STAGE)
            .unwrap();
        let source = std::fs::read_to_string(output.log().unwrap().path()).unwrap();
        let records = source
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|record| {
            !record["fields"]["message"]
                .as_str()
                .unwrap()
                .contains('\u{1b}')
        }));
        assert!(
            String::from_utf8(terminal_bytes())
                .unwrap()
                .contains('\u{1b}')
        );
    }

    #[test]
    fn failure_chain_preserves_distinct_causes() {
        let typed = tola_typst::CompileError::input("source unavailable");
        let error = anyhow::Error::new(typed)
            .context("source unavailable")
            .context("read the site")
            .context("read the site");
        let shown = failure_chain(&error);
        assert_eq!(shown.matches("read the site").count(), 2);
        assert_eq!(shown.matches("source unavailable").count(), 2);
        assert!(error.downcast_ref::<tola_typst::CompileError>().is_some());
        assert_eq!(error.chain().count(), 4);
    }

    #[test]
    fn transparent_wrappers_render_source_once() {
        let directory = tempfile::tempdir().unwrap();
        let entry = directory.path().join("site.typ");
        std::fs::write(&entry, "#missing").unwrap();
        let world = tola_typst::TypstWorld::builder(&entry, directory.path())
            .with_local_cache()
            .no_fonts()
            .build(&tola_typst::BundleCancellation::new())
            .unwrap();
        let failure =
            tola_typst::compile_bundle_world(&world, &tola_typst::BundleCancellation::new())
                .unwrap_err();
        let expected = failure.error().to_string();
        let diagnostic = Diagnostic::new(
            tola_build::codes::typst::COMPILE,
            Severity::Error,
            "missing name",
        );
        let error: anyhow::Error =
            tola_build::diagnostic::DiagnosticError::attach(failure.into(), vec![diagnostic])
                .into();
        assert_eq!(failure_chain(&error), expected);
        assert!(
            error
                .chain()
                .any(|cause| cause.is::<tola_typst::CompileError>())
        );
        assert!(tola_build::diagnostic::attached(&error).is_some());
        assert_eq!(error.chain().count(), 3);
    }
}
