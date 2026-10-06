//! One replacement of a byte range with text.

use std::ops::Range;

/// A replacement one answer asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    /// The bytes the replacement covers.
    pub range: Range<usize>,
    /// The text that replaces them.
    pub text: String,
    /// Where the insertion point lands after the edit, as a byte offset into `text`; `None`
    /// leaves it where the caller had it.
    pub insertion: Option<usize>,
}
