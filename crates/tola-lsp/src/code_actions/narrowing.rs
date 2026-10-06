//! The narrowing refactor a metadata chain justifies: keep one source's records.
//!
//! The chain the cursor sits in must be a `filter` predicate over the record's own name; the
//! action then adds a condition comparing the record's path with the chosen source.

use lsp_types::{CodeActionOrCommand, Uri};
use tola_typst::typst::foundations::Repr;
use tola_typst::typst::syntax::ast::AstNode;
use tola_typst::typst::syntax::{LinkedNode, Source, SyntaxKind, ast};
use tola_typst_syntax::names::SourceNames;

use super::edits::rewrite_action;

/// The collection closure a metadata chain narrows.
pub(crate) struct Narrowing {
    /// The closure parameter the chain's records are bound to, as the condition reads it.
    pub root: String,
    /// The receiver whose compiled values must prove native array filtering.
    pub receiver: std::ops::Range<usize>,
    /// The closure body's byte extent, which the condition extends.
    pub body: std::ops::Range<usize>,
    /// Whether the author already parenthesized the body, so the condition need not.
    pub parenthesized: bool,
}

/// One source a narrowing action offers.
pub(crate) struct NarrowingSource {
    /// The source's path below the site root, which the action title names.
    pub site_path: String,
    /// The path below `build.content-dir` the record has, which the condition compares.
    pub content_path: String,
}

/// The collection closure one metadata chain at `cursor` narrows, when the cursor sits in one.
///
/// The cursor must read a field chain inside a `filter` closure over the record's own name: the
/// action adds a condition that keeps one source, which only a predicate over the records means.
pub(crate) fn narrowing_closure(source: &Source, cursor: usize) -> Option<Narrowing> {
    let closure = tola_typst_syntax::syntax::cursor_leaves(source, cursor)
        .into_iter()
        .flatten()
        .find_map(|leaf| {
            let mut node = leaf;
            while !node.is::<ast::Closure>() {
                node = node.parent()?.clone();
            }
            Some(node)
        })?;
    let receiver = filter_receiver(&closure)?;
    let expression = tola_typst_syntax::syntax::expression(source, cursor)?;
    let (root, _fields) = crate::query::field_chain(&expression)?;
    let root_name = root.cast::<ast::Ident>()?.get().to_string();
    let closure = closure.cast::<ast::Closure>()?;
    let parameters = closure.params().children().collect::<Vec<_>>();
    let [ast::Param::Pos(ast::Pattern::Normal(ast::Expr::Ident(parameter)))] =
        parameters.as_slice()
    else {
        return None;
    };
    let names = SourceNames::new(source.clone());
    let parameter = source.find(parameter.span())?;
    let declared = names.declared_at(parameter.range().start)?;
    if names.declared_at(root.range().start) != Some(declared) {
        return None;
    }
    let body = closure.body();
    // A block body is a statement list, not the predicate the condition extends.
    if matches!(body, ast::Expr::CodeBlock(_)) {
        return None;
    }
    let parenthesized = matches!(body, ast::Expr::Parenthesized(_));
    let body = source.find(body.span())?.range();
    Some(Narrowing {
        root: root_name,
        receiver,
        body,
        parenthesized,
    })
}

fn filter_receiver(closure: &LinkedNode<'_>) -> Option<std::ops::Range<usize>> {
    let call_node = closure
        .parent()
        .filter(|args| args.kind() == SyntaxKind::Args)
        .and_then(|args| args.parent())?;
    let call = call_node.cast::<ast::FuncCall>()?;
    let arguments = call.args().items().collect::<Vec<_>>();
    if !matches!(arguments.as_slice(), [ast::Arg::Pos(argument)] if argument.span() == closure.span())
    {
        return None;
    }
    let callee = call_node.find(call.callee().span())?;
    let access = callee.cast::<ast::FieldAccess>()?;
    (access.field().get() == "filter")
        .then(|| callee.find(access.target().span()).map(|node| node.range()))?
}

/// One narrowing action per source the chain's records were observed on.
///
/// The path guard runs before the predicate, so fields belonging only to the chosen source are
/// never read on other records. The author's predicate keeps its precedence inside parentheses.
pub(crate) fn narrowing_actions(
    uri: &Uri,
    source: &Source,
    narrowing: &Narrowing,
    sources: &[NarrowingSource],
) -> Vec<CodeActionOrCommand> {
    let Some(range) = crate::position::utf16_range(source.lines(), narrowing.body.clone()) else {
        return Vec::new();
    };
    let body = &source.text()[narrowing.body.clone()];
    let body = if narrowing.parenthesized {
        body.to_owned()
    } else {
        format!("({body})")
    };
    sources
        .iter()
        .map(|candidate| {
            rewrite_action(
                uri,
                format!("restrict to `{}`", candidate.site_path),
                range,
                format!(
                    "{}.path == {} and {body}",
                    narrowing.root,
                    candidate.content_path.as_str().repr()
                ),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_actions::tests::{edits_of, uri};

    #[test]
    fn narrowing_preserves_literal_paths() {
        let source = Source::detached("#let kept = sources.filter(source => source.meta.draft)");
        let cursor = source.text().find("draft").expect("metadata field");
        let narrowing = narrowing_closure(&source, cursor).expect("filter predicate");
        for (path, literal) in [
            ("quoted\"name.typ", "\"quoted\\\"name.typ\""),
            ("back\\slash.typ", "\"back\\\\slash.typ\""),
            ("line\nbreak.typ", "\"line\\nbreak.typ\""),
        ] {
            let actions = narrowing_actions(
                &uri(),
                &source,
                &narrowing,
                &[NarrowingSource {
                    site_path: path.to_owned(),
                    content_path: path.to_owned(),
                }],
            );
            assert_eq!(
                edits_of(&actions, &format!("restrict to `{path}`"))[0].1,
                format!("source.path == {literal} and (source.meta.draft)")
            );
        }
    }

    #[test]
    fn narrowing_requires_record_predicate() {
        for (text, accepted) in [
            ("#sources.filter(source => source.meta.draft)", true),
            ("#sources.filter(source => (source.meta.draft))", true),
            ("#filter(source => source.meta.draft)", false),
            ("#sources.filter(source => source.meta.draft, true)", false),
            (
                "#sources.filter(predicate: source => source.meta.draft)",
                false,
            ),
            (
                "#sources.filter((source, other) => source.meta.draft)",
                false,
            ),
            (
                "#sources.filter((source: none) => source.meta.draft)",
                false,
            ),
            (
                "#sources.filter(((source, other)) => source.meta.draft)",
                false,
            ),
            ("#sources.filter(source => { source.meta.draft })", false),
            (
                "#sources.filter(source => if true { let source = (:); source.meta.draft } else { false })",
                false,
            ),
        ] {
            let source = Source::detached(text);
            let cursor = source.text().find("draft").unwrap();
            assert_eq!(
                narrowing_closure(&source, cursor).is_some(),
                accepted,
                "{text}"
            );
        }
    }
}
