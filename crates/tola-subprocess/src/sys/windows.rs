//! Windows: job-object containment, and pipe reads stopped by a monitor thread.
//!
//! A synchronous pipe read has no timeout, and Rust's child pipes are anonymous
//! pipes, which do not support overlapped I/O. Cancelling the pending read from a
//! monitor thread with `CancelSynchronousIo` is therefore the only way to observe
//! a stop without leaving the reading thread behind.

use std::io::{self, Read};
use std::process::ExitStatus;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use process_wrap::std::{CommandWrap, JobObject};

/// The Win32 calls this crate needs, behind safe wrappers.
///
/// `CancelSynchronousIo` requires `THREAD_TERMINATE` access to the thread whose
/// call is cancelled, so the monitor opens a handle to itself once and reuses it.
mod platform {
    #![allow(unsafe_code)]

    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    use windows_sys::Win32::System::IO::CancelSynchronousIo;
    use windows_sys::Win32::System::Threading::{GetCurrentThreadId, OpenThread, THREAD_TERMINATE};

    /// Open the calling thread with the access right `CancelSynchronousIo` requires.
    pub(super) fn open_current_thread() -> io::Result<OwnedHandle> {
        // SAFETY: opening the current thread with a known access right; a null
        // return is reported as an error below.
        let handle = unsafe { OpenThread(THREAD_TERMINATE, 0, GetCurrentThreadId()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `OpenThread` returned an owned handle, which closes with the
        // returned value and could not otherwise be released.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    /// Mark the target thread's pending synchronous operation as cancelled.
    ///
    /// The call returns whether a request was found; a thread with nothing pending
    /// reports `ERROR_NOT_FOUND`, which is not an error for the monitor.
    pub(super) fn cancel_pending_io(thread: &OwnedHandle) {
        // SAFETY: the handle was opened with `THREAD_TERMINATE` and stays owned by
        // the caller for the duration of the call.
        unsafe {
            CancelSynchronousIo(thread.as_raw_handle());
        }
    }
}

use super::{ReaderStop, Stopped};

/// A child pipe this platform can read with a stop request.
///
/// Windows needs only the blocking read itself: cancellation reaches the pending
/// call through the thread's monitor, not through the descriptor.
pub(crate) trait Pipe: Read + Send + 'static {}

impl<T: Read + Send + 'static> Pipe for T {}

/// The monitor checks the stop request this often.
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Place the child in a job object so terminating it reaches every descendant.
pub(crate) fn wrap(mut command: CommandWrap) -> CommandWrap {
    command.wrap(JobObject);
    command
}

/// The signal that terminated the child, or `None` when it exited on its own.
///
/// Windows has no signals: a terminated process reports an exit code instead.
pub fn terminating_signal(_status: &ExitStatus) -> Option<i32> {
    None
}

/// A pipe read end that observes a stop request while its read is pending.
pub(crate) struct Reader<T: Pipe> {
    inner: T,
    stop: ReaderStop,
    monitor: Option<Monitor>,
}

impl<T: Pipe> Reader<T> {
    /// Retain the stream; the monitor is installed on the first read, once the
    /// thread performing synchronous I/O is known.
    pub(crate) fn new(inner: T, stop: ReaderStop) -> io::Result<Self> {
        Ok(Self {
            inner,
            stop,
            monitor: None,
        })
    }
}

impl<T: Pipe> Read for Reader<T> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self
            .monitor
            .as_ref()
            .is_none_or(|monitor| monitor.owner != std::thread::current().id())
        {
            // Join the previous owner's monitor before installing another.
            self.monitor = None;
            self.monitor = Some(Monitor::new(self.stop.clone())?);
        }
        let result = self
            .monitor
            .as_ref()
            .expect("the read monitor is installed")
            .run(&self.stop, || self.inner.read(bytes));
        // A cancel request can lose the race against a read that already returned
        // bytes: only a failed read is reported as stopped.
        match result {
            Err(_) if self.stop.is_requested() => Err(io::Error::other(Stopped)),
            result => result,
        }
    }
}

/// One monitor thread per reading thread, joined by `Drop`.
struct Monitor {
    owner: std::thread::ThreadId,
    done: std::sync::Arc<AtomicBool>,
    armed: std::sync::Arc<Mutex<bool>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Monitor {
    fn new(stop: ReaderStop) -> io::Result<Self> {
        let handle = platform::open_current_thread()?;
        let done = std::sync::Arc::new(AtomicBool::new(false));
        let stopped = std::sync::Arc::clone(&done);
        let armed = std::sync::Arc::new(Mutex::new(false));
        let observed = std::sync::Arc::clone(&armed);
        let worker = std::thread::Builder::new()
            .name("tola-subprocess-monitor".into())
            .spawn(move || {
                while !stopped.load(Ordering::Acquire) {
                    if stop.is_requested() {
                        // Serialize arming with cancellation so a finished read
                        // cannot cancel the thread's next unrelated operation.
                        let armed = observed
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if *armed {
                            platform::cancel_pending_io(&handle);
                        }
                    }
                    std::thread::park_timeout(STOP_POLL_INTERVAL);
                }
            })?;
        Ok(Self {
            owner: std::thread::current().id(),
            done,
            armed,
            worker: Some(worker),
        })
    }

    fn run<R>(
        &self,
        stop: &ReaderStop,
        operation: impl FnOnce() -> io::Result<R>,
    ) -> io::Result<R> {
        *self
            .armed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        let armed = Armed(&self.armed);
        let result = operation();
        drop(armed);
        if stop.is_requested() && result.is_err() {
            return Err(io::Error::other(Stopped));
        }
        result
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

/// Disarms cancellation for the duration of one operation.
struct Armed<'a>(&'a Mutex<bool>);

impl Drop for Armed<'_> {
    fn drop(&mut self) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream whose next read returns `outcome`, optionally requesting a stop
    /// first, so both outcomes are deterministic.
    struct ScriptedRead {
        outcome: Result<usize, io::ErrorKind>,
        stop: Option<ReaderStop>,
    }

    impl Read for ScriptedRead {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if let Some(stop) = &self.stop {
                stop.request();
            }
            match self.outcome {
                Ok(read) => {
                    bytes[..read].fill(b'x');
                    Ok(read)
                }
                Err(kind) => Err(kind.into()),
            }
        }
    }

    fn reader(
        stop: &ReaderStop,
        outcome: Result<usize, io::ErrorKind>,
        request_stop: bool,
    ) -> Reader<ScriptedRead> {
        Reader::new(
            ScriptedRead {
                outcome,
                stop: request_stop.then(|| stop.clone()),
            },
            stop.clone(),
        )
        .unwrap()
    }

    #[test]
    fn completed_read_beats_concurrent_stop() {
        let stop = ReaderStop::default();
        let mut reader = reader(&stop, Ok(4), true);
        let mut bytes = [0; 8];

        assert_eq!(reader.read(&mut bytes).unwrap(), 4);
        assert_eq!(&bytes[..4], b"xxxx");
    }

    #[test]
    fn failed_read_reports_the_stop_request() {
        let stop = ReaderStop::default();
        let mut reader = reader(&stop, Err(io::ErrorKind::BrokenPipe), true);

        let error = reader.read(&mut [0; 8]).unwrap_err();

        assert!(error.get_ref().is_some_and(|inner| inner.is::<Stopped>()));
    }
}
