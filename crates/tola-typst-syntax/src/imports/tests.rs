use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use typst_library::foundations::PathOrStr;
use typst_syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};

use super::*;

fn file(path: &str) -> FileId {
    RootedPath::new(VirtualRoot::Project, VirtualPath::new(path).unwrap()).intern()
}

fn graph(sources: &[(&str, &str)]) -> NameGraph {
    proven(sources, &[])
}

fn proven(sources: &[(&str, &str)], targets: &[(&str, &str, &str)]) -> NameGraph {
    let texts: Vec<(FileId, &str)> = sources
        .iter()
        .map(|&(path, text)| (file(path), text))
        .collect();
    NameGraph::new(
        sources.iter().map(|(path, text)| {
            Arc::new(SourceNames::new(Source::new(file(path), (*text).into())))
        }),
        |query| match query {
            ImportQuery::Path { file, path } => PathOrStr::Str(path.into())
                .resolve(file)
                .ok()
                .map(|path| path.intern()),
            ImportQuery::Expression {
                file: holder,
                range,
            } => {
                let text = texts
                    .iter()
                    .find(|(id, _)| *id == holder)
                    .map(|(_, text)| *text)?;
                targets
                    .iter()
                    .find(|&&(stmt, expr, _)| file(stmt) == holder && &text[range.clone()] == expr)
                    .map(|&(_, _, target)| file(target))
            }
        },
        None,
        || Ok::<(), std::convert::Infallible>(()),
    )
    .unwrap()
}

fn binding_at(graph: &NameGraph, path: &str, marker: &str) -> BindingId {
    let id = file(path);
    let at = graph
        .source(id)
        .unwrap()
        .source()
        .text()
        .find(marker)
        .unwrap();
    graph.selected(id, at).unwrap().binding.unwrap()
}

fn renamed(graph: &NameGraph, binding: BindingId, name: &str) -> HashMap<FileId, String> {
    let mut srcs: HashMap<_, _> = graph
        .sources()
        .map(|src| (src.source().id(), src.source().text().to_owned()))
        .collect();
    let mut edits = graph.rename(binding, name).unwrap();
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
    for edit in edits {
        srcs.get_mut(&edit.file)
            .unwrap()
            .replace_range(edit.range, &edit.replacement);
    }
    srcs
}

#[test]
fn exports_cancel_mid_scan() {
    let lib: String = (0..32).map(|i| format!("#let alpha{i} = 1\n")).collect();
    let graph = graph(&[
        ("lib.typ", &lib),
        ("main.typ", "#import \"lib.typ\" as library\n#library"),
    ]);
    let binding = binding_at(&graph, "main.typ", "library");
    for module in [false, true] {
        let mut checked = 0;
        let mut check = || {
            checked += 1;
            if checked >= 8 {
                Err("interrupted")
            } else {
                Ok(())
            }
        };
        let err = if module {
            graph
                .module_exports_matching(file("main.typ"), 0, &[], "a", &mut check)
                .err()
                .expect("interrupted enumeration")
        } else {
            graph
                .binding_exports(binding, "a", &mut check)
                .err()
                .expect("interrupted enumeration")
        };
        assert_eq!(err, "interrupted");
    }
}

#[test]
fn module_exports_list_items() {
    let graph = graph(&[
        ("lib.typ", "#let greet() = [hello]\n#let api = 1\n"),
        ("main.typ", "#import \"lib.typ\": greet\n#greet()"),
    ]);
    let exports = graph.module_exports(file("main.typ"), 0);
    assert!(exports.contains(&("greet".to_owned(), NameClass::Function)));
    assert!(exports.contains(&("api".to_owned(), NameClass::Value)));
}

#[test]
fn rename_import_inserts_alias() {
    let graph = graph(&[
        ("lib.typ", "#let greet() = [hello]"),
        ("main.typ", "#import \"lib.typ\": greet\n#greet()"),
    ]);
    let text = renamed(&graph, binding_at(&graph, "main.typ", "greet()"), "welcome");
    assert_eq!(
        text[&file("main.typ")],
        "#import \"lib.typ\": greet as welcome\n#welcome()"
    );
    assert_eq!(text[&file("lib.typ")], "#let greet() = [hello]");
}

