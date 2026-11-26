use lsp_types::CompletionTextEdit;
use serde_json::json;

use crate::query::tests::*;

#[test]
fn incomplete_call_member_completion() {
    let mut site = QuerySession::two_documents();
    let items = site.completion(
        r#"#import "@tola/document:0.0.0": current-document
    #context current-document().|
    #metadata((title: [Later])) <tola-meta>"#,
    );
    let labels = completion_labels(&items);
    for field in ["output", "route", "location"] {
        assert!(labels.contains(&field), "{labels:?}");
    }
    let output = item_labeled(&items, "output")
        .label_details
        .as_ref()
        .and_then(|d| d.description.as_deref())
        .unwrap();
    assert!(
        output.contains("a.html") && output.contains("b.html"),
        "{output}"
    );
    assert!(!site.config.build().publish_dir.exists());
}

#[test]
fn import_aliases_vs_shadowing() {
    let mut site = QuerySession::new();
    let items = site.completion(
        r#"#import "@tola/document:0.0.0": current-document as identity
    #let current-document() = (not-native: true)
    #context identity().|"#,
    );
    let alias = completion_labels(&items);
    assert!(alias.contains(&"output"), "{alias:?}");
    assert!(!alias.contains(&"not-native"), "{alias:?}");

    let shadow = site.completion(
        r#"#import "@tola/document:0.0.0": current-document
    #let render(current-document) = [#context current-document().|]
    #render(() => (custom: 7))"#,
    );
    assert!(
        !completion_labels(&shadow).contains(&"output"),
        "{shadow:?}"
    );
}

#[test]
fn completion_insert_replace_ranges() {
    let mut site = QuerySession::new();
    let completion = site.reply(
        "textDocument/completion",
        "#let keeper = 1\n#let k = kee|per\n",
    );
    let item = completion
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "keeper")
        .expect("a `keeper` completion");
    assert_eq!(
        item["textEdit"]["insert"],
        json!({ "start": { "line": 1, "character": 9 }, "end": { "line": 1, "character": 12 } }),
        "{item}"
    );
    assert_eq!(
        item["textEdit"]["replace"],
        json!({ "start": { "line": 1, "character": 9 }, "end": { "line": 1, "character": 15 } }),
        "{item}"
    );
    assert_eq!(item["textEdit"]["newText"], "keeper");

    let completion = site.reply(
        "textDocument/completion",
        "#let keeper = 1\n#let k = keeper|\n",
    );
    let item = completion
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == "keeper")
        .expect("a `keeper` completion");
    assert_eq!(
        item["textEdit"]["range"],
        json!({ "start": { "line": 1, "character": 9 }, "end": { "line": 1, "character": 15 } }),
        "{item}"
    );
    assert!(item["textEdit"].get("insert").is_none(), "{item}");
}

#[test]
fn broken_site_import_completion() {
    let mut site = QuerySession::new();
    let items = site.completion("#import \"@tola/do|\"\n#unknown_after_import");
    assert!(
        completion_labels(&items).contains(&"@tola/document:0.0.0"),
        "{items:?}"
    );
}

#[test]
fn doc_comment_template_completion() {
    let mut site = QuerySession::new();
    let items = site.completion("///|\n#let f(a, b) = a\n");
    let item = item_labeled(&items, "Document function");
    let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
        panic!("expected template edit: {:?}", item.text_edit);
    };
    assert_eq!(
        edit.range,
        lsp_types::Range::new(
            lsp_types::Position::new(0, 3),
            lsp_types::Position::new(0, 3),
        )
    );
    assert!(edit.new_text.contains("$0"), "{}", edit.new_text);
    assert!(edit.new_text.contains("a ($1)"), "{}", edit.new_text);
    assert!(edit.new_text.contains("b ($3)"), "{}", edit.new_text);
    assert!(edit.new_text.contains("-> $5"), "{}", edit.new_text);

    for marked in ["///|\n#let x = 1\n", "///|\n\n#let f(a) = a\n"] {
        let items = site.completion(marked);
        assert!(
            items.iter().all(|i| i.label != "Document function"),
            "{marked:?}: {:?}",
            completion_labels(&items)
        );
    }
}

#[test]
fn ignored_files_excluded_from_paths() {
    let mut site = QuerySession::new();
    site.site.write("templates/page.typ", "");
    site.site.write("scratch/notes.typ", "");
    site.site.write(".gitignore", "scratch/\n");
    let items = site.completion("#include \"scr|\"\n");
    let labels = completion_labels(&items);
    assert!(!labels.iter().any(|l| l.contains("scratch")), "{items:?}");
}

