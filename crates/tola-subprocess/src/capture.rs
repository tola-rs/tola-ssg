//! Bounded retention of one child stream's bytes.

use std::collections::VecDeque;

/// Bytes retained at each end of one stream; a larger stream reports the rest as omitted.
pub(crate) const HEAD_TAIL_BYTES: usize = 7 * 1024;

/// Which of the child's two output streams a chunk or capture belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

impl Stream {
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Stdout => 0,
            Self::Stderr => 1,
        }
    }

    /// The stream name used in caller-facing output.
    pub fn name(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

/// The bounded head and tail of one stream, plus what was dropped between them.
#[derive(Debug, Default)]
pub struct Captured {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total_bytes: u64,
    stopped_early: bool,
}

impl Captured {
    pub(crate) fn extend(&mut self, mut bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len() as u64);
        if self.head.len() < HEAD_TAIL_BYTES {
            let retained = (HEAD_TAIL_BYTES - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..retained]);
            bytes = &bytes[retained..];
        }
        if bytes.is_empty() {
            return;
        }
        if bytes.len() >= HEAD_TAIL_BYTES {
            self.tail.clear();
            self.tail
                .extend(bytes[bytes.len() - HEAD_TAIL_BYTES..].iter().copied());
        } else {
            let overflow = self
                .tail
                .len()
                .saturating_add(bytes.len())
                .saturating_sub(HEAD_TAIL_BYTES);
            self.tail.drain(..overflow);
            self.tail.extend(bytes.iter().copied());
        }
    }

    pub(crate) fn mark_stopped_early(&mut self) {
        self.stopped_early = true;
    }

    /// Bytes dropped between the retained head and tail.
    pub fn omitted(&self) -> u64 {
        self.total_bytes
            .saturating_sub((self.head.len() + self.tail.len()) as u64)
    }

    /// Whether reading ended because it was stopped, not because the stream closed.
    pub fn stopped_early(&self) -> bool {
        self.stopped_early
    }

    /// Every retained byte, with the caller's notice inserted where bytes were
    /// dropped. The notice is rendered only when something was omitted.
    pub fn render(&self, notice: impl FnOnce(u64) -> String) -> Vec<u8> {
        let omitted = self.omitted();
        let mut bytes = Vec::with_capacity(self.head.len() + self.tail.len() + 96);
        bytes.extend_from_slice(&self.head);
        if omitted > 0 {
            bytes.push(b'\n');
            bytes.extend_from_slice(notice(omitted).as_bytes());
            bytes.push(b'\n');
        }
        bytes.extend(&self.tail);
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(omitted: u64) -> String {
        format!("... {omitted} bytes omitted ...")
    }

    #[test]
    fn head_and_tail_survive_the_bound() {
        for length in [
            0,
            1,
            HEAD_TAIL_BYTES - 1,
            HEAD_TAIL_BYTES,
            HEAD_TAIL_BYTES + 1,
            2 * HEAD_TAIL_BYTES - 1,
            2 * HEAD_TAIL_BYTES,
            2 * HEAD_TAIL_BYTES + 1,
            3 * HEAD_TAIL_BYTES + 1,
        ] {
            let bytes: Vec<_> = (0..length).map(|offset| offset as u8).collect();
            let omitted = length.saturating_sub(2 * HEAD_TAIL_BYTES) as u64;
            let expected = if omitted == 0 {
                bytes.clone()
            } else {
                [
                    &bytes[..HEAD_TAIL_BYTES],
                    format!("\n{}\n", notice(omitted)).as_bytes(),
                    &bytes[length - HEAD_TAIL_BYTES..],
                ]
                .concat()
            };
            for chunk_length in [1, 997, HEAD_TAIL_BYTES, HEAD_TAIL_BYTES + 1, length.max(1)] {
                let mut captured = Captured::default();
                captured.extend(&[]);
                for chunk in bytes.chunks(chunk_length) {
                    captured.extend(chunk);
                }
                let mut noticed_omitted = None;
                let rendered = captured.render(|count| {
                    noticed_omitted = Some(count);
                    notice(count)
                });
                assert_eq!(rendered, expected);
                assert_eq!(captured.omitted(), omitted);
                assert_eq!(noticed_omitted, (omitted > 0).then_some(omitted));
                assert!(!captured.stopped_early());
            }
        }
    }

    #[test]
    fn stopped_read_marks_the_capture() {
        let mut captured = Captured::default();
        captured.extend(b"partial");

        captured.mark_stopped_early();

        assert!(captured.stopped_early());
        assert_eq!(captured.render(notice), b"partial");
    }
}
