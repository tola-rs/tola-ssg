//! How the syntax of one source encloses an offset.
//!
//! A caller asks which text to reach for next: the innermost syntax node at the offset, then each
//! node that contains it, out to the whole source.

use std::ops::Range;

use typst_syntax::{LinkedNode, Side, Source};

use crate::position::{self, Utf16Position};

/// The chain of containing byte ranges at each offset, innermost first, ending at the source.
pub fn nesting(source: &Source, at: &[Utf16Position]) -> Option<Vec<Vec<Range<usize>>>> {
    let mut selections = Vec::with_capacity(at.len());
    for position in at {
        let cursor = position::byte_offset(source.lines(), *position).ok()?;
        // A marker at the end of a name still names it, so the leaf ending there is the
        // first one asked for.
        let root = LinkedNode::new(source.root());
        let leaf = root
            .leaf_at(cursor, Side::Before)
            .or_else(|| root.leaf_at(cursor, Side::After))?;
        selections.push(nested(leaf));
    }
    Some(selections)
}

/// The chain of containing ranges, innermost first, ending at the whole source.
fn nested(node: LinkedNode<'_>) -> Vec<Range<usize>> {
    let mut chain: Vec<Range<usize>> = Vec::new();
    let mut current = Some(node);
    while let Some(ancestor) = current {
        let range = ancestor.range();
        if chain.last() != Some(&range) {
            chain.push(range);
        }
        current = ancestor.parent().cloned();
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chain at the offset the text marks, from the innermost range outward.
    fn chain(marked: &str) -> Vec<String> {
        let at = marked.find('|').expect("an offset marker");
        let text = marked.replace('|', "");
        let source = Source::detached(text.clone());
        let line_starts = source.lines();
        let position = position::utf16_range(line_starts, at..at)
            .expect("a position inside the source")
            .start;
        nesting(&source, &[position])
            .expect("one selection")
            .into_iter()
            .next()
            .expect("a chain")
            .into_iter()
            .map(|range| text[range].to_owned())
            .collect()
    }

    #[test]
    fn chain_grows_from_name_to_source() {
        let chain = chain("= He|ad\n");
        assert_eq!(chain.first().map(String::as_str), Some("Head"));
        assert_eq!(chain.last().map(String::as_str), Some("= Head\n"));
    }

    #[test]
    fn marker_after_name_still_names_it() {
        assert_eq!(
            chain("#let value = 1|\n").first().map(String::as_str),
            Some("1")
        );
    }
}