#[test]
fn call_named_parameter_completions() {
    type Case<'a> = (&'a [(&'a str, &'a str)], &'a str, &'a [&'a str]);
    let cases: [Case; 5] = [
        (&[], "#align(|)\n", &["alignment"]),
        (&[], "#align(a|)\n", &["alignment"]),
        (
            &[
                (
                    "content/shapes.typ",
                    "#let line(close: false, name: \"x\") = close\n",
                ),
                (
                    "content/draw.typ",
                    "#import \"shapes.typ\"\n#let draw = shapes\n",
                ),
            ],
            "#import \"draw.typ\": draw\n#draw.line(|\n",
            &["close", "name"],
        ),
        (&[], "#let d = (a: 1)\n#d.at(|\n", &["default"]),
        (&[], "#let f(value: 1) = value\n#f(|\n", &["value"]),
    ];
    for (files, marked, expected) in cases {
        let mut site = QuerySession::new();
        for (rel, text) in files.iter().copied() {
            site.site.write(rel, text);
        }
        let items = site.completion(marked);
        let labels = completion_labels(&items);
        for label in expected {
            assert!(labels.contains(label), "{marked:?}: {labels:?}");
        }
    }
}

#[test]
#[cfg(unix)]
fn path_escapes_and_written_spelling() {
    let mut site = QuerySession::new();
    site.site.write("templates/title.typ", "Title\n");
    let items = site.completion("#include \"\\u{2f}te|\"\n");
    let item = item_labeled(&items, "/templates/title.typ");
    assert_eq!(insertion_text(item), "/templates/title.typ");
    assert_eq!(
        item.filter_text.as_deref(),
        Some("\\u{2f}templates/title.typ")
    );

    site.site.write("content/quo\"ted.typ", "Quoted\n");
    let items = site.completion("#include \"|\"\n");
    let item = item_labeled(&items, "quo\"ted.typ");
    assert_eq!(insertion_text(item), "quo\\\"ted.typ");
    let completed = format!("#include \"{}|\"\n", insertion_text(item));
    let Some(lsp_types::GotoDefinitionResponse::Scalar(loc)) = site.definition(&completed) else {
        panic!("the completed string reaches its file");
    };
    assert_eq!(
        loc.uri,
        crate::uri::from_file_path(&site.config.root().join("content/quo\"ted.typ")).unwrap()
    );
    let opened = crate::sources::load(
        &loc.uri,
        site.config.root(),
        &Default::default(),
        &site.session.resources().source_boundary(&site.config),
        None,
        None,
    )
    .expect("the completed file opens");
    assert_eq!(opened.text(), "Quoted\n");

    site.site.write("content/chapter:notes.typ", "Notes\n");
    let items = site.completion("#include \"chapter:|notes.typ\"\n");
    let item = item_labeled(&items, "chapter:notes.typ");
    assert_eq!(insertion_text(item), "chapter:notes.typ");
}

#[test]
fn nonpath_arguments_offer_no_files() {
    let mut site = QuerySession::new();
    site.site.write("assets/cover.svg", "<svg/>");
    for marked in [
        "#image(alt: \"|",
        "#read(encoding: \"|",
        "#read(\"data.csv\", encoding: \"utf|8\")",
        "#bibliography(\"refs.bib\", style: \"ap|a\")",
        "#bibliography(\"refs.bib\", \"m|\")",
        "#let read(text) = text\n#read(\"|\")",
        "#let read(text) = text\n#read(\"|",
        "#let render(read) = read(\"|\")",
    ] {
        let items = site.completion(marked);
        assert!(
            !completion_labels(&items).contains(&"../assets/cover.svg"),
            "{marked}: {items:?}"
        );
    }
}

#[test]
fn path_completion_filters_by_role() {
    let mut site = QuerySession::new();
    site.site.write("content/more.bib", "");
    site.site.write("content/styles/custom.csl", "");
    site.site.write("content/styles/refs.bib", "");
    site.site.write("content/my.tmTheme", "");
    site.site.write("content/my.sublime-syntax", "");

    for (marked, expected, excluded) in [
        ("#bibliography((\"first.bib\", \"m|\"))", "more.bib", None),
        ("#bibliography((\"first.bib\", \"m|", "more.bib", None),
        (
            "#bibliography(\"refs.bib\", style: \"styles/|custom.csl\")",
            "styles/custom.csl",
            Some("styles/refs.bib"),
        ),
        (
            "#bibliography(\"refs.bib\", style: \"styles/|\")",
            "styles/custom.csl",
            Some("styles/refs.bib"),
        ),
        (
            "#set raw(theme: \"my|\")",
            "my.tmTheme",
            Some("my.sublime-syntax"),
        ),
        (
            "#raw(\"text\", theme: \"my|\")",
            "my.tmTheme",
            Some("my.sublime-syntax"),
        ),
        (
            "#set raw(syntaxes: (\"other.sublime-syntax\", \"my|\"))",
            "my.sublime-syntax",
            Some("my.tmTheme"),
        ),
    ] {
        let items = site.completion(marked);
        let labels = completion_labels(&items);
        assert!(labels.contains(&expected), "{marked}: {labels:?}");
        if let Some(excluded) = excluded {
            assert!(!labels.contains(&excluded), "{marked}: {labels:?}");
        }
    }

    let items = site.completion("#let raw(theme) = theme\n#raw(theme: \"my|\")");
    assert!(!completion_labels(&items).contains(&"my.tmTheme"));
}

#[test]
fn reader_path_edit_ranges() {
    let mut site = QuerySession::new();
    site.site.write("assets/cover.svg", "<svg/>");
    site.site.write("data/site.json", "{}");
    let items = site.completion("#image(\"asse|\")\n");
    let item = items
        .iter()
        .find(|i| i.label.ends_with("assets/cover.svg"))
        .expect("the cover image");
    let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
        panic!("a plain path edit: {:?}", item.text_edit);
    };
    assert_eq!(edit.range.start.character, 8);
    assert_eq!(edit.range.end.character, 12);
    assert_eq!(edit.new_text, "../assets/cover.svg");

    for marked in [
        "#read(\"da|\")\n",
        "#let read(text) = text\n#std.read(\"da|\")\n",
        "#read(\"da|\")\n#let read(text) = text\n",
    ] {
        let items = site.completion(marked);
        assert!(
            completion_labels(&items)
                .iter()
                .any(|l| l.ends_with("data/site.json")),
            "{marked}: {items:?}"
        );
    }

    for marked in ["#image(\"asse|\nBody", "#image(\"asse|\nBody \"quoted\""] {
        let items = site.completion(marked);
        let item = items
            .iter()
            .find(|i| i.label.ends_with("assets/cover.svg"))
            .expect("the unfinished cover path");
        let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
            panic!("a plain path edit: {:?}", item.text_edit);
        };
        assert_eq!(edit.range.end.line, 0, "{marked:?}");
        assert_eq!(edit.new_text, "../assets/cover.svg\"", "{marked:?}");
    }
}

