//! Running one declared command to its exit, with captured or inherited streams.

use std::{
    ffi::{OsStr, OsString},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command as StdCommand, Stdio},
    thread,
    time::Duration,
};

use process_wrap::std::CommandWrap;

use crate::capture::Stream;
use crate::child::Child;
use crate::relay::Relay;
use crate::{Cancellation, Error, Exit, Result, Stop, sys};

const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Receives each stream's output while a command runs.
///
/// Callbacks run on the calling thread of [`Command::run`]. Each stream preserves
/// read order; ordering between streams is unspecified. Slow callbacks delay
/// cancellation polling and drain shutdown.
pub trait Observer {
    /// Whether output and completion callbacks are wanted. Queried once at start.
    fn observes_output(&self) -> bool {
        true
    }

    fn output(&mut self, stream: Stream, bytes: &[u8]);

    fn stopped(&mut self, _stop: &Stop) {}

    /// Called once per stream after its last output callback on EOF or drain stop.
    ///
    /// Completion is guaranteed when a started run returns `Ok`, including cancellation.
    fn finished(&mut self, stream: Stream);
}

impl Observer for () {
    fn observes_output(&self) -> bool {
        false
    }

    fn output(&mut self, _: Stream, _: &[u8]) {}

    fn finished(&mut self, _: Stream) {}
}

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

    /// First item is the program; an empty iterator yields an empty program that fails to spawn.
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

    pub fn cwd(mut self, dir: impl AsRef<Path>) -> Self {
        self.cwd = Some(dir.as_ref().to_owned());
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, val: impl Into<OsString>) -> Self {
        self.env.push((key.into(), Some(val.into())));
        self
    }

    pub fn env_remove(mut self, key: impl Into<OsString>) -> Self {
        self.env.push((key.into(), None));
        self
    }

    pub fn program(&self) -> &OsStr {
        &self.program
    }

    /// Run a non-interactive command with null stdin and bounded output capture.
    pub fn run(&self, cancel: &impl Cancellation, obs: &mut impl Observer) -> Result<Exit> {
        if cancel.is_cancelled() {
            return Ok(Exit::cancelled(None));
        }
        let child = self.spawn()?;
        Self::observe(child, cancel, obs)
    }

    /// Run with inherited stdio without capturing output.
    ///
    /// On Unix, a foreground caller lends its controlling terminal to the child's
    /// process group before exec and restores it on return or unwind. Only one run
    /// in this process may own the controlling terminal at a time.
    pub fn run_inherited(&self, cancel: &(impl Cancellation + ?Sized)) -> Result<Stop> {
        if cancel.is_cancelled() {
            return Ok(Stop::Cancelled(None));
        }
        let mut cmd = self.command();
        cmd.stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let mut term = sys::InheritedTerminal::prepare(&mut cmd).map_err(Error::Terminal)?;
        let outcome = (|| {
            if cancel.is_cancelled() {
                return Ok(Stop::Cancelled(None));
            }
            let wrapper = sys::wrap(CommandWrap::from(cmd))
                .spawn()
                .map_err(Error::Spawn)?;
            Child::new(wrapper).wait_for_stop(cancel, || {
                thread::sleep(CHILD_POLL_INTERVAL);
                Ok(())
            })
        })();
        // Child is stopped before reclaiming the terminal; Drop retries failed restoration.
        let restored = term.restore().map_err(Error::Terminal);
        outcome.and_then(|stop| restored.map(|()| stop))
    }

    fn observe(
        mut child: Child,
        cancel: &impl Cancellation,
        obs: &mut impl Observer,
    ) -> Result<Exit> {
        let stdout = take_pipe(child.wrapper_mut().stdout().take())?;
        let stderr = take_pipe(child.wrapper_mut().stderr().take())?;
        let (mut relay, tx) = Relay::new(obs.observes_output());
        relay.spawn(stdout, Stream::Stdout, &tx)?;
        relay.spawn(stderr, Stream::Stderr, &tx)?;
        drop(tx);
        capture_until_exit(&mut child, relay, cancel, obs)
    }

    fn spawn(&self) -> Result<Child> {
        let mut cmd = self.command();
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = sys::wrap(CommandWrap::from(cmd))
            .spawn()
            .map_err(Error::Spawn)?;
        Ok(Child::new(child))
    }

    fn command(&self) -> StdCommand {
        let mut cmd = StdCommand::new(&self.program);
        cmd.args(&self.args);
        if let Some(dir) = &self.cwd {
            cmd.current_dir(dir);
        }
        for (k, v) in &self.env {
            match v {
                Some(v) => {
                    cmd.env(k, v);
                }
                None => {
                    cmd.env_remove(k);
                }
            }
        }
        cmd
    }
}

