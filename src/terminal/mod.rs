//! Terminal presentation and interactive input.

pub(crate) mod clipboard;
pub(crate) mod code;
pub(crate) mod color;
pub(crate) mod development;
mod diagnostic;
pub(crate) mod documentation;
mod hooks;
mod input;
mod limits;
mod pager;
mod path;
mod progress;
mod prompt;
pub(crate) mod session;
mod sink;
mod source;
pub(crate) mod style;
pub(crate) mod text;
pub(crate) mod ui;
pub(crate) mod wrap;

use clap::ColorChoice;

pub(crate) use diagnostic::render as render_diagnostic;
pub(crate) use hooks::{EVERY_STAGE, PRE_PUBLICATION_STAGES, overview as hook_overview};
pub(crate) use pager::PagerCommand;
pub(crate) use path::{
    display_path, display_path_as_given, display_path_toward_home, display_path_within,
};
pub(crate) use progress::{
    describe_change, describe_outputs, format_duration, plural_count, serving_line, site_summary,
};
pub(crate) use source::SourceFiles;
pub(crate) use style::Palette;

pub(crate) use prompt::InputCancelled;
use prompt::PromptReader;
pub(crate) use sink::{OutputSink, StdoutClosed};
use std::io;

const INDENT_UNIT: &str = "  ";

/// Best-effort restoration for an application panic hook.
pub(crate) fn restore() {
    prompt::restore();
    session::restore();
}

pub(crate) fn append_indent(output: &mut String, level: usize) {
    for _ in 0..level {
        output.push_str(INDENT_UNIT);
    }
}

/// Shared command output for one process invocation.
///
/// Clones share a synchronized sink, so output blocks never interleave.
#[derive(Clone)]
pub struct Terminal {
    sink: OutputSink,
    prompt: PromptReader,
    sources: std::sync::Arc<std::sync::RwLock<Option<std::sync::Arc<source::SourceFiles>>>>,
    palette: Palette,
    stdout_use_color: bool,
    pager: Option<PagerCommand>,
    quiet: bool,
    diagnostic_limits: limits::DiagnosticLimits,
}

impl Terminal {
    /// A background service reports through its owner, without writing or retaining a transcript.
    pub(crate) fn silent() -> Self {
        Self::with_sink(OutputSink::discard(), false, true)
    }

    pub fn new(color: ColorChoice, quiet: bool, pager: Option<PagerCommand>) -> Self {
        let mut terminal = Self::with_sink(
            OutputSink::process(),
            color::enabled(color, color::Stream::Stderr),
            quiet,
        );
        terminal.stdout_use_color = color::enabled(color, color::Stream::Stdout);
        terminal.pager = pager;
        terminal
    }

    pub(crate) fn with_sink(sink: OutputSink, use_color: bool, quiet: bool) -> Self {
        Self {
            prompt: PromptReader::new(sink.clone()),
            sources: std::sync::Arc::new(std::sync::RwLock::new(None)),
            palette: Palette::new(use_color),
            stdout_use_color: use_color,
            pager: None,
            sink,
            quiet,
            diagnostic_limits: limits::DiagnosticLimits::default(),
        }
    }

    pub(crate) fn sink(&self) -> OutputSink {
        self.sink.clone()
    }

    pub(crate) fn uses_color(&self) -> bool {
        self.palette.uses_color()
    }

    /// The styling every role of this terminal's status text is drawn with.
    pub(crate) fn palette(&self) -> Palette {
        self.palette
    }

    pub(crate) fn stdout_uses_color(&self) -> bool {
        self.stdout_use_color
    }

    pub(crate) fn stdout_columns(&self) -> Option<usize> {
        use std::io::IsTerminal;
        if !io::stdout().is_terminal() {
            return None;
        }
        let (columns, _) = crossterm::terminal::size().ok()?;
        (columns > 0).then_some(usize::from(columns))
    }

    /// Install the source texts diagnostics render snippets from.
    ///
    /// A command that loaded a site configuration provides them; commands without a site keep
    /// rendering diagnostics without snippets.
    pub fn set_source_files(&self, sources: source::SourceFiles) {
        if let Ok(mut slot) = self.sources.write() {
            *slot = Some(std::sync::Arc::new(sources));
        }
    }

    pub(crate) fn is_quiet(&self) -> bool {
        self.quiet
    }

    /// Column count of the terminal this command writes to, when it has one.
    pub(crate) fn columns(&self) -> Option<usize> {
        use std::io::IsTerminal;
        if !io::stderr().is_terminal() {
            return None;
        }
        let (columns, _) = crossterm::terminal::size().ok()?;
        (columns > 0).then_some(usize::from(columns))
    }

