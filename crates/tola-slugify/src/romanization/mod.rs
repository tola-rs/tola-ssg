//! Rule-based romanization of kana and Hangul syllable runs.

mod hangul;
mod kana;

/// Append the romanization of the leading script run in `src` to `out`, returning bytes read.
pub(crate) fn romanize(src: &str, out: &mut String) -> Option<usize> {
    kana::romanize(src, out).or_else(|| hangul::romanize(src, out))
}