#[test]
fn import_items_match_module_exports() {
    let pkg = tola_packages::builtin_package(&tola_packages::TolaPackage::Document.spec())
        .expect("the document package");
    let mut site = QuerySession::with_program(FAILED_PROGRAM);
    for marked in [
        "#import \"@tola/document:0.0.0\": p|",
        "#import \"@tola/document:0.0.0\": pl|",
        "#import \"@tola/document:0.0.0\": plain-t|",
        "#import \"@tola/document:0.0.0\":|",
        "#import \"@tola/document:0.0.0\": |",
        "#import \"@tola/document:0.0.0\": pl |",
        "#import \"@tola/document:0.0.0\": pl, |",
    ] {
        let items = site.completion(marked);
        let labels = completion_labels(&items);
        assert!(labels.contains(&"plain-text"), "{marked:?}: {labels:?}");
        assert!(
            labels
                .iter()
                .all(|l| *l == "*" || pkg.exports().any(|export| export == *l)),
            "{marked:?}: {labels:?}"
        );
        assert!(
            !labels.contains(&"map") && !labels.contains(&"mark"),
            "{marked:?}: {labels:?}"
        );
    }
}

#[test]
fn local_module_import_colon_offers_wildcard() {
    let mut site = QuerySession::with_program(FAILED_PROGRAM);
    site.site.write(
        "components/home.typ",
        "#let home-features() = [Features]\n#let home-hero() = [Hero]\n",
    );
    let items = site.completion("#import \"/components/home.typ\":|");
    let labels = completion_labels(&items);
    for label in ["*", "home-features", "home-hero"] {
        assert!(labels.contains(&label), "{label:?} missing from {labels:?}");
    }
}
