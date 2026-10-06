//! The rewrites the selected element justifies: wrapping it in a content block or figure, and the
//! equation forms its dollars spell.
//!
//! A rewrite reads the syntax the selection covers, so it costs what that selection covers.

use lsp_types::{CodeActionKind, CodeActionOrCommand, Range, TextEdit, Uri};
use tola_typst::typst::syntax::{LinkedNode, Source, SyntaxKind, SyntaxMode, ast};

use super::ancestors::selection_ancestor;
use super::edits::{edit_action, rewrite_action};

/// Whether `at` sits in content: the leaf ending there answers, and the leaf opening there answers
/// when the ending leaf does not — a position right after an embedded expression reads as code
/// behind it and as content ahead of it.
fn content_at(source: &Source, at: usize) -> bool {
    tola_typst_syntax::syntax::cursor_leaves(source, at)
        .into_iter()
        .flatten()
        .any(|leaf| {
            matches!(
                leaf.mode_after(),
                Some(SyntaxMode::Markup | SyntaxMode::Math)
            )
        })
}

/// The action wrapping the selection in a content block.
///
/// A selection in markup or math is content the author can address as a value; one in code is
/// already a value, and wrapping it would restate it.
pub(super) fn wrap_content_block(
    uri: &Uri,
    source: &Source,
    span: &std::ops::Range<usize>,
) -> Vec<CodeActionOrCommand> {
    if span.is_empty() {
        return Vec::new();
    }
    // Both ends must sit in content: a selection reaching into code would move the brackets over
    // text they cannot enclose.
    if !content_at(source, span.start) || !content_at(source, span.end) {
        return Vec::new();
    }
    let (Some(start), Some(end)) = (
        crate::position::utf16_range(source.lines(), span.start..span.start),
        crate::position::utf16_range(source.lines(), span.end..span.end),
    ) else {
        return Vec::new();
    };
    vec![edit_action(
        uri,
        "wrap the selection in a content block".to_owned(),
        CodeActionKind::REFACTOR_REWRITE,
        vec![
            TextEdit {
                range: start,
                new_text: "#[".to_owned(),
            },
            TextEdit {
                range: end,
                new_text: "]".to_owned(),
            },
        ],
    )]
}

/// The inline and block rewrites the equation the selection sits inside justifies.
///
/// A block equation keeps its sentence punctuation inside the dollars, so a rewrite that makes
/// the equation block has the punctuation in with it.
pub(super) fn equation_rewrites(
    uri: &Uri,
    source: &Source,
    span: &std::ops::Range<usize>,
) -> Vec<CodeActionOrCommand> {
    let Some(equation) = selection_ancestor(source, span, |node| node.is::<ast::Equation>()) else {
        return Vec::new();
    };
    // The math child is read by kind: an empty body's span is a point, and a search by span
    // enters the dollar that ends there instead.
    let Some(body) = equation
        .children()
        .find(|child| child.kind() == SyntaxKind::Math)
    else {
        return Vec::new();
    };
    let mut children = equation.children();
    let Some(first_dollar) = children
        .by_ref()
        .take(1)
        .find(|child| child.kind() == SyntaxKind::Dollar)
    else {
        return Vec::new();
    };
    let Some(last_dollar) = children
        .rev()
        .take(1)
        .find(|child| child.kind() == SyntaxKind::Dollar)
    else {
        return Vec::new();
    };
    // An unclosed equation spells one dollar, and a rewrite has no second one to reach.
    if first_dollar.offset() == last_dollar.offset() {
        return Vec::new();
    }
    let body_range = body.range();
    let front = first_dollar.range().end..body_range.start;
    let back = body_range.end..last_dollar.range().start;
    let punctuation = punctuation_after(source, equation.range().end);
    let mut actions = Vec::new();
    let (title, gap) = if is_block_equation(&equation) {
        ("convert to an inline equation", "")
    } else {
        ("convert to a block equation", " ")
    };
    if let Some(action) = equation_rewrite(
        uri,
        source,
        front.clone(),
        back.clone(),
        punctuation,
        gap,
        title,
    ) {
        actions.push(action);
    }
    if let Some(action) = equation_rewrite(
        uri,
        source,
        front,
        back,
        punctuation,
        "\n",
        "convert to a multi-line block equation",
    ) {
        actions.push(action);
    }
    actions
}

