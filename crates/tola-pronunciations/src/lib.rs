//! Word and character pronunciation lookups for Han text.
//!
//! Each enabled source feature compiles in its lookup table; queries take the longest match at
//! each position, borrowing the input unchanged when no key matches or no table is active.
//!
//! ```
//! # #[cfg(feature = "unihan")]
//! # {
//! use tola_pronunciations::chinese_pronunciations;
//!
//! assert_eq!(chinese_pronunciations("中"), "zhong");
//! assert_eq!(chinese_pronunciations("tola"), "tola");
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::borrow::Cow;

mod han;
mod sources;
mod table;

#[cfg(feature = "cedict")]
mod cedict;

#[cfg(feature = "jmdict")]
mod jmdict;

#[cfg(feature = "unihan")]
mod unihan;

/// Rewrite `text` into space-separated Chinese pronunciations, keeping unmatched text verbatim.
pub fn chinese_pronunciations(text: &str) -> Cow<'_, str> {
    pronunciations(sources::CHINESE_TABLES, text)
}

/// Rewrite `text` into Japanese kana pronunciations, keeping unmatched text verbatim.
pub fn japanese_pronunciations(text: &str) -> Cow<'_, str> {
    pronunciations(sources::JAPANESE_TABLES, text)
}

fn pronunciations<'a>(tables: &[&table::PronunciationTable], text: &'a str) -> Cow<'a, str> {
    if tables.is_empty() || !han::contains_han(text) {
        return Cow::Borrowed(text);
    }
    match table::rewrite(tables, text) {
        Some(out) => Cow::Owned(out),
        None => Cow::Borrowed(text),
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{chinese_pronunciations, japanese_pronunciations};

    #[cfg(feature = "unihan")]
    #[test]
    fn word_spacing() {
        assert_eq!(chinese_pronunciations("中文"), "zhong wen");
        assert_eq!(chinese_pronunciations("中文abc"), "zhong wen abc");
        assert_eq!(chinese_pronunciations("中文，abc"), "zhong wen，abc");
    }

    #[cfg(feature = "cedict")]
    #[test]
    fn longest_word_wins() {
        assert_eq!(chinese_pronunciations("重庆"), "chong qing");
        assert_eq!(chinese_pronunciations("银行"), "yin hang");
        assert_eq!(chinese_pronunciations("中国人"), "zhong guo ren");
    }

    #[cfg(feature = "jmdict")]
    #[test]
    fn japanese_kana() {
        assert_eq!(japanese_pronunciations("東京"), "とうきょう");
        assert_eq!(japanese_pronunciations("新聞"), "しんぶん");
        assert_eq!(japanese_pronunciations("食べる"), "たべる");
    }

    #[test]
    fn unknown_borrows() {
        for text in ["", "tola", "Crème かな 한국어"] {
            assert!(matches!(
                chinese_pronunciations(text),
                Cow::Borrowed(unchanged) if unchanged == text
            ));
            assert!(matches!(
                japanese_pronunciations(text),
                Cow::Borrowed(unchanged) if unchanged == text
            ));
        }
    }

    #[cfg(any(
        not(any(feature = "cedict", feature = "unihan")),
        not(feature = "jmdict")
    ))]
    #[test]
    fn disabled_features_borrow() {
        #[cfg(not(any(feature = "cedict", feature = "unihan")))]
        assert!(matches!(
            chinese_pronunciations("中文"),
            Cow::Borrowed("中文")
        ));
        #[cfg(not(feature = "jmdict"))]
        assert!(matches!(
            japanese_pronunciations("東京"),
            Cow::Borrowed("東京")
        ));
    }

    #[cfg(any(feature = "cedict", feature = "jmdict"))]
    #[test]
    fn mixed_ascii_prefix() {
        #[cfg(feature = "cedict")]
        assert_eq!(chinese_pronunciations("3D打印"), "san d da yin");
        #[cfg(feature = "jmdict")]
        assert_eq!(japanese_pronunciations("2.5次元"), "にてんごじげん");
    }
}
