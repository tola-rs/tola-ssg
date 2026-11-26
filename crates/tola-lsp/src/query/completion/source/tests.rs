use std::sync::Arc;

use lsp_types::{CompletionItem, CompletionItemKind, InsertTextFormat};
use tola_build::cancellation::BuildCancellation;
use tola_typst::GLOBAL_LIBRARY;
use tola_typst::typst::foundations::PathOrStr;
use tola_typst::typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use tola_typst_syntax::imports::{ImportQuery, NameGraph};
use tola_typst_syntax::names::SourceNames;

use super::{source_import_completion, source_member_completion, source_name_completion};
use crate::query::tests::{FAILED_PROGRAM, QuerySession, insertion_text};

fn file(path: &str) -> FileId {
    RootedPath::new(VirtualRoot::Project, VirtualPath::new(path).unwrap()).intern()
}

fn graph(src: &Source, libs: &[Source]) -> NameGraph {
    NameGraph::new(
        std::iter::once(src)
            .chain(libs)
            .map(|s| Arc::new(SourceNames::new(s.clone()))),
        |query| match query {
            ImportQuery::Path { path, .. } if path.starts_with('@') => {
                let pkg = path.parse().ok()?;
                Some(
                    RootedPath::new(
                        VirtualRoot::Package(pkg),
                        VirtualPath::new("lib.typ").unwrap(),
                    )
                    .intern(),
                )
            }
            ImportQuery::Path { file, path } => PathOrStr::Str(path.into())
                .resolve(file)
                .ok()
                .map(|p| p.intern()),
            ImportQuery::Expression { .. } => None,
        },
        Some(&GLOBAL_LIBRARY),
        || Ok::<(), std::convert::Infallible>(()),
    )
    .unwrap()
}

fn named<'a>(items: &'a [CompletionItem], name: &str) -> &'a CompletionItem {
    items.iter().find(|item| item.label == name).unwrap()
}

fn description(item: &CompletionItem) -> &str {
    item.label_details
        .as_ref()
        .and_then(|d| d.description.as_deref())
        .unwrap()
}

fn documentation(item: &CompletionItem) -> &str {
    let Some(lsp_types::Documentation::MarkupContent(markup)) = &item.documentation else {
        panic!("completion carries Markdown documentation");
    };
    &markup.value
}

fn cold_items(marked: &str) -> Vec<CompletionItem> {
    let pos = marked.find('|').unwrap();
    let src = Source::detached(marked.replace('|', ""));
    let names = SourceNames::new(src.clone());
    let cancel = BuildCancellation::new();
    if marked.starts_with("#import") {
        source_import_completion(&graph(&src, &[]), &names, &src, pos, &cancel)
    } else {
        source_name_completion(None, &names, &src, pos, &cancel)
    }
    .unwrap()
    .unwrap()
}

fn cold_member_items(marked: &str) -> Option<Vec<CompletionItem>> {
    let pos = marked.find('|').unwrap();
    let src = Source::detached(marked.replace('|', ""));
    let names = SourceNames::new(src.clone());
    source_member_completion(None, &names, &src, pos, &BuildCancellation::new()).unwrap()
}

fn cold_imported_member_items(marked: &str, lib: &Source) -> Vec<CompletionItem> {
    let pos = marked.find('|').unwrap();
    let src = Source::new(file("main.typ"), marked.replace('|', ""));
    let names = SourceNames::new(src.clone());
    source_member_completion(
        Some(&graph(&src, std::slice::from_ref(lib))),
        &names,
        &src,
        pos,
        &BuildCancellation::new(),
    )
    .unwrap()
    .unwrap_or_else(|| panic!("{marked}"))
}

fn main_src(text: &str, libs: &[Source]) -> (Source, SourceNames, NameGraph) {
    let src = Source::new(file("main.typ"), text.to_owned());
    let names = SourceNames::new(src.clone());
    let g = graph(&src, libs);
    (src, names, g)
}

fn cold_names(text: &str, libs: &[Source]) -> Vec<CompletionItem> {
    let (src, names, g) = main_src(text, libs);
    source_name_completion(
        Some(&g),
        &names,
        &src,
        src.text().len(),
        &BuildCancellation::new(),
    )
    .unwrap()
    .unwrap()
}

fn cold_import_items(text: &str, libs: &[Source]) -> Vec<CompletionItem> {
    let (src, names, g) = main_src(text, libs);
    source_import_completion(
        &g,
        &names,
        &src,
        src.text().len(),
        &BuildCancellation::new(),
    )
    .unwrap()
    .unwrap()
}