    /// Write command progress unless quiet mode is enabled.
    pub fn status(&self, text: impl AsRef<str>) -> io::Result<()> {
        if self.quiet {
            return Ok(());
        }
        self.write_nonempty(progress::status(text.as_ref()))
    }

    /// Write a transient or secondary status line unless quiet mode is enabled.
    pub fn secondary(&self, text: impl AsRef<str>) -> io::Result<()> {
        if self.quiet {
            return Ok(());
        }
        self.write_nonempty(progress::secondary(text.as_ref(), self.palette))
    }

    /// Report the address the development server serves unless quiet mode is enabled.
    pub fn serving(&self, url: &str) -> io::Result<()> {
        if self.quiet {
            return Ok(());
        }
        self.sink
            .serving(&format!("{}\n", progress::serving_line(url, self.palette)))
    }

    pub fn summary(&self, text: impl AsRef<str>) -> io::Result<()> {
        if self.quiet {
            return Ok(());
        }
        self.write_nonempty(progress::summary(text.as_ref(), self.palette))
    }

    pub fn block(&self, text: impl AsRef<str>) -> io::Result<()> {
        self.write_nonempty(progress::text(text.as_ref()))
    }

    /// Write a complete block whose styling the caller already applied.
    ///
    /// Escaping exists for text Tola does not control; a block that has its own
    /// styling has already escaped the text inside it, so escaping again would print
    /// the escape codes instead of applying them.
    pub(crate) fn styled_block(&self, block: &str) -> io::Result<()> {
        if self.quiet {
            return Ok(());
        }
        self.write(block.to_owned())
    }

    pub(crate) fn hooks(&self, text: &str) -> io::Result<()> {
        if self.quiet {
            return Ok(());
        }
        self.sink
            .hooks(&format!("{}\n", text.trim_end_matches('\n')))
    }

    pub(crate) fn blank_line(&self) -> io::Result<()> {
        if self.quiet {
            return Ok(());
        }
        self.sink.write_stderr(b"\n")
    }

    pub(crate) fn activity(&self, text: &str) -> io::Result<()> {
        if self.quiet {
            return Ok(());
        }
        let mut text = text.to_owned();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        self.sink.activity(&text)
    }

    pub(crate) fn hook_output(
        &self,
        run: &development::HookRun,
        stream: &str,
        chunk: &str,
    ) -> io::Result<()> {
        if self.quiet || chunk.is_empty() {
            return Ok(());
        }
        self.sink.hook_output(run, stream, chunk, self.palette)
    }

    pub fn diagnostic(&self, diagnostic: &tola_build::diagnostic::Diagnostic) -> io::Result<()> {
        self.diagnostics(std::slice::from_ref(diagnostic))
    }

    pub fn diagnostics(
        &self,
        diagnostics: &[tola_build::diagnostic::Diagnostic],
    ) -> io::Result<()> {
        let mut transcript = self.render_diagnostics(diagnostics);
        if !transcript.is_empty() {
            transcript.push('\n');
        }
        self.sink.round(development::Round {
            diagnostics: diagnostics.to_vec(),
            transcript,
            observed_at: chrono::Local::now(),
        })
    }

    pub(crate) fn report_round(
        &self,
        completion: development::Completion<'_>,
        diagnostics: &[tola_build::diagnostic::Diagnostic],
    ) -> io::Result<()> {
        let mut transcript = String::new();
        if let development::Completion::Failed(message) = completion {
            transcript.push_str(&progress::failure(message, self.palette));
            transcript.push('\n');
        }
        let rendered = self.render_diagnostics(diagnostics);
        if !rendered.is_empty() {
            transcript.push_str(&rendered);
            transcript.push('\n');
        }
        if let development::Completion::Published(summary) = completion
            && !self.quiet
            && !summary.is_empty()
        {
            transcript.push_str(&progress::summary(summary, self.palette));
            transcript.push('\n');
        }
        self.sink.round(development::Round {
            diagnostics: diagnostics.to_vec(),
            transcript,
            observed_at: chrono::Local::now(),
        })
    }

    fn render_diagnostics(&self, diagnostics: &[tola_build::diagnostic::Diagnostic]) -> String {
        let sources = self.sources.read().ok().and_then(|slot| slot.clone());
        self.diagnostic_limits
            .render(diagnostics, sources.as_deref(), self.palette)
    }

    /// Write machine-readable output to stdout.
    pub fn write_stdout(&self, bytes: impl AsRef<[u8]>) -> anyhow::Result<()> {
        self.sink.write_stdout(bytes.as_ref())
    }

    /// Write machine-readable output followed by a newline.
    pub fn write_stdout_line(&self, bytes: impl AsRef<[u8]>) -> anyhow::Result<()> {
        self.sink.write_stdout_line(bytes.as_ref())
    }

