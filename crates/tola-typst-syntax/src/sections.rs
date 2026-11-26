//! The heading sections of one source.

use std::ops::Range;

use typst_syntax::ast::AstNode;
use typst_syntax::{LinkedNode, Source, SyntaxKind, ast};

/// A heading and the section it opens.
pub struct Section {
    /// The heading's own bytes, from its marker through its text.
    pub heading: Range<usize>,
    /// The heading together with every byte its section owns.
    pub body: Range<usize>,
    /// The section depth: the number of equals signs that open the heading.
    pub depth: usize,
    /// The heading's text, without surrounding whitespace.
    pub name: String,
}

/// Every heading section of the source, in document order.
///
/// A heading owns content until the next heading of the same or higher level, or the end of
/// its enclosing markup block.
pub fn sections(src: &Source) -> Vec<Section> {
    let mut found = Vec::new();
    record_sections(&LinkedNode::new(src.root()), src, &mut found);
    found
}

fn record_sections(node: &LinkedNode<'_>, src: &Source, found: &mut Vec<Section>) {
    let children = node.children().collect::<Vec<_>>();
    for (pos, child) in children.iter().enumerate() {
        if node.kind() == SyntaxKind::Markup {
            record_heading(node, &children, pos, child, src, found);
        }
        record_sections(child, src, found);
    }
}

fn record_heading(
    node: &LinkedNode<'_>,
    children: &[LinkedNode<'_>],
    pos: usize,
    child: &LinkedNode<'_>,
    src: &Source,
    found: &mut Vec<Section>,
) {
    let Some(heading) = child.cast::<ast::Heading>() else {
        return;
    };
    let depth = heading.depth().get();
    let end = children[pos + 1..]
        .iter()
        .find(|next| {
            next.cast::<ast::Heading>()
                .is_some_and(|next| next.depth().get() <= depth)
        })
        .map_or(node.range().end, |next| next.range().start);
    let body = child
        .find(heading.body().span())
        .expect("a heading carries its own body");
    found.push(Section {
        heading: child.range(),
        body: child.range().start..end,
        depth,
        name: src.text()[body.range()].trim().to_owned(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range_of(src: &Source, section: &Section) -> (String, String) {
        (
            src.text()[section.body.clone()].to_owned(),
            section.name.clone(),
        )
    }

    #[test]
    fn section_owns_body() {
        let src = Source::detached("= One\nfirst\n\n== Two\nsecond\n\n= Three\nthird\n");
        let found = sections(&src);
        assert_eq!(found.len(), 3);
        assert_eq!(
            found
                .iter()
                .map(|section| range_of(&src, section))
                .collect::<Vec<_>>(),
            [
                (
                    "= One\nfirst\n\n== Two\nsecond\n\n".to_owned(),
                    "One".to_owned()
                ),
                ("== Two\nsecond\n\n".to_owned(), "Two".to_owned()),
                ("= Three\nthird\n".to_owned(), "Three".to_owned()),
            ]
        );
    }

    #[test]
    fn section_empty_heading() {
        let src = Source::detached("= One");
        let found = sections(&src);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].body, found[0].heading);
    }
}
