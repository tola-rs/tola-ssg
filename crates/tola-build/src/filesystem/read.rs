//! Cancellable chunked reads from regular local input files.

use std::io::Read;
use std::ops::ControlFlow;
use std::path::Path;

use crate::cancellation::{BuildCancellation, BuildCancelled};

use super::file_handle::open_regular_file;

/// Read buffer shared by every chunked reader.
const READ_CHUNK_SIZE: usize = 16 * 1024;

#[derive(Debug, thiserror::Error)]
pub(crate) enum FileReadError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Cancelled(#[from] BuildCancelled),
}

impl FileReadError {
    /// Convert without adding a layer: consumers keep matching the underlying cause.
    pub(crate) fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::Io(source) => anyhow::Error::new(source),
            Self::Cancelled(cancelled) => anyhow::Error::new(cancelled),
        }
    }
}

/// Read `reader` in chunks, checking cancellation before and after every read.
///
/// Interrupted reads are retried. `consume` may stop early by returning
/// [`ControlFlow::Break`]; the completion value is returned to the caller.
pub(crate) fn read_chunks(
    reader: &mut impl Read,
    cancellation: &BuildCancellation,
    mut consume: impl FnMut(&[u8]) -> ControlFlow<()>,
) -> Result<ControlFlow<()>, FileReadError> {
    let mut buffer = [0u8; READ_CHUNK_SIZE];
    loop {
        cancellation.ensure_active()?;
        let read = reader.read(&mut buffer);
        cancellation.ensure_active()?;
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

pub(crate) fn read_file_chunks(
    path: &Path,
    cancellation: &BuildCancellation,
    mut consume: impl FnMut(&[u8]),
) -> Result<(), FileReadError> {
    cancellation.ensure_active()?;
    let opened = open_regular_file(path, true);
    cancellation.ensure_active()?;
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
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        let mut consumed = 0;
        let result = read_file_chunks(&path, &cancellation, |bytes| {
            consumed += bytes.len();
            canceller.cancel();
        });
        let error = anyhow::Error::new(result.unwrap_err());
        assert!(crate::cancellation::is_cancelled(&error));
        assert!(consumed > 0 && consumed < 32 * 1024);
    }

    #[test]
    fn regular_inputs_are_read_only() {
        use std::io::Write;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.css");
        std::fs::write(&path, b"original input").unwrap();
        let mut file = open_regular_file(&path, false).unwrap();
        assert!(file.write_all(b"replacement").is_err());
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"original input");
    }

    #[cfg(unix)]
    #[test]
    fn links_require_explicit_following() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.css");
        let link = directory.path().join("linked.css");
        std::fs::write(&target, b"linked input").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(open_regular_file(&link, false).is_err());
        let mut file = open_regular_file(&link, true).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"linked input");
    }

    #[cfg(windows)]
    #[test]
    fn device_handles_are_rejected() {
        let error = open_regular_file(Path::new("NUL"), true).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
