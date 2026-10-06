//! The edits Enter applies in one source.
//!
//! Two answers, in this order:
//!
//! - a construct that continues itself opens the next line with its own marker — a list or enum
//!   item, a documentation comment, a block equation;
//! - a position inside a bracketed construct opens the next line one level deeper than the line
//!   that construct opened on.
//!
//! Every other position answers nothing, so the editor keeps its own newline.

use typst_syntax::{LinkedNode, Side, Source, SyntaxKind};

use crate::edit::Edit;
use crate::position::{self, Utf16Position};

/// The edits a caller applies in place of Enter's own behaviour at `at`.
///
/// The indentation is read from the syntax, not from the current line's own leading spaces: a
/// caret inside `#box[` opens a line one level deeper than `#box[` itself, in the call's argument
/// list, the code block, or the nested content block that bracket opened.
pub fn continuations(source: &Source, at: Utf16Position) -> Option<Vec<Edit>> {
    let offset = position::byte_offset(source.lines(), at).ok()?;
    let leaf = LinkedNode::new(source.root()).leaf_at(offset, Side::Before)?;
    let indent = indent_at(source.text(), leaf.offset());

    if let Some(comment) = ancestor(&leaf, SyntaxKind::LineComment) {
        let prefix = comment_prefix(comment.leaf_text());
        // A lone `//` continues nothing: an author writes those in passing, and the next line is
        // rarely another one. `///` and `//!` document what follows, so they continue.
        if prefix == "//" && !in_group(&comment) {
            return None;
        }
        let text = format!("\n{indent}{prefix} ");
        let insertion = text.len();
        return Some(vec![insert(offset, text, Some(insertion))]);
    }

    if let Some(equation) = ancestor(&leaf, SyntaxKind::Equation) {
        let written = &source.text()[equation.range()];
        let body = written.trim_start_matches('$').trim_end_matches('$');
        // A line break inside an equation that already holds something is a plain newline.
        if !body.trim().is_empty() {
            return None;
        }
        // The insertion opens the body, above the closing line when the equation closes on one.
        let (open, insertion) = if body.contains('\n') {
            (format!("\n{indent}  "), 1 + indent.len() + 2)
        } else {
            (format!("\n{indent}  \n{indent}"), 1 + indent.len() + 2)
        };
        return Some(vec![insert(offset, open, Some(insertion))]);
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

    open(source, &leaf, offset)
}

/// The edit that opens a line inside the innermost bracketed construct `offset` sits in.
///
/// The innermost enclosing construct is the one that answers: a caret after `#box[` opens a line
/// inside the content block that bracket opened, and one already inside the block keeps the level
/// the block's own opening line was indented to.
///
/// A caret on the closing bracket of a construct it is not inside — the end of `#box[]`, whose
/// `]` ends both the content block and the call's arguments — is answered by the construct that
/// encloses the caret rather than by the one that merely ends there.
fn open(source: &Source, leaf: &LinkedNode<'_>, offset: usize) -> Option<Vec<Edit>> {
    let text = source.text();
    for node in ancestors(leaf) {
        if !opened(node.kind()) {
            continue;
        }
        let range = node.range();
        if offset <= range.start {
            continue;
        }
        // A construct holds the caret when its closer sits on a later line than the caret: the body
        // between them is where the opened line belongs. A construct whose closer is on the caret's
        // own line ends there — `#box[|]`, `#f(a: 1,|)` — so the line belongs to whatever encloses
        // the caret instead of to a body this construct never had.
        if !text[offset..range.end].contains('\n') && !text[range.start..offset].contains('\n') {
            continue;
        }
        // One level deeper than the line the construct opened on, which is the depth the author
        // indented that construct's body to — whether or not the caret is the first thing in it.
        let width = line_indent(source, range.start) + INDENT;
        return Some(vec![insert(
            offset,
            format!("\n{}", " ".repeat(width)),
            None,
        )]);
    }
    None
}

/// The kinds that open a line of their own: a code block, a content block, a call's arguments, an
/// array, and a dictionary.
///
/// A parenthesised expression is absent on purpose — its parentheses wrap one expression, which is
/// already indented by whatever opens it — and so is a case's alternatives, whose braces an editor
/// reads from the same nodes as the block they end.
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

/// The spaces that open the line `at` sits on.
fn line_indent(source: &Source, at: usize) -> usize {
    indent_at(source.text(), at).len()
}

/// The edit that replaces nothing at `offset` and inserts `text`, leaving the insertion at `insertion`.
fn insert(at: usize, text: String, insertion: Option<usize>) -> Edit {
    Edit {
        range: at..at,
        text,
        insertion,
    }
}

/// The nearest ancestor of `leaf` that is a `kind`.
fn ancestor<'a>(leaf: &LinkedNode<'a>, kind: SyntaxKind) -> Option<LinkedNode<'a>> {
    let mut node = leaf.clone();
    loop {
        if node.kind() == kind {
            return Some(node);
        }
        node = node.parent()?.clone();
    }
}

/// `leaf` and each node that contains it, innermost first.
fn ancestors<'a>(leaf: &'a LinkedNode<'a>) -> impl Iterator<Item = &'a LinkedNode<'a>> {
    std::iter::successors(Some(leaf), |node| node.parent())
}

