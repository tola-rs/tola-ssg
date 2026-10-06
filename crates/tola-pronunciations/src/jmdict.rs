//! Japanese word pronunciations from JMdict.

use crate::table::{self, Names, PronunciationTable};

/// The table's bytes, compressed by the generator.
static COMPRESSED: &[u8] = include_bytes!("../data/jmdict.zst");

/// The word pronunciations this build carries.
pub(crate) static TABLE: PronunciationTable =
    PronunciationTable::new(|| Names::parse(table::decompress(COMPRESSED)));
