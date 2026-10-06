//! The fixes one unread import justifies: the statement or spans its removal claims, and the one
//! action an editor's Organize Imports gesture runs.

use lsp_types::{CodeActionKind, CodeActionOrCommand, TextEdit, Uri};
use tola_build::diagnostic::UnreadRemoval;
use tola_typst::typst::syntax::Source;
use tola_typst_syntax::names::{ImportRemoval, SourceNames};

use super::edits::{echoed_range, edit_action, merged_removals, protocol_line};

/// The removal one unread-import report justifies, when the file's own index still reports the
/// same import on the same line — or the same spans.
///
/// The report is trusted only while the index agrees with its removal: a statement whose line
/// moved, or spans that no longer bound the same text, answers nothing rather than removing the
/// wrong bytes.
pub(super) fn unread_import_removal(
    source: &Source,
    names_index: &SourceNames,
    reported_names: &[String],
    reported_removal: &UnreadRemoval,
) -> Option<Vec<TextEdit>> {
    let unread = names_index.unread_imports().into_iter().find(|unread| {
        unread.names == reported_names
            && match (&unread.removal, reported_removal) {
                (ImportRemoval::Statement, UnreadRemoval::Statement { line }) => {
                    crate::position::utf16_range(
                        source.lines(),
                        unread.statement.start..unread.statement.start,
                    )
                    .map(|at| at.start.line as usize + 1)
                        == Some(*line)
                }
                (ImportRemoval::Spans(spans), UnreadRemoval::Spans { ranges }) => {
                    spans.len() == ranges.len()
                        && spans.iter().zip(ranges).all(|(span, range)| {
                            crate::position::utf16_range(source.lines(), span.clone())
                                == Some(echoed_range(*range))
                        })
                }
                _ => false,
            }
    })?;
    let ranges = match unread.removal {
        ImportRemoval::Statement => statement_removal(source.text(), unread.statement.clone())
            .and_then(|range| crate::position::utf16_range(source.lines(), range))
            .into_iter()
            .collect::<Vec<_>>(),
        ImportRemoval::Spans(spans) => spans
            .into_iter()
            .filter_map(|span| crate::position::utf16_range(source.lines(), span))
            .collect(),
    };
    let edits = merged_removals(
        ranges
            .into_iter()
            .map(|range| TextEdit {
                range,
                new_text: String::new(),
            })
            .collect(),
    );
    if edits.is_empty() {
        return None;
    }
    Some(edits)
}

/// The action that removes every unread import one request reports.
///
/// An editor's Organize Imports gesture runs this on its own, so it has the dead imports and
/// nothing else: every edit in one pass, and no per-import choice to make.
pub(super) fn organize_imports(uri: &Uri, edits: Vec<TextEdit>) -> CodeActionOrCommand {
    edit_action(
        uri,
        "organize imports".to_owned(),
        CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
        edits,
    )
}

/// The byte range removing one unused import statement claims.
///
/// A line the statement alone writes goes whole, its own break included, so the fix leaves no
/// empty line behind. A line that has anything else keeps it: only the statement is removed,
/// with the `#` that introduces it and the `;` that ends its expression, and the author's
/// remaining text stays where it was. A statement spanning several lines is removed whole the
/// same way.
///
/// The terminator is removed even when whitespace separates it from the statement: a `;` left
/// behind would stop ending an expression and start printing itself as the page's own text. A
/// statement written inside a code block is introduced by no `#`, so only its own bytes go.
fn statement_removal(
    text: &str,
    statement: std::ops::Range<usize>,
) -> Option<std::ops::Range<usize>> {
    let first = protocol_line(text, statement.start)?;
    let last = protocol_line(text, statement.end.saturating_sub(1).max(statement.start))?;
    // The introducer is on the statement's own line by construction; requiring it keeps every
    // slice below ordered.
    let introduced = statement
        .start
        .checked_sub(1)
        .filter(|at| *at >= first.start && text.as_bytes().get(*at) == Some(&b'#'));
    let terminated = match introduced {
        // Only horizontal whitespace is crossed: a break between the statement and its `;` ends
        // the line the author wrote, and the line below is not the statement's to remove.
        Some(_) => {
            let rest = &text[statement.end..];
            let terminator = rest
                .find(|character: char| !is_horizontal_space(character))
                .filter(|at| rest.as_bytes().get(*at) == Some(&b';'));
            terminator.map_or(statement.end, |at| statement.end + at + 1)
        }
        None => statement.end,
    };
    let removed = introduced.unwrap_or(statement.start)..terminated;
    let before = &text[first.start..removed.start];
    let content_end = text[removed.end..]
        .find(['\n', '\r'])
        .map_or(text.len(), |at| removed.end + at);
    let after = &text[removed.end..content_end];
    if before.chars().all(char::is_whitespace) && after.chars().all(char::is_whitespace) {
        return Some(first.start..last.end);
    }
    Some(removed)
}

