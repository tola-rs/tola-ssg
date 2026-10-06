//! Rule-based romanization of kana and Hangul syllable runs.
//!
//! The scripts supply syllables, but particles, morpheme boundaries, and name conventions
//! can require context these local rules do not infer. Han-word pronunciations belong to
//! `tola-pronunciations`; character-map fallback remains the caller's responsibility.

mod hangul;
mod kana;

/// Append the romanization of the script text at the start of `text`, and report the bytes
/// it read.
///
/// Answers `None` when no rule set here reads that text, and appends nothing in that
/// case, which leaves the character to the caller's own policy.
pub(crate) fn romanize(text: &str, out: &mut String) -> Option<usize> {
    kana::romanize(text, out).or_else(|| hangul::romanize(text, out))
}
