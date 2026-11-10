//! Where one Typst source fails to parse.

use std::ops::Range;

use typst_syntax::{LinkedNode, Source, SyntaxKind};

/// The byte range of the first construct the tree marks as an error, when it marks one.
///
/// The walk visits the tree in document order, so the range names the earliest failure the parser
/// recorded; a source may hold more.
pub fn first_error_range(source: &str) -> Option<Range<usize>> {
    first_error(&Source::detached(source))
}

pub(crate) fn first_error(source: &Source) -> Option<Range<usize>> {
    let mut pending = vec![LinkedNode::new(source.root())];
    while let Some(node) = pending.pop() {
        if node.kind() == SyntaxKind::Error {
            return Some(node.range());
        }
        pending.extend(node.children().rev());
    }
    None
}
