//! Editor hints for the file being edited.
//!
//! The official compiler reports what it evaluates; these are the mistakes it cannot see there —
//! a `break`, `continue`, or `return` outside the construct that gives it meaning, a value an
//! explicit `return` discards, a `set` or `show` statement no content in its block can affect, a
//! math spelling no scope defines, and a font family this environment does not include. A hint is
//! editor feedback only, never a build's diagnostic, and one whose range the compiler already
//! reported is dropped, so an evaluated mistake is stated once.

use std::path::Path;

use tola_build::diagnostic::{Diagnostic, DiagnosticCause, Severity, SourcePosition, SourceRange};
use tola_typst::TypstWorld;
use tola_typst::typst::World;
use tola_typst::typst::syntax::{LinkedNode, Source, ast};
use tola_typst_syntax::names::SourceNames;
use tola_typst_syntax::usage::Liveness;

use crate::codes;

mod branch;
mod contentless;
mod discard;
mod fonts;
mod imports;
mod math;

/// The hints one open document's index justifies, minus what the check already reported.
pub(super) fn document_diagnostics(
    path: &Path,
    root: &Path,
    names: &SourceNames,
    world: Option<&TypstWorld>,
    reported: &[Diagnostic],
) -> Vec<Diagnostic> {
    // A hint is spelled the way the compiler's own diagnostics for the document are, which only a
    // document inside the site, under a path an editor document can name, has.
    if path
        .strip_prefix(root)
        .ok()
        .and_then(Path::to_str)
        .is_none()
    {
        return Vec::new();
    }
    let relative = tola_build::filesystem::display_path(path, root);
    hints(names, world)
        .into_iter()
        .filter_map(|hint| {
            let diagnostic = diagnostic(hint, names.source(), &relative)?;
            let range = diagnostic.location.as_ref()?.range?;
            (!reported_here(reported, &relative, &range)).then_some(diagnostic)
        })
        .collect()
}

/// The liveness findings one open document's index justifies, as the diagnostics an editor reads.
pub(super) fn liveness_diagnostics(
    path: &Path,
    root: &Path,
    names: &SourceNames,
    liveness: &Liveness,
) -> Vec<Diagnostic> {
    let relative = tola_build::filesystem::display_path(path, root);
    let mut hints = Vec::new();
    for &declaration in &liveness.unused_bindings {
        let declared = &names.declarations()[declaration];
        hints.push(Hint {
            code: codes::editor::UNUSED_BINDING,
            message: format!("the binding `{}` is never read", declared.name),
            note: None,
            help: Some("read the value, or bind it as `_`".to_owned()),
            span: declared.range.clone(),
            cause: None,
        });
    }
    for store in &liveness.dead_stores {
        let declared = &names.declarations()[store.declaration];
        hints.push(Hint {
            code: codes::editor::DEAD_STORE,
            message: format!("the value stored to `{}` is never read", declared.name),
            note: None,
            help: Some("read the value, or evaluate it with `let _ =`".to_owned()),
            span: store.range.clone(),
            cause: None,
        });
    }
    hints.sort_by_key(|hint| hint.span.start);
    hints
        .into_iter()
        .filter_map(|hint| diagnostic(hint, names.source(), &relative))
        .collect()
}

/// One hint, before it is projected into a diagnostic.
struct Hint {
    code: tola_build::diagnostic::DiagnosticCode,
    message: String,
    note: Option<String>,
    help: Option<String>,
    span: std::ops::Range<usize>,
    /// The cause a correction reads, for a hint the compiler did not report itself.
    cause: Option<DiagnosticCause>,
}

/// The hints one source's own syntax justifies.
fn hints(names: &SourceNames, world: Option<&TypstWorld>) -> Vec<Hint> {
    let mut hints = Vec::new();
    walk(&LinkedNode::new(names.source().root()), &mut hints);
    fonts::warn_missing_families(names, world, &mut hints);
    // A math spelling resolves through the library the check compiled with, so only a check that
    // resolved a world can tell a name no scope defines from one the math scope binds.
    if let Some(world) = world {
        math::warn_undefined_names(names, world.library(), &mut hints);
    }
    hints.sort_by_key(|hint| hint.span.start);
    hints
}

/// Walk the syntax tree in order, letting each pass report what a node justifies.
fn walk(node: &LinkedNode<'_>, hints: &mut Vec<Hint>) {
    branch::warn_unenclosed(node, hints);
    if let Some(conditional) = node.cast::<ast::Conditional>() {
        contentless::warn_rules(node, conditional.if_body(), hints);
        if let Some(else_body) = conditional.else_body() {
            contentless::warn_rules(node, else_body, hints);
        }
    } else if let Some(while_loop) = node.cast::<ast::WhileLoop>() {
        contentless::warn_rules(node, while_loop.body(), hints);
    } else if let Some(for_loop) = node.cast::<ast::ForLoop>() {
        contentless::warn_rules(node, for_loop.body(), hints);
    } else if let Some(show) = node.cast::<ast::ShowRule>() {
        contentless::warn_rules(node, show.transform(), hints);
    } else if node.cast::<ast::Closure>().is_some() || node.cast::<ast::Contextual>().is_some() {
        discard::warn_discarded_values(node, hints);
    }
    for child in node.children() {
        walk(&child, hints);
    }
}

