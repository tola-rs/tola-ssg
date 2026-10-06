//! What a hook's output means to its author: status lines, live output, failure text.

use anyhow::Result;
use tola_subprocess::{Captured, Observer, Stream, terminating_signal};

use crate::cancellation::is_cancelled;
use crate::config::section::build::hooks::HookStage;
use crate::diagnostic::{Diagnostic, DiagnosticError, Severity};

use std::io;
use std::process::{ExitStatus, Output};
use std::time::Instant;

pub(super) fn render_capture(captured: &Captured) -> Vec<u8> {
    let mut bytes = captured.render(omitted_notice);
    if captured.stopped_early() {
        bytes.extend_from_slice(
            b"\n... Tola stopped reading output while a child command kept the stream open ...",
        );
    }
    bytes
}

pub(super) fn omitted_notice(omitted: u64) -> String {
    format!(
        "... {omitted} byte{} omitted ...",
        if omitted == 1 { "" } else { "s" }
    )
}

/// Report what a hook prints while it runs, tagged with the command and stream.
pub(super) struct HookObserver {
    stdout: Option<HookOutputEmitter>,
    stderr: Option<HookOutputEmitter>,
}

impl HookObserver {
    pub(super) fn new(hook: &str) -> Self {
        Self {
            stdout: HookOutputEmitter::new(hook, Stream::Stdout),
            stderr: HookOutputEmitter::new(hook, Stream::Stderr),
        }
    }

    fn emitter(&mut self, stream: Stream) -> Option<&mut HookOutputEmitter> {
        match stream {
            Stream::Stdout => self.stdout.as_mut(),
            Stream::Stderr => self.stderr.as_mut(),
        }
    }
}

impl Observer for HookObserver {
    fn observes_output(&self) -> bool {
        self.stdout.is_some() || self.stderr.is_some()
    }

    fn output(&mut self, stream: Stream, bytes: &[u8]) {
        if let Some(emitter) = self.emitter(stream) {
            emitter.write(bytes);
        }
    }

    fn finished(&mut self, stream: Stream) {
        if let Some(emitter) = self.emitter(stream) {
            emitter.finish();
        }
    }
}

/// Emit bounded chunks while retaining incomplete UTF-8 only until the next read.
struct HookOutputEmitter {
    hook: String,
    stream: Stream,
    incomplete: Vec<u8>,
}

impl HookOutputEmitter {
    fn new(hook: &str, stream: Stream) -> Option<Self> {
        if !tracing::enabled!(target: "tola::hook_output", tracing::Level::INFO) {
            return None;
        }
        Some(Self {
            hook: hook.to_owned(),
            stream,
            incomplete: Vec::new(),
        })
    }

    fn write(&mut self, bytes: &[u8]) {
        if !tracing::enabled!(target: "tola::hook_output", tracing::Level::INFO) {
            return;
        }
        if self.incomplete.is_empty()
            && let Ok(output) = std::str::from_utf8(bytes)
        {
            self.emit(output);
            return;
        }
        self.incomplete.extend_from_slice(bytes);
        let mut consumed = 0;
        while consumed < self.incomplete.len() {
            match std::str::from_utf8(&self.incomplete[consumed..]) {
                Ok(_) => {
                    consumed = self.incomplete.len();
                }
                Err(error) => {
                    consumed += error.valid_up_to();
                    let Some(invalid_bytes) = error.error_len() else {
                        break;
                    };
                    consumed += invalid_bytes;
                }
            }
        }
        if consumed > 0 {
            self.emit(&String::from_utf8_lossy(&self.incomplete[..consumed]));
            self.incomplete.drain(..consumed);
        }
    }

    fn finish(&self) {
        if !self.incomplete.is_empty() {
            self.emit(&String::from_utf8_lossy(&self.incomplete));
        }
    }

    fn emit(&self, output: &str) {
        tracing::info!(
            target: "tola::hook_output",
            hook = self.hook.as_str(),
            stream = self.stream.name(),
            output,
            "command output"
        );
    }
}

/// How one hook entry ended.
///
/// A cancelled entry is neither a success nor a failure, so the endings travel as separate
/// variants rather than as booleans a caller could set to contradict each other.
#[derive(Clone, Copy)]
pub(super) enum HookOutcome {
    Finished,
    Failed,
    Cancelled,
}

impl HookOutcome {
    fn classify<T>(outcome: &Result<T>) -> Self {
        match outcome {
            Ok(_) => Self::Finished,
            Err(error) if is_cancelled(error) => Self::Cancelled,
            Err(_) => Self::Failed,
        }
    }
}

