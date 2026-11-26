//! Unix: process-group containment, and pipe reads made stoppable with
//! `O_NONBLOCK` and `poll`.

use std::io::{self, Read};
use std::os::fd::AsFd;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

use process_wrap::std::{CommandWrap, ProcessGroup};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};

use super::{ReaderStop, Stopped};

mod terminal;
pub(crate) use terminal::InheritedTerminal;

/// A child pipe this platform can read with a stop request.
///
/// Unix needs the descriptor, to make the read nonblocking.
pub(crate) trait Pipe: Read + Send + 'static + AsFd {}

impl<T: Read + Send + 'static + AsFd> Pipe for T {}

/// Readiness waits check the stop request this often.
const STOP_POLL_TIMEOUT: Timespec = Timespec {
    tv_sec: 0,
    tv_nsec: 25_000_000,
};

/// Place the child in its own process group so the whole tree can be signalled.
pub(crate) fn wrap(mut cmd: CommandWrap) -> CommandWrap {
    cmd.wrap(ProcessGroup::leader());
    cmd
}

pub fn terminating_signal(status: &ExitStatus) -> Option<i32> {
    status.signal()
}

pub(crate) struct Reader<T: Pipe> {
    inner: T,
    stop: ReaderStop,
    flags: OFlags,
}

impl<T: Pipe> Reader<T> {
    pub(crate) fn new(inner: T, stop: ReaderStop) -> io::Result<Self> {
        let flags = fcntl_getfl(&inner)?;
        fcntl_setfl(&inner, flags | OFlags::NONBLOCK)?;
        Ok(Self { inner, stop, flags })
    }

    fn await_readable(&self) -> io::Result<()> {
        loop {
            self.abandon_if_stopped()?;
            let mut fds = [PollFd::new(&self.inner, PollFlags::IN)];
            match poll(&mut fds, Some(&STOP_POLL_TIMEOUT)) {
                Ok(0) | Err(rustix::io::Errno::INTR) => {}
                Ok(_) => return Ok(()),
                Err(err) => return Err(err.into()),
            }
        }
    }

    fn abandon_if_stopped(&self) -> io::Result<()> {
        if self.stop.is_requested() {
            Err(io::Error::other(Stopped))
        } else {
            Ok(())
        }
    }

    fn read_unless_stopped(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            match self.inner.read(buf) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    self.await_readable()?;
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {
                    self.abandon_if_stopped()?;
                }
                Err(_) if self.stop.is_requested() => return Err(io::Error::other(Stopped)),
                res => return res,
            }
        }
    }
}

impl<T: Pipe + Read> Read for Reader<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_unless_stopped(buf)
    }
}

impl<T: Pipe> Drop for Reader<T> {
    fn drop(&mut self) {
        let _ = fcntl_setfl(&self.inner, self.flags);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::BorrowedFd;
    use std::os::unix::net::UnixStream;

    struct ScriptedRead {
        stream: UnixStream,
        outcome: Result<usize, io::ErrorKind>,
        stop: Option<ReaderStop>,
    }

    impl AsFd for ScriptedRead {
        fn as_fd(&self) -> BorrowedFd<'_> {
            self.stream.as_fd()
        }
    }

    impl Read for ScriptedRead {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if let Some(stop) = &self.stop {
                stop.request();
            }
            match std::mem::replace(&mut self.outcome, Ok(0)) {
                Ok(n) => {
                    buf[..n].fill(b'x');
                    Ok(n)
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
        let (stream, _peer) = UnixStream::pair().unwrap();
        Reader::new(
            ScriptedRead {
                stream,
                outcome,
                stop: request_stop.then(|| stop.clone()),
            },
            stop.clone(),
        )
        .unwrap()
    }

    #[test]
    fn read_beats_concurrent_stop() {
        let stop = ReaderStop::default();
        let mut reader = reader(&stop, Ok(4), true);
        let mut buf = [0; 8];

        assert_eq!(reader.read(&mut buf).unwrap(), 4);
        assert_eq!(&buf[..4], b"xxxx");
    }

    #[test]
    fn failed_read_reports_stop() {
        let stop = ReaderStop::default();
        let mut reader = reader(&stop, Err(io::ErrorKind::BrokenPipe), true);

        let err = reader.read(&mut [0; 8]).unwrap_err();

        assert!(err.get_ref().is_some_and(|e| e.is::<Stopped>()));
    }

    #[test]
    fn interrupted_read_stop() {
        let stop = ReaderStop::default();
        let mut reader = reader(&stop, Err(io::ErrorKind::Interrupted), true);

        let err = reader.read(&mut [0; 8]).unwrap_err();

        assert!(err.get_ref().is_some_and(|e| e.is::<Stopped>()));
    }

    #[test]
    fn drop_restores_flags() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        let alias = stream.try_clone().unwrap();
        let flags = fcntl_getfl(&alias).unwrap();

        let reader = Reader::new(stream, ReaderStop::default()).unwrap();
        assert_ne!(
            fcntl_getfl(&alias).unwrap(),
            flags,
            "the reader did not enable nonblocking reads"
        );

        drop(reader);

        assert_eq!(fcntl_getfl(&alias).unwrap(), flags);
    }
}