/// One hint as the diagnostic the editor reads.
fn diagnostic(hint: Hint, source: &Source, path: &str) -> Option<Diagnostic> {
    // The location has the compiler's line model; a publish projects it into the protocol's.
    let range = tola_typst_syntax::position::utf16_range(source.lines(), hint.span)?;
    let mut diagnostic = Diagnostic::at_path(hint.code, Severity::Warning, path, hint.message)
        .with_position(
            range.start.line as usize + 1,
            range.start.character as usize + 1,
        )
        .with_source_range(SourceRange {
            start: SourcePosition {
                line: range.start.line as usize,
                character: range.start.character as usize,
            },
            end: SourcePosition {
                line: range.end.line as usize,
                character: range.end.character as usize,
            },
        });
    if let Some(note) = hint.note {
        diagnostic = diagnostic.with_note(note);
    }
    if let Some(help) = hint.help {
        diagnostic = diagnostic.with_help(help);
    }
    if let Some(cause) = hint.cause {
        diagnostic = diagnostic.with_cause(cause);
    }
    Some(diagnostic)
}

/// Whether the check already reported a diagnostic overlapping this range in this file.
fn reported_here(reported: &[Diagnostic], path: &str, range: &SourceRange) -> bool {
    reported.iter().any(|diagnostic| {
        diagnostic.location.as_ref().is_some_and(|location| {
            location.path == path
                && location
                    .range
                    .as_ref()
                    .is_some_and(|other| overlaps(other, range))
        })
    })
}

/// Whether two ranges inside one file overlap, compared as the protocol reads them.
///
/// Touching ranges stay distinct: a diagnostic that ends where another begins reports another
/// problem.
fn overlaps(left: &SourceRange, right: &SourceRange) -> bool {
    let at = |position: &SourcePosition| (position.line, position.character);
    at(&left.start) < at(&right.end) && at(&right.start) < at(&left.end)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tola_typst_syntax::usage::DeadStore;

    /// The hints one detached source justifies, as each hint's code and the source it covers.
    pub(super) fn findings(text: &str) -> Vec<(String, String)> {
        let names = SourceNames::new(Source::detached(text.to_owned()));
        crate::lint::hints(&names, None)
            .into_iter()
            .map(|hint| (hint.code.to_string(), text[hint.span.clone()].to_owned()))
            .collect()
    }

    /// Each hint's message and the help it has, in the order the pass reported them.
    pub(super) fn messages(hints: impl Iterator<Item = Hint>) -> Vec<(String, String)> {
        hints
            .map(|hint| (hint.message, hint.help.unwrap_or_default()))
            .collect()
    }

    #[test]
    fn reported_range_drops_its_hint() {
        let names = SourceNames::new(Source::detached("#break\n".to_owned()));
        let hint = hints(&names, None).remove(0);
        let reported = diagnostic(hint, names.source(), "content/page.typ").unwrap();
        let range = reported.location.as_ref().unwrap().range.unwrap();
        assert!(reported_here(
            std::slice::from_ref(&reported),
            "content/page.typ",
            &range
        ));
        assert!(!reported_here(&[reported], "content/other.typ", &range));
    }

    #[test]
    fn adjacent_reported_range_keeps_its_hint() {
        let at = |line, character| SourcePosition { line, character };
        let hint = SourceRange {
            start: at(0, 1),
            end: at(0, 6),
        };
        let before = Diagnostic::at_path(
            codes::editor::UNKNOWN_FONT,
            Severity::Warning,
            "content/page.typ",
            "another problem",
        )
        .with_source_range(SourceRange {
            start: at(0, 0),
            end: at(0, 1),
        });
        assert!(!reported_here(&[before], "content/page.typ", &hint));
    }

    #[test]
    fn liveness_findings_become_editor_diagnostics() {
        let source = Source::detached("#let first = 1\n#let second = 2\n#second\n".to_owned());
        let names = SourceNames::new(source.clone());
        let second = names.declarations()[1].range.clone();
        let liveness = Liveness {
            unused_bindings: vec![0],
            dead_stores: vec![DeadStore {
                declaration: 1,
                range: second,
            }],
        };
        let diagnostics = liveness_diagnostics(
            Path::new("/site/content/page.typ"),
            Path::new("/site"),
            &names,
            &liveness,
        );
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].code.to_string(), "editor.unused_binding");
        assert!(diagnostics[0].message.contains("`first`"));
        let location = diagnostics[0].location.as_ref().unwrap();
        assert_eq!(location.path, "content/page.typ");
        let range = location.range.as_ref().unwrap();
        assert_eq!((range.start.line, range.start.character), (0, 5));
        assert_eq!((range.end.line, range.end.character), (0, 10));
        assert_eq!(diagnostics[1].code.to_string(), "editor.dead_store");
        assert!(diagnostics[1].message.contains("`second`"));
        let range = diagnostics[1]
            .location
            .as_ref()
            .unwrap()
            .range
            .as_ref()
            .unwrap();
        assert_eq!((range.start.line, range.start.character), (1, 5));
        assert_eq!((range.end.line, range.end.character), (1, 11));
    }
}