pub(super) struct HookEntryReport {
    stage: HookStage,
    hook: String,
    started: Instant,
}

impl HookEntryReport {
    pub(super) fn started(stage: HookStage, hook: &str) -> Self {
        tracing::info!(
            target: "tola::hook_status",
            stage = stage.as_str(),
            hook,
            "Running hook"
        );
        Self {
            stage,
            hook: hook.to_owned(),
            started: Instant::now(),
        }
    }

    pub(super) fn finished(self, outcome: HookOutcome) {
        let (success, cancelled) = match outcome {
            HookOutcome::Finished => (true, false),
            HookOutcome::Failed => (false, false),
            HookOutcome::Cancelled => (false, true),
        };
        tracing::info!(
            target: "tola::hook_status",
            stage = self.stage.as_str(),
            hook = self.hook.as_str(),
            duration_seconds = self.started.elapsed().as_secs_f64(),
            success,
            cancelled,
            "Hook finished"
        );
    }
}

/// Declared-output checks run inside `run`, so missing promised files report the
/// entry as failed rather than finished.
pub(super) fn run_hook_entry<T>(
    stage: HookStage,
    hook: &str,
    run: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let report = HookEntryReport::started(stage, hook);
    let outcome = run();
    report.finished(HookOutcome::classify(&outcome));
    outcome
}

#[derive(Debug)]
pub(super) enum CommandFailure {
    /// The command ran and exited unsuccessfully, with its bounded captured streams.
    Exited {
        status: ExitStatus,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    NotStarted {
        program: String,
        reason: StartFailure,
    },
    /// Tola could not complete its own handling of the command.
    Interrupted { step: InterruptionStep },
}

impl std::fmt::Display for CommandFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exited { status, .. } => match status.code() {
                Some(code) => write!(formatter, "the command exited with status {code}"),
                None => {
                    if let Some(signal) = terminating_signal(status) {
                        return write!(formatter, "the command was terminated by signal {signal}");
                    }
                    formatter.write_str("the command terminated without an exit code")
                }
            },
            Self::NotStarted { program, .. } => {
                write!(formatter, "`{program}` could not be started")
            }
            Self::Interrupted { step } => formatter.write_str(step.describe()),
        }
    }
}

impl std::error::Error for CommandFailure {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StartFailure {
    NotFound,
    NotExecutable,
    /// The platform refused for a reason Tola does not name to its author.
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InterruptionStep {
    Capture,
    Read,
    Wait,
    Run,
}

impl InterruptionStep {
    fn describe(self) -> &'static str {
        match self {
            Self::Capture => "Tola could not capture what the command prints",
            Self::Read => "Tola could not read what the command printed",
            Self::Wait => "Tola could not wait for the command to finish",
            Self::Run => "Tola could not run the command",
        }
    }
}

pub(super) fn start_failure(program: &str, kind: io::ErrorKind) -> CommandFailure {
    let reason = match kind {
        io::ErrorKind::NotFound => StartFailure::NotFound,
        io::ErrorKind::PermissionDenied => StartFailure::NotExecutable,
        _ => StartFailure::Unavailable,
    };
    CommandFailure::NotStarted {
        program: program.to_owned(),
        reason,
    }
}

pub(super) fn check_exit(output: Output) -> Result<Output> {
    if output.status.success() {
        return Ok(output);
    }
    Err(anyhow::Error::new(CommandFailure::Exited {
        status: output.status,
        stdout: output.stdout,
        stderr: output.stderr,
    }))
}

/// Bounded command streams travel as notes, preserving the command's line breaks
/// instead of folding them into the diagnostic header.
pub(super) fn hook_command_error(
    stage: HookStage,
    identity: &str,
    error: anyhow::Error,
) -> anyhow::Error {
    let failure = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<CommandFailure>());
    let message = failure_message(identity, failure);
    let mut diagnostic = Diagnostic::new(diagnostic_code(stage), Severity::Error, message.clone());
    for note in failure_notes(failure) {
        diagnostic = diagnostic.with_note(note);
    }
    let diagnostic = diagnostic.with_help(failure_help(stage, failure));
    anyhow::Error::new(DiagnosticError::attach(
        error.context(message),
        vec![diagnostic],
    ))
}

/// Tola could not create the cache directory one hook identity owns, so its command never
/// starts; the author reads the affected hook, never the site's absolute paths.
pub(super) fn cache_directory_error(
    stage: HookStage,
    identity: &str,
    cause: std::io::Error,
) -> anyhow::Error {
    let message = format!("Tola could not create the cache directory for {identity}");
    let diagnostic = Diagnostic::new(diagnostic_code(stage), Severity::Error, message.clone())
        .with_help("Check that `.tola` is writable");
    anyhow::Error::new(DiagnosticError::attach(
        anyhow::Error::new(cause).context(message),
        vec![diagnostic],
    ))
}

