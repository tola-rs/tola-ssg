//! Han-range guard to skip table initialization on non-CJK text.

/// Broad CJK and ideographic ranges covering all generated dictionary keys.
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

pub(crate) fn contains_han(src: &str) -> bool {
    src.chars().any(is_han)
}

fn is_han(ch: char) -> bool {
    let cp = u32::from(ch);
    let idx = HAN.partition_point(|(start, _)| *start <= cp);
    idx > 0 && cp <= HAN[idx - 1].1
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use super::is_han;

    fn table_text(compressed: &[u8]) -> String {
        let mut buf = Vec::new();
        ruzstd::decoding::StreamingDecoder::new(compressed)
            .expect("valid zstd frame")
            .read_to_end(&mut buf)
            .expect("table decodes");
        String::from_utf8(buf).expect("table is UTF-8")
    }

    #[test]
    fn generated_keys_carry_han() {
        for tbl in [
            table_text(include_bytes!("../data/cedict.zst")),
            table_text(include_bytes!("../data/jmdict.zst")),
            table_text(include_bytes!("../data/unihan.zst")),
        ] {
            for line in tbl.lines() {
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let (key, _) = line.split_once('\t').expect("entry has tab");
                assert!(key.chars().any(is_han), "key `{key}` outside Han guard");
            }
        }
    }
}
