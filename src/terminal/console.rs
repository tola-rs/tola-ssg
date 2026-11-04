//! The command-line terminal facade: status lines, diagnostics, and paged documentation.

use std::io::{self, IsTerminal};
use std::sync::{Arc, RwLock};

use clap::ColorChoice;
use tola_diagnostic::Diagnostic;

use super::pager::PagerCommand;
use super::sink::OutputSink;
use super::source;
use super::style::Palette;
use super::{color, limits, progress, session, text};
use crate::dev::round::Completion;

fn terminal_columns() -> Option<usize> {
    let (columns, _) = crossterm::terminal::size().ok()?;
    (columns > 0).then_some(usize::from(columns))
}

/// Shared command output for one process invocation.
///
/// Clones share a synchronized sink, so output blocks never interleave.
#[derive(Clone)]
pub struct Terminal {
    sink: OutputSink,
    sources: Arc<RwLock<Option<Arc<source::SourceFiles>>>>,
    palette: Palette,
    stdout_use_color: bool,
    pager: Option<PagerCommand>,
    quiet: bool,
    diagnostic_limits: limits::DiagnosticLimits,
}

impl Terminal {
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
            sources: Arc::new(RwLock::new(None)),
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
        io::stdout().is_terminal().then(terminal_columns).flatten()
    }

    /// Install the source texts diagnostics render snippets from.
    ///
    /// A command that loaded a site configuration provides them; commands without a site keep
    /// rendering diagnostics without snippets.
    pub fn set_source_files(&self, sources: source::SourceFiles) {
        if let Ok(mut slot) = self.sources.write() {
            *slot = Some(Arc::new(sources));
        }
    }

    pub(crate) fn is_quiet(&self) -> bool {
        self.quiet
    }

    /// Column count of the terminal this command writes to, when it has one.
    pub(crate) fn columns(&self) -> Option<usize> {
        io::stderr().is_terminal().then(terminal_columns).flatten()
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

    pub(crate) fn hooks(&self, text: &str) -> io::Result<()> {
        if self.quiet {
            return Ok(());
        }
        self.sink
            .hooks(&format!("{}\n", text.trim_end_matches('\n')))
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
        run: &crate::dev::round::HookRun,
        stream: &str,
        chunk: &str,
    ) -> io::Result<()> {
        if self.quiet || chunk.is_empty() {
            return Ok(());
        }
        self.sink.hook_output(run, stream, chunk, self.palette)
    }

    pub fn diagnostic(&self, diagnostic: &Diagnostic) -> io::Result<()> {
        self.diagnostics(std::slice::from_ref(diagnostic))
    }

    pub fn diagnostics(&self, diagnostics: &[Diagnostic]) -> io::Result<()> {
        let mut transcript = self.render_diagnostics(diagnostics);
        if !transcript.is_empty() {
            transcript.push('\n');
        }
        self.publish_round(diagnostics, transcript)
    }

    pub(crate) fn report_round(
        &self,
        completion: Completion<'_>,
        diagnostics: &[Diagnostic],
    ) -> io::Result<()> {
        let mut transcript = String::new();
        if let Completion::Failed(message) = completion {
            transcript.push_str(&progress::failure(message, self.palette));
            transcript.push('\n');
        }
        let rendered = self.render_diagnostics(diagnostics);
        if !rendered.is_empty() {
            transcript.push_str(&rendered);
            transcript.push('\n');
        }
        if let Completion::Published(summary) = completion
            && !self.quiet
            && !summary.is_empty()
        {
            transcript.push_str(&progress::summary(summary, self.palette));
            transcript.push('\n');
        }
        self.publish_round(diagnostics, transcript)
    }

    fn publish_round(&self, diagnostics: &[Diagnostic], transcript: String) -> io::Result<()> {
        self.sink.round(crate::dev::round::Round {
            diagnostics: diagnostics.to_vec(),
            transcript,
            observed_at: chrono::Local::now(),
        })
    }

    fn render_diagnostics(&self, diagnostics: &[Diagnostic]) -> String {
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
    pub(crate) fn write_stdout_styled(&self, text: &text::StyledText) -> anyhow::Result<()> {
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
        session::ProcessTerminal.is_interactive()
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