fn diagnostic_code(stage: HookStage) -> crate::diagnostic::DiagnosticCode {
    match stage {
        // Consumer failure cannot undo the revision already committed.
        HookStage::AfterPublish => crate::codes::hook::AFTER_PUBLISH,
        _ => crate::codes::hook::COMMAND,
    }
}

fn failure_message(identity: &str, failure: Option<&CommandFailure>) -> String {
    match failure {
        Some(failure @ CommandFailure::Exited { .. }) => format!("{identity} failed: {failure}"),
        Some(CommandFailure::NotStarted { program, reason }) => {
            let reason = match reason {
                StartFailure::NotFound => "; the program was not found",
                StartFailure::NotExecutable => "; the program is not executable",
                StartFailure::Unavailable => "",
            };
            format!("{identity} could not start `{program}`{reason}")
        }
        Some(CommandFailure::Interrupted { step }) => {
            format!("{identity}: {}", step.describe())
        }
        None => format!("{identity} could not be run"),
    }
}

fn failure_notes(failure: Option<&CommandFailure>) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(CommandFailure::Exited { stdout, stderr, .. }) = failure {
        for (stream, bytes) in [("stderr", stderr), ("stdout", stdout)] {
            let text = String::from_utf8_lossy(bytes);
            let text = text.trim();
            if !text.is_empty() {
                notes.push(format!("{stream}:\n{}", bounded_child_output(text)));
            }
        }
    }
    notes
}

fn failure_help(stage: HookStage, failure: Option<&CommandFailure>) -> String {
    let key = format!("build.hooks.{}", stage.as_str());
    match failure {
        Some(CommandFailure::NotStarted {
            program,
            reason: StartFailure::NotFound,
        }) => format!("Install `{program}`, or correct `command` in `{key}`"),
        Some(CommandFailure::NotStarted {
            program,
            reason: StartFailure::NotExecutable,
        }) => format!("Make `{program}` executable, or correct `command` in `{key}`"),
        Some(CommandFailure::Interrupted { .. }) => "Run the command outside Tola".to_owned(),
        _ => format!("Fix the command, or remove the hook from `{key}`"),
    }
}

pub(super) fn format_cancelled_output(hook: &str, stdout: &[u8], stderr: &[u8]) -> String {
    let mut message = format!("Tola stopped `{hook}` before it finished");
    let stderr = String::from_utf8_lossy(stderr);
    if !stderr.trim().is_empty() {
        message.push_str("\nStderr:\n");
        message.push_str(stderr.trim());
    }
    let stdout = String::from_utf8_lossy(stdout);
    if !stdout.trim().is_empty() {
        message.push_str("\nStdout:\n");
        message.push_str(stdout.trim());
    }
    message
}

