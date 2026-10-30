//! Ordered terminal writes and development presentation for one command.

use std::io::{self, Write};
use std::sync::{Arc, OnceLock};

use super::Palette;
use super::ansi::{self, Painter};
use super::development::{DevScreen, HookRun, Round, stream_index};
use super::session::{Claim, Holder, ProcessTerminal, Terminal as _};

const OUTPUT_LOCK_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

#[derive(Clone)]
pub(crate) struct OutputSink {
    inner: Arc<SinkInner>,
}

struct SinkInner {
    lock: parking_lot::Mutex<Presentation>,
    stderr: StreamTarget,
    stdout: StreamTarget,
    cancellation: OnceLock<tola_build::cancellation::BuildCancellation>,
}

#[derive(Default)]
struct Presentation {
    hook: Option<HookLine>,
    hook_text: Vec<HookText>,
    development: Option<DevScreen>,
}

struct HookLine {
    run: HookRun,
    stream: usize,
    painter: Painter,
}

/// One hook execution's text, interpreted per stream while its command runs.
struct HookText {
    run: HookRun,
    streams: [ansi::Parser; 2],
}

impl HookText {
    fn new(run: HookRun) -> Self {
        Self {
            run,
            streams: [ansi::Parser::new(), ansi::Parser::new()],
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("standard output was closed before writing finished")]
pub(crate) struct StdoutClosed;

#[derive(Clone)]
enum StreamTarget {
    Stderr,
    Stdout,
    Buffered(BufferedOutput),
}

/// Frame writes hold only their destination, so a renderer cannot retain its owning sink.
#[derive(Clone)]
pub(crate) struct StreamWriter(StreamTarget);

impl Write for StreamWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Default)]
pub(crate) struct BufferedOutput(Arc<std::sync::Mutex<Vec<u8>>>);

#[allow(dead_code)]
impl BufferedOutput {
    pub(crate) fn bytes(&self) -> Vec<u8> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }
}

impl OutputSink {
    pub(crate) fn process() -> Self {
        Self::new(StreamTarget::Stderr, StreamTarget::Stdout)
    }