#[test]
fn math_field_skips_lexical_rename() {
    let graph = graph(&[("main.typ", "#let r = 10\n#r\n$arrow.r$")]);
    let binding = binding_at(&graph, "main.typ", "r = 10");
    let src = graph.source(file("main.typ")).unwrap().source().text();
    let refs: Vec<_> = graph
        .references(binding, true)
        .into_iter()
        .filter(|(id, _)| *id == file("main.typ"))
        .map(|(_, occ)| occ.range.start)
        .collect();
    assert_eq!(
        refs,
        [src.find("r = 10").unwrap(), src.find("#r").unwrap() + 1]
    );
    let text = renamed(&graph, binding, "width");
    assert_eq!(
        text[&file("main.typ")],
        "#let width = 10\n#width\n$arrow.r$"
    );
}

#[test]
fn rename_rejects_capture() {
    for (src, marker, name) in [
        ("#let old = 1\n#let taken = 2\n#old", "old", "taken"),
        ("#let old = 1\n#{ let taken = 2; old }", "old", "taken"),
        ("#let old = 1\n#taken\n#old", "old", "taken"),
        (
            "#let render(size: 2, ..rest) = size\n$render(size: 3, width: 4)$",
            "size",
            "width",
        ),
    ] {
        let graph = graph(&[("main.typ", src)]);
        assert!(
            matches!(
                graph.rename(binding_at(&graph, "main.typ", marker), name),
                Err(RenameError::Conflict(_))
            ),
            "{src}"
        );
    }
}

#[test]
fn rename_rejects_invalid_ident() {
    let graph = graph(&[("main.typ", "#let old = 1\n#old")]);
    let binding = binding_at(&graph, "main.typ", "old");
    for name in ["", "let", "_", "two words", "x.y", "1name"] {
        assert!(
            matches!(
                graph.rename(binding, name),
                Err(RenameError::InvalidIdentifier(_))
            ),
            "{name}"
        );
    }
}

#[test]
fn rename_param_updates_call_labels() {
    for (src, expected) in [
        (
            "#let render(size: 2) = size\n#render(size: 3)",
            "#let render(width: 2) = width\n#render(width: 3)",
        ),
        // Multi-letter callee parses as a math call; single letter stays math text beside a group.
        (
            "#let render(size: 2) = size\n$render(size: 3)$",
            "#let render(width: 2) = width\n$render(width: 3)$",
        ),
    ] {
        let graph = graph(&[("main.typ", src)]);
        let text = renamed(&graph, binding_at(&graph, "main.typ", "size"), "width");
        assert_eq!(text[&file("main.typ")], expected);
    }
}

#[test]
fn references_skip_shadowed_scope() {
    let graph = graph(&[
        ("lib.typ", "#let greet() = [hello]"),
        (
            "main.typ",
            "#import \"lib.typ\": greet\n#{ let greet = 1; greet }\n#greet()",
        ),
    ]);
    let refs = graph.references(binding_at(&graph, "lib.typ", "greet"), false);
    let uses: Vec<_> = refs
        .into_iter()
        .filter(|(id, _)| *id == file("main.typ"))
        .map(|(_, occ)| occ.range.start)
        .collect();
    assert_eq!(
        uses,
        [graph
            .source(file("main.typ"))
            .unwrap()
            .source()
            .text()
            .rfind("greet")
            .unwrap()]
    );
}

#[test]
fn pending_dynamic_import() {
    let srcs = [
        (
            "main.typ",
            "#let base = \"lib\"\n#import base + \".typ\": greet\n#greet",
        ),
        ("lib.typ", "#let greet = [hello]"),
    ];
    let graph = proven(&srcs, &[]);
    let main = file("main.typ");
    let stmt = PendingImport {
        file: main,
        statement: 0,
    };
    let text = srcs[0].1;
    let binding = binding_at(&graph, "main.typ", "greet");
    assert!(graph.definition(binding).is_none());
    assert_eq!(
        graph.unresolved_imports(main, text.find("greet").unwrap()),
        [stmt],
        "the item"
    );
    assert_eq!(
        graph.unresolved_imports(main, text.rfind("greet").unwrap()),
        [stmt],
        "the use"
    );
    assert_eq!(
        graph.unresolved_imports(file("lib.typ"), srcs[1].1.find("greet").unwrap()),
        [stmt],
        "the declaration the consumer may reach"
    );

    let named = [("main.typ", "#let api = [x]\n#import api: greet\n#greet")];
    let graph = proven(&named, &[]);
    assert_eq!(
        graph.unresolved_imports(main, named[0].1.rfind("greet").unwrap()),
        [PendingImport {
            file: main,
            statement: 0
        }]
    );
}

