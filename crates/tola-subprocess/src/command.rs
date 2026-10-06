//! Running one declared command to its exit, with both streams captured.

use std::{
    ffi::{OsStr, OsString},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command as StdCommand, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use process_wrap::std::CommandWrap;

use crate::capture::{Captured, Stream};
use crate::child::Child;
use crate::{Cancellation, Error, Exit, Result, Stop, sys};

/// How long draining permits further reads after the child stops.
const PIPE_DRAIN_GRACE: Duration = Duration::from_millis(250);
/// What a stopped reader still delivers: the bytes a stream can hold in flight, so a stream that
/// already reached its end still reports that end, while a writer that outlived the child cannot
/// extend the run past this much.
const STOP_DRAIN_BYTES: usize = 1 << 20;
/// Polling cadence when observer callbacks are not occupying the calling thread.
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);
const OUTPUT_CHUNK_BYTES: usize = 16 * 1024;
/// Queued relayed chunks are bounded here, so a slow observer cannot grow the
/// relay without bound.
const OUTPUT_RELAY_CAPACITY: usize = 2;

/// Receives each stream's output while a command runs.
///
/// Callbacks run on the calling thread of [`Command::run`]. Each stream preserves
/// read order; ordering between streams is unspecified. Slow callbacks delay
/// cancellation polling and drain shutdown.
pub trait Observer {
    /// Whether output and completion callbacks are wanted. Returning `false`
    /// skips both; the bounded capture is unaffected. Queried once when observation
    /// starts.
    fn observes_output(&self) -> bool {
        true
    }

    /// Each chunk exactly as it was read, before any capture bound applies.
    fn output(&mut self, stream: Stream, bytes: &[u8]);

    /// Called once per stream, after its last output callback, on EOF or drain stop.
    ///
    /// Completion is guaranteed when a started run returns `Ok`, including
    /// cancellation. Errors or unwinding may leave callbacks incomplete; a run
    /// cancelled before spawning has no streams to complete.
    fn finished(&mut self, stream: Stream);
}

/// Observes nothing: the default for a caller that wants only the captured output.
impl Observer for () {
    fn observes_output(&self) -> bool {
        false
    }

    fn output(&mut self, _: Stream, _: &[u8]) {}

    fn finished(&mut self, _: Stream) {}
}

/// One external command, described exactly as it will be executed.
#[derive(Debug, Default, Clone)]
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    env: Vec<(OsString, Option<OsString>)>,
}

impl Command {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    /// The first item is the program; an empty iterator leaves it empty, so the
    /// command fails to spawn.
    pub fn from_args<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let mut args = args.into_iter().map(Into::into);
        let program = args.next().unwrap_or_default();
        Self {
            program,
            args: args.collect(),
            ..Self::default()
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set the child's working directory.
    pub fn cwd(mut self, directory: impl AsRef<Path>) -> Self {
        self.cwd = Some(directory.as_ref().to_owned());
        self
    }

    /// Set one environment variable for the child. The last operation on a key wins.
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), Some(value.into())));
        self
    }

    /// Remove one environment variable from the child, including a previously set value.
    pub fn env_remove(mut self, key: impl Into<OsString>) -> Self {
        self.env.push((key.into(), None));
        self
    }

    pub fn program(&self) -> &OsStr {
        &self.program
    }

    /// Run a non-interactive command with null stdin and observe its output.
    ///
    /// Both streams are captured with a bounded head and tail. After the child
    /// stops, reads end at EOF or are stopped after a drain grace. Already-read
    /// bytes remain captured and queued callbacks are delivered before returning
    /// `Ok`. Slow callbacks and OS scheduling can delay shutdown; the grace is not
    /// a wall-clock return deadline.
    pub fn run(
        &self,
        cancellation: &impl Cancellation,
        observer: &mut impl Observer,
    ) -> Result<Exit> {
        if cancellation.is_cancelled() {
            return Ok(Exit::cancelled(None));
        }
        let child = self.spawn()?;
        self.observe(child, cancellation, observer)
    }

    /// Capture and await a child this command already started.
    pub(crate) fn observe(
        &self,
        mut child: Child,
        cancellation: &impl Cancellation,
        observer: &mut impl Observer,
    ) -> Result<Exit> {
        let stdout = take_pipe(child.wrapper_mut().stdout().take())?;
        let stderr = take_pipe(child.wrapper_mut().stderr().take())?;
        let (mut relay, events) = Relay::new(observer.observes_output());
        relay.spawn(stdout, Stream::Stdout, &events)?;
        relay.spawn(stderr, Stream::Stderr, &events)?;
        drop(events);
        self.await_stop(&mut child, relay, cancellation, observer)
    }

    fn spawn(&self) -> Result<Child> {
        let mut command = StdCommand::new(&self.program);
        command.args(&self.args);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(directory) = &self.cwd {
            command.current_dir(directory);
        }
        for (key, value) in &self.env {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
        let child = sys::wrap(CommandWrap::from(command))
            .spawn()
            .map_err(Error::Spawn)?;
        Ok(Child::new(child))
    }

    fn await_stop(
        &self,
        child: &mut Child,
        mut relay: Relay,
        cancellation: &impl Cancellation,
        observer: &mut impl Observer,
    ) -> Result<Exit> {
        let stop = loop {
            if cancellation.is_cancelled() {
                break Stop::Cancelled(child.terminate());
            }
            if let Some(status) = child.try_wait()? {
                // Terminate descendants before draining their pipes, so a reader
                // never waits on a process that outlived the direct child.
                child.terminate();
                break Stop::Exited(status);
            }
            // The first observed reader failure ends the run; cancellation wins at each recheck.
            relay.receive(CHILD_POLL_INTERVAL, observer)?;
        };
        let [stdout, stderr] = relay.finish(observer)?;
        Ok(Exit {
            stop,
            stdout,
            stderr,
        })
    }
}

