//! Postfix calls that wrap a written value.

use std::ops::Range;

use typst_syntax::{LinkedNode, Side, Source, SyntaxKind};

use crate::edit::Edit;

struct Call {
    name: &'static str,
    before: &'static str,
    after: &'static str,
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
pub fn wraps(src: &Source, at: usize) -> Vec<Wrap> {
    let Some(access) = field(src, at) else {
        return Vec::new();
    };
    let val = &src.text()[access.value.clone()];
    let written = &src.text()[access.written];
    WRAPS
        .iter()
        .filter(|call| call.name.starts_with(written))
        .map(|call| Wrap {
            name: call.name,
            documentation: call.documentation,
            edit: Edit {
                range: access.range.clone(),
                text: format!("{}{val}{}", call.before, call.after),
                insertion: Some(call.before.len()),
            },
        })
        .collect()
}

struct FieldAccess {
    value: Range<usize>,
    written: Range<usize>,
    range: Range<usize>,
}

fn field(src: &Source, at: usize) -> Option<FieldAccess> {
    let node = LinkedNode::new(src.root()).leaf_at(at, Side::Before)?;
    if node.kind() != SyntaxKind::Ident {
        return None;
    }
    if node
        .prev_leaf()
        .is_none_or(|prev| prev.kind() != SyntaxKind::Dot)
    {
        return None;
    }
    let access = node.parent()?;
    if access.kind() != SyntaxKind::FieldAccess {
        return None;
    }
    Some(FieldAccess {
        value: access.children().next()?.range(),
        written: node.range(),
        range: access.range(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(marked: &str, name: &str) -> Option<Wrap> {
        let cursor = marked.find('|').expect("a cursor marker");
        let src = Source::detached(marked.replace('|', ""));
        wraps(&src, cursor)
            .into_iter()
            .find(|wrap| wrap.name == name)
    }

    fn accepted_text(text: &str, edit: &Edit) -> String {
        let mut accepted = text.to_owned();
        accepted.replace_range(edit.range.clone(), &edit.text);
        accepted
    }

    #[test]
    fn wrap_replaces_field_access() {
        let align = wrap("#body.al|", "align").expect("the align wrap");
        assert_eq!(
            accepted_text("#body.al", &align.edit),
            "#align(center, body)"
        );
        assert_eq!(align.edit.insertion, Some("align(center, ".len()));

        let figure = wrap("#figure.fig|", "figure").expect("the figure wrap");
        assert_eq!(
            accepted_text("#figure.fig", &figure.edit),
            "#figure(figure)"
        );
    }

    #[test]
    fn wrap_filters_partial_name() {
        let src = Source::detached("#body.ali".to_owned());
        let names: Vec<&str> = wraps(&src, 7).into_iter().map(|wrap| wrap.name).collect();
        assert_eq!(names, ["align"]);
    }
}
