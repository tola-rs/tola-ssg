//! The value expressions one function body evaluates before an explicit `return` discards them.

use tola_typst::typst::syntax::LinkedNode;
use tola_typst::typst::syntax::SyntaxKind;
use tola_typst::typst::syntax::ast::{self, AstNode};

use crate::codes;

use super::Hint;

/// The explicit `return` a block's remaining expressions are discarded by.
#[derive(Clone, Copy, Default, PartialEq)]
struct Discard {
    /// A `return` with a value follows on this path.
    value: bool,
    /// A `return` without a value follows on this path.
    none: bool,
    /// The path already named the expression the `return` discards.
    warned: bool,
}

impl Discard {
    /// The state both branches agree on.
    fn merge(self, other: Self) -> Self {
        Self {
            value: self.value && other.value,
            none: self.none && other.none,
            warned: self.warned && other.warned,
        }
    }
}

/// The pending `return` states one walk position has.
#[derive(Clone, Copy)]
struct Flow {
    /// What follows the statement being walked: the pending `return`, or `None` when some path
    /// from it leaves the function without one.
    pending: Option<Discard>,
    /// Where the innermost enclosing loop leaves, which a `break` or `continue` reaches.
    loop_exit: LoopExit,
}

/// What a `break` or `continue` reaches.
#[derive(Clone, Copy)]
enum LoopExit {
    /// No loop encloses the branch; another hint reports it, and it ends its path with nothing
    /// pending.
    Nowhere,
    /// The exit of the innermost enclosing loop, whose own pending `return` the branch reaches
    /// because the loop's value is discarded there whether the body finishes or not.
    At(Option<Discard>),
}

/// The value expressions one function body evaluates before an explicit `return` discards them.
///
/// A value is reported only when every path from it reaches a `return` that discards it: each `if`
/// branch and loop is read on its own, and a path that leaves no pending `return` withholds the
/// finding.
pub(super) fn warn_discarded_values(function: &LinkedNode<'_>, hints: &mut Vec<Hint>) {
    let body = function
        .cast::<ast::Closure>()
        .map(|closure| closure.body().span())
        .or_else(|| {
            function
                .cast::<ast::Contextual>()
                .map(|contextual| contextual.body().span())
        });
    let Some(body) = body.and_then(|body| function.find(body)) else {
        return;
    };
    walk(
        &body,
        &mut Flow {
            pending: None,
            loop_exit: LoopExit::Nowhere,
        },
        hints,
    );
}

/// Walk one expression in evaluation order, newest statement first.
///
/// A closure's or `context`'s own body belongs to its own function and is not walked; the named
/// parameter defaults it evaluates where it is created are, because a `return` in one ends the
/// enclosing function like any other statement's.
fn walk(node: &LinkedNode<'_>, flow: &mut Flow, hints: &mut Vec<Hint>) {
    match node.kind() {
        // A block's body child holds its statements; the delimiters around it do not.
        SyntaxKind::CodeBlock | SyntaxKind::ContentBlock => {
            if let Some(body) = node
                .children()
                .find(|child| matches!(child.kind(), SyntaxKind::Code | SyntaxKind::Markup))
            {
                walk(&body, flow, hints);
            }
        }
        SyntaxKind::Code | SyntaxKind::Markup => statements(node, flow, hints),
        SyntaxKind::Closure | SyntaxKind::Contextual => defaults(node, flow, hints),
        SyntaxKind::LetBinding | SyntaxKind::DestructAssignment => {
            let created = node
                .cast::<ast::LetBinding>()
                .and_then(|binding| binding.init())
                .or_else(|| {
                    node.cast::<ast::DestructAssignment>()
                        .map(|assignment| assignment.value())
                });
            if let Some(created) = created.and_then(|init| node.find(init.span())) {
                created_closure(&created, flow, hints);
            }
        }
        SyntaxKind::Conditional => {
            let Some(conditional) = node.cast::<ast::Conditional>() else {
                return;
            };
            let parent = flow.pending;
            if let Some(body) = node.find(conditional.if_body().span()) {
                walk(&body, flow, hints);
            }
            let branch = flow.pending;
            flow.pending = parent;
            if let Some(else_body) = conditional.else_body()
                && let Some(body) = node.find(else_body.span())
            {
                walk(&body, flow, hints);
            }
            join(&mut flow.pending, branch);
        }
        SyntaxKind::WhileLoop | SyntaxKind::ForLoop => {
            let body = node
                .cast::<ast::WhileLoop>()
                .map(|loop_| loop_.body())
                .or_else(|| node.cast::<ast::ForLoop>().map(|loop_| loop_.body()));
            if let Some(body) = body.and_then(|body| node.find(body.span())) {
                // A loop may run its body zero times, so the body's own `return` discards the
                // values before the loop only when the path that skips the body agrees. A `break`
                // or `continue` leaves through that same exit, so the body is walked with it: the
                // loop's value is evaluated before the exit either way.
                let before = flow.pending;
                let enclosing = flow.loop_exit;
                flow.loop_exit = LoopExit::At(before);
                walk(&body, flow, hints);
                flow.loop_exit = enclosing;
                join(&mut flow.pending, before);
            }
        }
        SyntaxKind::FuncReturn => {
            let Some(return_) = node.cast::<ast::FuncReturn>() else {
                return;
            };
            flow.pending = Some(Discard {
                value: return_.body().is_some(),
                none: return_.body().is_none(),
                warned: false,
            });
        }
        SyntaxKind::LoopBreak | SyntaxKind::LoopContinue => {
            flow.pending = match flow.loop_exit {
                LoopExit::At(exit) => exit,
                LoopExit::Nowhere => Some(Discard::default()),
            };
        }
        _ => value(node, flow, hints),
    }
}

