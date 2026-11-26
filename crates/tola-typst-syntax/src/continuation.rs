//! The edits Enter applies in one source.

use typst_syntax::{LinkedNode, Side, Source, SyntaxKind};

use crate::edit::Edit;
use crate::position::{self, Utf16Position};

const INDENT: usize = 2;

/// The edits a caller applies in place of Enter's own behaviour at `at`.
pub fn continuations(src: &Source, at: Utf16Position) -> Option<Vec<Edit>> {
    let offset = position::byte_offset(src.lines(), at).ok()?;
    let leaf = LinkedNode::new(src.root()).leaf_at(offset, Side::Before)?;
    let indent = indent_at(src.text(), leaf.offset());

    if let Some(comment) = ancestor(&leaf, SyntaxKind::LineComment) {
        return comment_continuation(&comment, offset, &indent);
    }
    if let Some(equation) = ancestor(&leaf, SyntaxKind::Equation) {
        return equation_continuation(src, &equation, offset, &indent);
    }
    if let Some(item) = item_at(&leaf, offset) {
        let marker = if item.kind() == SyntaxKind::ListItem {
            "-"
        } else {
            "+"
        };
        let text = format!("\n{indent}{marker} ");
        let insertion = text.len();
        return Some(vec![insert(offset, text, Some(insertion))]);
    }
    open(src, &leaf, offset)
}

fn comment_continuation(
    comment: &LinkedNode<'_>,
    offset: usize,
    indent: &str,
) -> Option<Vec<Edit>> {
    let prefix = comment_prefix(comment.leaf_text());
    // Lone `//` does not continue; grouped `//`, `///`, and `//!` do.
    if prefix == "//" && !in_group(comment) {
        return None;
    }
    let text = format!("\n{indent}{prefix} ");
    let insertion = text.len();
    Some(vec![insert(offset, text, Some(insertion))])
}

fn equation_continuation(
    src: &Source,
    equation: &LinkedNode<'_>,
    offset: usize,
    indent: &str,
) -> Option<Vec<Edit>> {
    let written = &src.text()[equation.range()];
    let body = written.trim_start_matches('$').trim_end_matches('$');
    if !body.trim().is_empty() {
        return None;
    }
    let (open, insertion) = if body.contains('\n') {
        (format!("\n{indent}  "), 1 + indent.len() + 2)
    } else {
        (format!("\n{indent}  \n{indent}"), 1 + indent.len() + 2)
    };
    Some(vec![insert(offset, open, Some(insertion))])
}

fn open(src: &Source, leaf: &LinkedNode<'_>, offset: usize) -> Option<Vec<Edit>> {
    let text = src.text();
    for node in ancestors(leaf) {
        if !opened(node.kind()) {
            continue;
        }
        let range = node.range();
        if offset <= range.start {
            continue;
        }
        // Single-line constructs ending on the caret's line belong to their enclosing construct.
        if !text[offset..range.end].contains('\n') && !text[range.start..offset].contains('\n') {
            continue;
        }
        let width = indent_at(text, range.start).len() + INDENT;
        return Some(vec![insert(
            offset,
            format!("\n{}", " ".repeat(width)),
            None,
        )]);
    }
    None
}

fn opened(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::CodeBlock
            | SyntaxKind::ContentBlock
            | SyntaxKind::Args
            | SyntaxKind::Array
            | SyntaxKind::Dict
    )
}

fn insert(at: usize, text: String, insertion: Option<usize>) -> Edit {
    Edit {
        range: at..at,
        text,
        insertion,
    }
}

fn ancestor<'a>(leaf: &LinkedNode<'a>, kind: SyntaxKind) -> Option<LinkedNode<'a>> {
    ancestors(leaf).find(|node| node.kind() == kind).cloned()
}

fn ancestors<'a, 'leaf>(
    leaf: &'leaf LinkedNode<'a>,
) -> impl Iterator<Item = &'leaf LinkedNode<'a>> {
    std::iter::successors(Some(leaf), |node| node.parent())
}

fn comment_prefix(text: &str) -> String {
    let mut prefix = String::new();
    for ch in text.chars() {
        match ch {
            '/' => prefix.push('/'),
            '!' => prefix.push('!'),
            _ => break,
        }
    }
    prefix
}

fn in_group(leaf: &LinkedNode<'_>) -> bool {
    let Some(parent) = leaf.parent() else {
        return false;
    };
    let before = parent
        .children()
        .take(leaf.index())
        .rev()
        .take_while(|child| child.kind() != SyntaxKind::Parbreak)
        .filter(|child| child.kind() == SyntaxKind::LineComment)
        .count();
    let after = parent
        .children()
        .skip(leaf.index() + 1)
        .take_while(|child| child.kind() != SyntaxKind::Parbreak)
        .filter(|child| child.kind() == SyntaxKind::LineComment)
        .count();
    before + after > 0
}

fn item_at<'a>(leaf: &LinkedNode<'a>, offset: usize) -> Option<LinkedNode<'a>> {
    if let Some(item) =
        ancestor(leaf, SyntaxKind::ListItem).or_else(|| ancestor(leaf, SyntaxKind::EnumItem))
    {
        return (offset >= item.range().end).then_some(item);
    }
    if !matches!(leaf.kind(), SyntaxKind::Space | SyntaxKind::Parbreak) {
        return None;
    }
    let prev = leaf.prev_sibling()?;
    matches!(prev.kind(), SyntaxKind::ListItem | SyntaxKind::EnumItem).then(|| prev.clone())
}

fn indent_at(text: &str, at: usize) -> String {
    let start = text[..at].rfind('\n').map_or(0, |nl| nl + 1);
    text[start..at]
        .chars()
        .take_while(|ch| *ch == ' ')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn continued(marked: &str) -> Option<String> {
        let offset = marked.find('|').expect("an offset marker");
        let src = Source::detached(marked.replace('|', ""));
        let at = position::utf16_range(src.lines(), offset..offset)?.start;
        continuations(&src, at).map(|edits| {
            assert_eq!(edits.len(), 1, "{edits:?}");
            let edit = &edits[0];
            let mut text = edit.text.clone();
            if let Some(insertion) = edit.insertion {
                text.insert(insertion, '|');
            }
            text
        })
    }

    #[test]
    fn continuation_list_items() {
        assert_eq!(continued("- one|").as_deref(), Some("\n- |"));
        assert_eq!(continued("+ one|").as_deref(), Some("\n+ |"));
        assert_eq!(continued("  - indented|").as_deref(), Some("\n  - |"));
        assert_eq!(continued("- one| two"), None, "an offset inside the item");
    }

    #[test]
    fn continuation_doc_comments() {
        assert_eq!(continued("/// one|").as_deref(), Some("\n/// |"));
        assert_eq!(continued("/// one|\n/// two").as_deref(), Some("\n/// |"));
        assert_eq!(continued("//! one|").as_deref(), Some("\n//! |"));
        assert_eq!(continued("// one|"), None, "a lone line comment");
    }

    #[test]
    fn continuation_empty_equation() {
        assert_eq!(continued("$|$").as_deref(), Some("\n  |\n"));
        assert_eq!(continued("$x|$"), None, "an equation that holds something");
    }
}