fn take_pipe<T: Read + Send + 'static>(pipe: Option<T>) -> Result<T> {
    pipe.ok_or_else(|| Error::Capture(io::Error::other("a child stream is not captured")))
}

#[derive(Debug)]
enum StreamEnd {
    Eof,
    Stopped,
}

struct StreamCapture {
    captured: Captured,
    end: io::Result<StreamEnd>,
}

enum RelayEvent {
    Output(Stream, Box<[u8]>),
    Finished(Stream, Result<Captured>),
}

/// The relay keeps only the receiver: the senders live in the readers, so losing
/// the receiver releases blocked readers and both readers finishing closes the
/// channel.
struct Relay {
    stop: sys::ReaderStop,
    workers: [Option<thread::JoinHandle<()>>; 2],
    events: Option<mpsc::Receiver<RelayEvent>>,
    captures: [Option<Captured>; 2],
    relay_enabled: bool,
}

impl Relay {
    fn new(relay_enabled: bool) -> (Self, mpsc::SyncSender<RelayEvent>) {
        let (sender, events) = mpsc::sync_channel(OUTPUT_RELAY_CAPACITY);
        (
            Self {
                stop: sys::ReaderStop::default(),
                workers: [None, None],
                events: Some(events),
                captures: [None, None],
                relay_enabled,
            },
            sender,
        )
    }

    fn spawn(
        &mut self,
        pipe: impl sys::Pipe,
        stream: Stream,
        events: &mpsc::SyncSender<RelayEvent>,
    ) -> Result<()> {
        let index = stream.index();
        let reader = sys::Reader::new(pipe, self.stop.clone()).map_err(Error::Reader)?;
        let stop = self.stop.clone();
        let events = events.clone();
        let relay_enabled = self.relay_enabled;
        self.workers[index] = Some(
            thread::Builder::new()
                .name(format!("tola-subprocess-{}", stream.name()))
                .spawn(move || {
                    let captured =
                        capture_from(read_stream(reader, stop, &events, stream, relay_enabled));
                    let _ = events.send(RelayEvent::Finished(stream, captured));
                })
                .map_err(Error::Reader)?,
        );
        Ok(())
    }

    fn receive(&mut self, timeout: Duration, observer: &mut impl Observer) -> Result<()> {
        let disconnected = match self
            .events
            .as_ref()
            .expect("the relay is receiving")
            .recv_timeout(timeout)
        {
            Ok(event) => {
                self.deliver(event, observer)?;
                false
            }
            Err(mpsc::RecvTimeoutError::Timeout) => false,
            Err(mpsc::RecvTimeoutError::Disconnected) => true,
        };
        for worker in &mut self.workers {
            if worker.as_ref().is_some_and(thread::JoinHandle::is_finished) {
                join_reader(worker.take().expect("the reader finished"))?;
            }
        }
        if disconnected {
            // Closed pipes do not imply child exit; retain the cancellation polling cadence.
            thread::sleep(timeout);
        }
        Ok(())
    }

    fn deliver(&mut self, event: RelayEvent, observer: &mut impl Observer) -> Result<()> {
        match event {
            RelayEvent::Output(stream, bytes) => observer.output(stream, &bytes),
            RelayEvent::Finished(stream, captured) => {
                self.captures[stream.index()] = Some(captured?);
                if self.relay_enabled {
                    observer.finished(stream);
                }
            }
        }
        Ok(())
    }

    fn request_stop(&mut self) {
        self.stop.request();
        self.events.take();
    }

