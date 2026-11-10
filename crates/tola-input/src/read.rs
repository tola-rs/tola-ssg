//! Cancellable chunked reads from regular local input files.

use std::io::Read;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use tola_fs::open_regular_file;

/// Read buffer shared by every chunked reader.
const READ_CHUNK_SIZE: usize = 16 * 1024;

/// The caller requested cancellation while a read was in progress.
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

/// Read `reader` in chunks, checking cancellation before and after every read.
///
/// Interrupted reads are retried. `consume` may stop early by returning
/// [`ControlFlow::Break`]; the completion value is returned to the caller.
pub fn read_chunks(
    reader: &mut impl Read,
    cancellation: &AtomicBool,
    mut consume: impl FnMut(&[u8]) -> ControlFlow<()>,
) -> Result<ControlFlow<()>, FileReadError> {
    let mut buffer = [0u8; READ_CHUNK_SIZE];
    loop {
        ensure_active(cancellation)?;
        let read = reader.read(&mut buffer);
        ensure_active(cancellation)?;
        let count = match read {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Ok(ControlFlow::Continue(()));
        }
        if let ControlFlow::Break(()) = consume(&buffer[..count]) {
            return Ok(ControlFlow::Break(()));
        }
    }
}

pub fn read_file_chunks(
    path: &Path,
    cancellation: &AtomicBool,
    mut consume: impl FnMut(&[u8]),
) -> Result<(), FileReadError> {
    ensure_active(cancellation)?;
    let opened = open_regular_file(path, true);
    ensure_active(cancellation)?;
    let mut file = opened?;
    read_chunks(&mut file, cancellation, |chunk| {
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
    fn cancellation_stops_chunk_reads() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.css");
        std::fs::write(&path, [b'a'; 32 * 1024]).unwrap();
        let cancellation = AtomicBool::new(false);
        let mut consumed = 0;
        let result = read_file_chunks(&path, &cancellation, |bytes| {
            consumed += bytes.len();
            cancellation.store(true, Ordering::Relaxed);
        });
        assert!(matches!(result, Err(FileReadError::Cancelled(_))));
        assert!(consumed > 0 && consumed < 32 * 1024);
    }
}