/// Whether `character` is whitespace the terminator scan may cross without leaving the line.
///
/// The author's whitespace is not only the ASCII space and tab — a no-break space, a thin space, or
/// an ideographic space separates a statement from its `;` equally well, and a `;` left behind
/// prints itself. A break is not crossed: what follows one belongs to the line below, which is not
/// the statement's to remove.
fn is_horizontal_space(character: char) -> bool {
    character.is_whitespace() && !tola_typst_syntax::typst_syntax::is_newline(character)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_actions::tests::{actions_for, applied, edits_of};
    use lsp_types::{Position, Range};

    #[test]
    fn organize_imports_removes_the_dead_import() {
        let text = "#import \"@tola/host:0.0.0\": link\n#import \"@tola/web:0.0.0\": sitemap\n\n#link(\"/x\")[X]\n";
        let actions = actions_for(
            text,
            vec![serde_json::json!({
                "kind": "unread-import",
                "names": ["sitemap"],
                "removal": {"kind": "statement", "line": 2},
            })],
            vec![CodeActionKind::SOURCE_ORGANIZE_IMPORTS],
        );

        // The dead statement leaves whole; the import the page reads keeps its own line.
        assert_eq!(
            edits_of(&actions, "organize imports"),
            [(
                Range::new(Position::new(1, 0), Position::new(2, 0)),
                String::new()
            )]
        );
    }

    /// A break only the compiler's line model knows leaves the echo's protocol line intact.
    #[test]
    fn vertical_tab_keeps_the_import_line() {
        let text = "a\u{0B}b\n#import \"@tola/web:0.0.0\": sitemap\n";
        let actions = actions_for(
            text,
            vec![serde_json::json!({
                "kind": "unread-import",
                "names": ["sitemap"],
                "removal": {"kind": "statement", "line": 2},
            })],
            vec![CodeActionKind::QUICKFIX],
        );

        assert_eq!(
            edits_of(&actions, "remove the unused import of `sitemap`"),
            [(
                Range::new(Position::new(1, 0), Position::new(2, 0)),
                String::new()
            )]
        );
    }

    /// A statement sharing its line with other text keeps that text when it is removed.
    ///
    /// The removal the echo names is the statement's own line, so a line holding anything
    /// besides it cannot go whole: only the statement, with the `#` that introduces it, is
    /// removed, and what the author wrote beside it stays.
    #[test]
    fn unused_import_removal_keeps_its_line_text() {
        let text = "#import \"@tola/web:0.0.0\": sitemap; Hello there\n";
        let actions = actions_for(
            text,
            vec![serde_json::json!({
                "kind": "unread-import",
                "names": ["sitemap"],
                "removal": {"kind": "statement", "line": 1},
            })],
            vec![CodeActionKind::QUICKFIX],
        );

        assert_eq!(
            edits_of(&actions, "remove the unused import of `sitemap`"),
            [(
                Range::new(Position::new(0, 0), Position::new(0, 35)),
                String::new()
            )]
        );
        // The same statement alone on its line goes whole, its break included.
        let alone = "#import \"@tola/web:0.0.0\": sitemap\nHello there\n";
        let actions = actions_for(
            alone,
            vec![serde_json::json!({
                "kind": "unread-import",
                "names": ["sitemap"],
                "removal": {"kind": "statement", "line": 1},
            })],
            vec![CodeActionKind::QUICKFIX],
        );
        assert_eq!(
            edits_of(&actions, "remove the unused import of `sitemap`"),
            [(
                Range::new(Position::new(0, 0), Position::new(1, 0)),
                String::new()
            )]
        );
    }

    /// An unused import's `;` goes with it, whatever whitespace the author wrote before it.
    ///
    /// A `;` left behind stops ending the code expression and prints itself as the page's own
    /// text, so the removal takes the terminator — and only it: what the author wrote beside the
    /// statement stays.
    #[test]
    fn unused_import_removal_takes_its_terminator() {
        for (text, expected) in [
            ("#import \"helpers.typ\": orphan ;Hello\n", "Hello\n"),
            ("#import \"helpers.typ\": orphan ;\n", ""),
            ("#import \"helpers.typ\": orphan;\nHello\n", "Hello\n"),
            ("#import \"helpers.typ\": orphan; Hello\n", " Hello\n"),
            // The author's own horizontal whitespace separates the terminator equally well; the
            // `;` goes with it. A break is not crossed: the line below keeps its own text.
            ("#import \"helpers.typ\": orphan\u{00A0};Hello\n", "Hello\n"),
            ("#import \"helpers.typ\": orphan\u{3000};Hello\n", "Hello\n"),
            ("#import \"helpers.typ\": orphan\u{2009};Hello\n", "Hello\n"),
            ("#import \"helpers.typ\": orphan\n;Hello\n", ";Hello\n"),
        ] {
            let actions = actions_for(
                text,
                vec![serde_json::json!({
                    "kind": "unread-import",
                    "names": ["orphan"],
                    "removal": {"kind": "statement", "line": 1},
                })],
                vec![CodeActionKind::QUICKFIX],
            );
            let edits = edits_of(&actions, "remove the unused import of `orphan`");
            assert_eq!(applied(text, edits), expected, "{text:?}");
        }
    }

    #[test]
    fn unused_import_fix_removes_whole_group() {
        let text = "#import \"helpers.typ\": a, b, c\n#document(\"index.html\", format: \"html\")[]\n#let selected = b\n";
        let actions = actions_for(
            text,
            vec![serde_json::json!({
                "kind":"unread-import", "names":["a","c"],
                "removal":{"kind":"spans","ranges":[
                    {"start":{"line":0,"character":23},"end":{"line":0,"character":26}},
                    {"start":{"line":0,"character":27},"end":{"line":0,"character":30}}
                ]}
            })],
            vec![CodeActionKind::QUICKFIX],
        );
        let edits = edits_of(&actions, "remove the unused imports of `a`, `c`");
        let rewritten = applied(text, edits);
        assert_eq!(
            rewritten,
            "#import \"helpers.typ\": b\n#document(\"index.html\", format: \"html\")[]\n#let selected = b\n"
        );
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("content")).unwrap();
        std::fs::write(directory.path().join("tola.toml"), "").unwrap();
        std::fs::write(directory.path().join("site.typ"), &rewritten).unwrap();
        std::fs::write(
            directory.path().join("helpers.typ"),
            "#let a = 1\n#let b = 2\n#let c = 3\n",
        )
        .unwrap();
        let config = tola_build::config::loading::load_site_config(
            Some(&directory.path().join("tola.toml")),
            tola_typst::PackageLocations::default(),
            &tola_build::config::loading::BuildOverrides::default(),
        )
        .unwrap()
        .into_config();
        let mut session =
            tola_build::check::SourceDiagnosticSession::new(std::sync::Arc::new(config));
        let revision = session
            .inspect(
                Vec::new(),
                &tola_build::cancellation::BuildCancellation::new(),
            )
            .unwrap();
        assert!(revision.compiled(), "{:?}", revision.diagnostics());
    }
}