/// The expression children of a container, newest first.
fn statements(node: &LinkedNode<'_>, flow: &mut Flow, hints: &mut Vec<Hint>) {
    for statement in node
        .children()
        .filter(|child| child.cast::<ast::Expr>().is_some())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        walk(&statement, flow, hints);
    }
}

/// Walk the closure a binding initializes directly: its defaults run where the binding stands.
fn created_closure(node: &LinkedNode<'_>, flow: &mut Flow, hints: &mut Vec<Hint>) {
    if matches!(node.kind(), SyntaxKind::Closure | SyntaxKind::Contextual) {
        defaults(node, flow, hints);
    }
}

/// The named parameter defaults a closure or `context` evaluates where it is created.
///
/// The defaults run in the enclosing context, in parameter order, before the closure exists.
fn defaults(node: &LinkedNode<'_>, flow: &mut Flow, hints: &mut Vec<Hint>) {
    let Some(params) = node
        .children()
        .find(|child| child.kind() == SyntaxKind::Params)
    else {
        return;
    };
    for parameter in params.children() {
        if let Some(named) = parameter.cast::<ast::Named>()
            && let Some(default) = node.find(named.expr().span())
        {
            walk(&default, flow, hints);
        }
    }
}

/// One expression in block position: a value a pending `return` discards, or nothing this pass can
/// prove discarded.
///
/// A container's own evaluation is the block's evaluation, so the content it holds is walked; an
/// expression computed from other expressions is opaque, because the visitor this rule follows
/// does not report values inside one. A math body is reachable only through an `Equation`, which
/// is a value of its own, so no math-internal traversal is needed.
fn value(node: &LinkedNode<'_>, flow: &mut Flow, hints: &mut Vec<Hint>) {
    let Some(expr) = node.cast::<ast::Expr>() else {
        return;
    };
    match expr {
        // Expressions that hold no value of their own and no block-position value inside.
        ast::Expr::Ident(_)
        | ast::Expr::MathIdent(_)
        | ast::Expr::FieldAccess(_)
        | ast::Expr::FuncCall(_)
        | ast::Expr::MathFieldAccess(_)
        | ast::Expr::MathCall(_)
        | ast::Expr::Unary(_)
        | ast::Expr::Binary(_)
        | ast::Expr::ModuleImport(_) => return,
        // Containers whose evaluation is the block's own: their content is walked.
        ast::Expr::Parenthesized(inner) => {
            if let Some(inner) = node.find(inner.expr().span()) {
                walk(&inner, flow, hints);
            }
            return;
        }
        ast::Expr::Strong(_)
        | ast::Expr::Emph(_)
        | ast::Expr::Heading(_)
        | ast::Expr::ListItem(_)
        | ast::Expr::EnumItem(_) => {
            if let Some(body) = node
                .children()
                .find(|child| child.kind() == SyntaxKind::Markup)
            {
                walk(&body, flow, hints);
            }
            return;
        }
        ast::Expr::TermItem(_) => {
            for body in node
                .children()
                .filter(|child| child.kind() == SyntaxKind::Markup)
            {
                walk(&body, flow, hints);
            }
            return;
        }
        _ => {}
    }
    let Some(pending) = flow.pending.as_mut() else {
        return;
    };
    if pending.warned {
        return;
    }
    let discarded = (pending.value && discards_value(expr))
        || (pending.none && matches!(expr, ast::Expr::SetRule(_) | ast::Expr::ShowRule(_)));
    if discarded {
        pending.warned = true;
        hints.push(hint(node, expr));
    }
}

