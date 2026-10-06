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
pub(super) fn meta_call(source: &Source, cursor: usize) -> Option<MetaCall> {
    let names = tola_typst_syntax::names::SourceNames::new(source.clone());
    let node = call_around(source, cursor)?;
    let call = node.cast::<ast::FuncCall>()?;
    let callee = node.find(call.callee().span())?;
    if !calls_package_member(
        &names,
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
    use lsp_types::request as lsp_request;
    use lsp_types::request::Request as LspRequest;
    use lsp_types::{CompletionResponse, CompletionTextEdit, TextEdit};

    #[test]
    fn metadata_completion_offers_optional_fields() {
        let program = parsed_program(&format!(
            "{PAGE_SCHEMA}\n subtitle: describe(optional(str), \"Optional subtitle\"),"
        ));
        let mut site = QuerySession::with_program(&program);
        site.site.write("content/page.typ", SCHEMA_PAGE);
        let completion: Option<CompletionResponse> = serde_json::from_value(site_reply(
            &mut site,
            lsp_request::Completion::METHOD,
            &program.replace("source.meta.draft", "source.meta.sub|"),
            serde_json::json!({}),
        ))
        .unwrap();
        let items = match completion.unwrap() {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        };
        let field = item_labeled(&items, "subtitle");
        assert!(
            serde_json::to_string(&field.documentation)
                .unwrap()
                .contains("Optional subtitle")
        );
        assert_eq!(
            field
                .label_details
                .as_ref()
                .and_then(|details| details.description.as_deref()),
            Some("str")
        );
    }
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
    fn field_less_schema_keeps_general_completion() {
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

    /// A source no call's records cover says so where its declared keys are read.
    #[test]
    fn uncovered_source_says_no_call_parses_it() {
        let mut site = QuerySession::with_program(&summary_program());
        let hover = template_hover(
            &mut site,
            "templates/meta.typ",
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((tit|le: \"Page\"))\n",
        )
        .expect("a hover");
        assert!(
            hover.contains("no `parse-sources` call parses this source yet"),
            "{hover}"
        );
    }

    /// A completion at a dictionary's own end offers no key.
    #[test]
    fn closed_dictionary_end_offers_no_keys() {
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
    }

    /// A schema argument the world observed as two different declarations declares nothing; one
    /// declaration answers its own shape.
    #[test]
    fn ambiguous_schema_argument_declares_nothing() {
        let block = |schemas: &str| {
            format!(
                "#let first = page-schema\n\
                 #let second = schema((title: describe(optional(str, default: \"Other\"), \"unrelated title\"), draft: optional(bool, default: false)))\n\
                 #for declaration in ({schemas}) {{\n\
                   let parsed = parse-sources(all-sources(), declaration)\n\
                   let kept = parsed.filter(source => not source.meta.draft)\n\
                   for page in kept {{\n\
                     let chosen = page.meta.title\n\
                   }}\n\
                 }}\n"
            )
        };
        for (schemas, declared) in [("first,", true), ("first, second", false)] {
            let program = parsed_program(PAGE_SCHEMA) + &block(schemas);
            let mut site = QuerySession::with_program(&program);
            site.site.write("content/page.typ", SCHEMA_PAGE);
            let marked = program.replace("page.meta.title", "page.meta.ti|tle");
            let hover = site_hover(&mut site, &marked).expect("a hover");
            assert_eq!(
                hover.contains("the page title"),
                declared,
                "{schemas}: {hover}"
            );
            assert!(!hover.contains("unrelated title"), "{schemas}: {hover}");
            assert_eq!(
                hover.contains("Declared fields"),
                declared,
                "{schemas}: {hover}"
            );
        }
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

    /// A key completion answers the declared keys while the dictionary is still unfinished.
    #[test]
    fn meta_completion_answers_unfinished_key() {
        let mut site = QuerySession::with_program(&summary_program());
        let items = site.completion(
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta(( title: \"Home\", summ|",
        );
        assert_eq!(completion_labels(&items), ["summary"]);
        let item = item_labeled(&items, "summary");
        assert_eq!(
            item.label_details
                .as_ref()
                .and_then(|details| details.description.as_deref()),
            Some("str")
        );
        assert_eq!(
            serde_json::to_string(&item.documentation).unwrap(),
            r#"{"kind":"markdown","value":"```typc\nsummary?: str\n```\n\n---\n\nthe page summary"}"#
        );
    }

    /// A `parse-sources` call in a module the entry imports declares the site's schema too.
    #[test]
    fn meta_completion_sees_imported_schema() {
        let mut site = QuerySession::with_program(
            r#"#import "@tola/source:0.0.0": all-sources
#import "site/meta.typ": declared-meta
#let declared = declared-meta(all-sources())
"#,
        );
        site.site.write(
            "site/meta.typ",
            &format!(
                "#import \"@tola/source:0.0.0\": parse-sources\n\
                 #import \"@tola/schema:0.0.0\": describe, optional, schema\n\
                 #let page-schema = schema((\n{PAGE_SCHEMA}\n  summary: describe(optional(str), \"the page summary\"),\n))\n\
                 #let declared-meta(sources) = parse-sources(sources, page-schema)\n"
            ),
        );
        let items = site.completion(
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta(( title: \"Home\", summ|))",
        );
        let item = item_labeled(&items, "summary");
        let documentation = serde_json::to_string(&item.documentation).unwrap();
        assert!(
            documentation.contains("the page summary"),
            "{documentation}"
        );
    }

    /// A `#tola-meta` key completion replaces the prefix the author typed.
    #[test]
    fn meta_completion_replaces_the_typed_key() {
        let mut site = QuerySession::with_program(&summary_program());
        let items = site.completion(
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Page\", summ|))\n",
        );
        let item = item_labeled(&items, "summary");
        assert_eq!(
            item.text_edit,
            Some(CompletionTextEdit::Edit(TextEdit {
                range: lsp_types::Range::new(
                    lsp_types::Position::new(1, 27),
                    lsp_types::Position::new(1, 31),
                ),
                new_text: "summary: ${1}".to_owned(),
            }))
        );
    }

    /// A key the dictionary already writes is not offered again.
    #[test]
    fn meta_completion_hides_written_keys() {
        let mut site = QuerySession::with_program(&summary_program());
        let items = site.completion(
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Page\", |))\n",
        );
        let labels = completion_labels(&items);
        assert!(labels.contains(&"draft"), "{labels:?}");
        assert!(labels.contains(&"summary"), "{labels:?}");
        assert!(!labels.contains(&"title"), "{labels:?}");
    }

    /// A `#tola-meta` key answers the field line its schema declares.
    #[test]
    fn meta_key_answers_its_declared_field() {
        let mut site = QuerySession::with_program(&summary_program());
        let hover = site
            .hover_text(
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((tit|le: \"Page\"))\n",
            )
            .expect("a hover");
        assert!(
            hover.contains("```typc\ntitle: str = \"Untitled\"\n```"),
            "{hover}"
        );
        assert!(hover.contains("the page title"), "{hover}");
    }

    /// The `tola-meta` callee answers the package function, whether or not the call evaluates.
    #[test]
    fn meta_callee_answers_the_package_function() {
        let mut site = QuerySession::with_program(&summary_program());
        for marked in [
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola|-meta((title: \"Page\"))\n",
            "#import \"@tola/source:0.0.0\": tola-meta\n#tol|a-meta((title: \"Page\", summ))\n",
        ] {
            let hover = site.hover_text(marked).expect("a hover");
            assert!(
                hover.contains("let tola-meta(\n  payload: dictionary,\n) = content;"),
                "{marked:?}: {hover}"
            );
            assert!(
                hover.contains("Declare the metadata"),
                "{marked:?}: {hover}"
            );
        }
    }

    /// A source no call's records cover answers every declared schema, each field naming the
    /// declaration that has it.
    #[test]
    fn meta_key_names_its_declaration() {
        let program = summary_program()
            + "#let other-schema = schema((title: describe(optional(str, default: \"Other\"), \"the other title\")))\n#let second = parse-sources(all-sources(), other-schema)\n";
        let mut site = QuerySession::with_program(&program);
        let hover = template_hover(
            &mut site,
            "templates/meta.typ",
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((tit|le: \"Page\"))\n",
        )
        .expect("a hover");
        assert!(hover.contains("the page title"), "{hover}");
        assert!(hover.contains("the other title"), "{hover}");
        assert!(
            hover.contains("Declared by `page-schema` in `site.typ`."),
            "{hover}"
        );
        assert!(
            hover.contains("Declared by `other-schema` in `site.typ`."),
            "{hover}"
        );
    }
}