    fn finish(mut self, observer: &mut impl Observer) -> Result<[Captured; 2]> {
        let deadline = Instant::now() + PIPE_DRAIN_GRACE;
        while !self.captures.iter().all(Option::is_some) {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            self.receive(remaining, observer)?;
        }
        self.stop.request();
        // Keep receiving until the stopped readers release their senders. Dropping
        // the receiver here would discard queued output and completion callbacks.
        if let Some(events) = self.events.take() {
            for event in events {
                self.deliver(event, observer)?;
            }
        }
        for worker in &mut self.workers {
            if let Some(worker) = worker.take() {
                join_reader(worker)?;
            }
        }
        Ok(self
            .captures
            .each_mut()
            .map(|captured| captured.take().expect("both stream captures completed")))
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.request_stop();
        for worker in &mut self.workers {
            if let Some(worker) = worker.take() {
                let _ = worker.join();
            }
        }
    }
}

fn join_reader(worker: thread::JoinHandle<()>) -> Result<()> {
    worker
        .join()
        .map_err(|_| Error::Read(io::Error::other("a stream reader panicked")))
}

fn capture_from(captured: StreamCapture) -> Result<Captured> {
    match captured.end {
        Ok(StreamEnd::Eof) => Ok(captured.captured),
        Ok(StreamEnd::Stopped) => {
            let mut captured = captured.captured;
            captured.mark_stopped_early();
            Ok(captured)
        }
        Err(error) => Err(Error::Read(error)),
    }
}

