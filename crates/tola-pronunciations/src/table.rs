//! Pronunciation table storage and longest-prefix lookup.

use std::io::Read as _;
use std::sync::LazyLock;

/// Leading byte-pair index slots (`2^16 + 1` for end sentinel, ~256 KiB per table).
const RUN_SLOTS: usize = (1 << 16) + 1;

pub(crate) type PronunciationTable = LazyLock<Names>;

struct Entry {
    start: u32,
    name_len: u16,
    val_len: u16,
}

pub(crate) struct Names {
    raw: String,
    entries: Vec<Entry>,
    runs: Box<[u32; RUN_SLOTS]>,
}

fn leading_pair(bytes: &[u8]) -> Option<usize> {
    let first = *bytes.first()?;
    Some(((first as usize) << 8) | bytes.get(1).copied().unwrap_or(0) as usize)
}

#[cfg(any(feature = "cedict", feature = "jmdict", feature = "unihan"))]
pub(crate) fn decompress(compressed: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    ruzstd::decoding::StreamingDecoder::new(compressed)
        .expect("valid zstd pronunciation table")
        .read_to_end(&mut buf)
        .expect("pronunciation table decodes");
    buf.shrink_to_fit();
    buf
}

impl Names {
    #[cfg_attr(
        not(any(feature = "cedict", feature = "jmdict", feature = "unihan")),
        allow(
            dead_code,
            reason = "with no source enabled there is no table to parse"
        )
    )]
    pub(crate) fn parse(raw: Vec<u8>) -> Self {
        let text = String::from_utf8(raw).expect("generated table is UTF-8");
        u32::try_from(text.len()).expect("generated table fits in u32 offsets");
        let base = text.as_ptr() as usize;
        let mut entries = Vec::new();
        let mut runs: Box<[u32; RUN_SLOTS]> = vec![0; RUN_SLOTS]
            .try_into()
            .expect("run slot slice matches RUN_SLOTS");
        let mut run = 0;
        let mut prev_name = None;
        for line in text.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (name, val) = line
                .split_once('\t')
                .expect("table entry has tab separator");
            if let Some(prev) = prev_name {
                assert!(prev <= name, "table entries must be byte-sorted");
            }
            prev_name = Some(name);
            let head = leading_pair(name.as_bytes()).expect("non-empty entry key");
            while run <= head {
                runs[run] = entries.len() as u32;
                run += 1;
            }
            entries.push(Entry {
                start: (line.as_ptr() as usize - base) as u32,
                name_len: u16::try_from(name.len()).expect("entry name fits in u16"),
                val_len: u16::try_from(val.len()).expect("entry pronunciation fits in u16"),
            });
        }
        while run < RUN_SLOTS {
            runs[run] = entries.len() as u32;
            run += 1;
        }
        Self {
            raw: text,
            entries,
            runs,
        }
    }

    fn name(&self, entry: &Entry) -> &str {
        let start = entry.start as usize;
        &self.raw[start..start + entry.name_len as usize]
    }

    fn val(&self, entry: &Entry) -> &str {
        let start = entry.start as usize + entry.name_len as usize + 1;
        &self.raw[start..start + entry.val_len as usize]
    }

    fn run_of(&self, text: &str) -> &[Entry] {
        let bytes = text.as_bytes();
        let Some(&first) = bytes.first() else {
            return &[];
        };
        let (lo, hi) = if first < 0x80 {
            // Single-byte ASCII prefix: scan every second-byte slot under `first`.
            let p = (first as usize) << 8;
            (p, p | 0xFF)
        } else {
            let p = leading_pair(bytes).expect("non-empty slice has leading pair");
            (p, p)
        };
        &self.entries[self.runs[lo] as usize..self.runs[hi + 1] as usize]
    }

    fn longest_match(&self, text: &str) -> Option<(&str, &str)> {
        let mut cands = self.run_of(text);
        if cands.is_empty() {
            return None;
        }
        let mut best = None;
        for (off, ch) in text.char_indices() {
            let prefix = &text[..off + ch.len_utf8()];
            let idx = cands.partition_point(|e| self.name(e) < prefix);
            let Some(first) = cands.get(idx) else {
                break;
            };
            let name = self.name(first);
            if !name.starts_with(prefix) {
                break;
            }
            if name.len() == prefix.len() {
                best = Some((name, self.val(first)));
            }
            cands = &cands[idx..];
            cands = &cands[..cands.partition_point(|e| self.name(e).starts_with(prefix))];
        }
        best
    }
}

