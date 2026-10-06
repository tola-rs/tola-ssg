//! The tables this build carries, and the order a lookup consults them.

use crate::table::PronunciationTable;

#[cfg(feature = "cedict")]
use crate::cedict;
#[cfg(feature = "jmdict")]
use crate::jmdict;
#[cfg(feature = "unihan")]
use crate::unihan;

/// Every source that names a pronunciation for Chinese text, in the order a tie is decided.
///
/// A word source precedes a character source: a word and a character of the same length
/// both name the position they start at, and the word is the one read as a word.
pub(crate) const CHINESE_TABLES: &[&PronunciationTable] = &[
    #[cfg(feature = "cedict")]
    &cedict::TABLE,
    #[cfg(feature = "unihan")]
    &unihan::TABLE,
];

/// Every source that names a pronunciation for Japanese text.
///
/// The pronunciation a Japanese source names is kana, which the caller reads as its own
/// script: a rule set turns kana into Latin, and no table has to hold both.
pub(crate) const JAPANESE_TABLES: &[&PronunciationTable] = &[
    #[cfg(feature = "jmdict")]
    &jmdict::TABLE,
];