#[test]
fn resolved_dynamic_import() {
    for consumer in [
        "#let base = \"lib\"\n#import base + \".typ\": greet\n#greet",
        "#let base = \"lib\"\n#import base + \".typ\": *\n#greet",
    ] {
        let srcs = [("main.typ", consumer), ("lib.typ", "#let greet = [hello]")];
        let main = file("main.typ");
        let lib = file("lib.typ");
        let use_at = consumer.rfind("greet").unwrap();
        assert_eq!(
            proven(&srcs, &[]).unresolved_imports(main, use_at),
            [PendingImport {
                file: main,
                statement: 0
            }],
            "{consumer}"
        );
        let graph = proven(&srcs, &[("main.typ", "base + \".typ\"", "lib.typ")]);
        assert!(
            graph.unresolved_imports(main, use_at).is_empty(),
            "{consumer}"
        );
        let use_binding = graph.selected(main, use_at).unwrap().binding.unwrap();
        assert_eq!(graph.definition(use_binding).unwrap().0, lib, "{consumer}");
        let decl = binding_at(&graph, "lib.typ", "greet");
        let refs = graph.references(decl, true);
        let files: HashSet<_> = refs.iter().map(|(file, _)| *file).collect();
        assert_eq!(files, HashSet::from([main, lib]), "{consumer}");
        assert!(
            refs.iter()
                .any(|(file, occ)| *file == main && occ.range.start == use_at),
            "{consumer}"
        );
    }
}

#[test]
fn rename_rejects_unknown_consumer() {
    let srcs = [
        (
            "main.typ",
            "#let base = \"lib\"\n#import base + \".typ\": greet\n#greet",
        ),
        (
            "alias.typ",
            "#let base = \"lib\"\n#import base + \".typ\": greet as local\n#local",
        ),
        ("lib.typ", "#let greet = [hello]"),
    ];
    let graph = proven(&srcs, &[]);
    let decl = binding_at(&graph, "lib.typ", "greet");
    assert!(matches!(
        graph.rename(decl, "welcome"),
        Err(RenameError::UnknownImport(file)) if file == "alias.typ"
    ));
}

#[test]
fn rename_updates_proven_consumer() {
    for (consumer, expected) in [
        (
            "#let base = \"lib\"\n#import base + \".typ\": greet\n#greet",
            "#let base = \"lib\"\n#import base + \".typ\": welcome\n#welcome",
        ),
        (
            "#let base = \"lib\"\n#import base + \".typ\": *\n#greet",
            "#let base = \"lib\"\n#import base + \".typ\": *\n#welcome",
        ),
    ] {
        let graph = proven(
            &[("main.typ", consumer), ("lib.typ", "#let greet = [hello]")],
            &[("main.typ", "base + \".typ\"", "lib.typ")],
        );
        let text = renamed(&graph, binding_at(&graph, "lib.typ", "greet"), "welcome");
        assert_eq!(text[&file("main.typ")], expected);
        assert_eq!(text[&file("lib.typ")], "#let welcome = [hello]");
    }
}

#[test]
fn rename_rejects_package_decl() {
    let id = RootedPath::new(
        VirtualRoot::Package("@preview/demo:0.1.0".parse().unwrap()),
        VirtualPath::new("lib.typ").unwrap(),
    )
    .intern();
    let graph = NameGraph::new(
        [Arc::new(SourceNames::new(Source::new(
            id,
            "#let greet() = [hello]".into(),
        )))],
        |_| None,
        None,
        || Ok::<(), std::convert::Infallible>(()),
    )
    .unwrap();
    let at = graph
        .source(id)
        .unwrap()
        .source()
        .text()
        .find("greet")
        .unwrap();
    let binding = graph.selected(id, at).unwrap().binding.unwrap();
    assert!(matches!(
        graph.rename(binding, "welcome"),
        Err(RenameError::ReadOnly)
    ));
}