#[test]
fn call_snippets_by_position() {
    for (marked, label, expected) in [
        ("𝄞 #rep|\n", "repr", "repr(${1})"),
        ("#ran|", "range", "range(${1})"),
        ("#her|", "here", "here()${1}"),
        ("#let local(x) = rep|", "repr", "repr(${1})"),
        ("#rep|r(1)", "repr", "repr"),
        ("#rep|[body]", "repr", "repr"),
        ("#let alias = rep|", "repr", "repr"),
        ("#let result = 1 + rep|", "repr", "repr(${1})"),
        ("#let f(x) = x\n#f(rep|)", "repr", "repr"),
        ("#show raw: rep|", "repr", "repr"),
        (
            "#show heading: pagebr|",
            "pagebreak.with",
            "pagebreak.with(${1})",
        ),
        ("#show stron|: it => it", "strong", "strong"),
        ("#import math: ab|", "abs", "abs"),
    ] {
        let items = cold_items(marked);
        let item = named(&items, label);
        assert_eq!(insertion_text(item), expected, "{marked}");
        assert_eq!(
            item.insert_text_format == Some(InsertTextFormat::SNIPPET),
            expected.contains("${"),
            "{marked}"
        );
    }
}

#[test]
fn cold_and_warm_callable_agreement() {
    let lib = Source::new(
        file("lib.typ"),
        "/// Render meaning.\n/// - value (int): Input meaning.\n#let render(value, label: \"x\") = value"
            .to_owned(),
    );
    for (marked, label, inserted, sig_part) in [
        (
            "#let render(value, label: \"x\") = value\n#ren|",
            "render",
            "render(${1})",
            "render(",
        ),
        (
            "#import \"lib.typ\": render\n#ren|",
            "render",
            "render(${1})",
            "value: int",
        ),
        (
            "#import \"lib.typ\": render\n#let alias = render\n#ali|",
            "alias",
            "alias(${1})",
            "value: int",
        ),
        ("#let stamp() = 1\n#sta|", "stamp", "stamp()${1}", "stamp()"),
    ] {
        let text = marked.replace('|', "");
        let cold = cold_names(&text, std::slice::from_ref(&lib));
        let mut site = QuerySession::new();
        site.site.write("content/lib.typ", lib.text());
        site.site.write("content/document.typ", &text);
        let warm = site.completion(marked);
        for item in [named(&cold, label), named(&warm, label)] {
            assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION), "{marked}");
            assert!(
                description(item).contains(sig_part),
                "{marked}: {}",
                description(item)
            );
            assert_eq!(insertion_text(item), inserted, "{marked}");
            assert_eq!(
                item.insert_text_format,
                Some(InsertTextFormat::SNIPPET),
                "{marked}"
            );
        }
    }

    let mixed = "#let value = if false { (choice: rect) } else { (choice: 2) }\n#value.cho|ice\n";
    let pos = mixed.find('|').unwrap();
    let src = Source::new(file("main.typ"), mixed.replace('|', ""));
    let names = SourceNames::new(src.clone());
    let cold = source_member_completion(None, &names, &src, pos, &BuildCancellation::new())
        .unwrap()
        .unwrap();
    let mut site = QuerySession::new();
    let warm = site.completion(mixed);
    for items in [cold, warm] {
        let item = named(&items, "choice");
        assert_eq!(insertion_text(item), "choice");
        assert_eq!(item.kind, Some(CompletionItemKind::VARIABLE));
        assert!(item.documentation.is_none());
        assert_ne!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
    }
}

#[test]
fn member_calls_by_receiver() {
    let lib = Source::new(file("lib.typ"), "#let render(value) = value".to_owned());
    for (marked, label, inserted) in [
        (
            "#import \"lib.typ\" as lib\n#lib.ren|",
            "render",
            "render(${1})",
        ),
        ("#str.le|", "len", "len(${1})"),
        ("#let text = \"x\"\n#text.le|", "len", "len()${1}"),
    ] {
        let cold = cold_imported_member_items(marked, &lib);
        let mut site = QuerySession::new();
        site.site.write("content/lib.typ", lib.text());
        site.site
            .write("content/document.typ", &marked.replace('|', ""));
        let warm = site.completion(marked);
        for item in [named(&cold, label), named(&warm, label)] {
            assert_eq!(insertion_text(item), inserted, "{marked}");
        }
    }
}