fn capture_until_exit(
    child: &mut Child,
    mut relay: Relay,
    cancel: &impl Cancellation,
    obs: &mut impl Observer,
) -> Result<Exit> {
    let stop = child.wait_for_stop(cancel, || relay.receive(CHILD_POLL_INTERVAL, obs))?;
    obs.stopped(&stop);
    let [stdout, stderr] = relay.finish(obs)?;
    Ok(Exit {
        stop,
        stdout,
        stderr,
    })
}

fn take_pipe<T: Read + Send + 'static>(pipe: Option<T>) -> Result<T> {
    pipe.ok_or_else(|| Error::Capture(io::Error::other("a child stream is not captured")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NO_CANCELLATION;
    use crate::relay::{RelayEvent, capture_from, read_stream};
    use std::{
        cell::RefCell,
        io::Write,
        net::{TcpListener, TcpStream},
        time::Instant,
    };

    const CHILD_MODE: &str = "TOLA_SUBPROCESS_TEST";
    const CHILD_CONTROL: &str = "TOLA_SUBPROCESS_CONTROL";
    const CHILD_ENV: &str = "TOLA_SUBPROCESS_VALUE";
    const CHILD_STDIN: &[u8] = b"caller input\n";
    const CHILD_STDOUT: &[u8] = b"root:\0\xffstdout\n";
    const CHILD_STDERR: &[u8] = b"root:\0\xfestderr\n";
    const CHILD_ARGUMENT: &str = "one argument; $value with spaces\nand Unicode: 树";
    const CHILD_READY_TIMEOUT: Duration = Duration::from_secs(30);

    #[cfg(windows)]
    fn escape_containment(_: &mut std::process::Command) {}

    #[derive(Default)]
    struct RelayedOutput {
        streams: [Vec<u8>; 2],
        finished: [bool; 2],
    }

    impl Observer for RelayedOutput {
        fn output(&mut self, stream: Stream, bytes: &[u8]) {
            let idx = stream.index();
            assert!(!self.finished[idx], "output followed stream completion");
            self.streams[idx].extend_from_slice(bytes);
        }

        fn finished(&mut self, stream: Stream) {
            let idx = stream.index();
            assert!(!self.finished[idx], "stream completed twice");
            self.finished[idx] = true;
        }
    }

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

    #[cfg(unix)]
    fn escape_containment(cmd: &mut std::process::Command) {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    #[test]
    fn process_child() {
        let Ok(mode) = std::env::var(CHILD_MODE) else {
            return;
        };
        match run_child_mode(&mode) {
            Ok(code) => std::process::exit(code),
            Err(err) => {
                eprintln!("subprocess child run failed: {err}");
                std::process::exit(1);
            }
        }
    }

    fn run_child_mode(mode: &str) -> io::Result<i32> {
        match mode {
            "inherited-cancel-tree" => {
                inherited_cancel_tree();
                return Ok(0);
            }
            "inherited-exit-tree" => {
                inherited_exit_tree();
                return Ok(0);
            }
            "inherited-owner" => {
                let dir = std::env::temp_dir().canonicalize()?;
                let stop = process_child_command("inherited-io")
                    .arg(CHILD_ARGUMENT)
                    .cwd(&dir)
                    .env(CHILD_ENV, "inherited environment")
                    .env("TOLA_SUBPROCESS_CWD", &dir)
                    .run_inherited(&NO_CANCELLATION)
                    .map_err(io::Error::other)?;
                if !matches!(stop, Stop::Exited(s) if s.code() == Some(37)) {
                    return Err(io::Error::other("the inherited exit status was lost"));
                }
                return Ok(0);
            }
            "inherited-io" => {
                let mut input = [0; CHILD_STDIN.len()];
                io::stdin().read_exact(&mut input)?;
                if input.as_slice() != CHILD_STDIN
                    || std::env::args_os().last().as_deref() != Some(OsStr::new(CHILD_ARGUMENT))
                    || std::env::var(CHILD_ENV).as_deref() != Ok("inherited environment")
                    || std::env::current_dir()?.canonicalize()?
                        != std::env::var_os("TOLA_SUBPROCESS_CWD").unwrap()
                {
                    return Err(io::Error::other("the inherited command inputs changed"));
                }
                io::stdout().write_all(CHILD_STDOUT)?;
                io::stdout().flush()?;
                io::stderr().write_all(CHILD_STDERR)?;
                io::stderr().flush()?;
                return Ok(37);
            }
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
            "descendant" | "quiet-tree" => {}
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
        let descendant = if matches!(mode, "tree" | "quiet-tree" | "escaped-tree") {
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
                    b'E' if matches!(mode, "tree" | "quiet-tree") => return Ok(37),
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

    fn ready_child(mode: &str) -> (Child, TcpStream) {
        let (command, listener) = child_with_control(mode);
        let child = command.spawn().unwrap();
        let deadline = Instant::now() + CHILD_READY_TIMEOUT;
        loop {
            if let Some((_, peer)) = accept_control(&listener) {
                return (child, peer);
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

    fn run_piped_child(mode: &str) -> std::process::Output {
        let argv = process_child_argv();
        let mut child = StdCommand::new(&argv[0])
            .args(&argv[1..])
            .env(CHILD_MODE, mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(CHILD_STDIN).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    #[test]
    fn preserves_caller_stdin() {
        run_piped_child("stdin-owner");
    }

    #[test]
    fn inherited_stdio() {
        let out = run_piped_child("inherited-owner");
        assert!(
            out.stdout
                .windows(CHILD_STDOUT.len())
                .any(|b| b == CHILD_STDOUT)
        );
        assert_eq!(out.stderr, CHILD_STDERR);
    }

    #[test]
    fn inherited_cancel_before_spawn() {
        let cancelled: &dyn Cancellation = &|| true;
        let stop = Command::new("").run_inherited(cancelled).unwrap();
        assert!(matches!(stop, Stop::Cancelled(None)));
    }

    #[test]
    fn inherited_cancel_descendants() {
        assert_inherited_owner("inherited-cancel-tree");
    }

    fn inherited_cancel_tree() {
        let (cmd, listener) = child_with_control("quiet-tree");
        let peers = RefCell::new(Vec::new());
        let deadline = Instant::now() + CHILD_READY_TIMEOUT;
        let stop = cmd
            .run_inherited(&|| {
                if let Some(peer) = accept_control(&listener) {
                    peers.borrow_mut().push(peer);
                }
                peers.borrow().len() == 2 || Instant::now() > deadline
            })
            .unwrap();

        assert!(stop.is_cancelled());
        let mut peers = peers.into_inner();
        assert_eq!(peers.len(), 2, "the child tree did not become ready");
        for (_, peer) in &mut peers {
            assert_control_closed(peer);
        }
    }

    #[test]
    fn inherited_exit_descendants() {
        assert_inherited_owner("inherited-exit-tree");
    }

    fn inherited_exit_tree() {
        let (cmd, listener) = child_with_control("quiet-tree");
        let peers = RefCell::new(Vec::new());
        let sent = std::cell::Cell::new(false);
        let deadline = Instant::now() + CHILD_READY_TIMEOUT;
        let stop = cmd
            .run_inherited(&|| {
                if let Some(peer) = accept_control(&listener) {
                    peers.borrow_mut().push(peer);
                }
                if peers.borrow().len() == 2 && !sent.replace(true) {
                    let mut peers = peers.borrow_mut();
                    let (_, root) = peers.iter_mut().find(|(role, _)| *role == b'R').unwrap();
                    root.write_all(b"E").unwrap();
                }
                Instant::now() > deadline
            })
            .unwrap();

        assert!(matches!(stop, Stop::Exited(s) if s.code() == Some(37)));
        for (_, peer) in &mut peers.into_inner() {
            assert_control_closed(peer);
        }
    }

    fn assert_inherited_owner(mode: &str) {
        let argv = process_child_argv();
        let out = StdCommand::new(&argv[0])
            .args(&argv[1..])
            .env(CHILD_MODE, mode)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn eof_finishes_streams() {
        let mut obs = RelayedOutput::default();
        let exit = process_child_command("success")
            .run(&NO_CANCELLATION, &mut obs)
            .unwrap();

        assert!(matches!(exit.stop, Stop::Exited(s) if s.success()));
        assert_eq!(obs.finished, [true; 2]);
        assert_eq!(obs.streams[1], CHILD_STDERR);
        for (cap, seen) in [exit.stdout, exit.stderr].iter().zip(obs.streams) {
            assert!(!cap.stopped_early());
            assert_eq!(seen, cap.render(|_| String::new()));
        }
    }

    #[test]
    fn env_last_wins() {
        for (cmd, expected) in [
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
            let exit = cmd.run(&NO_CANCELLATION, &mut ()).unwrap();
            assert!(exit.stop.status().unwrap().success());
            assert_eq!(exit.stderr.render(|_| String::new()), expected.as_bytes());
        }
    }

    #[test]
    fn reader_failure_stops_child() {
        let mut polls_before_err = Vec::new();
        for panic_reader in [false, true] {
            let (mut child, mut peer) = ready_child("held-streams");
            let deadline = Instant::now() + CHILD_READY_TIMEOUT;
            let (other, _writer) = io::pipe().unwrap();
            let (mut relay, tx) = Relay::new(false);
            let worker_tx = tx.clone();
            let stop = relay.stop.clone();
            relay.workers[0] = Some(thread::spawn(move || {
                assert!(!panic_reader, "stream reader panic");
                let cap = capture_from(read_stream(
                    BrokenPipeReader,
                    stop,
                    &worker_tx,
                    Stream::Stdout,
                    false,
                ));
                let _ = worker_tx.send(RelayEvent::Finished(Stream::Stdout, cap));
            }));
            relay.spawn(other, Stream::Stderr, &tx).unwrap();
            drop(tx);
            while !relay.workers[0].as_ref().unwrap().is_finished() {
                assert!(
                    Instant::now() < deadline,
                    "the failed reader did not finish"
                );
                thread::yield_now();
            }
            let polls = std::cell::Cell::new(0);
            let err = capture_until_exit(
                &mut child,
                relay,
                &|| {
                    polls.set(polls.get() + 1);
                    polls.get() == 5
                },
                &mut (),
            )
            .unwrap_err();
            assert!(matches!(err, Error::Read(_)), "{err:?}");
            if !panic_reader {
                assert!(
                    err.source_io()
                        .get_ref()
                        .is_some_and(|inner| inner.is::<PipeFailure>())
                );
            }
            polls_before_err.push(polls.get());
            drop(child);
            assert_control_closed(&mut peer);
        }
        assert!(
            polls_before_err.iter().all(|p| *p < 5),
            "reader failure waited for cancellation: {polls_before_err:?}"
        );
    }

    #[test]
    fn observer_panic_stops_child() {
        struct PanickingObserver;
        impl Observer for PanickingObserver {
            fn output(&mut self, _: Stream, _: &[u8]) {
                panic!("output callback panic");
            }
            fn finished(&mut self, _: Stream) {}
        }
        let (child, mut peer) = ready_child("held-streams");
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Command::observe(child, &NO_CANCELLATION, &mut PanickingObserver)
        }));
        assert!(panicked.is_err());
        assert_control_closed(&mut peer);
    }

    #[test]
    fn cancel_terminates_tree() {
        let (cmd, listener) = child_with_control("tree");
        let peers = RefCell::new(Vec::new());
        let exit = cmd
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
    fn cancelled_bounded_output() {
        let (cmd, listener) = child_with_control("diagnostic");
        let peer = RefCell::new(None);
        let exit = cmd
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
