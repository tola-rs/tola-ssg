//! Tracing filters and terminal/file output.

use std::fmt;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tracing::{Event, Metadata, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::format::{JsonFields, Writer};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, FormattedFields};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::{LookupSpan, SpanRef};

use super::LogFile;
use crate::terminal::{OutputSink, Palette, Terminal};

pub(crate) fn install(verbosity: u8, terminal: Terminal, log: Option<LogFile>) -> Result<()> {
    let rust_log = match std::env::var("RUST_LOG") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => return Err(error).context("`RUST_LOG` is not valid UTF-8"),
    };
    subscriber(verbosity, terminal, log, rust_log.as_deref())?
        .try_init()
        .context("command logging could not start")
}

fn subscriber(
    verbosity: u8,
    terminal: Terminal,
    log: Option<LogFile>,
    rust_log: Option<&str>,
) -> Result<tracing::Dispatch> {
    let filter = console_filter(verbosity, rust_log)?;
    let sink = terminal.sink();
    let palette = terminal.palette();
    let show_hooks = !terminal.is_quiet();
    let console = tracing_subscriber::fmt::layer()
        .with_ansi(palette.uses_color())
        .event_format(TraceEventFormat {
            palette,
            started: Instant::now(),
        })
        .with_writer(move || writer(sink.clone()))
        .with_filter(filter)
        .with_filter(filter_fn(is_console_event));
    let file = log.map(|log| {
        tracing_subscriber::fmt::layer()
            .json()
            .event_format(JsonRecordFormat)
            .with_writer(move || log.writer())
            .with_filter(file_filter(verbosity))
    });
    let subscriber = tracing_subscriber::registry()
        .with(console)
        .with(
            HookOutput { terminal }.with_filter(filter_fn(move |metadata| {
                show_hooks
                    && ((metadata.is_span() && metadata.target() == "tola::dev")
                        || metadata.target() == "tola::hook_status"
                        || (verbosity > 0 && metadata.target() == "tola::hook_output"))
            })),
        )
        .with(file);
    Ok(tracing::Dispatch::new(subscriber))
}

fn console_filter(verbosity: u8, rust_log: Option<&str>) -> Result<EnvFilter> {
    let default = match verbosity {
        0 => "off",
        _ => workspace_filter(verbosity),
    };
    match rust_log.filter(|value| !value.trim().is_empty()) {
        Some(filter) => EnvFilter::try_new(filter).context("`RUST_LOG` is not a valid filter"),
        None => Ok(EnvFilter::new(default)),
    }
}

/// Default filter for workspace events.
///
/// File logs always record them; the console shows them from the first `-v`. One table serves
/// both, so the two thresholds cannot disagree.
fn workspace_filter(verbosity: u8) -> &'static str {
    if verbosity >= 2 {
        "off,tola=trace,tola_typst=trace"
    } else {
        "off,tola=debug,tola_typst=debug"
    }
}

fn file_filter(verbosity: u8) -> EnvFilter {
    EnvFilter::new(workspace_filter(verbosity))
}

fn is_console_event(metadata: &Metadata<'_>) -> bool {
    !matches!(
        metadata.target(),
        "tola::terminal" | "tola::hook_output" | "tola::hook_status"
    )
}

pub(crate) struct TraceWriter {
    sink: OutputSink,
    bytes: Vec<u8>,
}

pub(crate) fn writer(sink: OutputSink) -> TraceWriter {
    TraceWriter {
        sink,
        bytes: Vec::new(),
    }
}

impl Write for TraceWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for TraceWriter {
    fn drop(&mut self) {
        if !self.bytes.is_empty() {
            let rendered = normalize_trace_event(&self.bytes);
            let _ = self.sink.write_trace(rendered.as_bytes());
        }
    }
}

fn normalize_trace_event(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_end_matches(['\r', '\n']);
    let mut rendered = crate::terminal::text::indent_continuation_lines(text);
    rendered.push('\n');
    rendered
}

/// Formats one `tracing` event as a record line under the file's shared envelope.
struct JsonRecordFormat;

