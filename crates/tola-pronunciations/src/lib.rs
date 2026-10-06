//! The pronunciations Tola names for text, and the tables that name them.
//!
//! A pronunciation is how text is said, which is not how it is written: `重庆` is said
//! `chong qing`. A table names the pronunciation of a word or a single character, and the
//! lookup prefers the longest name a position starts with, so a word beats the
//! characters it is built from.
//!
//! A match uses the pronunciation stored in its source table; text no table names
//! keeps its own characters.
//!
//! Every source is a feature; only the enabled ones are compiled in. With none
//! enabled, each function returns its input unchanged.
//!
//! Output storage is allocated only after a match. Every generated key carries a Han
//! character, so text carrying none of the CJK code points a key is written in returns
//! without parsing a table.
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

/// Rewrite `text` into the pronunciations its words and characters have in Chinese.
///
/// Each position takes the longest name the enabled tables carry, so a word wins
/// over the characters it is spelled with. Pronunciations are separated from each other,
/// and text no table names keeps its own characters.
pub fn chinese_pronunciations(text: &str) -> Cow<'_, str> {
    if sources::CHINESE_TABLES.is_empty() || !han::contains_han(text) {
        return Cow::Borrowed(text);
    }
    match table::rewrite(sources::CHINESE_TABLES, text) {
        Some(rewritten) => Cow::Owned(rewritten),
        None => Cow::Borrowed(text),
    }
}

/// Rewrite `text` into the pronunciations its words have in Japanese.
///
/// A Japanese word is named by kana, the script Japanese writes its own words with:
/// `東京` is `とうきょう`, which a rule set reads as Latin.
/// Text no table names keeps its own characters.
pub fn japanese_pronunciations(text: &str) -> Cow<'_, str> {
    if sources::JAPANESE_TABLES.is_empty() || !han::contains_han(text) {
        return Cow::Borrowed(text);
    }
    match table::rewrite(sources::JAPANESE_TABLES, text) {
        Some(rewritten) => Cow::Owned(rewritten),
        None => Cow::Borrowed(text),
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{chinese_pronunciations, japanese_pronunciations};

    #[cfg(feature = "unihan")]
    #[test]
    fn pronunciations_keep_neighbours_apart() {
        assert_eq!(chinese_pronunciations("中文"), "zhong wen");
        assert_eq!(chinese_pronunciations("中文abc"), "zhong wen abc");
        assert_eq!(chinese_pronunciations("中文，abc"), "zhong wen，abc");
    }

    #[cfg(feature = "cedict")]
    #[test]
    fn word_pronunciation_beats_its_characters() {
        assert_eq!(chinese_pronunciations("重庆"), "chong qing");
        assert_eq!(chinese_pronunciations("银行"), "yin hang");
        assert_eq!(chinese_pronunciations("中国人"), "zhong guo ren");
    }

    #[cfg(feature = "jmdict")]
    #[test]
    fn japanese_words_read_as_kana() {
        assert_eq!(japanese_pronunciations("東京"), "とうきょう");
        assert_eq!(japanese_pronunciations("新聞"), "しんぶん");
        assert_eq!(japanese_pronunciations("食べる"), "たべる");
    }

    #[test]
    fn unknown_text_is_borrowed() {
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
    fn missing_sources_leave_text_borrowed() {
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
    fn mixed_names_keep_ascii_prefixes() {
        #[cfg(feature = "cedict")]
        assert_eq!(chinese_pronunciations("3D打印"), "san d da yin");
        #[cfg(feature = "jmdict")]
        assert_eq!(japanese_pronunciations("2.5次元"), "にてんごじげん");
    }
}