/// The marker a comment line starts with: its slashes, and a `!` among them.
fn comment_prefix(text: &str) -> String {
    let mut prefix = String::new();
    for character in text.chars() {
        match character {
            '/' => prefix.push('/'),
            '!' => prefix.push('!'),
            _ => break,
        }
    }
    prefix
}

/// Whether a comment line belongs to a run of comment lines.
fn in_group(leaf: &LinkedNode<'_>) -> bool {
    let Some(parent) = leaf.parent() else {
        return false;
    };
    let before = parent
        .children()
        .take(leaf.index())
        .rev()
        .take_while(|child| !is_blank(child.kind()))
        .filter(|child| child.kind() == SyntaxKind::LineComment)
        .count();
    let after = parent
        .children()
        .skip(leaf.index() + 1)
        .take_while(|child| !is_blank(child.kind()))
        .filter(|child| child.kind() == SyntaxKind::LineComment)
        .count();
    before + after > 0
}

fn is_blank(kind: SyntaxKind) -> bool {
    matches!(kind, SyntaxKind::Parbreak)
}

/// The list or enum item the offset continues, when it sits at or after the item's own text.
fn item_at<'a>(leaf: &LinkedNode<'a>, offset: usize) -> Option<LinkedNode<'a>> {
    if let Some(item) =
        ancestor(leaf, SyntaxKind::ListItem).or_else(|| ancestor(leaf, SyntaxKind::EnumItem))
    {
        return (offset >= item.range().end).then_some(item);
    }
    if !matches!(leaf.kind(), SyntaxKind::Space | SyntaxKind::Parbreak) {
        return None;
    }
    let previous = leaf.prev_sibling()?;
    matches!(previous.kind(), SyntaxKind::ListItem | SyntaxKind::EnumItem).then(|| previous.clone())
}

/// The whitespace that opens the line `at` sits on.
fn indent_at(text: &str, at: usize) -> String {
    let start = text[..at].rfind('\n').map_or(0, |newline| newline + 1);
    text[start..at]
        .chars()
        .take_while(|character| *character == ' ')
        .collect()
}

/// One level of indentation.
const INDENT: usize = 2;

#[cfg(test)]
mod tests {
    use super::*;

    /// The text one Enter press inserts at the `|` marker, with the insertion it leaves marked.
    fn continued(marked: &str) -> Option<String> {
        let offset = marked.find('|').expect("a offset marker");
        let source = Source::detached(marked.replace('|', ""));
        let at = position::utf16_range(source.lines(), offset..offset)?.start;
        continuations(&source, at).map(|edits| {
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
    fn list_item_continues_its_own_marker() {
        assert_eq!(continued("- one|").as_deref(), Some("\n- |"));
        assert_eq!(continued("+ one|").as_deref(), Some("\n+ |"));
        assert_eq!(continued("  - indented|").as_deref(), Some("\n  - |"));
        assert_eq!(continued("- one| two"), None, "a offset inside the item");
    }

    #[test]
    fn only_doc_comments_continue() {
        assert_eq!(continued("/// one|").as_deref(), Some("\n/// |"));
        assert_eq!(continued("/// one|\n/// two").as_deref(), Some("\n/// |"));
        assert_eq!(continued("//! one|").as_deref(), Some("\n//! |"));
        assert_eq!(continued("// one|"), None, "a lone line comment");
    }

    #[test]
    fn empty_equation_opens_body() {
        assert_eq!(continued("$|$").as_deref(), Some("\n  |\n"));
        assert_eq!(continued("$x|$"), None, "an equation that holds something");
    }

    #[test]
    fn opened_construct_indents_its_body() {
        assert_eq!(continued("#box[\n|]").as_deref(), Some("\n  "));
        assert_eq!(continued("#box[|\n]").as_deref(), Some("\n  "));
        assert_eq!(continued("#{\n|}\n").as_deref(), Some("\n  "));
        assert_eq!(continued("#f(\n|)\n").as_deref(), Some("\n  "));
        assert_eq!(continued("#let x = (\n|)\n").as_deref(), Some("\n  "));
    }

    #[test]
    fn nested_construct_adds_one_level_to_its_opener_line() {
        assert_eq!(
            continued("#box[\n  #box[|\n  ]\n]").as_deref(),
            Some("\n    ")
        );
        assert_eq!(
            continued("#box[\n  #box[\n    text|\n  ]\n]").as_deref(),
            Some("\n    ")
        );
    }

    #[test]
    fn caret_before_the_closer_answers_nothing() {
        assert_eq!(continued("#box[|]"), None);
        assert_eq!(continued("#{|}"), None);
        assert_eq!(continued("#f(|)"), None);
        assert_eq!(continued("#f(a: 1,|)"), None);
        assert_eq!(continued("#table(columns: 2)[a|][b]"), None);
    }

    #[test]
    fn plain_text_answers_nothing() {
        assert_eq!(continued("A paragraph|"), None);
        assert_eq!(continued("= Head|"), None);
        assert_eq!(continued("#set text(red|)"), None);
    }
}
