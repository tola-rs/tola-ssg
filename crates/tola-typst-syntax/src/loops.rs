//! The control flow of the loop one cursor sits in.

use std::ops::Range;

use typst_syntax::{LinkedNode, Side, Source, SyntaxKind};

/// The control-flow keywords one cursor's loop owns, as byte ranges.
///
/// The loop's own keyword and every `break` or `continue` inside it, in document order; a nested
/// loop, closure, or `context` keeps its own. A cursor outside a loop owns none, and a keyword in
/// markup keeps the `#` the author wrote.
pub fn keywords(source: &Source, cursor: usize) -> Vec<Range<usize>> {
    let root = LinkedNode::new(source.root());
    let Some(leaf) = [Side::Before, Side::After]
        .into_iter()
        .find_map(|side| root.leaf_at(cursor, side))
    else {
        return Vec::new();
    };
    let Some(loop_node) = enclosing_loop(leaf) else {
        return Vec::new();
    };
    let mut keywords = Vec::new();
    if let Some(keyword) = loop_node.children().find(|child| child.kind().is_keyword()) {
        keywords.push(written(source, keyword.range()));
    }
    collect_breaks(&loop_node, source, &mut keywords);
    keywords.sort_by_key(|range| range.start);
    keywords
}

/// The loop the cursor's own control flow belongs to.
///
/// Walking out from the cursor's leaf, the first control keyword decides: the nearest loop above
/// it is the one it belongs to, and a closure or `context` between the two keeps it out of any
/// loop.
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

/// Every `break` or `continue` the loop holds, without descending into the loops, closures, and
/// contexts that keep their own.
fn collect_breaks(node: &LinkedNode<'_>, source: &Source, keywords: &mut Vec<Range<usize>>) {
    for child in node.children() {
        match child.kind() {
            SyntaxKind::ForLoop
            | SyntaxKind::WhileLoop
            | SyntaxKind::Closure
            | SyntaxKind::Contextual => {}
            SyntaxKind::LoopBreak | SyntaxKind::LoopContinue => {
                keywords.push(written(source, child.range()));
            }
            _ => collect_breaks(&child, source, keywords),
        }
    }
}

/// The range the author wrote for one keyword, including a leading `#` in markup.
fn written(source: &Source, range: Range<usize>) -> Range<usize> {
    match range.start.checked_sub(1) {
        Some(start) if source.text().as_bytes().get(start) == Some(&b'#') => start..range.end,
        _ => range,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keywords one marked cursor answers, as the text it covers.
    fn keywords_of(marked: &str) -> Vec<String> {
        let cursor = marked.find('|').expect("a cursor marker");
        let source = Source::detached(marked.replace('|', ""));
        keywords(&source, cursor)
            .into_iter()
            .map(|range| source.text()[range].to_owned())
            .collect()
    }

    #[test]
    fn loop_keyword_owns_its_breaks() {
        assert_eq!(
            keywords_of("#f|or x in (1, 2) {\n  if x == 1 { break }\n  continue\n}\n"),
            ["#for", "break", "continue"]
        );
    }

    #[test]
    fn break_answers_its_loop() {
        assert_eq!(
            keywords_of("#for x in (1, 2) {\n  if x == 1 { br|eak }\n  continue\n}\n"),
            ["#for", "break", "continue"]
        );
    }

    #[test]
    fn nested_loop_keeps_its_own_breaks() {
        assert_eq!(
            keywords_of("#for x in (1,) {\n  for y in (2,) { br|eak }\n  continue\n}\n"),
            ["for", "break"]
        );
    }

    #[test]
    fn closure_keeps_its_own_control_flow() {
        assert_eq!(
            keywords_of("#for x in (1,) {\n  let f = () => { br|eak }\n}\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn cursor_outside_loop_owns_nothing() {
        for marked in [
            "#for x in (1, 2) {\n  let y = |x\n}\n",
            "#if x == 1 { br|eak }\n",
            "Body te|xt\n",
        ] {
            assert_eq!(keywords_of(marked), Vec::<String>::new(), "{marked:?}");
        }
    }
}
