//! The code points a generated key is written with.

/// The CJK ranges a key can be written in: those of the BMP, the whole of the two planes Han
/// extends into, and the ideographic marks beside them.
///
/// The ranges are wider than the Han script on purpose, so a key added under a later Unicode
/// version still falls inside them. `every_generated_key_carries_han` holds the shipped
/// tables to the other half of the invariant: their keys stay inside these ranges.
const HAN: &[(u32, u32)] = &[
    (0x2E80, 0x2FFF),
    (0x3005, 0x3007),
    (0x3021, 0x3029),
    (0x3038, 0x303B),
    (0x3400, 0x4DBF),
    (0x4E00, 0x9FFF),
    (0xF900, 0xFAFF),
    (0x16FE0, 0x16FFF),
    (0x20000, 0x2FFFF),
    (0x30000, 0x3FFFF),
];

/// Whether `text` carries a character a generated key could be written with.
///
/// A lookup only rewrites a key it reads from the text itself, so text with none of these
/// characters holds no key and parses no table.
pub(crate) fn contains_han(text: &str) -> bool {
    text.chars().any(is_han)
}

fn is_han(character: char) -> bool {
    let point = u32::from(character);
    let after = HAN.partition_point(|(start, _)| *start <= point);
    after > 0 && point <= HAN[after - 1].1
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use super::is_han;

    /// One committed table, decoded from the frame the generator wrote.
    fn table_text(compressed: &[u8]) -> String {
        let mut text = Vec::new();
        ruzstd::decoding::StreamingDecoder::new(compressed)
            .expect("a committed table is a zstd frame")
            .read_to_end(&mut text)
            .expect("a committed table decodes");
        String::from_utf8(text).expect("a generated table is UTF-8")
    }

    /// A key the skip missed would be a word the tables never name.
    #[test]
    fn every_generated_key_carries_han() {
        for table in [
            table_text(include_bytes!("../data/cedict.zst")),
            table_text(include_bytes!("../data/jmdict.zst")),
            table_text(include_bytes!("../data/unihan.zst")),
        ] {
            for line in table.lines() {
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let (key, _) = line
                    .split_once('\t')
                    .expect("a generated table names a pronunciation after a tab");
                assert!(
                    key.chars().any(is_han),
                    "the key `{key}` carries no character the skip accepts"
                );
            }
        }
    }
}
