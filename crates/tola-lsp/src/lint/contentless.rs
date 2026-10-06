//! The `set` and `show` statements a block gives no content to affect.

use tola_typst::typst::syntax::LinkedNode;
use tola_typst::typst::syntax::SyntaxKind;
use tola_typst::typst::syntax::ast::{self, AstNode};

use crate::codes;

use super::Hint;

/// Warn about each `set` or `show` statement a block cannot give content to affect.
///
/// Only a block whose own value is what the statement would style or transform is affected: a show
/// transform, an `if` branch, or a loop body. A trailing `break` or `continue` leaves the block
/// equally contentless.
pub(super) fn warn_rules(node: &LinkedNode<'_>, block: ast::Expr<'_>, hints: &mut Vec<Hint>) {
    for rule in rules(block) {
        let Some(rule) = node.find(rule.span()) else {
            continue;
        };
        let keyword = if rule.kind() == SyntaxKind::SetRule {
            "set"
        } else {
            "show"
        };
        hints.push(Hint {
            code: codes::editor::INEFFECTIVE_SET_SHOW,
            message: format!("the `{keyword}` statement has no effect"),
            note: Some("the block produces no content for it to affect".to_owned()),
            help: Some("give the block content the rule can affect".to_owned()),
            span: rule.range(),
            cause: None,
        });
    }
}

/// The `set` and `show` statements a block holds when nothing else in it gives them content to
/// affect.
fn rules(block: ast::Expr<'_>) -> Vec<ast::Expr<'_>> {
    match block {
        ast::Expr::CodeBlock(block) => tail_rules(block.body().exprs()),
        ast::Expr::ContentBlock(block) => tail_rules(block.body().exprs()),
        _ => Vec::new(),
    }
}

/// The rules no statement after them can give content to.
///
/// A rule styles the statements that follow it in its block, so a later statement that can hold
/// content — anything but a binding, another rule, or a branch that ends the block — makes the
/// rules collected before it effective. What remains affects the empty tail only.
fn tail_rules<'a>(exprs: impl Iterator<Item = ast::Expr<'a>>) -> Vec<ast::Expr<'a>> {
    let mut rules = Vec::new();
    for expr in exprs {
        match expr {
            ast::Expr::SetRule(_) | ast::Expr::ShowRule(_) => rules.push(expr),
            // A branch past these statements cannot reach them.
            ast::Expr::LoopBreak(_) | ast::Expr::LoopContinue(_) => break,
            expr if expr.to_untyped().kind().is_trivia() => {}
            _ => rules.clear(),
        }
    }
    rules
}

#[cfg(test)]
mod tests {
    use crate::lint::tests::findings;

    #[test]
    fn contentless_rules_are_hinted() {
        for (text, rule) in [
            ("#if false {\n  set text(red)\n}\n", "set text(red)"),
            ("#if false {\n  show: text(red)\n}\n", "show: text(red)"),
            ("#show raw: {\n  set text(red)\n}\n", "set text(red)"),
            ("#show: {\n  set text(red)\n}\n", "set text(red)"),
            // Content before a rule is already evaluated; the empty tail is what it would style.
            (
                "#if false {\n  [content]\n  set text(red)\n}\n",
                "set text(red)",
            ),
            (
                "#for i in range(10) {\n  show: it => it\n}\n",
                "show: it => it",
            ),
            (
                "#for i in range(10) {\n  show: it => it\n  break\n}\n",
                "show: it => it",
            ),
            (
                "#for i in range(10) {\n  show: it => it\n  continue\n}\n",
                "show: it => it",
            ),
        ] {
            assert_eq!(
                findings(text),
                [("editor.ineffective_set_show".to_owned(), rule.to_owned())],
                "{text:?}"
            );
        }
    }

    #[test]
    fn blocks_with_content_keep_their_rules() {
        for text in [
            "#if false {\n  set text(red)\n  [content]\n}\n",
            "#let f() = {\n  show: it => it\n  [Test]\n}\n",
            "#let f() = [\n  #show: it => it\n  Test\n]\n",
            "#while false {\n  [0]\n}\n",
            "#let f() = {\n  set text(red)\n}\n",
        ] {
            assert!(findings(text).is_empty(), "{text:?}");
        }
    }
}