/// Replace matched keys with spaces around word boundaries; combining marks stay attached to
/// preceding text so callers normalizing output handle them explicitly.
pub(crate) fn rewrite(sources: &[&PronunciationTable], src: &str) -> Option<String> {
    if sources.is_empty() {
        return None;
    }
    let mut out = None;
    let mut rest = src;
    let mut after_match = false;
    while let Some(ch) = rest.chars().next() {
        // Longest key wins; ties keep the first source in slice order.
        let mut found = None;
        for tbl in sources {
            let Some(cand) = tbl.longest_match(rest) else {
                continue;
            };
            if found.is_none_or(|(name, _): (&str, &str)| cand.0.len() > name.len()) {
                found = Some(cand);
            }
        }
        match found {
            Some((name, val)) => {
                let buf = out.get_or_insert_with(|| {
                    let mut prefix = String::with_capacity(src.len());
                    prefix.push_str(&src[..src.len() - rest.len()]);
                    prefix
                });
                if !buf.is_empty() && !buf.ends_with(' ') {
                    buf.push(' ');
                }
                buf.push_str(val);
                after_match = true;
                rest = &rest[name.len()..];
            }
            None => {
                let len = ch.len_utf8();
                if let Some(buf) = &mut out {
                    if after_match && ch.is_alphanumeric() && !buf.ends_with(' ') {
                        buf.push(' ');
                    }
                    buf.push_str(&rest[..len]);
                }
                after_match = false;
                rest = &rest[len..];
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Names, PronunciationTable, rewrite};

    static ASCII_TABLE: PronunciationTable = PronunciationTable::new(|| {
        Names::parse(b"# names and their pronunciations\nab\tx\nabc\ty\nc\tz\n".to_vec())
    });

    #[test]
    fn longest_wins() {
        assert_eq!(rewrite(&[&ASCII_TABLE], "abc"), Some("y".into()));
    }

    #[test]
    fn word_boundaries() {
        for (src, expected) in [
            ("ab c", "x z"),
            ("é界xabcδ", "é界x y δ"),
            ("xab c", "x x z"),
            ("ab-c", "x- z"),
            ("ab, c", "x, z"),
        ] {
            assert_eq!(rewrite(&[&ASCII_TABLE], src).as_deref(), Some(expected));
        }
    }

    #[test]
    fn unknown_stays_unmatched() {
        for src in ["zzz", "11", "é界", ""] {
            assert_eq!(rewrite(&[&ASCII_TABLE], src), None);
        }
    }

    #[test]
    fn empty_sources() {
        assert_eq!(rewrite(&[], "abc東京"), None);
    }

    #[test]
    fn longer_beats_precedence() {
        static PREFIX_TABLE: PronunciationTable =
            PronunciationTable::new(|| Names::parse(b"ab\tprefix\n".to_vec()));
        assert_eq!(
            rewrite(&[&PREFIX_TABLE, &ASCII_TABLE], "abc"),
            Some("y".into())
        );
    }

    #[test]
    fn earlier_source_tie() {
        static FIRST: PronunciationTable =
            PronunciationTable::new(|| Names::parse(b"abc\tfirst\n".to_vec()));
        static SECOND: PronunciationTable =
            PronunciationTable::new(|| Names::parse(b"abc\tsecond\n".to_vec()));
        assert_eq!(rewrite(&[&FIRST, &SECOND], "abc"), Some("first".into()));
        assert_eq!(rewrite(&[&SECOND, &FIRST], "abc"), Some("second".into()));
    }

    #[test]
    fn shared_leading_bytes() {
        static PAIR_TABLE: PronunciationTable = PronunciationTable::new(|| {
            Names::parse("一\tyi\n一心\tone heart\n中\tzhong\n".as_bytes().to_vec())
        });
        static NEIGHBOURS_TABLE: PronunciationTable =
            PronunciationTable::new(|| Names::parse("中\tzhong\n乀\tfu\n".as_bytes().to_vec()));
        static ONE_BYTE_TABLE: PronunciationTable = PronunciationTable::new(|| {
            Names::parse("3\tthree\n3D\tthree de\n".as_bytes().to_vec())
        });

        for (tbl, cases) in [
            (
                &PAIR_TABLE,
                &[("中", "zhong"), ("一", "yi"), ("一心", "one heart")][..],
            ),
            (&NEIGHBOURS_TABLE, &[("乀", "fu"), ("中", "zhong")][..]),
            (
                &ONE_BYTE_TABLE,
                &[("3x", "three x"), ("3D", "three de")][..],
            ),
        ] {
            for &(src, expected) in cases {
                assert_eq!(rewrite(&[tbl], src).as_deref(), Some(expected), "{src}");
            }
        }
    }
}
