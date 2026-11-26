use typst_syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};

use super::*;

fn declaration(marked: &str) -> (SourceNames, usize) {
    let cursor = marked.find('|').unwrap();
    let index = detached_index(&marked.replace('|', ""));
    let decl = index.declared_at(cursor).expect("bound name");
    (index, decl)
}

fn detached_index(text: &str) -> SourceNames {
    SourceNames::new(Source::detached(text))
}

fn file(path: &str) -> FileId {
    RootedPath::new(VirtualRoot::Project, VirtualPath::new(path).unwrap()).intern()
}

fn project(path: &str, text: &str) -> SourceNames {
    SourceNames::new(Source::new(file(path), text.to_owned()))
}

#[test]
fn unread_skips_wildcard_import() {
    let index = detached_index("#import \"lib.typ\": *\n");
    assert!(index.unread_imports().is_empty());
}

#[test]
fn unread_reports_shadowed_import() {
    let index = detached_index("#import \"lib.typ\": value\n#let value = 1\n#value\n");
    let unread = index.unread_imports();
    assert_eq!(unread.len(), 1, "{unread:?}");
    assert_eq!(unread[0].names, ["value"]);
}

#[test]
fn selected_records_import_items() {
    let target = project("lib.typ", "");
    let page = project("page.typ", "#import \"lib.typ\": a.b as c, d\n");
    let interfaces = selected_interfaces([&page], |_, path| {
        (path == "lib.typ").then_some(target.source().id())
    });
    assert!(!interfaces.unresolved);
    let selected = &interfaces.selected[&target.source().id()];
    assert!(!selected.whole);
    assert_eq!(selected.names.iter().collect::<Vec<_>>(), ["a", "d"]);
}

#[test]
fn reportable_unread_respects_selection() {
    let src = project("lib.typ", "#import \"other.typ\": dropped\n");

    let reported = src.reportable_unread_imports(&SelectedInterfaces::default());
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert_eq!(reported[0].names, ["dropped"]);

    let mut selected = SelectedInterfaces::default();
    selected
        .selected
        .insert(src.source().id(), SelectedNames::default());
    selected
        .selected
        .get_mut(&src.source().id())
        .unwrap()
        .names
        .insert("dropped".to_owned());
    assert!(
        src.reportable_unread_imports(&selected).is_empty(),
        "a selected name reaches the import"
    );

    let unresolved = SelectedInterfaces {
        unresolved: true,
        ..SelectedInterfaces::default()
    };
    assert!(
        src.reportable_unread_imports(&unresolved).is_empty(),
        "an unresolved source may reach any name"
    );
}

#[test]
fn decl_parameter_patterns() {
    for marked in [
        "#let f(pos) = pos|",
        "#let f(named: 1) = named|",
        "#let f((left, (right, ..rest))) = right|",
        "#let f(..args) = args|",
        "#let f = ((x, y), flag: false, ..rest) => rest|",
    ] {
        let (index, id) = declaration(marked);
        assert!(
            matches!(index.declarations()[id].kind, DeclarationKind::Parameter),
            "{marked}"
        );
    }
}

#[test]
fn resolve_recursive_fn() {
    let marked = "#let recurse(n) = if n > 0 { recurse|(n - 1) }";
    let (index, id) = declaration(marked);
    let at = marked.find("recurse").unwrap();
    assert_eq!(index.declarations()[id].range, at..at + "recurse".len());
}

#[test]
fn resolve_inner_shadow_ends() {
    let marked = "#let value = 1\n#{ let value = 2; value }\n#value|";
    let (index, id) = declaration(marked);
    let outer = marked.find("value").unwrap();
    assert_eq!(index.declarations()[id].range, outer..outer + "value".len());
}