    fn new(stderr: StreamTarget, stdout: StreamTarget) -> Self {
        Self {
            inner: Arc::new(SinkInner {
                lock: parking_lot::Mutex::new(Presentation::default()),
                stderr,
                stdout,
                cancellation: OnceLock::new(),
            }),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn buffered() -> (Self, BufferedOutput) {
        let output = BufferedOutput::default();
        let target = StreamTarget::Buffered(output.clone());
        (Self::new(target.clone(), target), output)
    }

    pub(crate) fn set_cancellation(
        &self,
        cancellation: tola_build::cancellation::BuildCancellation,
    ) {
        let _ = self.inner.cancellation.set(cancellation);
    }

    pub(crate) fn write_stderr(&self, bytes: &[u8]) -> io::Result<()> {
        let mut presentation = self.lock()?;
        self.finish_hook_line(&mut presentation)?;
        self.print(&mut presentation, bytes)
    }

    /// Console tracing cannot wait for a view holding this lock. The tracing file layer
    /// records the event independently, including while the console belongs to a view.
    pub(crate) fn write_trace(&self, bytes: &[u8]) -> io::Result<()> {
        let Some(mut presentation) = self.inner.lock.try_lock() else {
            return Ok(());
        };
        if let Some(screen) = &mut presentation.development {
            screen.prepare()?;
        }
        self.finish_hook_line(&mut presentation)?;
        self.print(&mut presentation, bytes)
    }

    fn print(&self, presentation: &mut Presentation, bytes: &[u8]) -> io::Result<()> {
        match &mut presentation.development {
            Some(screen) => {
                screen.activity(&String::from_utf8_lossy(bytes));
                Ok(())
            }
            None => self.inner.stderr.write(bytes),
        }
    }

    pub(crate) fn open_development(
        &self,
        palette: Palette,
        log: Option<String>,
    ) -> io::Result<bool> {
        let terminal = ProcessTerminal::new(self);
        if !terminal.is_interactive() || !terminal.can_draw() {
            return Ok(false);
        }
        let (columns, rows) = terminal.size()?;
        let claim = Claim::take(Holder::View).map_err(io::Error::other)?;
        let mut presentation = self.lock()?;
        self.finish_hook_line(&mut presentation)?;
        presentation.development = Some(DevScreen::open(
            StreamWriter(self.inner.stderr.clone()),
            palette,
            log,
            claim,
            ratatui::layout::Size::new(columns, rows),
        )?);
        Ok(true)
    }

    pub(crate) fn close_development(&self) -> io::Result<()> {
        // Restoring terminal modes finishes even after command cancellation.
        let mut presentation = self.inner.lock.lock();
        match presentation.development.take() {
            Some(mut screen) => screen.finish(),
            None => Ok(()),
        }
    }

    pub(crate) fn draw_development(
        &self,
        event: Option<&crossterm::event::Event>,
        cancellation: &crate::cancellation::Cancellation,
    ) -> io::Result<bool> {
        let mut presentation = self.lock()?;
        let Some(screen) = &mut presentation.development else {
            return Ok(true);
        };
        if event.is_some_and(|event| screen.answer_event(event, cancellation)) {
            return Ok(true);
        }
        screen.draw()?;
        Ok(false)
    }

    pub(crate) fn round(&self, round: Round) -> io::Result<()> {
        let mut presentation = self.lock()?;
        self.finish_hook_line(&mut presentation)?;
        match &mut presentation.development {
            Some(screen) => {
                screen.round(round);
                Ok(())
            }
            None => self.inner.stderr.write(round.transcript.as_bytes()),
        }
    }

    pub(crate) fn activity(&self, text: &str) -> io::Result<()> {
        let mut presentation = self.lock()?;
        self.finish_hook_line(&mut presentation)?;
        match &mut presentation.development {
            Some(screen) => {
                screen.activity(text);
                Ok(())
            }
            None => self.inner.stderr.write(text.as_bytes()),
        }
    }

    pub(crate) fn hooks(&self, text: &str) -> io::Result<()> {
        let mut presentation = self.lock()?;
        self.finish_hook_line(&mut presentation)?;
        match &mut presentation.development {
            Some(screen) => {
                screen.hooks(text);
                Ok(())
            }
            None => self.inner.stderr.write(text.as_bytes()),
        }
    }

    pub(crate) fn serving(&self, text: &str) -> io::Result<()> {
        let mut presentation = self.lock()?;
        self.finish_hook_line(&mut presentation)?;
        match &mut presentation.development {
            Some(screen) => {
                screen.serving(text);
                Ok(())
            }
            None => self.inner.stderr.write(text.as_bytes()),
        }
    }

    pub(crate) fn hook_started(&self, run: HookRun, stage: &str) -> io::Result<bool> {
        let mut presentation = self.lock()?;
        if let Some(screen) = &mut presentation.development {
            screen.job_started(run, stage);
            return Ok(true);
        }
        Ok(false)
    }

    pub(crate) fn hook_finished(&self, run: &HookRun) -> io::Result<bool> {
        let mut presentation = self.lock()?;
        if let Some(screen) = &mut presentation.development {
            screen.job_finished(run);
            return Ok(true);
        }
        if presentation
            .hook
            .as_ref()
            .is_some_and(|line| line.run == *run)
        {
            self.finish_hook_line(&mut presentation)?;
        }
        presentation.hook_text.retain(|text| text.run != *run);
        Ok(false)
    }

    pub(crate) fn hook_output(
        &self,
        run: &HookRun,
        stream: &str,
        chunk: &str,
        palette: Palette,
    ) -> io::Result<()> {
        let mut presentation = self.lock()?;
        if let Some(screen) = &mut presentation.development {
            screen.job_output(run, stream, chunk);
            return Ok(());
        }
        self.write_hook_chunk(&mut presentation, run, stream, chunk, palette)
    }

    fn write_hook_chunk(
        &self,
        presentation: &mut Presentation,
        run: &HookRun,
        stream: &str,
        chunk: &str,
        palette: Palette,
    ) -> io::Result<()> {
        if chunk.is_empty() {
            return Ok(());
        }
        let stream = stream_index(stream);
        if presentation
            .hook
            .as_ref()
            .is_some_and(|line| line.run != *run || line.stream != stream)
        {
            self.finish_hook_line(presentation)?;
        }
        let position = match presentation
            .hook_text
            .iter()
            .position(|text| text.run == *run)
        {
            Some(position) => position,
            None => {
                presentation.hook_text.push(HookText::new(run.clone()));
                presentation.hook_text.len() - 1
            }
        };
        let label = if stream == 0 {
            run.name.clone()
        } else {
            format!("{} stderr", run.name)
        };
        let mut rendered = String::new();
        for segment in presentation.hook_text[position].streams[stream].push(chunk) {
            if segment.text == "\n" {
                if let Some(line) = &mut presentation.hook {
                    line.painter.reset(&mut rendered);
                }
                rendered.push('\n');
                presentation.hook = None;
                continue;
            }
            if presentation.hook.is_none() {
                rendered.push_str(&super::progress::stream_label(&label, palette));
                presentation.hook = Some(HookLine {
                    run: run.clone(),
                    stream,
                    painter: Painter::new(palette.uses_color()),
                });
            }
            presentation
                .hook
                .as_mut()
                .expect("a hook line is open")
                .painter
                .write(&segment, &mut rendered);
        }
        if rendered.is_empty() {
            return Ok(());
        }
        self.print(presentation, rendered.as_bytes())
    }

    pub(crate) fn write_stderr_nonblocking(&self, bytes: &[u8]) -> io::Result<()> {
        if let Some(mut presentation) = self.inner.lock.try_lock() {
            self.finish_hook_line(&mut presentation)?;
        }
        self.inner.stderr.write(bytes)
    }

    pub(crate) fn write_stdout(&self, bytes: &[u8]) -> anyhow::Result<()> {
        self.inner.stdout.write(bytes).map_err(stdout_error)
    }

    pub(crate) fn write_stdout_line(&self, bytes: &[u8]) -> anyhow::Result<()> {
        self.inner.stdout.write(bytes).map_err(stdout_error)?;
        self.inner.stdout.write(b"\n").map_err(stdout_error)
    }

    pub(crate) fn with_stderr_lock<R, E>(
        &self,
        operation: impl FnOnce(&Self) -> Result<R, E>,
    ) -> Result<R, E>
    where
        E: From<io::Error>,
    {
        let mut presentation = self.lock()?;
        self.finish_hook_line(&mut presentation)?;
        operation(self)
    }

    pub(crate) fn write_stderr_locked(&self, bytes: &[u8]) -> io::Result<()> {
        self.inner.stderr.write(bytes)
    }

    fn lock(&self) -> io::Result<parking_lot::MutexGuard<'_, Presentation>> {
        let cancellation = self.inner.cancellation.get().cloned().unwrap_or_default();
        loop {
            if let Some(mut guard) = self.inner.lock.try_lock_for(OUTPUT_LOCK_POLL_INTERVAL) {
                if let Some(screen) = &mut guard.development {
                    screen.prepare()?;
                }
                return Ok(guard);
            }
            cancellation.ensure_active().map_err(io::Error::other)?;
        }
    }

    fn finish_hook_line(&self, presentation: &mut Presentation) -> io::Result<()> {
        let Some(mut line) = presentation.hook.take() else {
            return Ok(());
        };
        let mut rendered = String::new();
        if let Some(text) = presentation
            .hook_text
            .iter_mut()
            .find(|text| text.run == line.run)
            && let Some(carriage_return) = text.streams[line.stream].flush()
        {
            line.painter.write(&carriage_return, &mut rendered);
        }
        line.painter.reset(&mut rendered);
        rendered.push('\n');
        self.print(presentation, rendered.as_bytes())
    }
}

impl StreamTarget {
    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Stderr => crate::sys::write_process_stderr(bytes),
            Self::Stdout => {
                let mut stdout = io::stdout();
                stdout.write_all(bytes)?;
                stdout.flush()
            }
            Self::Buffered(output) => {
                output
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .extend_from_slice(bytes);
                Ok(())
            }
        }
    }
}

