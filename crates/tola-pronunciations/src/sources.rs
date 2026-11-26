//! Enabled pronunciation tables in tiebreak precedence order.

use crate::table::PronunciationTable;

#[cfg(feature = "cedict")]
use crate::cedict;
#[cfg(feature = "jmdict")]
use crate::jmdict;
#[cfg(feature = "unihan")]
use crate::unihan;

/// Chinese tables in precedence order (words before single characters).
pub(crate) const CHINESE_TABLES: &[&PronunciationTable] = &[
    #[cfg(feature = "cedict")]
    &cedict::TABLE,
    #[cfg(feature = "unihan")]
    &unihan::TABLE,
];

/// Japanese tables (delivering kana for downstream romanization).
pub(crate) const JAPANESE_TABLES: &[&PronunciationTable] = &[
    #[cfg(feature = "jmdict")]
    &jmdict::TABLE,
];
