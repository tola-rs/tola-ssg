//! The ancestor a selection sits inside: the element a rewrite acts on.

use tola_typst::typst::syntax::{LinkedNode, Source};

/// The first ancestor `stop` names on the climb from the selection's leaves, when it also holds
/// the whole selection.
///
/// The climb ends at that node: one that does not hold the whole selection answers nothing, and
/// no outer ancestor answers in its place. A stop condition that itself demands containment —
/// the node a figure can hold — ends the climb at the same node.
pub(super) fn selection_ancestor<'a>(
    source: &'a Source,
    span: &std::ops::Range<usize>,
    stop: impl Fn(&LinkedNode<'a>) -> bool,
) -> Option<LinkedNode<'a>> {
    tola_typst_syntax::syntax::cursor_leaves(source, span.start)
        .into_iter()
        .flatten()
        .find_map(|leaf| {
            let mut node = leaf;
            while !stop(&node) {
                node = node.parent()?.clone();
            }
            let range = node.range();
            (range.start <= span.start && span.end <= range.end).then_some(node)
        })
}