fn stdout_error(error: io::Error) -> anyhow::Error {
    if error.kind() == io::ErrorKind::BrokenPipe {
        StdoutClosed.into()
    } else {
        anyhow::Error::new(error).context("cannot write to stdout")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdout_writes_add_no_sequences() {
        let (sink, output) = OutputSink::buffered();
        sink.write_stdout(b"plain").unwrap();
        assert_eq!(String::from_utf8(output.bytes()).unwrap(), "plain");

        let (sink, output) = OutputSink::buffered();
        sink.write_stdout_line(b"plain").unwrap();
        assert_eq!(String::from_utf8(output.bytes()).unwrap(), "plain\n");
    }

    #[test]
    fn blocked_write_observes_cancellation() {
        let (sink, captured) = OutputSink::buffered();
        let canceller = tola_build::cancellation::BuildCanceller::new();
        sink.set_cancellation(canceller.token());
        let guard = sink.inner.lock.lock();
        let waiting = sink.clone();
        let (finished, completion) = std::sync::mpsc::channel();
        let worker =
            std::thread::spawn(move || {
                let error = waiting.write_stderr(b"Cancelled").unwrap_err();
                finished
                    .send(error.get_ref().is_some_and(|inner| {
                        inner.is::<tola_build::cancellation::BuildCancelled>()
                    }))
                    .unwrap();
            });
        canceller.cancel();
        assert!(
            completion
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
        );
        drop(guard);
        worker.join().unwrap();
        assert!(captured.is_empty());
    }

    #[test]
    fn broken_stdout_is_closed_stream() {
        let closed = stdout_error(io::Error::from(io::ErrorKind::BrokenPipe));
        assert!(closed.is::<StdoutClosed>());
        let other = stdout_error(io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(!other.is::<StdoutClosed>());
        assert_eq!(other.to_string(), "cannot write to stdout");
        assert_eq!(
            other
                .chain()
                .find_map(|cause| cause.downcast_ref::<io::Error>())
                .unwrap()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn held_lock_still_writes_stderr() {
        let (sink, output) = OutputSink::buffered();
        let canceller = tola_build::cancellation::BuildCanceller::new();
        sink.set_cancellation(canceller.token());
        canceller.cancel();
        let held = sink.inner.lock.lock();
        sink.write_stderr_nonblocking(b"panic\n").unwrap();
        assert_eq!(String::from_utf8(output.bytes()).unwrap(), "panic\n");
        drop(held);
    }

    fn hook_run(name: &str) -> HookRun {
        HookRun {
            scope: 0,
            name: name.to_owned(),
        }
    }

    #[test]
    fn hook_colors_stay_with_their_execution() {
        let (sink, output) = OutputSink::buffered();
        let first = hook_run("first");
        let second = hook_run("second");
        let palette = Palette::new(true);
        sink.hook_output(&first, "stderr", "\x1b[31mred", palette)
            .unwrap();
        sink.hook_output(&second, "stderr", "\x1b[32mgreen\n", palette)
            .unwrap();
        sink.hook_output(&first, "stderr", "again\n", palette)
            .unwrap();
        let shown = String::from_utf8(output.bytes()).unwrap();
        assert!(shown.contains("\x1b[0;31mred\x1b[0m\n"), "{shown:?}");
        assert!(shown.contains("\x1b[0;32mgreen\x1b[0m\n"), "{shown:?}");
        assert!(shown.contains("\x1b[0;31magain\x1b[0m\n"), "{shown:?}");
    }
}
