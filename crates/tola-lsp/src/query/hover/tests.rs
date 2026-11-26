use tola_typst::typst::syntax::Source;

use crate::position;
use crate::query::tests::*;

#[test]
fn parameter_hover_observed_types() {
    for (first, second, expected) in [
        ("1", "\"text\"", "int | str"),
        ("x => x", "\"text\"", "function | str"),
        ("x => x", "(x, y) => x + y", "function"),
        ("std", "math", "module"),
    ] {
        let mut site = QuerySession::new();
        let calls = format!("\n#render({first})\n#render({second})\n");
        let hover = site
            .hover_text(&format!(
                "#let render(value) = {{ let kept = val|ue; none }}{calls}",
            ))
            .expect("the observed parameter has a hover");
        assert!(
            hover.contains(&format!("let value = {expected};")),
            "{hover}"
        );
        let sig = site
            .hover_text(&format!(
                "#let ren|der(value) = {{ let kept = value; none }}{calls}",
            ))
            .expect("the function has a hover");
        assert!(sig.contains(&format!("value: {expected}")), "{sig}");
    }
}

#[test]
fn nonvalue_positions_return_no_hover() {
    let mut site = QuerySession::new();
    for marked in [
        "#let value = 1\n#value   | \n",
        "#let value = 1\n|    \n",
        "#let page = (tit|le: \"Hello\", text: \"Body\")\n",
    ] {
        assert!(site.hover(marked).is_none(), "{marked:?}");
    }
}

#[test]
fn imported_hover_range() {
    let mut site = QuerySession::new();
    for marked in [
        "#import \"@tola/address:0.0.0\": out|put-of\n",
        "🦊 #import \"@tola/address:0.0.0\": |output-of as address\n",
    ] {
        let hover = site.hover(marked).expect("an imported export's hover");
        let src = Source::detached(marked.replace('|', ""));
        let range = hover.range.expect("the selected name's range");
        let start = position::byte_offset(src.lines(), range.start).unwrap();
        let end = position::byte_offset(src.lines(), range.end).unwrap();
        assert_eq!(&src.text()[start..end], "output-of");
    }
}

#[test]
fn repaired_reference_label_only() {
    let mut site = QuerySession::new();
    let hover = site
        .hover_text("#let f(..xs) = xs.len()\n#f(<nope|)\n")
        .expect("a hover about the label");
    assert!(hover.contains("declares the label `nope`"), "{hover}");
    assert!(!hover.contains("Sampled"), "{hover}");
}

#[test]
fn markup_embedded_binding_hover() {
    let mut site = QuerySession::new();
    for marked in [
        "#let v = 1\nMarkup with #|v inside\n",
        "#let v = 1\nMarkup with #v| inside\n",
    ] {
        let hover = site
            .hover_text(marked)
            .unwrap_or_else(|| panic!("{marked:?} asks about `v`"));
        assert!(
            hover.contains("```typc\nlet v = int;\n```"),
            "{marked:?} answered {hover}"
        );
    }
}

#[test]
fn asset_url_hover_and_completion() {
    let mut site = QuerySession::with_published_assets();
    let items = site.completion(
        "#import \"@tola/host:0.0.0\": asset-url\n#metadata(asset-url(\"/brand/|\"))\n",
    );
    assert_eq!(completion_labels(&items), ["/brand/logo.svg"]);

    let hover = site
        .hover_text(
            "#import \"@tola/host:0.0.0\": asset-url\n#metadata(asset-url(\"/brand/logo.sv|g\"))\n",
        )
        .expect("a asset-url argument answers");
    assert!(hover.contains("published at"), "{hover}");
    assert!(hover.contains("/brand/logo.svg"), "{hover}");
}

#[test]
fn library_callee_signatures_and_shadowing() {
    let mut site = QuerySession::new();
    let dead = site
        .hover_text("#let dead() = [#t|ext(size: 10pt)[x]]\n")
        .expect("a hover for the library callee");
    assert!(dead.starts_with("```typc\nlet text("), "{dead}");

    let math = site
        .hover_text("$sqr|t(4)$")
        .expect("a hover for a math function");
    assert!(math.contains("let sqrt("), "{math}");

    let shadowed = site
        .hover_text("#let sqrt(x) = x\n$sqr|t(4)$")
        .expect("a hover for the local function");
    assert!(shadowed.contains("sqrt"), "{shadowed}");
    assert!(!shadowed.contains("A square root."), "{shadowed}");
}
