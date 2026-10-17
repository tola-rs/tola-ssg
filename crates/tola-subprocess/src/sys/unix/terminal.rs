//! Foreground ownership for a contained command using the caller's terminal.

#![allow(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, TryLockError};

static TERMINAL_OWNER: Mutex<()> = Mutex::new(());

/// Keeps ownership until the child is stopped and the terminal has been restored.
pub(crate) struct InheritedTerminal(Option<ForegroundTerminal>);

struct ForegroundTerminal {
    file: File,
    previous_foreground: libc::pid_t,
    attributes: libc::termios,
    _owner: MutexGuard<'static, ()>,
}

impl InheritedTerminal {
    pub(crate) fn prepare(command: &mut Command) -> io::Result<Self> {
        if !io::stdin().is_terminal() && !io::stdout().is_terminal() && !io::stderr().is_terminal()
        {
            return Ok(Self(None));
        }
        let owner = match TERMINAL_OWNER.try_lock() {
            Ok(owner) => owner,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "another command in this process owns the terminal",
                ));
            }
        };
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open("/dev/tty")
        {
            Ok(file) => file,
            // A terminal descriptor need not be this process's controlling tty.
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(libc::ENXIO | libc::ENODEV | libc::ENOTTY | libc::ENOENT)
                ) =>
            {
                return Ok(Self(None));
            }
            Err(error) => return Err(error),
        };
        let fd = file.as_raw_fd();
        // SAFETY: the owned descriptor remains open; both calls take no pointers.
        let foreground = unsafe { libc::tcgetpgrp(fd) };
        if foreground == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: getpgrp has no arguments or preconditions.
        if foreground != unsafe { libc::getpgrp() } {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "the caller is not the terminal's foreground process group",
            ));
        }
        let mut attributes = MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr initializes the writable termios on success.
        if unsafe { libc::tcgetattr(fd, attributes.as_mut_ptr()) } == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the successful tcgetattr above initialized the entire value.
        let attributes = unsafe { attributes.assume_init() };
        // SAFETY: this callback only uses async-signal-safe POSIX calls and
        // constructs errors from errno. The guard keeps fd open through spawn.
        // Handoff happens before exec, so the new group cannot read early and
        // stop on SIGTTIN while the parent is still returning from spawn.
        unsafe {
            command.pre_exec(move || {
                if libc::setpgid(0, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                with_sigttou_blocked(true, || set_foreground(fd, libc::getpid()))
            });
        }
        Ok(Self(Some(ForegroundTerminal {
            file,
            previous_foreground: foreground,
            attributes,
            _owner: owner,
        })))
    }

    pub(crate) fn restore(&mut self) -> io::Result<()> {
        let Some(terminal) = &self.0 else {
            return Ok(());
        };
        let fd = terminal.file.as_raw_fd();
        with_sigttou_blocked(false, || {
            set_foreground(fd, terminal.previous_foreground)?;
            loop {
                // SAFETY: fd is owned and attributes came from tcgetattr for it.
                if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &terminal.attributes) } == 0 {
                    return Ok(());
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        })?;
        self.0 = None;
        Ok(())
    }
}

