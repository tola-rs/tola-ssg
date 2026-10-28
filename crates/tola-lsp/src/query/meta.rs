//! The `#tola-meta` call and key the cursor addresses, as this file writes them.

use std::collections::BTreeSet;
use std::ops::Range;

use tola_typst::typst::syntax::ast::{self, AstNode};
use tola_typst::typst::syntax::{LinkedNode, Source, SyntaxKind};

use super::imports::calls_package_member;

/// The `#tola-meta` call the cursor addresses, as the author wrote it.
pub(super) struct MetaCall {
    /// The bytes of the called name.
    pub(super) callee: Range<usize>,
    /// The bytes of the call's own dictionary argument.
    pub(super) dict: Range<usize>,
}

/// One key being written inside a `#tola-meta` dictionary.
pub(super) struct MetaKey {
    /// The bytes completion replaces: the key the author typed so far.
    pub(super) typed: Range<usize>,
    /// Every key the dictionary already writes.
    pub(super) written: BTreeSet<String>,
}

/// The `#tola-meta` call the cursor addresses, when this file establishes the callee as the
/// package function and the call writes a dictionary argument.
pub(super) fn meta_call(
    source: &Source,
    cursor: usize,
    names: &tola_typst_syntax::names::SourceNames,
) -> Option<MetaCall> {
    let node = call_around(source, cursor)?;
    let call = node.cast::<ast::FuncCall>()?;
    let callee = node.find(call.callee().span())?;
    if !calls_package_member(
        names,
        &callee,
        tola_packages::SOURCE_PACKAGE,
        tola_packages::TOLA_META,
    ) {
        return None;
    }
    let argument = call.args().items().find_map(|item| match item {
        ast::Arg::Pos(expr) => Some(expr.span()),
        _ => None,
    })?;
    let dict = node.find(argument)?;
    if !matches!(
        dict.kind(),
        SyntaxKind::Dict | SyntaxKind::Parenthesized | SyntaxKind::Array
    ) {
        return None;
    }
    Some(MetaCall {
        callee: callee.range(),
        dict: dict.range(),
    })
}

/// The innermost function call whose bytes cover the cursor.
///
/// A call the author is still writing has no leaf at the cursor when the file ends inside it, so
/// the call is found by containment rather than by climbing from a leaf.
pub(super) fn call_around(source: &Source, cursor: usize) -> Option<LinkedNode<'_>> {
    let mut found: Option<LinkedNode<'_>> = None;
    let mut pending = vec![LinkedNode::new(source.root())];
    while let Some(node) = pending.pop() {
        pending.extend(node.children());
        if node.cast::<ast::FuncCall>().is_some()
            && node.range().start <= cursor
            && cursor <= node.range().end
            && found
                .as_ref()
                .is_none_or(|found| node.range().len() < found.range().len())
        {
            found = Some(node);
        }
    }
    found
}

/// The key the cursor writes inside a `#tola-meta` dictionary, or `None` when the cursor writes a
/// value there.
pub(super) fn meta_key(source: &Source, cursor: usize, call: &MetaCall) -> Option<MetaKey> {
    if !(call.dict.start <= cursor && cursor <= call.dict.end) {
        return None;
    }
    let dict = tola_typst_syntax::syntax::node_at_range(source, &call.dict)?;
    let mut written = BTreeSet::new();
    let mut typed = None;
    let mut value = false;
    for child in dict.children() {
        if let Some(named) = child.cast::<ast::Named>() {
            if let Some(name) = child.find(named.name().span()) {
                written.insert(named.name().get().to_string());
                if name.range().start <= cursor && cursor <= name.range().end {
                    typed = Some(name.range());
                }
            }
            if let Some(expr) = child.find(named.expr().span()) {
                value |= expr.range().start <= cursor && cursor <= expr.range().end;
            }
        } else if let Some(keyed) = child.cast::<ast::Keyed>() {
            if let ast::Expr::Str(key) = keyed.key()
                && let Some(key) = child.find(key.span())
                && key.range().start <= cursor
                && cursor <= key.range().end
            {
                return None;
            }
            if let Some(expr) = child.find(keyed.expr().span()) {
                value |= expr.range().start <= cursor && cursor <= expr.range().end;
            }
        }
    }
    if value && typed.is_none() {
        return None;
    }
    let typed = match typed {
        Some(typed) => typed,
        None => match dict
            .children()
            .filter(|child| !child.kind().is_trivia())
            .find(|child| {
                child.range().start <= cursor
                    && cursor <= child.range().end
                    && !matches!(child.kind(), SyntaxKind::LeftParen | SyntaxKind::RightParen)
            }) {
            Some(child) if matches!(child.kind(), SyntaxKind::Error | SyntaxKind::Ident) => {
                child.range()
            }
            Some(_) => return None,
            // A cursor past the dictionary stands in no key and no child covers it: offering
            // one would insert outside the dictionary the author wrote.
            None if cursor >= call.dict.end => return None,
            None => cursor..cursor,
        },
    };
    Some(MetaKey { typed, written })
}

