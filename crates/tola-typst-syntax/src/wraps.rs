//! The calls that wrap a value the author has already written.
//!
//! A wrap rewrites the value the dot stands on rather than the name after it, so the author keeps
//! what they wrote: `#body.al` becomes `#align(center, body)`. Every wrap is a Typst call, so each
//! one is valid wherever an expression is.

use std::ops::Range as ByteRange;

use typst_syntax::{LinkedNode, Side, Source, SyntaxKind};

use crate::edit::Edit;

/// The call one wrap makes.
struct Call {
    /// The name the author types after the dot.
    name: &'static str,
    /// The text the wrap opens with.
    before: &'static str,
    /// The text the wrap closes with.
    after: &'static str,
    /// What the wrap does, in the site author's words.
    documentation: &'static str,
}

/// One wrap the value at an offset accepts, with the edit it makes.
pub struct Wrap {
    /// The name the author types after the dot.
    pub name: &'static str,
    /// What the wrap does, in the site author's words.
    pub documentation: &'static str,
    /// The edit that puts the value inside the wrap.
    pub edit: Edit,
}

const WRAPS: &[Call] = &[
    Call {
        name: "align",
        before: "align(center, ",
        after: ")",
        documentation: "Place the value at one alignment.",
    },
    Call {
        name: "figure",
        before: "figure(",
        after: ")",
        documentation: "Caption the value as a figure.",
    },
    Call {
        name: "text",
        before: "text(size: ",
        after: ")",
        documentation: "Set the value in a chosen size.",
    },
    Call {
        name: "fill",
        before: "text(fill: ",
        after: ")",
        documentation: "Fill the value with a colour.",
    },
    Call {
        name: "strong",
        before: "strong(",
        after: ")",
        documentation: "Increase the value's font weight.",
    },
    Call {
        name: "emph",
        before: "emph(",
        after: ")",
        documentation: "Set the value in italic.",
    },
    Call {
        name: "link",
        before: "link(",
        after: ")",
        documentation: "Turn the value into a hyperlink.",
    },
];

/// The wraps the value under the cursor accepts.
pub fn wraps(source: &Source, at: usize) -> Vec<Wrap> {
    let Some((receiver, written)) = field(source, at) else {
        return Vec::new();
    };
    let value = &source.text()[receiver.clone()];
    let written = &source.text()[written];
    WRAPS
        .iter()
        .filter(|call| call.name.starts_with(written))
        .map(|call| Wrap {
            name: call.name,
            documentation: call.documentation,
            edit: Edit {
                range: receiver.clone(),
                text: format!("{}{value}{}", call.before, call.after),
                insertion: Some(call.before.len()),
            },
        })
        .collect()
}

/// The receiver and the written name of the field access the cursor is inside.
fn field(source: &Source, at: usize) -> Option<(ByteRange<usize>, ByteRange<usize>)> {
    let node = LinkedNode::new(source.root()).leaf_at(at, Side::Before)?;
    if node.kind() != SyntaxKind::Ident {
        return None;
    }
    // The name after the dot is the one whose value the wraps replace; the target is the value.
    if node
        .prev_leaf()
        .is_none_or(|previous| previous.kind() != SyntaxKind::Dot)
    {
        return None;
    }
    let access = node.parent()?;
    if access.kind() != SyntaxKind::FieldAccess {
        return None;
    }
    let target = access.children().next()?;
    Some((target.range(), node.range()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wrap of `name` at the cursor the text marks.
    fn wrap(marked: &str, name: &str) -> Option<Wrap> {
        let cursor = marked.find('|').expect("a cursor marker");
        let source = Source::detached(marked.replace('|', ""));
        wraps(&source, cursor)
            .into_iter()
            .find(|wrap| wrap.name == name)
    }

    #[test]
    fn wrap_replaces_the_value_it_wraps() {
        let align = wrap("#body.al|", "align").expect("the align wrap");
        assert_eq!(align.edit.text, "align(center, body)");
        assert_eq!(&"#body.al"[align.edit.range.clone()], "body");
        assert_eq!(align.edit.insertion, Some("align(center, ".len()));

        let figure = wrap("#figure.fig|", "figure").expect("the figure wrap");
        assert_eq!(figure.edit.text, "figure(figure)");
    }

    #[test]
    fn partial_name_keeps_the_named_wrap() {
        let source = Source::detached("#body.ali".to_owned());
        let names: Vec<&str> = wraps(&source, 7)
            .into_iter()
            .map(|wrap| wrap.name)
            .collect();
        assert_eq!(names, ["align"]);
    }

    #[test]
    fn offset_without_field_answers_nothing() {
        for text in ["#body|", "#body.|", "#(1 + 2)|", "Body text|"] {
            let at = text.find('|').unwrap();
            let source = Source::detached(text.replace('|', ""));
            assert!(
                wraps(&source, at).is_empty(),
                "{text:?} writes no field access"
            );
        }
    }
}
