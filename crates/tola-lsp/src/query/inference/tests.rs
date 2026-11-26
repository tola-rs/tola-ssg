use std::cell::Cell;

use tola_build::cancellation::{BuildCancellation, BuildCancelled, BuildCanceller};
use tola_typst::typst::syntax::Source;
use tola_typst_syntax::names::SourceNames;

use super::binding_is_written;
use crate::query::source_member_completion;
use crate::query::tests::*;

#[test]
fn destructured_bindings_project_values() {
    for marked in [
        "#let (value,) = ((current: 1),)\n#value.|",
        "#let (ignored, value) = ((wrong: 1), (current: 1))\n#value.|",
        "#let (..rest, value) = (0, (wrong: 1), (current: 1))\n#value.|",
        "#let (ignored, ..rest) = (0, (current: 1), (extra: 1))\n#let (value, ..) = rest\n#value.|",
        "#let ((value,),) = (((current: 1),),)\n#value.|",
        "#let (value,) = (value: (current: 1))\n#value.|",
        "#let (slot: (value,),) = (slot: ((current: 1),))\n#value.|",
        "#let (ignored, ..rest) = (ignored: 0, current: 1)\n#rest.|",
        "#let make() = ((current: 1),)\n#let (value,) = make()\n#value.|",
    ] {
        let pos = marked.find('|').unwrap();
        let src = Source::detached(marked.replace('|', ""));
        let names = SourceNames::new(src.clone());
        let cold = source_member_completion(None, &names, &src, pos, &BuildCancellation::new())
            .unwrap()
            .unwrap_or_default();
        for items in [
            cold,
            QuerySession::with_program(FAILED_PROGRAM)
                .completion(&format!("#panic(\"page\")\n{marked}")),
        ] {
            let labels = completion_labels(&items);
            assert!(labels.contains(&"current"), "{marked}: {labels:?}");
            for absent in ["wrong", "flatten", "slot", "ignored"] {
                assert!(!labels.contains(&absent), "{marked}: {labels:?}");
            }
        }
    }
}

#[test]
fn method_call_active_parameter() {
    let mut site = QuerySession::with_program(FAILED_PROGRAM);
    let sig = site
        .signature(
            "#panic(\"page\")\n#let f(x) = (current: x.len())\n#f(\"test\").current.bit-lshift(|)",
        )
        .expect("typed member signature");
    assert!(
        sig.signatures[0]
            .label
            .starts_with("f(\"test\").current.bit-lshift("),
        "{sig:?}"
    );
    assert!(!sig.signatures[0].label.contains("self"), "{sig:?}");
    let params = sig.signatures[0]
        .parameters
        .as_ref()
        .expect("method parameters");
    assert_eq!(params.len(), 1, "{sig:?}");
    assert_eq!(
        params[0].label,
        lsp_types::ParameterLabel::Simple("shift:".into()),
        "{sig:?}"
    );
    assert_eq!(sig.signatures[0].active_parameter, Some(0), "{sig:?}");
}

#[test]
fn mutation_scan_cancellation() {
    let src = Source::detached("#let value = (old: 1)\n#let unrelated = 1\n#value = (current: 1)");
    let names = SourceNames::new(src);
    let canceller = BuildCanceller::new();
    let cancel = canceller.token();
    let visits = Cell::new(0);
    let err = binding_is_written(
        &names,
        0,
        names.source().text().len(),
        |_| false,
        || {
            visits.set(visits.get() + 1);
            if visits.get() == 3 {
                canceller.cancel();
            }
            cancel.ensure_active()?;
            Ok(())
        },
    )
    .unwrap_err();
    assert!(err.is::<BuildCancelled>());
}

#[test]
fn unproven_patterns_stay_unknown() {
    for marked in [
        "#let (value,) = unknown\n#value.|",
        "#let (value, missing) = ((current: 1),)\n#value.|",
        "#let (value, missing) = (value: (current: 1))\n#value.|",
    ] {
        let pos = marked.find('|').unwrap();
        let src = Source::detached(marked.replace('|', ""));
        let names = SourceNames::new(src.clone());
        assert!(
            source_member_completion(None, &names, &src, pos, &BuildCancellation::new())
                .unwrap()
                .is_none(),
            "{marked}"
        );
        let items = QuerySession::with_program(FAILED_PROGRAM)
            .completion(&format!("#panic(\"page\")\n{marked}"));
        let labels = completion_labels(&items);
        assert!(!labels.contains(&"current"), "{marked}: {labels:?}");
    }
}

#[test]
fn rest_parameters_capture_arguments() {
    let mut site = QuerySession::with_program(FAILED_PROGRAM);
    let completions = site.completion(
        "#panic(\"page\")\n#let capture(used: 0, ..args) = args\n#capture(1, used: 2, extra: 3).|",
    );
    let labels = completion_labels(&completions);
    for expected in ["pos", "named", "extra", "map", "filter"] {
        assert!(labels.contains(&expected), "{labels:?}");
    }
    for absent in ["join", "used"] {
        assert!(!labels.contains(&absent), "{labels:?}");
    }
}