#[test]
fn export_documentation_sources() {
    let lib = Source::new(
        file("library.typ"),
        "/// Library meaning.\n/// - value (int): Input meaning.\n/// -> int\n#let render(value: 1) = value"
            .to_owned(),
    );
    let reexport = Source::new(
        file("exports.typ"),
        "/// Export meaning.\n#import \"library.typ\": render as external".to_owned(),
    );
    let libs = [lib, reexport];
    for (text, label, meaning, import) in [
        (
            "#import \"library.typ\": ren",
            "render",
            "Library meaning.",
            true,
        ),
        (
            "#import \"library.typ\": render as alias\n#ali",
            "alias",
            "Library meaning.",
            false,
        ),
        (
            "#import \"library.typ\": *\n#ren",
            "render",
            "Library meaning.",
            false,
        ),
        (
            "#import \"exports.typ\": ext",
            "external",
            "Export meaning.",
            true,
        ),
    ] {
        let items = if import {
            cold_import_items(text, &libs)
        } else {
            cold_names(text, &libs)
        };
        let completed = named(&items, label);
        assert!(
            description(completed).contains(&format!("{label}(")),
            "{completed:?}"
        );
        assert!(
            description(completed).contains("value: int"),
            "{completed:?}"
        );
        assert!(documentation(completed).contains(meaning), "{completed:?}");
    }

    let pkg = tola_packages::builtin_package(&tola_packages::TolaPackage::Document.spec()).unwrap();
    let pkg_libs = pkg
        .files()
        .filter(|(path, _)| path.ends_with(".typ"))
        .map(|(path, src)| {
            let id = RootedPath::new(
                VirtualRoot::Package(pkg.spec()),
                VirtualPath::new(path).unwrap(),
            )
            .intern();
            Source::new(id, src.into_owned())
        })
        .collect::<Vec<_>>();
    let documented = pkg
        .export_documentation(&["current-document".to_owned()])
        .unwrap()
        .remove(0);
    for (text, label, import) in [
        (
            "#import \"@tola/document:0.0.0\": current-doc",
            "current-document",
            true,
        ),
        (
            "#import \"@tola/document:0.0.0\": current-document as identity\n#iden",
            "identity",
            false,
        ),
    ] {
        let items = if import {
            cold_import_items(text, &pkg_libs)
        } else {
            cold_names(text, &pkg_libs)
        };
        let completed = named(&items, label);
        assert!(
            description(completed).contains(&format!("{label}(")),
            "{completed:?}"
        );
        assert!(documentation(completed).contains(&documented.documentation.summary));
    }
}

#[test]
fn math_and_code_library_namespaces() {
    for (marked, expected) in [("$sq|$", "sqrt"), ("$st|$", "std"), ("#st|", "std")] {
        let items = cold_items(marked);
        assert!(
            items.iter().any(|i| i.label == expected),
            "{marked}: {items:?}"
        );
    }
    for (marked, expected) in [
        ("#std.math.eq|", "equation"),
        ("#std.str.le|", "len"),
        ("$sqrt.wh|$", "where"),
    ] {
        let items = cold_member_items(marked).unwrap();
        assert!(
            items.iter().any(|i| i.label == expected),
            "{marked}: {items:?}"
        );
    }
}

#[test]
fn utf16_completion_edits() {
    let marked = "#let café(value) = value\n😀 #ca|fé";
    let items = cold_items(marked);
    let completed = named(&items, "café");
    let Some(lsp_types::CompletionTextEdit::InsertAndReplace(edit)) = &completed.text_edit else {
        panic!("the cursor splits the inserted prefix from the replaced name");
    };
    assert_eq!(edit.insert.start, lsp_types::Position::new(1, 4));
    assert_eq!(edit.insert.end, lsp_types::Position::new(1, 6));
    assert_eq!(edit.replace.end, lsp_types::Position::new(1, 8));
    assert_eq!(edit.new_text, "café(${1})");
}

#[test]
fn source_expression_members() {
    for (marked, expected) in [
        ("#\"text\".|", "split"),
        ("#let value = \"text\"\n#value.|", "split"),
        ("#let value = (1, 2)\n#value.|", "map"),
        ("#let value = (custom: 1)\n#value.|", "custom"),
        ("#let value = 1 + 2\n#value.|", "signum"),
        (
            "#let value = if true { (left: 1) } else { (right: 2) }\n#value.|",
            "right",
        ),
        ("#let f(x) = (custom: x)\n#f(1).|", "custom"),
        ("#let f(x: 1) = (custom: x)\n#f().|", "custom"),
        ("#let f((x, y)) = (custom: x)\n#f((1, 2)).|", "custom"),
        ("#let f(..args) = args\n#f(extra: 1).|", "extra"),
        (
            "#let f(x) = (custom: x)\n#let alias = f\n#alias(1).|",
            "custom",
        ),
        ("#let f = ((x) => (custom: x))\n#f(1).|", "custom"),
        (
            "#let outer(x) = { let inner() = x; inner() }\n#outer((captured: 1)).|",
            "captured",
        ),
        ("#let value = str(1)\n#value.|", "split"),
        ("#let convert = str\n#convert(1).|", "split"),
        ("#let value = \"a,b\".split(\",\")\n#value.|", "map"),
        ("#let f(x) = (current: x.len())\n#f(\"test\").|", "current"),
        (
            "#let f(x) = (current: x.len())\n#f(\"test\").current.|",
            "signum",
        ),
        ("#let value = (current: unknown)\n#value.|", "current"),
        (
            "#let f((..rest, value)) = value\n#f((0, str(1))).|",
            "split",
        ),
        (
            "#let (ignored, value) = (0, (current: str(1)))\n#value.current.|",
            "split",
        ),
        (
            "#let (ignored, ..rest) = (ignored: 0, current: str(1))\n#rest.|",
            "current",
        ),
        (
            "#let outer(x) = { let inner() = (current: x.len()); inner() }\n#outer(\"test\").current.|",
            "signum",
        ),
    ] {
        let cold = cold_member_items(marked).unwrap_or_else(|| panic!("{marked}"));
        assert!(
            cold.iter().any(|i| i.label == expected),
            "{marked}: {cold:?}"
        );
    }
    let warm = QuerySession::with_program(FAILED_PROGRAM)
        .completion("#panic(\"page\")\n#let f(x) = (current: x.len())\n#f(\"test\").current.|");
    assert!(warm.iter().any(|i| i.label == "signum"), "{warm:?}");
}

