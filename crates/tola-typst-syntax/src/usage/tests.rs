use std::cell::Cell;
use std::collections::HashMap;

use typst_syntax::Source;

use super::*;
use crate::names::SelectedNames;

fn analyzed_with(text: &str, selected: &SelectedInterfaces) -> (SourceNames, Liveness) {
    let names = SourceNames::new(Source::detached(text.to_owned()));
    let live = names
        .liveness(selected, || false)
        .expect("an uncancelled analysis");
    (names, live)
}

fn liveness(text: &str) -> Liveness {
    analyzed_with(text, &SelectedInterfaces::default()).1
}

fn unused_names(names: &SourceNames, live: &Liveness) -> Vec<String> {
    live.unused_bindings
        .iter()
        .map(|&decl| names.declarations()[decl].name.clone())
        .collect()
}

fn dead_offsets(live: &Liveness) -> Vec<usize> {
    live.dead_stores
        .iter()
        .map(|store| store.range.start)
        .collect()
}

#[test]
fn liveness_unused_bindings() {
    for (label, text, exports_read, expected) in [
        (
            "shadowed",
            "#let x = 1\n#let x = 2\n#x\n",
            false,
            &["x"][..],
        ),
        (
            "after return",
            "#let f() = {\n  let y = 1\n  return\n  y\n}\n#f()\n",
            false,
            &["y"][..],
        ),
        (
            "self recursive",
            "#let f() = {\n  f()\n}\n",
            false,
            &["f"][..],
        ),
        ("write only", "#let x = 0\n#(x = 1)\n", false, &["x"][..]),
        (
            "loop binding",
            "#for item in (1, 2) {\n}\n",
            false,
            &["item"][..],
        ),
        (
            "destruct field target",
            "#let d = (k: 1)\n#let other = 0\n#((d.k, other) = (2, 3))\n",
            false,
            &["other"][..],
        ),
        (
            "shadowed export",
            "#let value = 1\n#let value = 2\n",
            true,
            &["value"][..],
        ),
    ] {
        let selected = SelectedInterfaces {
            unresolved: exports_read,
            ..SelectedInterfaces::default()
        };
        let (names, live) = analyzed_with(text, &selected);
        assert_eq!(unused_names(&names, &live), expected, "{label}");
    }
}

#[test]
fn liveness_dead_stores() {
    for (label, text, dead) in [
        (
            "overwritten",
            "#let x = 0\n#(x = 1)\n#(x = 2)\n#x\n",
            &["x = 0", "x = 1"][..],
        ),
        (
            "branch overwrite",
            "#let x = 0\n#if true {\n  x = 1\n} else {\n  x = 2\n}\n#x\n",
            &["x = 0"][..],
        ),
        (
            "iteration local",
            "#let x = 0\n#for i in (1, 2) {\n  x = i\n  x = i + 1\n}\n#x\n",
            &["x = i\n"][..],
        ),
        (
            "closure capture at creation",
            "#let x = 0\n#let f = () => x\n#(x = 1)\n",
            &["x = 1"][..],
        ),
        (
            "context capture at creation",
            "#let x = 0\n#let c = context x\n#(x = 1)\n",
            &["x = 1"][..],
        ),
        (
            "parameter default at creation",
            "#let x = 0\n#let f = (value: x) => value\n#(x = 1)\n",
            &["x = 1"][..],
        ),
        (
            "short circuit read",
            "#let x = 0\n#let y = false and x > 0\n#(x = 1)\n",
            &["x = 1"][..],
        ),
        (
            "default writes precede captures",
            "#let value = 0\n#let f = (p: { value = 1; none }) => value\n#f()\n",
            &["value = 0"][..],
        ),
        (
            "assignment reads after its value",
            "#let value = 0\n#(value += { value = 3; 2 })\n#value\n",
            &["value = 0"][..],
        ),
        (
            "destructuring reads follow each target",
            "#let index = 0\n#let values = (0, 0)\n#((index, values.at(index)) = (1, 2))\n#values\n",
            &["index = 0"][..],
        ),
        (
            "captured compound write",
            "#let x = 0\n#let f = () => {\n  x += 1\n}\n#f()\n#(x = 2)\n",
            &["x = 2"][..],
        ),
    ] {
        let expected: Vec<usize> = dead
            .iter()
            .map(|needle| text.find(needle).unwrap())
            .collect();
        assert_eq!(dead_offsets(&liveness(text)), expected, "{label}");
    }
}

#[test]
fn liveness_live_read_paths() {
    for text in [
        "#let x = 0\n#if true { x } else { x = 1 }\n#x\n",
        "#let x = 0\n#for i in (1, 2) {\n  x\n  x = i\n}\n#x\n",
        "#let x = 0\n#for i in (1, 2) {\n  break\n  x = 1\n}\n#x\n",
        "#let x = 0\n#let f = () => {\n  x = 1\n}\n#x\n",
    ] {
        assert!(liveness(text).dead_stores.is_empty(), "{text}");
    }
}

#[test]
fn liveness_skips_non_local_bindings() {
    for text in [
        "#let f() = {\n  f()\n}\n#f()\n",
        "#import \"lib.typ\": item\n",
        "#let f(parameter) = 1\n#f(1)\n",
    ] {
        let live = liveness(text);
        assert!(live.unused_bindings.is_empty(), "{text}");
        assert!(live.dead_stores.is_empty(), "{text}");
    }
}

#[test]
fn liveness_respects_selected_exports() {
    let text = "#let x = 0\n#(x = 1)\n";
    let names = SourceNames::new(Source::detached(text.to_owned()));
    let unselected = names
        .liveness(&SelectedInterfaces::default(), || false)
        .expect("an uncancelled analysis");
    assert_eq!(unselected.unused_bindings, [0]);
    assert!(unselected.dead_stores.is_empty());

    let mut sel_names = SelectedNames::default();
    sel_names.names.insert("x".to_owned());
    let selected = SelectedInterfaces {
        unresolved: false,
        selected: HashMap::from([(names.source().id(), sel_names)]),
    };
    let licensed = names
        .liveness(&selected, || false)
        .expect("an uncancelled analysis");
    assert!(licensed.unused_bindings.is_empty());
    assert_eq!(dead_offsets(&licensed), [text.find("x = 0").unwrap()]);
}

#[test]
fn liveness_cancellation() {
    let names = SourceNames::new(Source::detached("#let x = 1\n#x\n".repeat(512)));
    let calls = Cell::new(0usize);
    let midway = || {
        calls.set(calls.get() + 1);
        calls.get() >= 3
    };
    assert!(
        names
            .liveness(&SelectedInterfaces::default(), midway)
            .is_none()
    );
}

#[test]
fn liveness_skips_broken_syntax() {
    for text in [
        "#let x = 1\n#let broken = (]\n",
        "#let before = 0\n#for i in { (); break } {\n}\n#before\n",
        "#let before = 0\n#while { break } {\n}\n#before\n",
    ] {
        let live = liveness(text);
        assert!(live.unused_bindings.is_empty(), "{text}");
        assert!(live.dead_stores.is_empty(), "{text}");
    }
}