/// The rewrite putting `gap` between each dollar and the body, or between the two dollars of an
/// equation that holds nothing.
fn equation_rewrite(
    uri: &Uri,
    source: &Source,
    front: std::ops::Range<usize>,
    back: std::ops::Range<usize>,
    punctuation: Option<(char, Range)>,
    gap: &str,
    title: &str,
) -> Option<CodeActionOrCommand> {
    let front = crate::position::utf16_range(source.lines(), front)?;
    let back = crate::position::utf16_range(source.lines(), back)?;
    let mut edits = Vec::new();
    if front == back {
        // An empty equation leaves one gap: both dollars meet the body, so one insertion holds
        // the whole rewrite, the sentence punctuation included.
        let text = match punctuation.as_ref() {
            Some((character, _)) if !gap.is_empty() => format!("{gap}{character}{gap}"),
            _ => gap.to_owned(),
        };
        if !text.is_empty() {
            edits.push(TextEdit {
                range: front,
                new_text: text,
            });
        }
    } else {
        edits.push(TextEdit {
            range: front,
            new_text: gap.to_owned(),
        });
        edits.push(TextEdit {
            range: back,
            new_text: if gap.is_empty() {
                String::new()
            } else {
                punctuation
                    .as_ref()
                    .map(|(character, _)| format!("{character}{gap}"))
                    .unwrap_or_else(|| gap.to_owned())
            },
        });
    }
    if !gap.is_empty()
        && let Some((_, range)) = &punctuation
    {
        edits.push(TextEdit {
            range: *range,
            new_text: String::new(),
        });
    }
    if edits.is_empty() {
        return None;
    }
    Some(edit_action(
        uri,
        title.to_owned(),
        CodeActionKind::REFACTOR_REWRITE,
        edits,
    ))
}

/// Whether the equation is written as a block: whitespace stands between each dollar and the body.
fn is_block_equation(equation: &LinkedNode<'_>) -> bool {
    let is_space =
        |node: Option<&LinkedNode<'_>>| node.is_some_and(|node| node.kind() == SyntaxKind::Space);
    let mut children = equation.children().skip(1);
    let mut first = children.next();
    if first
        .as_ref()
        .is_some_and(|first| first.is_empty() && first.kind() == SyntaxKind::Math)
    {
        first = children.next();
    }
    is_space(first.as_ref()) && is_space(equation.children().nth_back(1).as_ref())
}

/// The punctuation immediately after an equation, which its block form has inside the dollars.
///
/// A run of punctuation is left alone: only the last mark of a sentence belongs to the equation,
/// and a mark the author wrote for the sentence stays where it is.
fn punctuation_after(source: &Source, end: usize) -> Option<(char, Range)> {
    let mut characters = source.text().get(end..)?.chars();
    let character = characters.next()?;
    if !(character.is_ascii_punctuation() || NON_ASCII_PUNCTUATION.contains(character))
        || characters
            .next()
            .is_some_and(|next| next.is_ascii_punctuation())
    {
        return None;
    }
    let range = crate::position::utf16_range(source.lines(), end..end + character.len_utf8())?;
    Some((character, range))
}

/// The sentence-closing punctuation an equation has inside its block dollars, beyond ASCII.
const NON_ASCII_PUNCTUATION: &str = "。，、；：？！…—·）］｝〉》」』】〕”’";

