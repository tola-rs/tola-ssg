//! Cancellable chunked reads from regular local input files.

use std::io::Read;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use tola_fs::open_regular_file;

const READ_CHUNK_SIZE: usize = 16 * 1024;

/// Read cancelled by caller flag.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("read cancelled")]
pub struct Cancelled;

#[derive(Debug, thiserror::Error)]
pub enum FileReadError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Cancelled(#[from] Cancelled),
}

fn ensure_active(cancelled: &AtomicBool) -> Result<(), Cancelled> {
    if cancelled.load(Ordering::Relaxed) {
        Err(Cancelled)
    } else {
        Ok(())
    }
}

/// Read `reader` in chunks, checking cancellation before and after each read and retrying `Interrupted`.
pub fn read_chunks(
    reader: &mut impl Read,
    cancelled: &AtomicBool,
    mut consume: impl FnMut(&[u8]) -> ControlFlow<()>,
) -> Result<ControlFlow<()>, FileReadError> {
    let mut buf = [0u8; READ_CHUNK_SIZE];
    loop {
        ensure_active(cancelled)?;
        let res = reader.read(&mut buf);
        ensure_active(cancelled)?;
        let n = match res {
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            other => other?,
        };
        if n == 0 {
            return Ok(ControlFlow::Continue(()));
        }
        if let ControlFlow::Break(()) = consume(&buf[..n]) {
            return Ok(ControlFlow::Break(()));
        }
    }
}

pub fn read_file_chunks(
    path: &Path,
    cancelled: &AtomicBool,
    mut consume: impl FnMut(&[u8]),
) -> Result<(), FileReadError> {
    ensure_active(cancelled)?;
    let opened = open_regular_file(path, true);
    ensure_active(cancelled)?;
    let mut file = opened?;
    read_chunks(&mut file, cancelled, |chunk| {
        consume(chunk);
        ControlFlow::Continue(())
    })
    .map(|_| ())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_stops_reads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.css");
        std::fs::write(&path, [b'a'; 32 * 1024]).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut read = 0;
        let res = read_file_chunks(&path, &cancelled, |bytes| {
            read += bytes.len();
            cancelled.store(true, Ordering::Relaxed);
        });
        assert!(matches!(res, Err(FileReadError::Cancelled(_))));
        assert!(read > 0 && read < 32 * 1024);
    }
}
