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

    #[test]
    fn loop_encloses_its_branches() {
        assert!(source_messages("#for x in (1, 2) { if x == 1 { break } }\n").is_empty());
        // A closure is its own function: the loop outside it does not enclose the branch.
        assert_eq!(
            source_messages("#for x in (1, 2) { let f = () => { break }; f() }\n").len(),
            1
        );
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
        assert!(source_messages("#let f() = { return 1 }\n#f()\n").is_empty());
    }

    #[test]
    fn closure_default_break_belongs_to_the_loop() {
        assert!(findings("#for x in (1, 2) {\n  let f = (a: { break }) => a\n}\n").is_empty());
    }

    #[test]
    fn closure_default_return_belongs_to_the_function() {
        assert!(findings("#let outer() = {\n  let f = (a: { return }) => a\n}\n").is_empty());
        assert_eq!(
            findings("#let f = (a: { return }) => a\n"),
            [(
                "editor.return_outside_function".to_owned(),
                "return".to_owned()
            )]
        );
    }

    #[test]
    fn loop_head_break_stays_with_the_loop() {
        for text in [
            "#for x in { break; () } {\n}\n",
            "#while { false; break } {\n}\n",
            "#while { true; break } {\n}\n",
        ] {
            assert!(findings(text).is_empty(), "{text:?}");
        }
    }
}