/// The name of the key the cursor stands in inside a `#tola-meta` dictionary.
pub(super) fn meta_key_at(source: &Source, cursor: usize, call: &MetaCall) -> Option<Range<usize>> {
    if !(call.dict.start <= cursor && cursor <= call.dict.end) {
        return None;
    }
    let dict = tola_typst_syntax::syntax::node_at_range(source, &call.dict)?;
    for child in dict.children() {
        let Some(named) = child.cast::<ast::Named>() else {
            continue;
        };
        let Some(name) = child.find(named.name().span()) else {
            continue;
        };
        if name.range().start <= cursor && cursor <= name.range().end {
            return Some(name.range());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::query::tests::*;

    /// A `parse-sources` call that names its schema cannot build, so it declares nothing.
    #[test]
    fn named_schema_argument_declares_nothing() {
        let program = parsed_program(PAGE_SCHEMA).replace(
            "parse-sources(all-sources(), page-schema)",
            "parse-sources(all-sources(), schema: page-schema)",
        );
        let mut site = QuerySession::with_program(&program);
        let items = site.completion(
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Page\", |))\n",
        );
        let labels = completion_labels(&items);
        assert!(!labels.contains(&"draft"), "{labels:?}");
        assert!(!labels.contains(&"title"), "{labels:?}");
    }

    /// A schema that declares no fields leaves the general completion lane the answer.
    #[test]
    fn empty_schema_leaves_completion_open() {
        let program = r#"#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": schema
#let page-schema = schema((:))
#let declared = parse-sources(all-sources(), page-schema)
"#;
        let mut site = QuerySession::with_program(program);
        let items = site.completion(
            "#import \"@tola/source:0.0.0\": tola-meta\n#let later = 1\n#tola-meta((title: \"Page\", la|))\n",
        );
        assert!(completion_labels(&items).contains(&"later"), "{items:?}");
    }

    /// A key one covering call rejects is offered only where every call declares or accepts it.
    #[test]
    fn meta_completion_offers_only_accepted_keys() {
        let program = r#"#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/schema:0.0.0": describe, optional, schema
#let strict-schema = schema((title: describe(optional(str), "the page title")), unknown: "error")
#let open-schema = schema((summary: describe(optional(str), "the page summary")), unknown: "keep")
#let first = parse-sources(all-sources(), strict-schema)
#let second = parse-sources(all-sources(), open-schema)
"#;
        let mut site = QuerySession::with_program(program);
        site.site.write("content/document.typ", SCHEMA_PAGE);
        let items =
            site.completion("#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta(( |))\n");
        let labels = completion_labels(&items);
        assert!(labels.contains(&"title"), "{labels:?}");
        assert!(!labels.contains(&"summary"), "{labels:?}");
    }

    /// A dictionary offers the keys it can still take: none at its own end, and inside it only the
    /// unwritten ones.
    #[test]
    fn dictionary_completion_offers_only_unwritten_keys() {
        let mut site = QuerySession::with_program(&summary_program());
        let closed = site.completion(
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Page\")|)\n",
        );
        let closed = completion_labels(&closed);
        assert!(!closed.contains(&"draft"), "{closed:?}");
        let open = site.completion(
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Page\", |))\n",
        );
        let open = completion_labels(&open);
        assert!(open.contains(&"draft"), "{open:?}");
        assert!(open.contains(&"summary"), "{open:?}");
        assert!(!open.contains(&"title"), "{open:?}");
    }

    /// A `#tola-meta` key completion offers the fields the site's schema declares, with the
    /// declaration and documentation each has.
    #[test]
    fn meta_completion_offers_declared_keys() {
        let mut site = QuerySession::with_program(&summary_program());
        let items = site.completion(
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Page\", summ|))\n",
        );
        let item = item_labeled(&items, "summary");
        assert_eq!(
            item.label_details
                .as_ref()
                .and_then(|details| details.description.as_deref()),
            Some("str")
        );
        let documentation = serde_json::to_string(&item.documentation).unwrap();
        assert!(
            documentation.contains("```typc\\nsummary?: str\\n```"),
            "{documentation}"
        );
        assert!(
            documentation.contains("the page summary"),
            "{documentation}"
        );
    }
}
