//! Cancellable reads of explicitly requested standard input.

use crate::cancellation::Cancellation;
use anyhow::Result;

/// Read all standard input, stopping if the CLI is cancelled.
#[cfg(unix)]
pub(super) fn read_stdin(cancellation: &Cancellation) -> Result<Vec<u8>> {
    let stdin = std::io::stdin();
    read_unix(stdin.lock(), &cancellation.token())
}

#[cfg(unix)]
fn read_unix<R: std::io::Read + std::os::fd::AsFd>(
    mut input: R,
    cancellation: &tola_build::cancellation::BuildCancellation,
) -> Result<Vec<u8>> {
    use rustix::event::Timespec;

    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        cancellation.ensure_active()?;
        let timeout = Timespec {
            tv_sec: 0,
            tv_nsec: 50_000_000,
        };
        let ready = match wait_unix(&input, &timeout) {
            Err(rustix::io::Errno::INTR) => continue,
            result => result?,
        };
        if ready == 0 {
            continue;
        }
        cancellation.ensure_active()?;
        match input.read(&mut buffer) {
            Ok(0) => {
                cancellation.ensure_active()?;
                return Ok(output);
            }
            Ok(count) => output.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
}

// On macOS poll does not support /dev/tty or /dev/null. select does, and the
// borrowed descriptor stays open for the whole call.
#[cfg(all(unix, target_vendor = "apple"))]
fn wait_unix(
    input: &impl std::os::fd::AsFd,
    timeout: &rustix::event::Timespec,
) -> rustix::io::Result<usize> {
    use rustix::event::{FdSetElement, fd_set_insert, fd_set_num_elements, select};
    use std::os::fd::AsRawFd;
    let fd = input.as_fd();
    let raw = fd.as_raw_fd();
    let nfds = raw.checked_add(1).ok_or(rustix::io::Errno::INVAL)?;
    let mut fds = vec![FdSetElement::default(); fd_set_num_elements(1, nfds)];
    fd_set_insert(&mut fds, raw);
    // SAFETY: `fd` remains borrowed and open, and is the only descriptor in the set.
    unsafe { select(nfds, Some(&mut fds), None, None, Some(timeout)) }.map(|count| count as usize)
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn wait_unix(
    input: &impl std::os::fd::AsFd,
    timeout: &rustix::event::Timespec,
) -> rustix::io::Result<usize> {
    use rustix::event::{PollFd, PollFlags, poll};
    let mut fd = [PollFd::new(input, PollFlags::IN)];
    poll(&mut fd, Some(timeout))
}

/// Windows can cancel a synchronous read on the worker itself, then join it.
#[cfg(windows)]
pub(super) fn read_stdin(cancellation: &Cancellation) -> Result<Vec<u8>> {
    use std::io::Read;
    use std::os::windows::io::AsRawHandle;
    let token = cancellation.token();
    token.ensure_active()?;
    let worker_token = token.clone();
    let worker = std::thread::spawn(move || -> Result<Vec<u8>> {
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        let mut output = Vec::new();
        let mut buffer = [0; 8192];
        loop {
            worker_token.ensure_active()?;
            match input.read(&mut buffer) {
                Ok(0) => return Ok(output),
                Ok(count) => output.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    worker_token.ensure_active()?;
                    return Err(error.into());
                }
            }
        }
    });
    while !worker.is_finished() {
        if token.is_cancelled() {
            // SAFETY: `worker` owns this thread handle until join below. Repeating closes the
            // race where cancellation arrives between two synchronous reads.
            unsafe {
                windows_sys::Win32::System::IO::CancelSynchronousIo(worker.as_raw_handle());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let result = worker
        .join()
        .map_err(|_| anyhow::anyhow!("stdin reader stopped unexpectedly"))?;
    token.ensure_active()?;
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::sync::mpsc;
    use std::time::Duration;
    use tola_build::cancellation::BuildCanceller;

    #[test]
    fn reads_until_eof_without_changing_bytes() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"hello\0\xff").unwrap();
        writer.shutdown(std::net::Shutdown::Write).unwrap();
        assert_eq!(
            read_unix(reader, &BuildCanceller::default().token()).unwrap(),
            b"hello\0\xff"
        );
    }

    #[test]
    fn cancellation_pipe_without_closing_writer() {
        let (reader, _writer) = UnixStream::pair().unwrap();
        let canceller = BuildCanceller::default();
        let token = canceller.token();
        let (send, receive) = mpsc::channel();
        let (started, ready) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            send.send(read_unix(reader, &token)).unwrap();
        });
        ready.recv().unwrap();
        std::thread::sleep(Duration::from_millis(75));
        canceller.cancel();
        let result = receive
            .recv_timeout(Duration::from_secs(2))
            .expect("reader stops");
        assert!(result.is_err());
        worker.join().unwrap();
    }
}