impl Drop for InheritedTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn set_foreground(fd: RawFd, group: libc::pid_t) -> io::Result<()> {
    loop {
        // SAFETY: callers keep fd open and supply a process group in this session.
        if unsafe { libc::tcsetpgrp(fd, group) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// Changing the foreground group from the background must not stop this process.
/// The mask is thread-local; no process-wide signal disposition is changed.
fn with_sigttou_blocked(
    after_fork: bool,
    operation: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let mut blocked = MaybeUninit::<libc::sigset_t>::uninit();
    let mut previous = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: the signal-set calls initialize and then mutate their valid output.
    unsafe {
        if libc::sigemptyset(blocked.as_mut_ptr()) == -1
            || libc::sigaddset(blocked.as_mut_ptr(), libc::SIGTTOU) == -1
        {
            return Err(io::Error::last_os_error());
        }
    }
    mask_signals(
        after_fork,
        libc::SIG_BLOCK,
        blocked.as_ptr(),
        previous.as_mut_ptr(),
    )?;
    let outcome = operation();
    let restored = mask_signals(
        after_fork,
        libc::SIG_SETMASK,
        previous.as_ptr(),
        std::ptr::null_mut(),
    );
    outcome.and(restored)
}

fn mask_signals(
    after_fork: bool,
    how: libc::c_int,
    set: *const libc::sigset_t,
    previous: *mut libc::sigset_t,
) -> io::Result<()> {
    // SAFETY: the caller supplies initialized input and writable output (or null).
    // In the forked child use POSIX's async-signal-safe sigprocmask. In the
    // multithreaded parent use the explicitly thread-local pthread interface.
    if after_fork {
        if unsafe { libc::sigprocmask(how, set, previous) } == -1 {
            return Err(io::Error::last_os_error());
        }
    } else {
        let error = unsafe { libc::pthread_sigmask(how, set, previous) };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
    }
    Ok(())
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use crate::{Command as ExternalCommand, NO_CANCELLATION, Stop};
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use std::cell::Cell;
    use std::io::{Read, Write};
    use std::os::fd::FromRawFd;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Child, ExitStatus, Stdio};
    use std::time::{Duration, Instant};

    const MODE: &str = "TOLA_SUBPROCESS_TERMINAL_TEST";
    const DEADLINE: Duration = Duration::from_secs(10);

    fn attributes(fd: RawFd) -> libc::termios {
        let mut attributes = MaybeUninit::<libc::termios>::uninit();
        // SAFETY: the caller supplies an open terminal; the success initializes it.
        assert_eq!(unsafe { libc::tcgetattr(fd, attributes.as_mut_ptr()) }, 0);
        unsafe { attributes.assume_init() }
    }

    /// Runs only in an isolated session under the PTY regressions below.
    #[test]
    fn terminal_child() {
        let Ok(mode) = std::env::var(MODE) else {
            return;
        };
        let original = attributes(libc::STDIN_FILENO);
        // SAFETY: stdin is the controlling terminal set by PtyRun::new.
        let foreground = unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) };
        match mode.as_str() {
            "read" => {
                let deadline = Instant::now() + DEADLINE / 2;
                let stop = ExternalCommand::new("/bin/sh")
                    .args([
                        "-c",
                        "IFS= read -r line; [ \"$line\" = 'terminal input' ] || exit 9; stty -echo -icanon; printf 'CHILD-READ-OK\\n'; exit 37",
                    ])
                    .run_inherited(&|| Instant::now() > deadline)
                    .unwrap();
                assert!(matches!(stop, Stop::Exited(status) if status.code() == Some(37)));
            }
            "spawn-error" => {
                assert!(matches!(
                    ExternalCommand::new("/no/such/tola-subprocess-command")
                        .run_inherited(&NO_CANCELLATION),
                    Err(crate::Error::Spawn(_))
                ));
            }
            "cancel" | "panic" => {
                let changed = Cell::new(false);
                let deadline = Instant::now() + DEADLINE / 2;
                let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    ExternalCommand::new("/bin/sh")
                        .args(["-c", "stty -echo -icanon; sleep 60"])
                        .run_inherited(&|| {
                            if attributes(libc::STDIN_FILENO).c_lflag & libc::ICANON == 0 {
                                changed.set(true);
                                assert_ne!(mode, "panic", "cancel callback panic");
                                return true;
                            }
                            Instant::now() > deadline
                        })
                }));
                assert!(
                    changed.get(),
                    "the command did not change terminal settings"
                );
                if mode == "panic" {
                    assert!(run.is_err());
                } else {
                    assert!(run.unwrap().unwrap().is_cancelled());
                }
            }
            "interrupt" => {
                let deadline = Instant::now() + DEADLINE / 2;
                let stop = ExternalCommand::new("/bin/sh")
                    .args(["-c", "printf 'INTERRUPT-READY\\n'; IFS= read -r line"])
                    .run_inherited(&|| Instant::now() > deadline)
                    .unwrap();
                assert!(matches!(stop, Stop::Exited(status)
                    if status.signal() == Some(libc::SIGINT) || status.code() == Some(130)));
            }
            _ => panic!("unknown terminal child mode"),
        }
        // SAFETY: the inherited tty remains open throughout this test process.
        assert_eq!(unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) }, foreground);
        let restored = attributes(libc::STDIN_FILENO);
        // BSD sets PENDIN when returning to canonical mode, to reprocess any
        // queued input. It is kernel state rather than a changed user setting.
        assert_eq!(
            restored.c_lflag & !libc::PENDIN,
            original.c_lflag & !libc::PENDIN
        );
        assert_eq!(restored.c_iflag, original.c_iflag);
        assert_eq!(restored.c_oflag, original.c_oflag);
        assert_eq!(restored.c_cc, original.c_cc);
        println!("TERMINAL-RESTORED");
        io::stdout().flush().unwrap();
        std::process::exit(0);
    }

    struct PtyRun {
        master: File,
        child: Child,
    }

    impl PtyRun {
        fn new(mode: &str) -> Self {
            let mut master = -1;
            let mut slave = -1;
            // SAFETY: both output pointers are valid; null selects default tty settings.
            assert_eq!(
                unsafe {
                    libc::openpty(
                        &mut master,
                        &mut slave,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            // SAFETY: openpty returned two new descriptors owned by this scope.
            let master = unsafe { File::from_raw_fd(master) };
            let slave = unsafe { File::from_raw_fd(slave) };
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "sys::unix::terminal::tests::terminal_child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(MODE, mode)
                .stdin(Stdio::from(slave.try_clone().unwrap()))
                .stdout(Stdio::from(slave.try_clone().unwrap()))
                .stderr(Stdio::from(slave));
            // SAFETY: only async-signal-safe calls run after fork. The stdio setup
            // has already made the slave fd stdin before this callback executes.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1
                        || libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as _, 0) == -1
                    {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let child = command.spawn().unwrap();
            let flags = rustix::fs::fcntl_getfl(&master).unwrap();
            rustix::fs::fcntl_setfl(&master, flags | rustix::fs::OFlags::NONBLOCK).unwrap();
            Self { master, child }
        }

        fn finish(&mut self, input: Option<&[u8]>, interrupt: bool) -> (ExitStatus, Vec<u8>) {
            if let Some(input) = input {
                self.master.write_all(input).unwrap();
            }
            let deadline = Instant::now() + DEADLINE;
            let mut output = Vec::new();
            let mut interrupted = false;
            loop {
                let mut bytes = [0; 4096];
                loop {
                    match self.master.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(read) => output.extend_from_slice(&bytes[..read]),
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        // Linux reports EIO, macOS EOF, when the slave closes.
                        Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) => panic!("PTY read: {error}"),
                    }
                }
                if interrupt
                    && !interrupted
                    && output
                        .windows(b"INTERRUPT-READY".len())
                        .any(|bytes| bytes == b"INTERRUPT-READY")
                {
                    self.master.write_all(&[3]).unwrap();
                    interrupted = true;
                }
                if let Some(status) = self.child.try_wait().unwrap() {
                    return (status, output);
                }
                assert!(
                    Instant::now() < deadline,
                    "the terminal command did not finish: {}",
                    String::from_utf8_lossy(&output)
                );
                let mut descriptors = [PollFd::new(&self.master, PollFlags::IN)];
                let _ = poll(
                    &mut descriptors,
                    Some(&Timespec {
                        tv_sec: 0,
                        tv_nsec: 10_000_000,
                    }),
                );
            }
        }
    }

    impl Drop for PtyRun {
        fn drop(&mut self) {
            // SAFETY: this isolated PTY belongs to the test session. On a failed
            // test, stop both its foreground command and its session owner.
            unsafe {
                let foreground = libc::tcgetpgrp(self.master.as_raw_fd());
                if foreground > 0 {
                    libc::kill(-foreground, libc::SIGKILL);
                }
                libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn contains(output: &[u8], marker: &[u8]) -> bool {
        output.windows(marker.len()).any(|bytes| bytes == marker)
    }

    #[test]
    fn inherited_terminal_reads_and_restores_after_exit() {
        let (status, output) = PtyRun::new("read").finish(Some(b"terminal input\n"), false);
        assert!(status.success(), "{}", String::from_utf8_lossy(&output));
        assert!(contains(&output, b"CHILD-READ-OK"));
        assert!(contains(&output, b"TERMINAL-RESTORED"));
    }

    #[test]
    fn inherited_terminal_restores_after_abnormal_exit() {
        for mode in ["spawn-error", "cancel", "panic"] {
            let (status, output) = PtyRun::new(mode).finish(None, false);
            assert!(
                status.success(),
                "{mode}: {}",
                String::from_utf8_lossy(&output)
            );
            assert!(contains(&output, b"TERMINAL-RESTORED"));
        }
    }

    #[test]
    fn inherited_terminal_sends_interrupt_to_child() {
        let (status, output) = PtyRun::new("interrupt").finish(None, true);
        assert!(status.success(), "{}", String::from_utf8_lossy(&output));
        assert!(contains(&output, b"TERMINAL-RESTORED"));
    }
}
