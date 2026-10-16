//! Where one Typst source fails to parse.

use std::ops::Range;

use typst_syntax::{LinkedNode, Source, SyntaxKind};

/// The byte range of the first construct the tree marks as an error, when it marks one.
///
/// The walk visits the tree in document order, so the range names the earliest failure the parser
/// recorded; a source may hold more.
pub fn first_error_range(source: &str) -> Option<Range<usize>> {
    let parsed = Source::detached(source);
    let mut pending = vec![LinkedNode::new(parsed.root())];
    while let Some(node) = pending.pop() {
        if node.kind() == SyntaxKind::Error {
            return Some(node.range());
        }
        pending.extend(node.children().rev());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsable_source_marks_no_error() {
        assert_eq!(first_error_range("= Title\n#let x = 1\n"), None);
    }

    #[test]
    fn unparsable_source_names_where_it_fails() {
        let source = "= Title\n#let = \n";
        let broken = source.find("#let").expect("the broken line");

        let range = first_error_range(source).expect("the tree marks a place");

        assert!(range.start >= broken, "{range:?}");
    }
}
