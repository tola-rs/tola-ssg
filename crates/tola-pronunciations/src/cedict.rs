//! Chinese word pronunciations from CC-CEDICT.

use crate::table::{self, Names, PronunciationTable};

/// The table's bytes, compressed by the generator.
static COMPRESSED: &[u8] = include_bytes!("../data/cedict.zst");

/// The word pronunciations this build carries.
pub(crate) static TABLE: PronunciationTable =
    PronunciationTable::new(|| Names::parse(table::decompress(COMPRESSED)));