    /// Write styled text to stdout under this invocation's stdout colour decision.
    ///
    /// Paging stays with [`Terminal::write_documentation`]: text a command prints as part of its
    /// own output must not open a pager.
    pub(crate) fn write_stdout_styled(
        &self,
        text: &documentation::StyledText,
    ) -> anyhow::Result<()> {
        let rendered = text.paint_all(self.stdout_use_color);
        self.sink.write_stdout(rendered.as_bytes())
    }

    /// Write the `tola help` documentation, through the pager when one is configured.
    ///
    /// A pager that cannot start leaves a note on stderr, and the documentation reaches stdout
    /// instead.
    pub fn write_documentation(&self, bytes: impl AsRef<[u8]>) -> anyhow::Result<()> {
        if let Some(pager) = &self.pager {
            match pager.start() {
                Ok(started) => {
                    started.write(bytes.as_ref())?;
                    return Ok(());
                }
                Err(error) => {
                    tracing::debug!(
                        target: "tola::terminal",
                        error = %error,
                        "pager did not start"
                    );
                    let _ = self.sink.write_stderr(
                        format!(
                            "could not start the pager `{}`\n",
                            pager.program().to_string_lossy()
                        )
                        .as_bytes(),
                    );
                }
            }
        }
        self.write_stdout(bytes)
    }

    pub(crate) fn is_interactive(&self) -> bool {
        self.prompt.is_interactive()
    }

    pub(crate) fn select_many(
        &self,
        text: &str,
        choices: &[&str],
        cancelled: &dyn Fn() -> bool,
    ) -> anyhow::Result<Vec<usize>> {
        self.prompt
            .select_many(text, choices, self.uses_color(), cancelled)
    }

