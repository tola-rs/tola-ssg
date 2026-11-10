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

/// The tola-meta call the cursor addresses, when the callee and its dictionary are established.
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

/// Whether range covers cursor, both ends included.
fn covers_cursor(range: &Range<usize>, cursor: usize) -> bool {
    range.start <= cursor && cursor <= range.end
}

/// The innermost call covering the cursor, found by containment when it has no leaf.
pub(super) fn call_around(source: &Source, cursor: usize) -> Option<LinkedNode<'_>> {
    let mut found: Option<LinkedNode<'_>> = None;
    let mut pending = vec![LinkedNode::new(source.root())];
    while let Some(node) = pending.pop() {
        pending.extend(node.children());
        if node.cast::<ast::FuncCall>().is_some()
            && covers_cursor(&node.range(), cursor)
            && found
                .as_ref()
                .is_none_or(|found| node.range().len() < found.range().len())
        {
            found = Some(node);
        }
    }
    found
}

/// The dictionary argument's node, when the cursor stands inside it.
fn dict_at_cursor<'a>(
    source: &'a Source,
    cursor: usize,
    call: &MetaCall,
) -> Option<LinkedNode<'a>> {
    if !covers_cursor(&call.dict, cursor) {
        return None;
    }
    tola_typst_syntax::syntax::node_at_range(source, &call.dict)
}

/// The key of one `name: value` entry, when the cursor stands in that key.
fn named_key_at(child: &LinkedNode<'_>, cursor: usize) -> Option<Range<usize>> {
    let named = child.cast::<ast::Named>()?;
    let name = child.find(named.name().span())?;
    covers_cursor(&name.range(), cursor).then(|| name.range())
}

/// The key the cursor writes in a tola-meta dictionary, or None over a value.
pub(super) fn meta_key(source: &Source, cursor: usize, call: &MetaCall) -> Option<MetaKey> {
    let dict = dict_at_cursor(source, cursor, call)?;
    let mut written = BTreeSet::new();
    let mut typed = None;
    let mut value = false;
    for child in dict.children() {
        if let Some(named) = child.cast::<ast::Named>() {
            if child.find(named.name().span()).is_some() {
                written.insert(named.name().get().to_string());
            }
            typed = typed.or_else(|| named_key_at(&child, cursor));
            if let Some(expr) = child.find(named.expr().span()) {
                value |= covers_cursor(&expr.range(), cursor);
            }
        } else if let Some(keyed) = child.cast::<ast::Keyed>() {
            if let ast::Expr::Str(key) = keyed.key()
                && let Some(key) = child.find(key.span())
                && covers_cursor(&key.range(), cursor)
            {
                return None;
            }
            if let Some(expr) = child.find(keyed.expr().span()) {
                value |= covers_cursor(&expr.range(), cursor);
            }
        }
    }
    let typed = match typed {
        Some(typed) => typed,
        None if value => return None,
        None => match dict
            .children()
            .filter(|child| !child.kind().is_trivia())
            .find(|child| {
                covers_cursor(&child.range(), cursor)
                    && !matches!(child.kind(), SyntaxKind::LeftParen | SyntaxKind::RightParen)
            }) {
            Some(child) if matches!(child.kind(), SyntaxKind::Error | SyntaxKind::Ident) => {
                child.range()
            }
            Some(_) => return None,
            // A cursor past the dictionary stands in no key; offering one would insert outside it.
            None if cursor >= call.dict.end => return None,
            None => cursor..cursor,
        },
    };
    Some(MetaKey { typed, written })
}

/// The name of the key the cursor stands in inside a `#tola-meta` dictionary.
pub(super) fn meta_key_at(source: &Source, cursor: usize, call: &MetaCall) -> Option<Range<usize>> {
    let dict = dict_at_cursor(source, cursor, call)?;
    dict.children()
        .find_map(|child| named_key_at(&child, cursor))
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
#import "@tola/schema:0.0.0": describe, optional, schema, union
#let strict-schema = schema((title: describe(optional(str), "the page title")), unknown: "error")
#let open-schema = union(
  schema((summary: describe(optional(str), "the page summary"))),
  schema((:), unknown: "keep"),
)
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

    #[test]
    fn meta_completion_describes_input_before_parsing() {
        let program = parsed_program(&format!("{PAGE_SCHEMA}\n  published: optional(timestamp),"))
            .replace(
                "describe, optional, schema",
                "describe, optional, schema, timestamp",
            );
        let mut site = QuerySession::with_program(&program);
        let items = site.completion(
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Page\", |))\n",
        );
        let draft = item_labeled(&items, "draft");
        let documentation = serde_json::to_string(&draft.documentation).unwrap();
        assert!(
            documentation.contains("draft?: bool = false"),
            "{documentation}"
        );
        let published = item_labeled(&items, "published");
        assert_eq!(
            published
                .label_details
                .as_ref()
                .and_then(|details| details.description.as_deref()),
            Some("datetime | str")
        );
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
        let summary = item_labeled(&items, "summary");
        assert_eq!(
            summary
                .label_details
                .as_ref()
                .and_then(|details| details.description.as_deref()),
            Some("str")
        );
        let documentation = serde_json::to_string(&summary.documentation).unwrap();
        assert!(documentation.contains("summary?: str"), "{documentation}");
        assert!(
            documentation.contains("the page summary"),
            "{documentation}"
        );
    }
}
