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
            reused: bool,
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
                    "reused" => self.reused = value,
                    _ => {}
                }
            }
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn fmt::Debug) {}
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let run = crate::dev::round::HookRun {
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
            if fields.reused {
                let _ = self.terminal.activity(&format!("Reused {identity}"));
                return;
            }
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

