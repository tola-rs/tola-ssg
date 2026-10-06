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
/// A heading owns the content that follows it until the next heading of the same or a higher
/// level, so a section ends where its parent's markup ends. A heading inside a content block
/// owns only content inside that block.
pub fn sections(source: &Source) -> Vec<Section> {
    let mut sections = Vec::new();
    record_sections(&LinkedNode::new(source.root()), source, &mut sections);
    sections
}

/// Record every heading section of `node`, in document order.
fn record_sections(node: &LinkedNode<'_>, source: &Source, sections: &mut Vec<Section>) {
    let children = node.children().collect::<Vec<_>>();
    for (position, child) in children.iter().enumerate() {
        if node.kind() == SyntaxKind::Markup {
            record_heading(node, &children, position, child, source, sections);
        }
        record_sections(child, source, sections);
    }
}

/// Record `child`'s section when it is a heading of the markup `node`.
fn record_heading(
    node: &LinkedNode<'_>,
    children: &[LinkedNode<'_>],
    position: usize,
    child: &LinkedNode<'_>,
    source: &Source,
    sections: &mut Vec<Section>,
) {
    let Some(heading) = child.cast::<ast::Heading>() else {
        return;
    };
    let depth = heading.depth().get();
    let end = children[position + 1..]
        .iter()
        .find(|next| {
            next.cast::<ast::Heading>()
                .is_some_and(|next| next.depth().get() <= depth)
        })
        .map_or(node.range().end, |next| next.range().start);
    let body = child
        .find(heading.body().span())
        .expect("a heading carries its own body");
    sections.push(Section {
        heading: child.range(),
        body: child.range().start..end,
        depth,
        name: source.text()[body.range()].trim().to_owned(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range_of(source: &Source, section: &Section) -> (String, String) {
        (
            source.text()[section.body.clone()].to_owned(),
            section.name.clone(),
        )
    }

    #[test]
    fn section_owns_content_after_its_heading() {
        let source = Source::detached("= One\nfirst\n\n== Two\nsecond\n\n= Three\nthird\n");
        let sections = sections(&source);
        assert_eq!(sections.len(), 3);
        assert_eq!(
            sections
                .iter()
                .map(|section| range_of(&source, section))
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
    fn heading_owns_its_block_content() {
        let source =
            Source::detached("= One\n#block[\n  == Two\n  inside\n]\nafter\n\n= Three\nouter\n");
        let sections = sections(&source);
        assert_eq!(
            sections
                .iter()
                .map(|section| section.name.as_str())
                .collect::<Vec<_>>(),
            ["One", "Two", "Three"]
        );
        assert_eq!(
            &source.text()[sections[1].body.clone()],
            "== Two\n  inside\n"
        );
        assert_eq!(
            &source.text()[sections[0].body.clone()],
            "= One\n#block[\n  == Two\n  inside\n]\nafter\n\n"
        );
    }

    #[test]
    fn heading_without_content_owns_its_line() {
        let source = Source::detached("= One");
        let sections = sections(&source);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].body, sections[0].heading);
    }
}