impl<S> FormatEvent<S, JsonFields> for JsonRecordFormat
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn format_event(
        &self,
        context: &FmtContext<'_, S, JsonFields>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let mut fields = String::new();
        JsonFields::new().format_fields(Writer::new(&mut fields), event)?;
        let spans = context.event_scope().map(|scope| {
            scope
                .from_root()
                .map(|span| span_json(&span))
                .collect::<Vec<_>>()
        });
        let line = super::record_line(
            event.metadata().level().as_str(),
            event.metadata().target(),
            spans.as_deref(),
            &fields,
        );
        writer.write_str(&line)
    }
}

/// One span's recorded fields, which the JSON field formatter wrote as an object.
fn span_json<S>(span: &SpanRef<'_, S>) -> String
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    match span.extensions().get::<FormattedFields<JsonFields>>() {
        Some(fields) if !fields.is_empty() => fields.to_string(),
        _ => "{}".to_owned(),
    }
}

struct TraceEventFormat {
    palette: Palette,
    started: Instant,
}

impl<S, N> FormatEvent<S, N> for TraceEventFormat
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        _context: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let metadata = event.metadata();
        let level = self.palette.level_word(*metadata.level());
        write!(
            writer,
            "{:.3}s {} {}: ",
            self.started.elapsed().as_secs_f64(),
            level,
            short_target(metadata.target())
        )?;
        let mut fields = ConsoleFields::default();
        event.record(&mut fields);
        if let Some(message) = fields.message {
            write!(writer, "{message}")?;
        }
        for (name, value) in fields.values {
            write!(writer, " {name}={value}")?;
        }
        writeln!(writer)
    }
}

#[derive(Default)]
struct ConsoleFields {
    message: Option<String>,
    values: Vec<(String, String)>,
}

impl tracing::field::Visit for ConsoleFields {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(crate::terminal::text::multiline(value));
        } else {
            self.values.push((
                crate::terminal::text::single_line(field.name()),
                crate::terminal::text::single_line(value),
            ));
        }
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        self.record_str(field, &format!("{value:?}"));
    }
}

struct HookOutput {
    terminal: Terminal,
}

impl<S: Subscriber + for<'lookup> LookupSpan<'lookup>> tracing_subscriber::Layer<S> for HookOutput {
    fn on_event(&self, event: &Event<'_>, context: tracing_subscriber::layer::Context<'_, S>) {
        #[derive(Default)]
        struct Fields {
            stage: String,
            hook: String,
            stream: String,
            output: String,
            duration_seconds: Option<f64>,
            success: bool,
            cancelled: bool,
        }
        impl tracing::field::Visit for Fields {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                match field.name() {
                    "stage" => self.stage = value.to_owned(),
                    "hook" => self.hook = value.to_owned(),
                    "stream" => self.stream = value.to_owned(),
                    "output" => self.output.push_str(value),
                    _ => {}
                }
            }
            fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
                if field.name() == "duration_seconds" {
                    self.duration_seconds = Some(value);
                }
            }
            fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
                match field.name() {
                    "success" => self.success = value,
                    "cancelled" => self.cancelled = value,
                    _ => {}
                }
            }
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn fmt::Debug) {}
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let run = crate::terminal::development::HookRun {
            scope: context
                .event_scope(event)
                .and_then(|scope| scope.from_root().last())
                .map_or(0, |span| span.id().into_u64()),
            name: crate::terminal::text::single_line(&fields.hook),
        };
        let sink = self.terminal.sink();
        if event.metadata().target() == "tola::hook_status" {
            let palette = self.terminal.palette();
            let stage = crate::terminal::text::single_line(&fields.stage);
            let hook = crate::terminal::text::single_line(&fields.hook);
            let identity = format!(
                "{}/{}",
                palette.hook_stage(&stage),
                palette.hook_name(&hook)
            );
            match fields.duration_seconds {
                Some(seconds) => {
                    if fields.cancelled {
                        // The command boundary prints the one cancellation line for the whole
                        // build, so a cancelled hook adds none of its own.
                        let _ = sink.hook_finished(&run);
                        return;
                    }
                    let duration =
                        crate::terminal::format_duration(Duration::from_secs_f64(seconds));
                    let (result, rest) = if fields.success {
                        ("Finished", format!("{identity} in {duration}"))
                    } else {
                        ("Failed", format!("{identity} after {duration}"))
                    };
                    let result = palette.hook_result(fields.success, result);
                    if matches!(sink.hook_finished(&run), Ok(false)) {
                        let _ = self.terminal.activity(&format!("{result} {rest}"));
                    }
                }
                None => {
                    if matches!(sink.hook_started(run, &stage), Ok(false)) {
                        let _ = self.terminal.activity(&format!("Running {identity}"));
                    }
                }
            }
            return;
        }
        let _ = self
            .terminal
            .hook_output(&run, &fields.stream, &fields.output);
    }
}