/// The action wrapping the selected element in a figure with a caption.
pub(super) fn figure_wrap(
    uri: &Uri,
    source: &Source,
    span: &std::ops::Range<usize>,
) -> Vec<CodeActionOrCommand> {
    let Some(node) = selection_ancestor(source, span, |node| {
        let range = node.range();
        range.start <= span.start && span.end <= range.end && figure_element(node).is_some()
    }) else {
        return Vec::new();
    };
    let Some((name, hash)) = figure_element(&node) else {
        return Vec::new();
    };
    let text = &source.text()[node.range()];
    let hash = if hash { "#" } else { "" };
    let new_text = if text.contains('\n') {
        format!("{hash}figure(\n  caption: [Caption],\n  {text},\n)")
    } else {
        format!("{hash}figure(caption: [Caption], {text})")
    };
    let Some(range) = crate::position::utf16_range(source.lines(), node.range()) else {
        return Vec::new();
    };
    vec![rewrite_action(
        uri,
        format!("wrap the {name} in a figure with a caption"),
        range,
        new_text,
    )]
}

/// The element a figure can hold, and whether the wrapping call needs a hash of its own.
///
/// A call, code block, or content block an author selected already has the hash that made it
/// an expression; a raw block in markup has none, and the figure call takes one.
fn figure_element(node: &LinkedNode<'_>) -> Option<(&'static str, bool)> {
    match node.kind() {
        SyntaxKind::FuncCall => {
            let call = node.cast::<ast::FuncCall>()?;
            let ast::Expr::Ident(ident) = call.callee() else {
                return None;
            };
            match ident.get().as_str() {
                "image" => Some(("image", false)),
                "table" => Some(("table", false)),
                "raw" => Some(("raw", false)),
                _ => None,
            }
        }
        SyntaxKind::CodeBlock => Some(("code block", false)),
        SyntaxKind::ContentBlock => Some(("content block", false)),
        SyntaxKind::Raw => {
            let markup = matches!(
                node.mode_after(),
                Some(SyntaxMode::Markup | SyntaxMode::Math)
            );
            Some(("raw block", markup))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_actions::selection_actions;
    use crate::code_actions::tests::{edits_of, uri};
    use lsp_types::Position;

    #[test]
    fn selection_wraps_in_content_block() {
        // The brackets land where the selection starts and ends, whichever leaves bound it:
        // inside markup, at the document's first position, or right after an embedded call.
        for (text, span, expected) in [
            (
                "text only\n",
                Range::new(Position::new(0, 1), Position::new(0, 4)),
                [(Position::new(0, 1), "#["), (Position::new(0, 4), "]")],
            ),
            (
                "text only\n",
                Range::new(Position::new(0, 0), Position::new(0, 4)),
                [(Position::new(0, 0), "#["), (Position::new(0, 4), "]")],
            ),
            (
                "x #f(1) y\n",
                Range::new(Position::new(0, 0), Position::new(0, 7)),
                [(Position::new(0, 0), "#["), (Position::new(0, 7), "]")],
            ),
        ] {
            let actions = selection_actions(&uri(), &Source::detached(text), span);
            let edits = edits_of(&actions, "wrap the selection in a content block");
            assert_eq!(edits.len(), 2, "{text:?}");
            assert!(
                edits.iter().all(|(range, _)| range.start == range.end),
                "{text:?}"
            );
            assert_eq!(
                edits
                    .iter()
                    .map(|(range, new_text)| (range.start, new_text.as_str()))
                    .collect::<Vec<_>>(),
                expected,
                "{text:?}"
            );
        }
    }

    #[test]
    fn selection_crossing_code_offers_no_wrap() {
        let actions = selection_actions(
            &uri(),
            &Source::detached("text #let x = 1\n"),
            Range::new(Position::new(0, 0), Position::new(0, 7)),
        );

        assert!(edits_of(&actions, "wrap the selection in a content block").is_empty());
    }

    #[test]
    fn inline_equation_offers_block_forms() {
        let actions = selection_actions(
            &uri(),
            &Source::detached("$x + 1$, and more\n"),
            Range::new(Position::new(0, 3), Position::new(0, 5)),
        );

        // A block equation keeps its sentence punctuation inside the dollars.
        assert_eq!(
            edits_of(&actions, "convert to a block equation"),
            [
                (
                    Range::new(Position::new(0, 1), Position::new(0, 1)),
                    " ".to_owned()
                ),
                (
                    Range::new(Position::new(0, 6), Position::new(0, 6)),
                    ", ".to_owned()
                ),
                (
                    Range::new(Position::new(0, 7), Position::new(0, 8)),
                    String::new()
                ),
            ]
        );
        assert_eq!(
            edits_of(&actions, "convert to a multi-line block equation"),
            [
                (
                    Range::new(Position::new(0, 1), Position::new(0, 1)),
                    "\n".to_owned()
                ),
                (
                    Range::new(Position::new(0, 6), Position::new(0, 6)),
                    ",\n".to_owned()
                ),
                (
                    Range::new(Position::new(0, 7), Position::new(0, 8)),
                    String::new()
                ),
            ]
        );
    }

    #[test]
    fn block_equation_converts_to_inline() {
        let actions = selection_actions(
            &uri(),
            &Source::detached("$ x + 1 $\n"),
            Range::new(Position::new(0, 3), Position::new(0, 5)),
        );

        assert_eq!(
            edits_of(&actions, "convert to an inline equation"),
            [
                (
                    Range::new(Position::new(0, 1), Position::new(0, 2)),
                    String::new()
                ),
                (
                    Range::new(Position::new(0, 7), Position::new(0, 8)),
                    String::new()
                ),
            ]
        );
    }

    #[test]
    fn empty_equation_keeps_punctuation_inside() {
        let actions = selection_actions(
            &uri(),
            &Source::detached("$$."),
            Range::new(Position::new(0, 1), Position::new(0, 1)),
        );

        assert_eq!(
            edits_of(&actions, "convert to a block equation"),
            [
                (
                    Range::new(Position::new(0, 1), Position::new(0, 1)),
                    " . ".to_owned()
                ),
                (
                    Range::new(Position::new(0, 2), Position::new(0, 3)),
                    String::new()
                ),
            ]
        );
    }

    /// A selection starting inside a nested equation and reaching past it is not answered by the
    /// outer equation: the inner one does not hold it, and no outer one answers in its place.
    #[test]
    fn outer_equation_does_not_answer_the_nested_selection() {
        let text = "$ a + #{ let inner = $x + 1$; inner } $";
        let across = selection_actions(
            &uri(),
            &Source::detached(text),
            Range::new(Position::new(0, 22), Position::new(0, 39)),
        );
        for title in [
            "convert to an inline equation",
            "convert to a block equation",
            "convert to a multi-line block equation",
        ] {
            assert!(edits_of(&across, title).is_empty(), "{title}");
        }
        // The inner equation still answers a selection it holds.
        let inside = selection_actions(
            &uri(),
            &Source::detached(text),
            Range::new(Position::new(0, 22), Position::new(0, 27)),
        );
        assert!(!edits_of(&inside, "convert to a block equation").is_empty());
    }

    #[test]
    fn image_call_offers_figure_wrap() {
        let actions = selection_actions(
            &uri(),
            &Source::detached("#image(\"logo.svg\")\n"),
            Range::new(Position::new(0, 1), Position::new(0, 18)),
        );

        // The hash the call already has stays outside the replaced range.
        assert_eq!(
            edits_of(&actions, "wrap the image in a figure with a caption"),
            [(
                Range::new(Position::new(0, 1), Position::new(0, 18)),
                "figure(caption: [Caption], image(\"logo.svg\"))".to_owned(),
            )]
        );
    }

    #[test]
    fn leading_raw_block_wraps_with_hash() {
        let actions = selection_actions(
            &uri(),
            &Source::detached("```typ\ncode\n```\n"),
            Range::new(Position::new(0, 0), Position::new(2, 3)),
        );

        assert_eq!(
            edits_of(&actions, "wrap the raw block in a figure with a caption"),
            [(
                Range::new(Position::new(0, 0), Position::new(2, 3)),
                "#figure(\n  caption: [Caption],\n  ```typ\ncode\n```,\n)".to_owned(),
            )]
        );
    }
}