#[test]
fn source_import_module_identity() {
    let lib = Source::new(
        file("library.typ"),
        "/// Render meaning.\n#let render(value: 1) = (custom: value)".to_owned(),
    );
    for (marked, expected) in [
        (
            "#import \"library.typ\": render as local\n#local().|",
            "custom",
        ),
        (
            "#import \"library.typ\" as library\n#library.render().|",
            "custom",
        ),
        ("#import \"library.typ\" as library\n#library.|", "render"),
        (
            "#let f() = { import \"library.typ\": render; render() }\n#f().|",
            "custom",
        ),
    ] {
        let items = cold_imported_member_items(marked, &lib);
        assert!(
            items.iter().any(|i| i.label == expected),
            "{marked}: {items:?}"
        );
    }
}

#[test]
fn writes_invalidate_initializers() {
    for (marked, expected) in [
        (
            "#let value = (old: 1)\n#(value = (current: 2))\n#value.|",
            None,
        ),
        (
            "#let value = (old: 1)\n#value.remove(\"old\")\n#value.|",
            None,
        ),
        (
            "#let value = (old: 1)\n#value.o|ld\n#(value = (current: 2))",
            Some("old"),
        ),
        (
            "#let value = (old: 1)\n#{ let value = (inner: 1); value = (current: 2) }\n#value.o|ld",
            Some("old"),
        ),
    ] {
        let cold = cold_member_items(marked);
        if let Some(expected) = expected {
            assert!(
                cold.unwrap().iter().any(|i| i.label == expected),
                "{marked}"
            );
        } else {
            assert!(cold.is_none(), "{marked}: {cold:?}");
            let mut site = QuerySession::with_program(FAILED_PROGRAM);
            let warm = site.completion(&format!("#panic(\"page\")\n{marked}"));
            assert!(!warm.iter().any(|i| i.label == "old"), "{marked}: {warm:?}");
        }
    }
}

#[test]
fn uncertain_calls_omit_guesses() {
    for (marked, expected) in [
        (
            "#let last((..rest, x)) = x\n#last((0, (wrong: 1), (right: 1))).|",
            Some("right"),
        ),
        (
            "#let last((x, ..rest, y)) = y\n#last((0, 1, (wrong: 1), (right: 1))).|",
            Some("right"),
        ),
        (
            "#let choose(flag) = if flag { (1, 2).at(0) } else { (wrong: 1) }\n#choose(true).|",
            None,
        ),
        ("#let take(x) = x\n#take(if false { (wrong: 1) }).|", None),
        (
            "#let take(x) = x\n#take(if true { (left: 1) } else { (right: 1) }).|",
            None,
        ),
        (
            "#let f() = { let ignored = { return (right: 1) }; (wrong: 1) }\n#f().|",
            None,
        ),
        (
            "#let f() = { let inner(x: { return (right: 1) }) = x; (wrong: 1) }\n#f().|",
            None,
        ),
        (
            "#let f() = { let inner() = { return (unused: 1) }; (right: 1) }\n#f().|",
            Some("right"),
        ),
    ] {
        let items = cold_member_items(marked);
        let labels = items
            .as_ref()
            .map(|items| items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>());
        match expected {
            Some(expected) => assert!(
                labels.as_ref().is_some_and(|l| l.contains(&expected)),
                "{marked}: {labels:?}"
            ),
            None => assert!(items.is_none(), "{marked}: {labels:?}"),
        }
    }
}
