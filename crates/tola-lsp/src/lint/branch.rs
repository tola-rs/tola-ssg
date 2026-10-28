//! A `break`, `continue`, or `return` no enclosing construct gives meaning.

use tola_typst::typst::syntax::{LinkedNode, SyntaxKind};

use crate::codes;

use super::Hint;

/// Report each `break` or `continue` no loop encloses, and each `return` no function encloses.
pub(super) fn warn_unenclosed(node: &LinkedNode<'_>, hints: &mut Vec<Hint>) {
    match node.kind() {
        SyntaxKind::LoopBreak | SyntaxKind::LoopContinue => {
            if !enclosed_by_loop(node) {
                let keyword = if node.kind() == SyntaxKind::LoopBreak {
                    "break"
                } else {
                    "continue"
                };
                hints.push(Hint {
                    code: codes::editor::BRANCH_OUTSIDE_LOOP,
                    message: format!("`{keyword}` outside a loop"),
                    note: None,
                    help: Some("move it into a loop, or remove it".to_owned()),
                    span: node.range(),
                    cause: None,
                });
            }
        }
        SyntaxKind::FuncReturn if !enclosed_by_function(node) => {
            hints.push(Hint {
                code: codes::editor::RETURN_OUTSIDE_FUNCTION,
                message: "`return` outside a function".to_owned(),
                note: None,
                help: Some("move it into a function, or remove it".to_owned()),
                span: node.range(),
                cause: None,
            });
        }
        _ => {}
    }
}

/// Whether a loop encloses the statement before a function boundary does.
///
/// A closure is its own function, so a `break` inside one cannot reach an outer loop; a call
/// argument is evaluated inside the enclosing loop, so it can. A named parameter's default value
/// is evaluated where the closure is created, so it stays in the enclosing context.
fn enclosed_by_loop(node: &LinkedNode<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            SyntaxKind::ForLoop | SyntaxKind::WhileLoop => return true,
            SyntaxKind::Closure if current.kind() != SyntaxKind::Params => return false,
            SyntaxKind::Contextual => return false,
            _ => {}
        }
        current = parent;
    }
    false
}

/// Whether a function encloses the statement.
///
/// A named parameter's default value is evaluated where the closure is created, so a `return` in
/// it belongs to the enclosing function, not to the closure it parameterizes.
fn enclosed_by_function(node: &LinkedNode<'_>) -> bool {
    let mut current = node;
    while let Some(parent) = current.parent() {
        match parent.kind() {
            SyntaxKind::Contextual => return true,
            SyntaxKind::Closure if current.kind() != SyntaxKind::Params => return true,
            _ => {}
        }
        current = parent;
    }
    false
}

#[cfg(test)]
mod tests {
    use crate::lint::tests::{findings, messages};
    use tola_typst::typst::syntax::Source;
    use tola_typst_syntax::names::SourceNames;

    /// The hints one detached source justifies, with each hint's message.
    fn source_messages(text: &str) -> Vec<(String, String)> {
        let names = SourceNames::new(Source::detached(text.to_owned()));
        messages(crate::lint::hints(&names, None).into_iter())
    }

    #[test]
    fn branch_statements_outside_loop_are_hinted() {
        assert_eq!(
            source_messages("#break\n"),
            [(
                "`break` outside a loop".to_owned(),
                "move it into a loop, or remove it".to_owned()
            )]
        );
        assert_eq!(
            source_messages("#let f() = { continue }\n#f()\n"),
            [(
                "`continue` outside a loop".to_owned(),
                "move it into a loop, or remove it".to_owned()
            )]
        );
    }
    /// The construct that encloses a branch decides whether it is hinted: a loop encloses its
    /// own head and body, a closure and a default expression are their own function.
    #[test]
    fn enclosing_construct_decides_the_hint() {
        // Rows: (source, the hint it justifies, if any).
        let cases = [
            ("#for x in (1, 2) { if x == 1 { break } }\n", None),
            (
                "#for x in (1, 2) { let f = () => { break }; f() }\n",
                Some(("editor.branch_outside_loop", "break")),
            ),
            ("#for x in { break; () } {\n}\n", None),
            ("#while { false; break } {\n}\n", None),
            ("#while { true; break } {\n}\n", None),
            (
                "#for x in (1, 2) {\n  let f = (a: { break }) => a\n}\n",
                None,
            ),
            ("#let f() = { return 1 }\n#f()\n", None),
            (
                "#let outer() = {\n  let f = (a: { return }) => a\n}\n",
                None,
            ),
        ];
        for (text, expected) in cases {
            let expected = expected.map(|(code, covered)| (code.to_owned(), covered.to_owned()));
            let found: Vec<_> = findings(text);
            match expected {
                Some(one) => assert_eq!(found, [one], "{text:?}"),
                None => assert!(found.is_empty(), "{text:?} justified {found:?}"),
            }
        }
    }

    #[test]
    fn return_outside_function_is_hinted() {
        assert_eq!(
            source_messages("#return\n"),
            [(
                "`return` outside a function".to_owned(),
                "move it into a function, or remove it".to_owned()
            )]
        );
    }
}