fn read_stream(
    mut pipe: impl Read,
    stop: sys::ReaderStop,
    events: &mpsc::SyncSender<RelayEvent>,
    stream: Stream,
    relay_enabled: bool,
) -> StreamCapture {
    let mut captured = Captured::default();
    let mut chunk = [0u8; OUTPUT_CHUNK_BYTES];
    // The bytes delivered since the stop request.
    let mut drained = 0usize;
    let end = loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break Ok(StreamEnd::Eof),
            Ok(read) => {
                captured.extend(&chunk[..read]);
                // Only relaying needs an owned chunk. Beyond the bounded queue,
                // each reader can hold one chunk while waiting to send.
                if relay_enabled
                    && events
                        .send(RelayEvent::Output(stream, chunk[..read].into()))
                        .is_err()
                {
                    break Ok(StreamEnd::Stopped);
                }
                if stop.is_requested() {
                    drained = drained.saturating_add(read);
                    if drained > STOP_DRAIN_BYTES {
                        break Ok(StreamEnd::Stopped);
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error)
                if error
                    .get_ref()
                    .is_some_and(|inner| inner.is::<sys::Stopped>()) =>
            {
                break Ok(StreamEnd::Stopped);
            }
            Err(error) => break Err(error),
        }
    };
    // This drops the pipe reader and joins its platform monitor before the owner
    // is notified, so no reader outlives the run.
    drop(pipe);
    StreamCapture { captured, end }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NO_CANCELLATION;
    use std::{
        cell::RefCell,
        io::Write,
        net::{TcpListener, TcpStream},
    };

    const CHILD_MODE: &str = "TOLA_SUBPROCESS_TEST";
    const CHILD_CONTROL: &str = "TOLA_SUBPROCESS_CONTROL";
    const CHILD_ENV: &str = "TOLA_SUBPROCESS_VALUE";
    const CHILD_STDIN: &[u8] = b"caller input\n";
    const CHILD_STDOUT: &[u8] = b"root:\0\xffstdout\n";
    const CHILD_STDERR: &[u8] = b"root:\0\xfestderr\n";
    const CHILD_READY_TIMEOUT: Duration = Duration::from_secs(30);
    /// An escaped descendant never closes its pipes, so any finite bound proves the
    /// capture ignored it rather than waiting for end-of-file; a passing run returns
    /// well inside it, and the bound only decides how long a regression takes to fail.
    const CAPTURE_DEADLINE: Duration = Duration::from_secs(5);

    /// Whether a descendant can leave the run's containment at all: a unix process group can be
    /// left by starting another, a Windows job object cannot.
    const CONTAINMENT_ESCAPABLE: bool = cfg!(unix);

    /// Leave the run's containment, so the descendant outlives the process tree it started under.
    #[cfg(unix)]
    fn escape_containment(command: &mut std::process::Command) {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    /// A job object keeps the descendant inside the run's tree, so there is nothing to leave.
    #[cfg(windows)]
    fn escape_containment(_: &mut std::process::Command) {}

    #[derive(Default)]
    struct RelayedOutput {
        streams: [Vec<u8>; 2],
        finished: [bool; 2],
    }

    impl Observer for RelayedOutput {
        fn output(&mut self, stream: Stream, bytes: &[u8]) {
            let index = stream.index();
            assert!(!self.finished[index], "output followed stream completion");
            self.streams[index].extend_from_slice(bytes);
        }

        fn finished(&mut self, stream: Stream) {
            let index = stream.index();
            assert!(!self.finished[index], "stream completed twice");
            self.finished[index] = true;
        }
    }

    /// Every read fails with its own payload, so the caller can tell the error
    /// apart from one the reader synthesised.
    struct BrokenPipeReader;

    #[derive(Debug, thiserror::Error)]
    #[error("pipe read failure")]
    struct PipeFailure;

    impl Read for BrokenPipeReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, PipeFailure))
        }
    }

    fn process_child_argv() -> [OsString; 4] {
        [
            std::env::current_exe().unwrap().into_os_string(),
            "--exact".into(),
            "command::tests::process_child".into(),
            "--nocapture".into(),
        ]
    }

    fn process_child_command(mode: &str) -> Command {
        Command::from_args(process_child_argv()).env(CHILD_MODE, mode)
    }

    /// Re-executed by process regressions; without [`CHILD_MODE`] the normal test
    /// run must not enter the child protocol.
    #[test]
    fn process_child() {
        let Ok(mode) = std::env::var(CHILD_MODE) else {
            return;
        };
        let result = run_child_mode(&mode);
        match result {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                eprintln!("subprocess child run failed: {error}");
                std::process::exit(1);
            }
        }
    }

    fn run_child_mode(mode: &str) -> io::Result<i32> {
        match mode {
            "environment" => {
                io::stderr().write_all(
                    std::env::var(CHILD_ENV)
                        .unwrap_or_else(|_| "missing".into())
                        .as_bytes(),
                )?;
                return Ok(0);
            }
            "stdin-reader" => {
                return Ok(i32::from(io::stdin().read(&mut [0])? != 0));
            }
            "stdin-owner" => {
                let exit = process_child_command("stdin-reader")
                    .run(&NO_CANCELLATION, &mut ())
                    .map_err(io::Error::other)?;
                if !matches!(exit.stop, Stop::Exited(status) if status.success()) {
                    return Err(io::Error::other("the command consumed caller stdin"));
                }
                let mut caller_input = [0; CHILD_STDIN.len()];
                io::stdin().read_exact(&mut caller_input)?;
                if caller_input.as_slice() != CHILD_STDIN {
                    return Err(io::Error::other("caller stdin was changed"));
                }
                return Ok(0);
            }
            "large" => {
                io::copy(&mut io::repeat(b'x').take(20_000), &mut io::stderr())?;
                io::copy(&mut io::repeat(b'y').take(20_000), &mut io::stdout())?;
                io::stdout().flush()?;
                return Ok(1);
            }
            "diagnostic" => {
                io::stderr().write_all(&b"diagnostic".repeat(10_000))?;
                io::stderr().flush()?;
            }
            "descendant" => {}
            "success" | "tree" | "escaped-tree" | "held-streams" => {
                io::stdout().write_all(CHILD_STDOUT)?;
                io::stdout().flush()?;
                io::stderr().write_all(CHILD_STDERR)?;
                io::stderr().flush()?;
                if mode == "success" {
                    return Ok(0);
                }
            }
            _ => return Err(io::Error::other("unknown child mode")),
        }
        let descendant = if matches!(mode, "tree" | "escaped-tree") {
            let mut command = std::process::Command::new(process_child_argv()[0].clone());
            command
                .args(&process_child_argv()[1..])
                .env(CHILD_MODE, "descendant");
            if mode == "escaped-tree" {
                escape_containment(&mut command);
            }
            Some(command.spawn()?)
        } else {
            None
        };
        let result = (|| {
            let address = std::env::var(CHILD_CONTROL).map_err(io::Error::other)?;
            let mut control = TcpStream::connect(address)?;
            let deadline = Some(CHILD_READY_TIMEOUT);
            control.set_read_timeout(deadline)?;
            control.set_write_timeout(deadline)?;
            control.write_all(if mode == "descendant" { b"D" } else { b"R" })?;
            loop {
                let mut message = [0];
                control.read_exact(&mut message)?;
                match message[0] {
                    b'P' => control.write_all(b"p")?,
                    b'W' if matches!(mode, "descendant" | "held-streams") => {
                        let stdout = io::stdout()
                            .write_all(b"late stdout\n")
                            .and_then(|()| io::stdout().flush());
                        let stderr = io::stderr()
                            .write_all(b"late stderr\n")
                            .and_then(|()| io::stderr().flush());
                        let closed = [stdout, stderr].map(|result| {
                            if result.is_err_and(|error| error.kind() == io::ErrorKind::BrokenPipe)
                            {
                                b'B'
                            } else {
                                b'O'
                            }
                        });
                        control.write_all(&closed)?;
                    }
                    b'E' if mode == "tree" => return Ok(37),
                    b'E' if matches!(mode, "descendant" | "escaped-tree") => return Ok(0),
                    _ => return Err(io::Error::other("unexpected child control message")),
                }
            }
        })();
        if result.is_err()
            && let Some(mut descendant) = descendant
        {
            let _ = descendant.kill();
            let _ = descendant.wait();
        }
        // Only an explicit root-first exit leaves the descendant alive; its own
        // control connection still has a finite failure deadline.
        result
    }

    fn control_listener() -> TcpListener {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        listener
    }

    /// A process child that dials the returned listener once it is ready.
    fn child_with_control(mode: &str) -> (Command, TcpListener) {
        let listener = control_listener();
        let command = process_child_command(mode)
            .env(CHILD_CONTROL, listener.local_addr().unwrap().to_string());
        (command, listener)
    }

    fn ready_child(mode: &str) -> (Command, Child, TcpStream) {
        let (command, listener) = child_with_control(mode);
        let child = command.spawn().unwrap();
        let deadline = Instant::now() + CHILD_READY_TIMEOUT;
        loop {
            if let Some((_, peer)) = accept_control(&listener) {
                return (command, child, peer);
            }
            assert!(Instant::now() < deadline, "the child did not become ready");
            thread::yield_now();
        }
    }

    fn accept_control(listener: &TcpListener) -> Option<(u8, TcpStream)> {
        match listener.accept() {
            Ok((mut control, _)) => {
                control.set_nonblocking(false).unwrap();
                control.set_read_timeout(Some(CHILD_READY_TIMEOUT)).unwrap();
                control
                    .set_write_timeout(Some(CHILD_READY_TIMEOUT))
                    .unwrap();
                let mut role = [0];
                control.read_exact(&mut role).unwrap();
                Some((role[0], control))
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => None,
            Err(error) => panic!("accepting child control: {error}"),
        }
    }

    fn assert_control_closed(control: &mut TcpStream) {
        let mut byte = [0];
        match control.read(&mut byte) {
            Ok(0) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                ) => {}
            result => panic!("the child retained its control connection: {result:?}"),
        }
    }

    #[test]
    fn command_preserves_caller_stdin() {
        let argv = process_child_argv();
        let mut caller = StdCommand::new(&argv[0])
            .args(&argv[1..])
            .env(CHILD_MODE, "stdin-owner")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        caller.stdin.take().unwrap().write_all(CHILD_STDIN).unwrap();
        let output = caller.wait_with_output().unwrap();

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn eof_finishes_each_stream() {
        let mut observer = RelayedOutput::default();
        let exit = process_child_command("success")
            .run(&NO_CANCELLATION, &mut observer)
            .unwrap();

        assert!(matches!(exit.stop, Stop::Exited(status) if status.success()));
        assert_eq!(observer.finished, [true; 2]);
        assert_eq!(observer.streams[1], CHILD_STDERR);
        for (capture, observed) in [exit.stdout, exit.stderr].iter().zip(observer.streams) {
            assert!(!capture.stopped_early());
            assert_eq!(observed, capture.render(|_| String::new()));
        }
    }

    /// A stream that reached its end before the reader noticed is no stopped capture: the bytes
    /// it holds and the EOF behind them belong to the run, so a stop request must not cut them.
    #[test]
    fn stop_request_does_not_truncate_eof() {
        let (reader, mut writer) = io::pipe().unwrap();
        writer.write_all(b"body").unwrap();
        drop(writer);
        let stop = sys::ReaderStop::default();
        stop.request();
        let (sender, events) = mpsc::sync_channel(OUTPUT_RELAY_CAPACITY);
        let capture = read_stream(
            sys::Reader::new(reader, stop.clone()).unwrap(),
            stop,
            &sender,
            Stream::Stdout,
            false,
        );
        drop(events);

        assert!(matches!(capture.end, Ok(StreamEnd::Eof)));
        assert_eq!(capture.captured.render(|_| String::new()), b"body");
    }

    /// A writer that outlived the child cannot hold the run: a stopped reader ends after the bytes
    /// the stream held in flight, however much the writer keeps producing.
    #[test]
    fn live_writer_does_not_hold_the_reader() {
        let (reader, mut writer) = io::pipe().unwrap();
        let stop = sys::ReaderStop::default();
        let (sender, events) = mpsc::sync_channel(OUTPUT_RELAY_CAPACITY);
        let reading_stop = stop.clone();
        let worker = thread::spawn(move || {
            read_stream(
                sys::Reader::new(reader, reading_stop.clone()).unwrap(),
                reading_stop,
                &sender,
                Stream::Stdout,
                false,
            )
        });
        drop(events);
        stop.request();

        let block = vec![b'x'; OUTPUT_CHUNK_BYTES];
        let deadline = Instant::now() + CAPTURE_DEADLINE;
        loop {
            if writer.write_all(&block).is_err() {
                break;
            }
            if worker.is_finished() {
                break;
            }
            assert!(Instant::now() < deadline, "the reader outlived its stop");
        }
        let capture = worker.join().unwrap();

        assert!(matches!(capture.end, Ok(StreamEnd::Stopped)));
    }

    #[test]
    fn output_arrives_before_stream_end() {
        let (reader, mut writer) = io::pipe().unwrap();
        let (other, _other_writer) = io::pipe().unwrap();
        let mut observer = RelayedOutput::default();
        let (mut relay, events) = Relay::new(true);
        relay.spawn(reader, Stream::Stdout, &events).unwrap();
        relay.spawn(other, Stream::Stderr, &events).unwrap();
        drop(events);

        writer.write_all(b"ready\n").unwrap();
        let deadline = Instant::now() + CHILD_READY_TIMEOUT;
        while observer.streams[0].is_empty() && Instant::now() < deadline {
            let _ = relay.receive(Duration::from_millis(50), &mut observer);
        }
        assert_eq!(observer.streams, [b"ready\n".to_vec(), Vec::new()]);

        drop(writer);
        let captures = relay.finish(&mut observer).unwrap();
        assert_eq!(captures[0].render(|_| String::new()), b"ready\n");
    }

    #[test]
    fn reader_error_surfaces_the_pipe_failure() {
        let (sender, events) = mpsc::sync_channel(OUTPUT_RELAY_CAPACITY);
        let stopped = sys::ReaderStop::default();
        let capture = read_stream(BrokenPipeReader, stopped, &sender, Stream::Stdout, false);
        drop(events);

        let error = capture.end.expect_err("a failed read is not EOF");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert!(
            error
                .get_ref()
                .is_some_and(|inner| inner.is::<PipeFailure>())
        );
    }

    #[test]
    fn drain_stop_finishes_each_stream() {
        let (_command, mut child, mut peer) = ready_child("held-streams");
        let mut observer = RelayedOutput::default();
        let (mut relay, events) = Relay::new(true);
        let stdout = take_pipe(child.wrapper_mut().stdout().take()).unwrap();
        let stderr = take_pipe(child.wrapper_mut().stderr().take()).unwrap();
        relay.spawn(stdout, Stream::Stdout, &events).unwrap();
        relay.spawn(stderr, Stream::Stderr, &events).unwrap();
        drop(events);

        let deadline = Instant::now() + CHILD_READY_TIMEOUT;
        while !observer.streams[0].ends_with(CHILD_STDOUT) || observer.streams[1] != CHILD_STDERR {
            relay
                .receive(
                    deadline.saturating_duration_since(Instant::now()),
                    &mut observer,
                )
                .expect("the child did not deliver both streams");
        }
        let captures = relay.finish(&mut observer).unwrap();

        assert_eq!(observer.finished, [true; 2]);
        assert_eq!(observer.streams[1], CHILD_STDERR);
        for (capture, observed) in captures.iter().zip(observer.streams) {
            assert!(capture.stopped_early());
            assert_eq!(observed, capture.render(|_| String::new()));
        }
        peer.write_all(b"W").unwrap();
        let mut closed = [0; 2];
        peer.read_exact(&mut closed).unwrap();
        assert_eq!(&closed, b"BB");
        child.terminate();
        assert_control_closed(&mut peer);
    }

    #[test]
    fn environment_uses_last_operation() {
        for (command, expected) in [
            (
                process_child_command("environment")
                    .env(CHILD_ENV, "first")
                    .env_remove(CHILD_ENV),
                "missing",
            ),
            (
                process_child_command("environment")
                    .env_remove(CHILD_ENV)
                    .env(CHILD_ENV, "second"),
                "second",
            ),
            (
                process_child_command("environment")
                    .env(CHILD_ENV, "first")
                    .env(CHILD_ENV, "second"),
                "second",
            ),
        ] {
            let exit = command.run(&NO_CANCELLATION, &mut ()).unwrap();
            assert!(exit.stop.status().unwrap().success());
            assert_eq!(exit.stderr.render(|_| String::new()), expected.as_bytes());
        }
    }

    #[test]
    fn reader_failure_interrupts_running_child() {
        let mut polls_before_error = Vec::new();
        for panic_reader in [false, true] {
            let (command, mut child, mut peer) = ready_child("held-streams");
            let deadline = Instant::now() + CHILD_READY_TIMEOUT;
            let (other, _writer) = io::pipe().unwrap();
            let (mut relay, events) = Relay::new(false);
            let sender = events.clone();
            let stop = relay.stop.clone();
            relay.workers[0] = Some(thread::spawn(move || {
                assert!(!panic_reader, "stream reader panic");
                let captured = capture_from(read_stream(
                    BrokenPipeReader,
                    stop,
                    &sender,
                    Stream::Stdout,
                    false,
                ));
                let _ = sender.send(RelayEvent::Finished(Stream::Stdout, captured));
            }));
            relay.spawn(other, Stream::Stderr, &events).unwrap();
            drop(events);
            while !relay.workers[0].as_ref().unwrap().is_finished() {
                assert!(
                    Instant::now() < deadline,
                    "the failed reader did not finish"
                );
                thread::yield_now();
            }
            let polls = std::cell::Cell::new(0);
            let error = command
                .await_stop(
                    &mut child,
                    relay,
                    &|| {
                        polls.set(polls.get() + 1);
                        polls.get() == 5
                    },
                    &mut (),
                )
                .unwrap_err();
            assert!(matches!(error, Error::Read(_)), "{error:?}");
            if !panic_reader {
                assert!(
                    error
                        .source_io()
                        .get_ref()
                        .is_some_and(|inner| inner.is::<PipeFailure>())
                );
            }
            polls_before_error.push(polls.get());
            drop(child);
            assert_control_closed(&mut peer);
        }
        assert!(
            polls_before_error.iter().all(|polls| *polls < 5),
            "reader failure waited for cancellation: {polls_before_error:?}"
        );
    }

    #[test]
    fn closed_streams_keep_cancellation_cadence() {
        let (command, mut child, mut peer) = ready_child("held-streams");
        let deadline = Instant::now() + CHILD_READY_TIMEOUT;
        let (stdout, stdout_writer) = io::pipe().unwrap();
        let (stderr, stderr_writer) = io::pipe().unwrap();
        drop((stdout_writer, stderr_writer));
        let (mut relay, events) = Relay::new(false);
        relay.spawn(stdout, Stream::Stdout, &events).unwrap();
        relay.spawn(stderr, Stream::Stderr, &events).unwrap();
        drop(events);
        for worker in relay.workers.iter().flatten() {
            while !worker.is_finished() {
                assert!(Instant::now() < deadline, "the reader did not reach EOF");
                thread::yield_now();
            }
        }
        let polls = std::cell::Cell::new(0);
        let started = Instant::now();
        let exit = command
            .await_stop(
                &mut child,
                relay,
                &|| {
                    polls.set(polls.get() + 1);
                    started.elapsed() >= Duration::from_millis(100)
                },
                &mut (),
            )
            .unwrap();
        assert!(exit.stop.is_cancelled());
        assert!(
            polls.get() <= 20,
            "closed streams caused {} cancellation polls",
            polls.get()
        );
        assert_control_closed(&mut peer);
    }

    #[test]
    fn observer_panic_releases_running_child() {
        struct PanickingObserver;
        impl Observer for PanickingObserver {
            fn output(&mut self, _: Stream, _: &[u8]) {
                panic!("output callback panic");
            }
            fn finished(&mut self, _: Stream) {}
        }
        let (command, child, mut peer) = ready_child("held-streams");
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            command.observe(child, &NO_CANCELLATION, &mut PanickingObserver)
        }));
        assert!(panic.is_err());
        assert_control_closed(&mut peer);
    }

    #[test]
    fn missing_program_fails_the_spawn() {
        let error = Command::new("tola-subprocess-missing-program")
            .run(&NO_CANCELLATION, &mut ())
            .unwrap_err();

        assert!(matches!(error, Error::Spawn(_)), "{error:?}");
    }

    #[test]
    fn cancelled_run_before_spawn_never_starts() {
        let exit = Command::new("tola-subprocess-missing-program")
            .run(&|| true, &mut ())
            .unwrap();

        assert!(exit.stop.is_cancelled());
        assert_eq!(exit.stdout.render(|_| String::new()), b"");
    }

    #[test]
    fn inherited_pipe_does_not_block_capture() {
        let (command, listener) = child_with_control("tree");
        let mut child = command.spawn().unwrap();
        let mut peers = Vec::new();
        let deadline = Instant::now() + CHILD_READY_TIMEOUT;
        while peers.len() < 2 {
            if let Some(peer) = accept_control(&listener) {
                peers.push(peer);
            }
            assert!(
                Instant::now() < deadline,
                "the child's descendants did not become ready"
            );
            thread::yield_now();
        }
        let root = &mut peers.iter_mut().find(|(role, _)| *role == b'R').unwrap().1;
        root.write_all(b"E").unwrap();
        let deadline = Instant::now() + CHILD_READY_TIMEOUT;
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "the direct child's exit was not observable"
            );
            thread::yield_now();
        };
        assert_eq!(status.code(), Some(37));
        assert_control_closed(root);

        let descendant = &mut peers.iter_mut().find(|(role, _)| *role == b'D').unwrap().1;
        descendant.write_all(b"P").unwrap();
        let mut reply = [0];
        descendant.read_exact(&mut reply).unwrap();
        assert_eq!(reply, *b"p");

        let started = Instant::now();
        let exit = command.observe(child, &NO_CANCELLATION, &mut ()).unwrap();
        assert!(
            started.elapsed() < CAPTURE_DEADLINE,
            "the capture waited for the descendant's pipes"
        );
        assert_eq!(exit.stop.status().unwrap(), status);
        assert_eq!(exit.stderr.render(|_| String::new()), CHILD_STDERR);
        let stdout = exit.stdout.render(|_| String::new());
        assert!(
            stdout
                .windows(CHILD_STDOUT.len())
                .any(|window| window == CHILD_STDOUT),
            "the root's stdout was lost: {:?}",
            String::from_utf8_lossy(&stdout)
        );
        assert_control_closed(descendant);
    }

    #[test]
    fn escaped_descendant_never_delays() {
        // Only a process group can be left, so the scenario exists where containment is escapable.
        if !CONTAINMENT_ESCAPABLE {
            return;
        }
        let (command, listener) = child_with_control("escaped-tree");
        let peers = RefCell::new(Vec::new());
        let exit_sent_at = std::cell::Cell::new(None);
        let started = Instant::now();
        let exit = command
            .run(
                &|| {
                    if let Some(peer) = accept_control(&listener) {
                        peers.borrow_mut().push(peer);
                    }
                    if peers.borrow().len() == 2 && exit_sent_at.get().is_none() {
                        let mut peers = peers.borrow_mut();
                        let root = &mut peers.iter_mut().find(|(role, _)| *role == b'R').unwrap().1;
                        root.write_all(b"E").unwrap();
                        exit_sent_at.set(Some(Instant::now()));
                    }
                    match exit_sent_at.get() {
                        Some(sent_at) => sent_at.elapsed() >= CAPTURE_DEADLINE,
                        None => started.elapsed() >= CHILD_READY_TIMEOUT,
                    }
                },
                &mut (),
            )
            .unwrap();
        let exit_sent_at = exit_sent_at.get().expect("the control handshake completed");
        assert!(
            exit_sent_at.elapsed() < CAPTURE_DEADLINE,
            "the capture waited for the escaped descendant's pipes"
        );
        assert_eq!(exit.stop.status().unwrap().code(), Some(0));
        let mut peers = peers.into_inner();
        let root = &mut peers.iter_mut().find(|(role, _)| *role == b'R').unwrap().1;
        assert_control_closed(root);
        assert!(
            exit.stderr
                .render(|_| String::new())
                .starts_with(CHILD_STDERR)
        );

        let descendant = &mut peers.iter_mut().find(|(role, _)| *role == b'D').unwrap().1;
        descendant.write_all(b"P").unwrap();
        let mut reply = [0];
        descendant.read_exact(&mut reply).unwrap();
        assert_eq!(reply, *b"p", "the capture killed the escaped descendant");
        descendant.write_all(b"W").unwrap();
        let mut closed = [0; 2];
        descendant.read_exact(&mut closed).unwrap();
        assert_eq!(&closed, b"BB", "the readers outlived the capture");
        descendant.write_all(b"E").unwrap();
        assert_control_closed(descendant);
    }

    #[test]
    fn cancel_terminates_the_whole_tree() {
        let (command, listener) = child_with_control("tree");
        let peers = RefCell::new(Vec::new());
        let exit = command
            .run(
                &|| {
                    if let Some(peer) = accept_control(&listener) {
                        peers.borrow_mut().push(peer);
                    }
                    peers.borrow().len() == 2
                },
                &mut (),
            )
            .unwrap();

        assert!(exit.stop.is_cancelled());
        let mut peers = peers.into_inner();
        peers.sort_by_key(|(role, _)| *role);
        let roles: Vec<_> = peers.iter().map(|(role, _)| *role).collect();
        assert_eq!(roles.as_slice(), b"DR");
        for (_, peer) in &mut peers {
            assert_control_closed(peer);
        }
    }

    #[test]
    fn cancelled_run_keeps_bounded_output() {
        let (command, listener) = child_with_control("diagnostic");
        let peer = RefCell::new(None);
        let exit = command
            .run(
                &|| {
                    if peer.borrow().is_none() {
                        *peer.borrow_mut() = accept_control(&listener);
                    }
                    peer.borrow().is_some()
                },
                &mut (),
            )
            .unwrap();

        assert!(exit.stop.is_cancelled());
        let stderr = exit
            .stderr
            .render(|omitted| format!("{omitted} bytes omitted"));
        assert!(stderr.ends_with(b"diagnostic"), "{}", stderr.len());
        assert!(stderr.len() < 16_000, "{}", stderr.len());
        assert!(
            std::str::from_utf8(&stderr)
                .unwrap()
                .contains("bytes omitted")
        );
        assert_control_closed(&mut peer.into_inner().unwrap().1);
    }
}