fn short_target(target: &str) -> &str {
    target
        .strip_prefix("tola::")
        .unwrap_or(target)
        .split("::")
        .next()
        .unwrap_or(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::log::record_bytes;
    use crate::cli::log::tests::normalized_record;

    /// The record's field names, independent of their written order.
    fn record_keys(record: &serde_json::Value) -> Vec<&str> {
        let mut keys = record
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        keys
    }

    /// A quiet command output that logs the session at `path` and keeps `max_warnings` warnings on
    /// its console, paired with the dispatcher that routes tracing into it.
    fn quiet_session(
        sink: OutputSink,
        path: &std::path::Path,
        max_warnings: usize,
    ) -> (crate::cli::output::CommandOutput, tracing::Dispatch) {
        let terminal = Terminal::with_sink(sink, false, true);
        terminal.set_diagnostic_limits(None, Some(max_warnings));
        let log = LogFile::prepare(path, |_, error| panic!("log write failed: {error}")).unwrap();
        log.record(
            "INFO",
            "tola::terminal",
            serde_json::json!({"kind": "command_started"}),
        );
        log.start().unwrap();
        let subscriber = subscriber(1, terminal.clone(), Some(log.clone()), None).unwrap();
        (
            crate::cli::output::CommandOutput::new(terminal, Some(log)),
            subscriber,
        )
    }

    /// The log file one command writes while `emit` runs through tracing's file layer.
    fn recorded_file(emit: impl FnOnce()) -> String {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let log = LogFile::prepare(&path, |_, error| panic!("log write failed: {error}")).unwrap();
        log.start().unwrap();
        let (sink, _) = OutputSink::buffered();
        let subscriber =
            subscriber(1, Terminal::with_sink(sink, false, false), Some(log), None).unwrap();
        tracing::dispatcher::with_default(&subscriber, emit);
        std::fs::read_to_string(&path).unwrap()
    }

    #[test]
    fn tracing_record_keeps_envelope_bytes() {
        let source = recorded_file(|| {
            tracing::info!(target: "tola::test", detail = "value", "pinned event");
        });
        let line = normalized_record(source.as_bytes());
        assert_eq!(
            line,
            "{\"timestamp\":\"<timestamp>\",\"level\":\"INFO\",\"target\":\"tola::test\",\"pid\":<pid>,\"fields\":{\"detail\":\"value\",\"message\":\"pinned event\"}}\n"
        );
        // Both producers share one envelope, so the command record of the same event writes the
        // same bytes tracing's file layer did.
        assert_eq!(
            line,
            normalized_record(&record_bytes(
                "INFO",
                "tola::test",
                serde_json::json!({"detail": "value", "message": "pinned event"}),
            ))
        );
    }

    #[test]
    fn spanned_record_keeps_envelope_bytes() {
        let source = recorded_file(|| {
            let span = tracing::debug_span!(target: "tola::compile", "build", revision = "one");
            let _entered = span.enter();
            tracing::debug!(target: "tola::compile", reused = true, "pinned event");
        });
        assert_eq!(
            normalized_record(source.as_bytes()),
            "{\"timestamp\":\"<timestamp>\",\"level\":\"DEBUG\",\"target\":\"tola::compile\",\"pid\":<pid>,\"span\":{\"revision\":\"one\"},\"spans\":[{\"revision\":\"one\"}],\"fields\":{\"message\":\"pinned event\",\"reused\":true}}\n"
        );
    }

    #[test]
    fn console_level_words_keep_their_styles() {
        let (sink, output) = OutputSink::buffered();
        let subscriber = subscriber(2, Terminal::with_sink(sink, true, false), None, None).unwrap();
        tracing::dispatcher::with_default(&subscriber, || {
            tracing::error!(target: "tola::test", "error message");
            tracing::warn!(target: "tola::test", "warn message");
            tracing::info!(target: "tola::test", "info message");
            tracing::debug!(target: "tola::test", "debug message");
            tracing::trace!(target: "tola::test", "trace message");
        });
        let shown = String::from_utf8(output.bytes()).unwrap();
        for (word, styled) in [
            ("error", "\u{1b}[1m\u{1b}[31merror\u{1b}[39m\u{1b}[0m"),
            ("warn", "\u{1b}[1m\u{1b}[33mwarn\u{1b}[39m\u{1b}[0m"),
            ("info", "\u{1b}[1m\u{1b}[32minfo\u{1b}[39m\u{1b}[0m"),
            ("debug", "\u{1b}[1m\u{1b}[36mdebug\u{1b}[39m\u{1b}[0m"),
            ("trace", "\u{1b}[2mtrace\u{1b}[0m"),
        ] {
            assert!(
                shown.contains(&format!("{styled} test: {word} message")),
                "{word}: {shown:?}"
            );
        }
    }

    #[test]
    fn hook_results_keep_their_styles() {
        let (sink, output) = OutputSink::buffered();
        let subscriber = subscriber(1, Terminal::with_sink(sink, true, false), None, None).unwrap();
        tracing::dispatcher::with_default(&subscriber, || {
            for success in [true, false] {
                tracing::info!(
                    target: "tola::hook_status",
                    stage = "generate-outputs",
                    hook = "search",
                    duration_seconds = 0.5,
                    success = success,
                    "Hook finished"
                );
            }
        });
        let shown = String::from_utf8(output.bytes()).unwrap();
        assert!(
            shown.contains(concat!(
                "\u{1b}[1m\u{1b}[32mFinished\u{1b}[39m\u{1b}[0m ",
                "\u{1b}[35mgenerate-outputs\u{1b}[39m/\u{1b}[36msearch\u{1b}[39m in 500 ms"
            )),
            "{shown:?}"
        );
        assert!(
            shown.contains(concat!(
                "\u{1b}[1m\u{1b}[31mFailed\u{1b}[39m\u{1b}[0m ",
                "\u{1b}[35mgenerate-outputs\u{1b}[39m/\u{1b}[36msearch\u{1b}[39m after 500 ms"
            )),
            "{shown:?}"
        );
    }

    #[test]
    fn multiline_events_stay_one_block() {
        assert_eq!(
            normalize_trace_event(b"debug exec: output=first\nsecond\n"),
            "debug exec: output=first\n  second\n"
        );
    }

    #[test]
    fn invalid_rust_log_filter_is_rejected() {
        let error = console_filter(0, Some("tola=invalid-level")).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("`RUST_LOG` is not a valid filter")
        );
    }

    #[test]
    fn disabled_color_still_escapes_controls() {
        let (sink, output) = OutputSink::buffered();
        let subscriber =
            subscriber(1, Terminal::with_sink(sink, false, false), None, None).unwrap();
        tracing::dispatcher::with_default(&subscriber, || {
            tracing::debug!(target: "tola::compile", source = "document.typ", detail = "\u{1b}[31m", "compiled");
        });
        let output = String::from_utf8(output.bytes()).unwrap();
        assert!(output.contains("source=document.typ"));
        assert!(output.contains("\\x1b[31m"));
        assert!(!output.contains('\u{1b}'));
    }

    #[test]
    fn script_output_stays_in_log() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let log = LogFile::prepare(&path, |_, error| panic!("log write failed: {error}")).unwrap();
        log.start().unwrap();
        let (sink, output) = OutputSink::buffered();
        let subscriber =
            subscriber(0, Terminal::with_sink(sink, false, false), Some(log), None).unwrap();
        tracing::dispatcher::with_default(&subscriber, || {
            tracing::info!(target: "tola::hook_output", hook = "assets", stream = "stdout", output = "script stdout");
            tracing::info!(target: "tola::hook_output", hook = "assets", stream = "stderr", output = "script stderr");
        });
        let shown = String::from_utf8(output.bytes()).unwrap();
        assert!(!shown.contains("script stdout"));
        assert!(!shown.contains("script stderr"));
        let source = std::fs::read_to_string(path).unwrap();
        let records = source
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert!(records.iter().any(|record| {
            record["target"] == "tola::hook_output" && record["fields"]["output"] == "script stdout"
        }));
        assert!(records.iter().any(|record| {
            record["target"] == "tola::hook_output" && record["fields"]["output"] == "script stderr"
        }));
    }

    #[test]
    fn verbose_output_escapes_controls() {
        let (sink, output) = OutputSink::buffered();
        let subscriber =
            subscriber(1, Terminal::with_sink(sink, false, false), None, None).unwrap();
        tracing::dispatcher::with_default(&subscriber, || {
            tracing::info!(target: "tola::hook_output", hook = "assets", stream = "stdout", output = "first\n");
            assert!(String::from_utf8_lossy(&output.bytes()).contains("[assets] first"));
            tracing::info!(target: "tola::hook_output", hook = "assets", stream = "stderr", output = "\u{1b}[31mfailed\n");
        });
        let shown = String::from_utf8(output.bytes()).unwrap();
        assert!(shown.contains("[assets stderr] failed"), "{shown:?}");
        assert!(!shown.contains('\u{1b}'));
    }

    #[test]
    fn overlapping_hooks_keep_separate_lines() {
        let (sink, output) = OutputSink::buffered();
        let subscriber = subscriber(
            1,
            Terminal::with_sink(sink, false, false),
            None,
            Some("off"),
        )
        .unwrap();
        tracing::dispatcher::with_default(&subscriber, || {
            let first = tracing::info_span!(target: "tola::dev", "round", round = 1);
            let second = tracing::info_span!(target: "tola::dev", "round", round = 2);
            let emit = |output: &str| {
                tracing::info!(target: "tola::hook_output", hook = "assets", stream = "stdout", output);
            };
            first.in_scope(|| emit("first"));
            second.in_scope(|| emit("second"));
            first.in_scope(|| {
                tracing::info!(target: "tola::hook_status", stage = "before-build", hook = "assets", duration_seconds = 0.1, success = false, cancelled = true);
            });
            second.in_scope(|| emit(" continuation\n"));
            first.in_scope(|| emit("ending\n"));
        });
        assert_eq!(
            String::from_utf8(output.bytes()).unwrap(),
            "[assets] first\n[assets] second continuation\n[assets] ending\n",
        );
    }

    #[test]
    fn file_debug_events_stay_off_console() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let (sink, output) = OutputSink::buffered();
        let log = LogFile::prepare(&path, |_, error| panic!("log write failed: {error}")).unwrap();
        let subscriber = subscriber(
            0,
            Terminal::with_sink(sink, false, false),
            Some(log.clone()),
            None,
        )
        .unwrap();
        log.record(
            "INFO",
            "tola::terminal",
            serde_json::json!({"kind": "command_started"}),
        );
        log.start().unwrap();
        tracing::dispatcher::with_default(&subscriber, || {
            let span = tracing::debug_span!(target: "tola::compile", "build", revision = "one");
            let _entered = span.enter();
            tracing::debug!(target: "tola::compile", reused = true, "first\nsecond");
            tracing::trace!(target: "tola::compile", "trace remains opt-in");
        });
        assert!(output.is_empty());
        let source = std::fs::read_to_string(path).unwrap();
        let records = source
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1]["level"], "DEBUG");
        assert_eq!(records[1]["fields"]["reused"], true);
        assert_eq!(records[1]["span"]["revision"], "one");
        assert!(records[1]["timestamp"].as_str().unwrap().ends_with('Z'));
        for record in &records {
            assert_eq!(record["pid"], std::process::id());
        }
        assert_eq!(
            record_keys(&records[0]),
            ["fields", "level", "pid", "target", "timestamp"]
        );
        assert_eq!(
            record_keys(&records[1]),
            [
                "fields",
                "level",
                "pid",
                "span",
                "spans",
                "target",
                "timestamp"
            ]
        );
    }

    #[test]
    fn held_console_keeps_file_events() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let log = LogFile::prepare(&path, |_, error| panic!("log write failed: {error}")).unwrap();
        log.start().unwrap();
        let (sink, captured) = OutputSink::buffered();
        let subscriber = subscriber(
            1,
            Terminal::with_sink(sink.clone(), false, false),
            Some(log),
            None,
        )
        .unwrap();
        sink.with_stderr_lock(|_| -> io::Result<()> {
            tracing::dispatcher::with_default(&subscriber, || {
                tracing::debug!(target: "tola::compile", "preview became ready");
            });
            Ok(())
        })
        .unwrap();
        assert!(captured.is_empty());
        let recorded: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(path).unwrap().trim()).unwrap();
        assert_eq!(recorded["fields"]["message"], "preview became ready");
    }

    #[test]
    fn quiet_console_keeps_limited_diagnostics() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let (sink, output) = OutputSink::buffered();
        let (messages, subscriber) = quiet_session(sink, &path, 1);

        tracing::dispatcher::with_default(&subscriber, || {
            messages.status("Building site").unwrap();
            tracing::info!(target: "tola::hook_status", stage = "generate-outputs", hook = "search", "Running hook");
            tracing::info!(target: "tola::hook_status", stage = "generate-outputs", hook = "search", duration_seconds = 0.5, success = false, "Hook finished");
            tracing::info!(target: "tola::hook_output", hook = "search", stream = "stdout", output = "script output");
            messages
                .diagnostics(&[
                    tola_build::diagnostic::Diagnostic::new(
                        tola_build::codes::typst::COMPILE,
                        tola_build::diagnostic::Severity::Warning,
                        "first warning",
                    ),
                    tola_build::diagnostic::Diagnostic::new(
                        tola_build::codes::config::INVALID,
                        tola_build::diagnostic::Severity::Warning,
                        "second warning",
                    ),
                    tola_build::diagnostic::Diagnostic::new(
                        tola_build::codes::hook::COMMAND,
                        tola_build::diagnostic::Severity::Error,
                        "command `search` failed\nStderr:\nmissing index\nStdout:\npartial result",
                    ),
                ])
                .unwrap();
        });
        let shown = String::from_utf8(output.bytes()).unwrap();
        assert!(!shown.contains("Building site"));
        assert!(!shown.contains("script output"));
        assert!(!shown.contains("Running generate-outputs/"));
        assert!(!shown.contains("Finished generate-outputs hook"));
        assert!(shown.contains("first warning"));
        assert!(!shown.contains("second warning"));
        assert!(shown.contains("missing index"));
        assert!(shown.contains("partial result"));
        assert!(shown.contains("1 warning not shown"));
    }

    #[test]
    fn session_log_keeps_every_record() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let (sink, _) = OutputSink::buffered();
        let (messages, subscriber) = quiet_session(sink, &path, 1);

        tracing::dispatcher::with_default(&subscriber, || {
            messages.status("Building site").unwrap();
            tracing::info!(target: "tola::hook_status", stage = "generate-outputs", hook = "search", duration_seconds = 0.5, success = false, "Hook finished");
            tracing::info!(target: "tola::hook_output", hook = "search", stream = "stdout", output = "script output");
            messages
                .diagnostics(&[
                    tola_build::diagnostic::Diagnostic::new(
                        tola_build::codes::typst::COMPILE,
                        tola_build::diagnostic::Severity::Warning,
                        "first warning",
                    ),
                    tola_build::diagnostic::Diagnostic::new(
                        tola_build::codes::config::INVALID,
                        tola_build::diagnostic::Severity::Warning,
                        "second warning",
                    ),
                    tola_build::diagnostic::Diagnostic::new(
                        tola_build::codes::hook::COMMAND,
                        tola_build::diagnostic::Severity::Error,
                        "command `search` failed\nStderr:\nmissing index\nStdout:\npartial result",
                    ),
                ])
                .unwrap();
            messages
                .write_stdout_line("{\"machine_output\":true}")
                .unwrap();
        });
        let source = std::fs::read_to_string(path).unwrap();
        let records = source
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            records
                .iter()
                .filter(|record| record["fields"]["kind"] == "diagnostic")
                .count(),
            3
        );
        assert!(
            records
                .iter()
                .any(|record| record["fields"]["diagnostic"]["code"]
                    == serde_json::json!(tola_build::codes::config::INVALID))
        );
        assert!(records.iter().any(|record| {
            record["fields"]["diagnostic"]["code"]
                == serde_json::json!(tola_build::codes::hook::COMMAND)
                && record["fields"]["diagnostic"]["message"]
                    .as_str()
                    .is_some_and(|message| {
                        message.contains("missing index") && message.contains("partial result")
                    })
        }));
        assert!(records.iter().any(|record| {
            record["target"] == "tola::hook_output" && record["fields"]["output"] == "script output"
        }));
        assert!(records.iter().any(|record| {
            record["target"] == "tola::hook_status"
                && record["fields"]["hook"] == "search"
                && record["fields"]["duration_seconds"] == 0.5
                && record["fields"]["success"] == false
        }));
        assert!(!source.contains("machine_output"));
    }
}