/// Whether a value expression's evaluation is discarded when a `return` value follows it.
fn discards_value(expr: ast::Expr<'_>) -> bool {
    matches!(
        expr,
        ast::Expr::Text(_)
            | ast::Expr::Linebreak(_)
            | ast::Expr::Escape(_)
            | ast::Expr::Shorthand(_)
            | ast::Expr::SmartQuote(_)
            | ast::Expr::Raw(_)
            | ast::Expr::Link(_)
            | ast::Expr::Label(_)
            | ast::Expr::Ref(_)
            | ast::Expr::Auto(_)
            | ast::Expr::Bool(_)
            | ast::Expr::Int(_)
            | ast::Expr::Float(_)
            | ast::Expr::Numeric(_)
            | ast::Expr::Str(_)
            | ast::Expr::MathText(_)
            | ast::Expr::MathShorthand(_)
            | ast::Expr::MathAlignPoint(_)
            | ast::Expr::MathPrimes(_)
            | ast::Expr::MathRoot(_)
            | ast::Expr::Equation(_)
            | ast::Expr::Array(_)
            | ast::Expr::Dict(_)
            | ast::Expr::ModuleInclude(_)
            | ast::Expr::SetRule(_)
            | ast::Expr::ShowRule(_)
    )
}

fn hint(node: &LinkedNode<'_>, expr: ast::Expr<'_>) -> Hint {
    Hint {
        code: codes::editor::DISCARDED_BY_RETURN,
        message: format!(
            "the {} is discarded by `return`",
            expr.to_untyped().kind().name()
        ),
        note: None,
        help: Some("ignore it explicitly with `let _ =`".to_owned()),
        span: node.range(),
        cause: None,
    }
}

/// Fold one branch's state into the state the other branches left.
///
/// A value is discarded only when every path reaches the same kind of `return`, so a branch that
/// leaves no pending return withholds the finding.
fn join(discard: &mut Option<Discard>, branch: Option<Discard>) {
    *discard = match (*discard, branch) {
        (Some(current), Some(branch)) => Some(branch.merge(current)),
        _ => None,
    };
}

#[cfg(test)]
mod tests {
    use crate::lint::tests::findings;