fn bounded_child_output(output: &str) -> String {
    const LIMIT: usize = 16 * 1024;
    if output.len() <= LIMIT {
        return output.to_owned();
    }

    let half = LIMIT / 2;
    let head_end = (0..=half)
        .rev()
        .find(|offset| output.is_char_boundary(*offset))
        .unwrap_or(0);
    let tail_start = (output.len() - half..output.len())
        .find(|offset| output.is_char_boundary(*offset))
        .unwrap_or(output.len());
    format!(
        "{}\n{}\n{}",
        &output[..head_end],
        omitted_notice(tail_start.saturating_sub(head_end) as u64),
        &output[tail_start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::hooks::command::{HookCall, HookDirectories, HookInvocation};
    use std::sync::mpsc;

    type ReportedOutput = (String, String, String);

    #[cfg(unix)]
    fn hook_invocation<'a>(
        arguments: &[&str],
        site_root: &'a std::path::Path,
    ) -> HookInvocation<'a> {
        HookInvocation::new(
            arguments,
            HookCall {
                site_root,
                name: "styles",
                mode: crate::mode::BuildMode::Production,
                directories: HookDirectories::BeforeBuild,
            },
        )
        .unwrap()
    }

    #[cfg(unix)]
    fn failed_command(arguments: &[&str]) -> Diagnostic {
        let directory = tempfile::tempdir().unwrap();
        let invocation = hook_invocation(arguments, directory.path());
        let error = invocation.run(None).unwrap_err();
        let stage = HookStage::BeforeBuild;
        let error = hook_command_error(stage, &crate::hooks::hook_identity(stage, "styles"), error);
        crate::diagnostic::attached(&error)
            .expect("a failed command has its diagnostic")
            .first()
            .expect("a failed command reports one diagnostic")
            .clone()
    }

    /// A failed command's notes hold each stream's own text, bounded to the note cap.
    #[cfg(unix)]
    #[test]
    fn failed_command_notes_bound_stream_text() {
        enum Expected {
            /// Both streams exceed the cap: each note keeps its head and tail.
            Bounded,
            /// A payload under the cap is kept exactly.
            Verbatim(&'static str),
        }
        let cases = [
            (
                "printf stderr-head >&2; yes x | head -c 100000 >&2; printf stderr-tail >&2; printf stdout-head; yes y | head -c 120000; printf stdout-tail; exit 1",
                Expected::Bounded,
            ),
            (
                r#"printf '{"error":"detail"}'; exit 1"#,
                Expected::Verbatim("{\"error\":\"detail\"}"),
            ),
            (
                "printf '<!DOCTYPE html><title>failure</title>'; exit 1",
                Expected::Verbatim("<!DOCTYPE html><title>failure</title>"),
            ),
        ];
        for (payload, expected) in cases {
            let diagnostic = failed_command(&["sh", "-c", payload]);

            assert_eq!(diagnostic.code, "hook.command");
            match expected {
                Expected::Bounded => {
                    assert_eq!(diagnostic.notes.len(), 2, "{:?}", diagnostic.notes);
                    for (note, stream) in diagnostic.notes.iter().zip(["stderr", "stdout"]) {
                        assert!(note.len() <= 16 * 1024);
                        assert!(note.contains(&format!("{stream}-head")));
                        assert!(note.ends_with(&format!("{stream}-tail")));
                    }
                }
                Expected::Verbatim(text) => {
                    assert_eq!(diagnostic.notes, [format!("stdout:\n{text}")]);
                }
            }
        }
    }

    /// A command killed by a signal names the terminating signal in its diagnostic.
    #[cfg(unix)]
    #[test]
    fn failed_command_names_the_terminating_signal() {
        let diagnostic = failed_command(&["sh", "-c", "kill -KILL $$"]);

        assert!(
            diagnostic
                .message
                .contains("the command was terminated by signal 9"),
            "{}",
            diagnostic.message
        );
    }

    #[cfg(unix)]
    #[test]
    fn streamed_output_reports_its_stream() {
        let (sender, received) = mpsc::channel();
        let output = tracing::subscriber::with_default(HookStreams { outputs: sender }, || {
            let directory = tempfile::tempdir().unwrap();
            let invocation = HookInvocation::new(
                &["sh", "-c", "printf 'ready\\n' >&2"],
                HookCall {
                    site_root: directory.path(),
                    name: "assets",
                    mode: crate::mode::BuildMode::Production,
                    directories: HookDirectories::BeforeBuild,
                },
            )
            .unwrap();
            invocation.run(None).unwrap()
        });

        assert_eq!(output.stderr, b"ready\n");
        assert_eq!(
            received.try_iter().collect::<Vec<_>>(),
            [(
                "assets".to_owned(),
                "stderr".to_owned(),
                "ready\n".to_owned()
            )]
        );
    }

    #[test]
    fn streamed_output_buffers_split_utf8() {
        let (sender, received) = mpsc::channel();
        tracing::subscriber::with_default(HookStreams { outputs: sender }, || {
            let mut emitter = HookOutputEmitter::new("assets", Stream::Stdout).unwrap();
            emitter.write(&[0xe4, 0xbd]);
            assert_eq!(received.try_iter().count(), 0);
            emitter.write(&[0xa0, 0xff, 0xe5]);
            emitter.write(&[0xa5, 0xbd, 0xf0]);
            emitter.finish();
        });
        let combined = received
            .try_iter()
            .map(|(_, _, output)| output)
            .collect::<String>();
        assert_eq!(combined, "你\u{fffd}好\u{fffd}");
    }

    #[derive(Default)]
    struct ReceivedOutput {
        hook: String,
        stream: String,
        output: String,
    }

    impl tracing::field::Visit for ReceivedOutput {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            match field.name() {
                "hook" => self.hook = value.to_owned(),
                "stream" => self.stream = value.to_owned(),
                "output" => self.output = value.to_owned(),
                _ => {}
            }
        }

        fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
    }

    struct HookStreams {
        outputs: mpsc::Sender<ReportedOutput>,
    }

    impl tracing::Subscriber for HookStreams {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            metadata.target() == "tola::hook_output"
        }

        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut output = ReceivedOutput::default();
            event.record(&mut output);
            let _ = self
                .outputs
                .send((output.hook, output.stream, output.output));
        }
    }
}
