//! The control flow of the loop one cursor sits in.

use std::ops::Range;

use typst_syntax::{LinkedNode, Side, Source, SyntaxKind};

/// The control-flow keywords one cursor's loop owns, as byte ranges.
///
/// Includes the loop keyword and its `break`/`continue` sites, excluding nested loops,
/// closures, and `context` blocks. Preserves a leading `#` in markup.
pub fn keywords(src: &Source, cursor: usize) -> Vec<Range<usize>> {
    let root = LinkedNode::new(src.root());
    let Some(leaf) = [Side::Before, Side::After]
        .into_iter()
        .find_map(|side| root.leaf_at(cursor, side))
    else {
        return Vec::new();
    };
    let Some(loop_node) = enclosing_loop(leaf) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    if let Some(kw) = loop_node.children().find(|child| child.kind().is_keyword()) {
        found.push(written(src, kw.range()));
    }
    collect_breaks(&loop_node, src, &mut found);
    found.sort_by_key(|range| range.start);
    found
}

fn enclosing_loop(mut node: LinkedNode<'_>) -> Option<LinkedNode<'_>> {
    let mut control = false;
    loop {
        match node.kind() {
            SyntaxKind::For
            | SyntaxKind::While
            | SyntaxKind::Break
            | SyntaxKind::Continue
            | SyntaxKind::LoopBreak
            | SyntaxKind::LoopContinue => control = true,
            SyntaxKind::ForLoop | SyntaxKind::WhileLoop if control => return Some(node),
            SyntaxKind::Closure | SyntaxKind::Contextual if control => return None,
            _ => {}
        }
        node = node.parent()?.clone();
    }
}

fn collect_breaks(node: &LinkedNode<'_>, src: &Source, found: &mut Vec<Range<usize>>) {
    for child in node.children() {
        match child.kind() {
            SyntaxKind::ForLoop
            | SyntaxKind::WhileLoop
            | SyntaxKind::Closure
            | SyntaxKind::Contextual => {}
            SyntaxKind::LoopBreak | SyntaxKind::LoopContinue => {
                found.push(written(src, child.range()));
            }
            _ => collect_breaks(&child, src, found),
        }
    }
}

fn written(src: &Source, range: Range<usize>) -> Range<usize> {
    match range.start.checked_sub(1) {
        Some(start) if src.text().as_bytes().get(start) == Some(&b'#') => start..range.end,
        _ => range,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keywords_of(marked: &str) -> Vec<String> {
        let cursor = marked.find('|').expect("a cursor marker");
        let src = Source::detached(marked.replace('|', ""));
        keywords(&src, cursor)
            .into_iter()
            .map(|range| src.text()[range].to_owned())
            .collect()
    }

    #[test]
    fn loop_owns_breaks() {
        assert_eq!(
            keywords_of("#f|or x in (1, 2) {\n  if x == 1 { break }\n  continue\n}\n"),
            ["#for", "break", "continue"]
        );
    }

    #[test]
    fn loop_closure_isolates_breaks() {
        assert_eq!(
            keywords_of("#for x in (1,) {\n  let f = () => { br|eak }\n}\n"),
            Vec::<String>::new()
        );
    }
}