    #[test]
    fn explicit_return_discards_earlier_values() {
        for (text, discarded) in [
            (
                "#let f() = {\n  [0]\n  if true {\n    [1]\n  } else {\n    [2]\n  }\n  return []\n}\n",
                &["1", "2"][..],
            ),
            (
                "#let f() = {\n  if true {\n    [1]\n  } else {\n    [2]\n  }\n  return []\n}\n",
                &["1", "2"][..],
            ),
            (
                "#let f() = {\n  set text(red)\n  return 1\n}\n",
                &["set text(red)"][..],
            ),
            (
                "#let f() = {\n  show: it => it\n  return 1\n}\n",
                &["show: it => it"][..],
            ),
            (
                "#let f() = for i in range(10) {\n  show: it => it\n  return []\n}\n",
                &["show: it => it"][..],
            ),
            ("#let f() = [\n  #(1, 2)\n  #return 1\n]\n", &["(1, 2)"][..]),
            (
                "#let f() = [\n  $ 1 2 3 $\n  #return 1\n]\n",
                &["$ 1 2 3 $"][..],
            ),
            (
                "#let f() = [\n  Hello -- Test -- World\n  #return 1\n]\n",
                &["World"][..],
            ),
            (
                "#let f(cond) = {\n  [x]\n  if cond { return 1 } else { return 2 }\n}\n",
                &["x"][..],
            ),
            (
                // A `break` leaves the loop where the loop itself leaves it, so the body's value
                // is discarded by the `return` that follows.
                "#let f() = {\n  while true {\n    [0]\n    break\n  }\n  return []\n}\n",
                &["0"][..],
            ),
        ] {
            let expected = discarded
                .iter()
                .map(|covered| {
                    (
                        "editor.discarded_by_return".to_owned(),
                        (*covered).to_owned(),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(findings(text), expected, "{text:?}");
        }
    }

    #[test]
    fn valueless_return_discards_only_rules() {
        for (text, discarded) in [
            (
                "#let f() = if true {\n  set text(red)\n  return\n} else {\n  return []\n}\n",
                &["set text(red)"][..],
            ),
            (
                "#let f() = if true {\n  set text(red)\n  return\n} else {\n  set text(blue)\n  return []\n}\n",
                &["set text(red)", "set text(blue)"][..],
            ),
            (
                "#let f() = for i in range(10) {\n  show: it => it\n  return\n}\n",
                &["show: it => it"][..],
            ),
        ] {
            let expected = discarded
                .iter()
                .map(|covered| {
                    (
                        "editor.discarded_by_return".to_owned(),
                        (*covered).to_owned(),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(findings(text), expected, "{text:?}");
        }
        assert!(findings("#let f() = {\n  [0]\n  return\n}\n").is_empty());
    }

    #[test]
    fn branch_merge_keeps_partial_returns() {
        assert_eq!(
            findings(
                "#let f() = {\n  if true {\n    [1]\n    return\n  } else {\n    [2]\n  }\n  return []\n}\n"
            ),
            [("editor.discarded_by_return".to_owned(), "2".to_owned())]
        );
        assert_eq!(
            findings(
                "#let f() = {\n  if true {\n    [1]\n  } else {\n    [2]\n    return\n  }\n  return []\n}\n"
            ),
            [("editor.discarded_by_return".to_owned(), "1".to_owned())]
        );
    }

    #[test]
    fn partial_paths_keep_their_values() {
        for text in [
            "#let f() = {\n  let x\n  x = (1,)\n  if x.len() == 0 {\n    return\n  }\n  return x\n}\n",
            "#let f() = {\n  if padding == none {\n    padding = 0\n  }\n\n  return padding\n}\n",
            "#let f() = {\n  while true {\n    [0]\n    break\n    return []\n  }\n}\n",
            "#let f() = {\n  1 + (1,)\n  return 0\n}\n",
            "#let f(cond) = {\n  [x]\n  if cond {\n    return 1\n  }\n}\n",
            "#let f() = {\n  [x]\n  while false {\n    return 1\n  }\n}\n",
        ] {
            assert!(findings(text).is_empty(), "{text:?}");
        }
    }

    #[test]
    fn closure_body_discards_its_own_values() {
        assert_eq!(
            findings(
                "#let f() = {\n  let g = () => {\n    [0]\n    return 1\n  }\n  return 2\n}\n"
            ),
            [("editor.discarded_by_return".to_owned(), "0".to_owned())]
        );
    }

    #[test]
    fn container_values_are_discarded() {
        for (text, covered) in [
            ("#let f() = {\n  ([0])\n  return 1\n}\n", "0"),
            ("#let f() = [\n  *bold*\n  #return 1\n]\n", "bold"),
            ("#let f() = [\n  = Heading\n  #return 1\n]\n", "Heading"),
        ] {
            assert_eq!(
                findings(text),
                [("editor.discarded_by_return".to_owned(), covered.to_owned())],
                "{text:?}"
            );
        }
    }

    #[test]
    fn closure_default_return_discards_outer_values() {
        assert_eq!(
            findings("#let f() = {\n  [x]\n  let g = (a: { return 1 }) => a\n}\n"),
            [("editor.discarded_by_return".to_owned(), "x".to_owned())]
        );
        // A default without a `return` leaves the outer value live.
        assert!(findings("#let f() = {\n  [x]\n  let g = (a: 1) => a\n}\n").is_empty());
    }
}
