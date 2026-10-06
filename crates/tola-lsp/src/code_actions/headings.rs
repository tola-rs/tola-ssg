//! The heading depth changes the selection justifies.

use lsp_types::{CodeActionOrCommand, Uri};
use tola_typst::typst::syntax::{Source, SyntaxKind, ast};

use super::ancestors::selection_ancestor;
use super::edits::rewrite_action;

/// The depth changes the heading the whole selection sits in justifies.
///
/// Only the marker is rewritten, so the heading's own text stays as the author wrote it.
pub(super) fn heading_depth(
    uri: &Uri,
    source: &Source,
    span: std::ops::Range<usize>,
) -> Vec<CodeActionOrCommand> {
    let Some(heading) = selection_ancestor(source, &span, |node| node.is::<ast::Heading>()) else {
        return Vec::new();
    };
    let depth = heading
        .cast::<ast::Heading>()
        .expect("the heading node is a heading")
        .depth()
        .get();
    let Some(marker) = heading
        .children()
        .find(|child| child.kind() == SyntaxKind::HeadingMarker)
    else {
        return Vec::new();
    };
    let Some(range) = crate::position::utf16_range(source.lines(), marker.range()) else {
        return Vec::new();
    };
    let mut actions = Vec::new();
    // A heading of depth one has no shallower form to rewrite.
    if depth > 1 {
        actions.push(rewrite_action(
            uri,
            "decrease the heading's depth".to_owned(),
            range,
            "=".repeat(depth - 1),
        ));
    }
    actions.push(rewrite_action(
        uri,
        "increase the heading's depth".to_owned(),
        range,
        "=".repeat(depth + 1),
    ));
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_actions::selection_actions;
    use crate::code_actions::tests::{edits_of, uri};
    use lsp_types::{CodeActionOrCommand, Position, Range};

    #[test]
    fn heading_depth_rewrites_the_marker_alone() {
        let actions = selection_actions(
            &uri(),
            &Source::detached("== Title\n\ntext\n"),
            Range::new(Position::new(0, 3), Position::new(0, 8)),
        );

        // The heading's own text stays as the author wrote it: only the marker changes.
        assert_eq!(
            edits_of(&actions, "decrease the heading's depth"),
            [(
                Range::new(Position::new(0, 0), Position::new(0, 2)),
                "=".to_owned(),
            )]
        );
        assert_eq!(
            edits_of(&actions, "increase the heading's depth"),
            [(
                Range::new(Position::new(0, 0), Position::new(0, 2)),
                "===".to_owned(),
            )]
        );
    }

    #[test]
    fn shallow_heading_offers_no_decrease() {
        let actions = selection_actions(
            &uri(),
            &Source::detached("= Title\n"),
            Range::new(Position::new(0, 2), Position::new(0, 7)),
        );

        assert_eq!(
            edits_of(&actions, "increase the heading's depth"),
            [(
                Range::new(Position::new(0, 0), Position::new(0, 1)),
                "==".to_owned(),
            )]
        );
        assert!(
            actions.iter().all(
                |action| !matches!(action, CodeActionOrCommand::CodeAction(action)
                    if action.title == "decrease the heading's depth")
            ),
            "{actions:?}"
        );
    }
}
