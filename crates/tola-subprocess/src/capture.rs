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
            let n = (HEAD_TAIL_BYTES - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..n]);
            bytes = &bytes[n..];
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

    pub fn omitted(&self) -> u64 {
        self.total_bytes
            .saturating_sub((self.head.len() + self.tail.len()) as u64)
    }

    pub fn stopped_early(&self) -> bool {
        self.stopped_early
    }

    /// Render retained bytes with `notice` inserted where bytes were dropped.
    pub fn render(&self, notice: impl FnOnce(u64) -> String) -> Vec<u8> {
        let omitted = self.omitted();
        let mut buf = Vec::with_capacity(self.head.len() + self.tail.len() + 96);
        buf.extend_from_slice(&self.head);
        if omitted > 0 {
            buf.push(b'\n');
            buf.extend_from_slice(notice(omitted).as_bytes());
            buf.push(b'\n');
        }
        buf.extend(&self.tail);
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(omitted: u64) -> String {
        format!("... {omitted} bytes omitted ...")
    }

    #[test]
    fn bounded_head_and_tail() {
        for len in [
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
            let bytes: Vec<_> = (0..len).map(|off| off as u8).collect();
            let omitted = len.saturating_sub(2 * HEAD_TAIL_BYTES) as u64;
            let expected = if omitted == 0 {
                bytes.clone()
            } else {
                [
                    &bytes[..HEAD_TAIL_BYTES],
                    format!("\n{}\n", notice(omitted)).as_bytes(),
                    &bytes[len - HEAD_TAIL_BYTES..],
                ]
                .concat()
            };
            for chunk_len in [1, 997, HEAD_TAIL_BYTES, HEAD_TAIL_BYTES + 1, len.max(1)] {
                let mut cap = Captured::default();
                cap.extend(&[]);
                for chunk in bytes.chunks(chunk_len) {
                    cap.extend(chunk);
                }
                let mut seen = None;
                let rendered = cap.render(|n| {
                    seen = Some(n);
                    notice(n)
                });
                assert_eq!(rendered, expected);
                assert_eq!(cap.omitted(), omitted);
                assert_eq!(seen, (omitted > 0).then_some(omitted));
                assert!(!cap.stopped_early());
            }
        }
    }

    #[test]
    fn stopped_early_flag() {
        let mut cap = Captured::default();
        cap.extend(b"partial");
        cap.mark_stopped_early();
        assert!(cap.stopped_early());
        assert_eq!(cap.render(notice), b"partial");
    }
}
