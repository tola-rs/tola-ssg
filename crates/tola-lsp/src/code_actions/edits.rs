//! The edits one action has, and the ranges they claim.
//!
//! One protocol range and the source's own bytes are two views of the same span, mapped in one
//! place so every action in this tree reads the same coordinates.

use std::collections::BTreeSet;

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Uri, WorkspaceEdit,
};
use tola_build::diagnostic::SourceRange;
use tola_typst::typst::syntax::Source;

/// The byte range one echoed protocol range addresses, absent when a position is past the
/// document or the range is reversed.
pub(super) fn byte_range(source: &Source, range: Range) -> Option<std::ops::Range<usize>> {
    let start = crate::position::byte_offset(source.lines(), range.start).ok()?;
    let end = crate::position::byte_offset(source.lines(), range.end).ok()?;
    (start <= end).then_some(start..end)
}

/// The protocol range one echoed diagnostic has, whose positions are the protocol's own.
pub(super) fn echoed_range(range: SourceRange) -> Range {
    Range::new(
        Position::new(range.start.line as u32, range.start.character as u32),
        Position::new(range.end.line as u32, range.end.character as u32),
    )
}

/// The byte range of the protocol line `byte` sits on, its own line ending included.
///
/// Removing a key means removing the whole line that writes it: the value beside it belongs to that
/// line, and a fix that removed less would leave an assignment without a name.
pub(super) fn protocol_line(text: &str, byte: usize) -> Option<std::ops::Range<usize>> {
    if byte > text.len() || !text.is_char_boundary(byte) {
        return None;
    }
    let start = text[..byte]
        .rfind(['\n', '\r'])
        .map_or(0, |break_at| break_at + 1);
    let end = match text[byte..].find(['\n', '\r']) {
        Some(break_at) => {
            let break_at = byte + break_at;
            // A carriage return followed by a line feed is one break, and both bytes go.
            break_at
                + if text[break_at..].starts_with("\r\n") {
                    2
                } else {
                    1
                }
        }
        None => text.len(),
    };
    Some(start..end)
}

/// `edits` with every claim merged: two removals overlapping or touching become one.
pub(super) fn merged_removals(mut edits: Vec<TextEdit>) -> Vec<TextEdit> {
    edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
    let mut merged: Vec<TextEdit> = Vec::with_capacity(edits.len());
    for edit in edits {
        if let Some(previous) = merged.last_mut()
            && edit.range.start <= previous.range.end
        {
            previous.range.end = previous.range.end.max(edit.range.end);
        } else {
            merged.push(edit);
        }
    }
    merged
}

/// The edits one `WorkspaceEdit` can hold, each span claimed once.
///
/// Candidates stand in the order the request justifies them, and each cause names its preferred
/// fix first — a correction before the removal of the same line — so a later candidate claiming
/// text an earlier one already claims is dropped rather than applied twice.
pub(super) fn disjoint_edits(edits: Vec<TextEdit>) -> Vec<TextEdit> {
    let mut seen = BTreeSet::new();
    let mut disjoint: Vec<TextEdit> = Vec::new();
    for edit in edits {
        if !seen.insert((edit.range.start, edit.range.end, edit.new_text.clone())) {
            continue;
        }
        if disjoint
            .iter()
            .any(|kept| edit.range.start < kept.range.end && kept.range.start < edit.range.end)
        {
            continue;
        }
        disjoint.push(edit);
    }
    disjoint
}

/// The code action with `edits` in `uri`, one workspace edit.
pub(super) fn edit_action(
    uri: &Uri,
    title: String,
    kind: CodeActionKind,
    edits: Vec<TextEdit>,
) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(kind),
        edit: Some(WorkspaceEdit {
            changes: Some(std::iter::once((uri.clone(), edits)).collect()),
            ..WorkspaceEdit::default()
        }),
        ..CodeAction::default()
    })
}

/// The code action rewriting `range` in `uri` with `new_text`.
pub(super) fn rewrite_action(
    uri: &Uri,
    title: String,
    range: Range,
    new_text: String,
) -> CodeActionOrCommand {
    edit_action(
        uri,
        title,
        CodeActionKind::REFACTOR_REWRITE,
        vec![TextEdit { range, new_text }],
    )
}
