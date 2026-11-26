//! How the syntax of one source encloses an offset.

use std::ops::Range;

use typst_syntax::{LinkedNode, Side, Source};

use crate::position::{self, Utf16Position};

/// The chain of containing byte ranges at each offset, innermost first, ending at the source.
pub fn nesting(src: &Source, at: &[Utf16Position]) -> Option<Vec<Vec<Range<usize>>>> {
    let mut selections = Vec::with_capacity(at.len());
    for pos in at {
        let cursor = position::byte_offset(src.lines(), *pos).ok()?;
        let root = LinkedNode::new(src.root());
        let leaf = root
            .leaf_at(cursor, Side::Before)
            .or_else(|| root.leaf_at(cursor, Side::After))?;
        selections.push(nested(leaf));
    }
    Some(selections)
}

fn nested(node: LinkedNode<'_>) -> Vec<Range<usize>> {
    let mut chain: Vec<Range<usize>> = Vec::new();
    let mut cur = Some(node);
    while let Some(ancestor) = cur {
        let range = ancestor.range();
        if chain.last() != Some(&range) {
            chain.push(range);
        }
        cur = ancestor.parent().cloned();
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(marked: &str) -> Vec<String> {
        let at = marked.find('|').expect("an offset marker");
        let text = marked.replace('|', "");
        let src = Source::detached(text.clone());
        let pos = position::utf16_range(src.lines(), at..at)
            .expect("a position inside the source")
            .start;
        nesting(&src, &[pos])
            .expect("one selection")
            .into_iter()
            .next()
            .expect("a chain")
            .into_iter()
            .map(|range| text[range].to_owned())
            .collect()
    }

    #[test]
    fn nesting_from_leaf_to_source() {
        let chain = chain("= He|ad\n");
        assert_eq!(chain.first().map(String::as_str), Some("Head"));
        assert_eq!(chain.last().map(String::as_str), Some("= Head\n"));
    }

    #[test]
    fn nesting_trailing_boundary() {
        assert_eq!(
            chain("#let value = 1|\n").first().map(String::as_str),
            Some("1")
        );
    }
}