    pub(crate) fn select_one(
        &self,
        text: &str,
        choices: &[&str],
        initial: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> anyhow::Result<usize> {
        self.prompt
            .select_one(text, choices, initial, self.uses_color(), cancelled)
    }

    pub(crate) fn read_line(
        &self,
        text: &str,
        default: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> anyhow::Result<String> {
        self.prompt.read_line(text, default, cancelled)
    }

    pub(crate) fn confirm(
        &self,
        text: &str,
        default: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> anyhow::Result<bool> {
        self.prompt.confirm(text, default, cancelled)
    }

    pub(crate) fn set_diagnostic_limits(
        &self,
        max_errors: Option<usize>,
        max_warnings: Option<usize>,
    ) {
        self.diagnostic_limits.set(max_errors, max_warnings);
    }

    fn write(&self, mut rendered: String) -> io::Result<()> {
        if !rendered.ends_with('\n') {
            rendered.push('\n');
        }
        self.sink.write_stderr(rendered.as_bytes())
    }

    fn write_nonempty(&self, rendered: String) -> io::Result<()> {
        if rendered.is_empty() {
            return Ok(());
        }
        self.write(rendered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::sink::BufferedOutput;
    use tola_build::diagnostic::Diagnostic;

    fn captured() -> (Terminal, BufferedOutput) {
        let (sink, output) = OutputSink::buffered();
        (Terminal::with_sink(sink, false, false), output)
    }

    fn captured_with_color() -> (Terminal, BufferedOutput) {
        let (sink, output) = OutputSink::buffered();
        (Terminal::with_sink(sink, true, false), output)
    }

    #[test]
    fn hook_chunks_preserve_lines() {
        for (chunks, expected) in [
            (vec!["abc", "def\n"], "[assets] abcdef\n"),
            (
                vec!["first\r", "\nsecond\r", "\n"],
                "[assets] first\n[assets] second\n",
            ),
        ] {
            let (terminal, output) = captured();
            for chunk in chunks {
                terminal
                    .hook_output(
                        &development::HookRun {
                            scope: 0,
                            name: "assets".to_owned(),
                        },
                        "stdout",
                        chunk,
                    )
                    .unwrap();
            }
            assert_eq!(String::from_utf8(output.bytes()).unwrap(), expected);
        }
    }

    #[test]
    fn status_finishes_hook_line() {
        let (terminal, output) = captured();
        terminal
            .hook_output(
                &development::HookRun {
                    scope: 0,
                    name: "assets".to_owned(),
                },
                "stdout",
                "partial",
            )
            .unwrap();
        terminal.status("Finished").unwrap();
        assert_eq!(
            String::from_utf8(output.bytes()).unwrap(),
            "[assets] partial\nFinished\n"
        );
    }

    #[test]
    fn styled_block_is_not_escaped_twice() {
        let (terminal, output) = captured_with_color();
        terminal
            .styled_block("Hooks:\n  \u{1b}[2mbefore-build\u{1b}[0m\n")
            .unwrap();
        terminal
            .block("Hooks:\n  \u{1b}[2mbefore-build\u{1b}[0m")
            .unwrap();

        assert_eq!(
            String::from_utf8(output.bytes()).unwrap(),
            "Hooks:\n  \u{1b}[2mbefore-build\u{1b}[0m\nHooks:\n  before-build\n"
        );
    }

    /// The layout always comes from `codespan-reporting`; the buffer choice decides whether its
    /// style bytes survive, so a wrong buffer silently flattens every diagnostic.
    #[test]
    fn diagnostic_escapes_only_with_color() {
        let diagnostic = Diagnostic::new(
            tola_build::codes::config::EXPERIMENTAL,
            tola_build::diagnostic::Severity::Warning,
            "experimental configuration is enabled",
        );

        let (plain, plain_output) = captured();
        plain.diagnostic(&diagnostic).unwrap();
        let plain = String::from_utf8(plain_output.bytes()).unwrap();

        let (styled, styled_output) = captured_with_color();
        styled.diagnostic(&diagnostic).unwrap();
        let styled = String::from_utf8(styled_output.bytes()).unwrap();

        assert!(
            styled.contains('\u{1b}'),
            "a colored terminal received uncolored output: {styled:?}"
        );
        assert!(
            !plain.contains('\u{1b}'),
            "an uncolored terminal received escape sequences: {plain:?}"
        );
    }

    #[test]
    fn limits_hide_surplus_diagnostics() {
        let (terminal, output) = captured();
        terminal.set_diagnostic_limits(Some(1), Some(0));
        let diagnostics = [
            Diagnostic::new(
                tola_build::codes::typst::COMPILE,
                tola_build::diagnostic::Severity::Error,
                "first failure",
            ),
            Diagnostic::new(
                tola_build::codes::config::INVALID,
                tola_build::diagnostic::Severity::Error,
                "second failure",
            ),
            Diagnostic::new(
                tola_build::codes::config::WARNING,
                tola_build::diagnostic::Severity::Warning,
                "retained warning",
            ),
        ];
        terminal.diagnostics(&diagnostics).unwrap();
        let shown = String::from_utf8(output.bytes()).unwrap();
        assert!(shown.contains("first failure"));
        assert!(!shown.contains("second failure"));
        assert!(!shown.contains("retained warning"));
        assert!(shown.contains("1 error not shown"));
        assert!(shown.contains("1 warning not shown"));
    }

    #[test]
    fn clones_share_one_destination() {
        let (terminal, output) = captured();
        let other = terminal.clone();
        terminal.block("first\n  continuation").unwrap();
        other.status("second").unwrap();

        assert_eq!(
            String::from_utf8(output.bytes()).unwrap(),
            "first\n  continuation\nsecond\n"
        );
    }

    #[test]
    fn blank_lines_write_exactly_one_row() {
        let (terminal, output) = captured();
        terminal.status("first").unwrap();
        terminal.status("").unwrap();
        terminal.block("\n").unwrap();
        terminal.blank_line().unwrap();
        terminal.block("second").unwrap();

        assert_eq!(
            String::from_utf8(output.bytes()).unwrap(),
            "first\n\nsecond\n"
        );
    }

    #[test]
    fn quiet_still_shows_diagnostics() {
        let (mut terminal, output) = captured();
        terminal.quiet = true;
        terminal.status("Building").unwrap();
        terminal.summary("Built").unwrap();
        terminal.blank_line().unwrap();
        terminal
            .diagnostic(&Diagnostic::new(
                tola_build::codes::typst::COMPILE,
                tola_build::diagnostic::Severity::Error,
                "source could not compile",
            ))
            .unwrap();
        assert_eq!(
            String::from_utf8(output.bytes()).unwrap(),
            "error[typst.compile]: source could not compile\n"
        );
    }

    #[test]
    fn quiet_region_preserves_failure_context() {
        let (sink, output) = OutputSink::buffered();
        let terminal = Terminal::with_sink(sink, false, true);
        let diagnostic = Diagnostic::new(
            tola_build::codes::typst::COMPILE,
            tola_build::diagnostic::Severity::Error,
            "expected expression",
        );
        terminal
            .report_round(
                development::Completion::Failed("Build failed; still serving the previous site"),
                &[diagnostic],
            )
            .unwrap();
        terminal
            .report_round(development::Completion::Published("Build succeeded"), &[])
            .unwrap();
        let shown = String::from_utf8(output.bytes()).unwrap();
        assert!(shown.contains("still serving the previous site"), "{shown}");
        assert!(shown.contains("error[typst.compile]"), "{shown}");
        assert!(!shown.contains("Rebuilt"), "{shown}");
        assert!(!shown.contains("Build succeeded"), "{shown}");
    }
}
