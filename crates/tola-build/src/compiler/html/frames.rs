//! Baseline alignment for inline frames in HTML export.
//!
//! For a frame of height H, baseline B (from its top), and text size S, Typst exports an SVG
//! of height H/S em. Its bottom initially sits on the surrounding text baseline. Raising it
//! by (B-H)/S em therefore aligns B with that baseline. The signed descent H-B comes from the
//! final layout, including text edges, paragraph leading, padding, and explicit baseline shifts.
//! Ink may overflow the viewport; its bounds do not determine baseline alignment.
//!
//! Typst 0.15.1 writes no such correction. [PR 8729] adds it to the encoder for every inline
//! frame; once a Typst release Tola adopts has it, delete this module and its call in
//! `crate::compiler::bundle`.
//!
//! [PR 8729]: https://github.com/typst/typst/pull/8729

use tola_typst::{BundleCancellation, BundleCompilation, HtmlFrameStyleError};

/// Align every inline frame with the surrounding text.
///
/// Returns how many frames changed.
pub(crate) fn align_inline_frames(
    compilation: &mut BundleCompilation,
    cancellation: &BundleCancellation,
) -> Result<usize, HtmlFrameStyleError> {
    compilation.style_html_frames(cancellation, |_, _, frame| {
        if !is_inline_level(frame) {
            return Ok(Vec::new());
        }
        let text_size = frame.text_size.to_pt();
        let shift = -frame.inner.descent().to_pt() / text_size;
        if text_size <= 0.0 || !shift.is_finite() {
            typst::diag::bail!(
                frame.span,
                "an inline frame cannot be aligned at this text size";
                hint: "Set a positive text size around the frame, such as `#set text(size: 11pt)`"
            );
        }
        if shift == 0.0 {
            return Ok(Vec::new());
        }
        Ok(vec![("vertical-align", format!("{shift}em"))])
    })
}

/// Whether a frame takes part in line layout.
///
/// Typst 0.15.1 marks block-level frames with `display` and leaves inline frames without it.
fn is_inline_level(frame: &typst_html::HtmlFrame) -> bool {
    !frame.css.iter().any(|property| property.name == "display")
}

#[cfg(test)]
mod tests {
    #[test]
    fn baseline_changes_only_inline_styles() {
        // The multiline equation is from https://github.com/typst/typst/issues/8516.
        // Its internal layout belongs to Typst; alignment must preserve that layout.
        let site = r#"
#document("native.html")[Plain *text*, $x^2$, and #link("https://example.test")[a link].]
#document("blocks.html")[
  #html.frame[First line.\ Second line.]
  #block(html.frame(rect(width: 1em, height: 1em)))
]
#document("paged.pdf", format: "pdf")[Text $x^2$ and $ 1 / x_2 $.]
#document("inline.html")[
  #box(html.frame[First line.\ Second line.])
  #box(html.frame(box(baseline: 1em, rect(width: 1em, height: 1em))))
  #box(html.frame($"(IBVP)" lr(\{#block($
    u_t + f(u)_x &= 0"," quad x in (a, b),\
    u(x, 0) &= u_0(x)
  $))$))
  #box(html.frame($"(IBVP)" lr(\{#box(baseline: 50%, $
    u_t + f(u)_x &= 0"," quad x in (a, b),\
    u(x, 0) &= u_0(x)
  $))$))
]
"#;
        let (_directory, realized, cancellation) = crate::compiler::tests::realized_site(site);
        let native = tola_typst::compile_bundle_world(&realized.world, &cancellation).unwrap();
        let before = native
            .export_entries(&Default::default(), &cancellation, None)
            .unwrap();
        let after = realized
            .compilation
            .export_entries(&Default::default(), &cancellation, None)
            .unwrap();
        for file in ["native.html", "blocks.html", "paged.pdf"] {
            let path = typst::syntax::VirtualPath::new(file).unwrap();
            assert!(
                before.get(&path).unwrap().bytes() == after.get(&path).unwrap().bytes(),
                "{file} must retain native output"
            );
        }
        let path = typst::syntax::VirtualPath::new("inline.html").unwrap();
        let before = std::str::from_utf8(before.get(&path).unwrap().bytes().as_slice()).unwrap();
        let after = std::str::from_utf8(after.get(&path).unwrap().bytes().as_slice()).unwrap();
        assert!(after.contains("vertical-align:"));
        let svgs = |html: &str| {
            html.split("<svg ")
                .skip(1)
                .map(|svg| format!("<svg {}</svg>", svg.split_once("</svg>").unwrap().0))
                .collect::<Vec<_>>()
        };
        let before = svgs(before);
        let after = svgs(after);
        assert_eq!(before.len(), 4);
        assert_eq!(before.len(), after.len());
        for (before, after) in before.iter().zip(&after) {
            assert_eq!(
                before.split_once('>').unwrap().1,
                after.split_once('>').unwrap().1
            );
            let before = roxmltree::Document::parse(before).unwrap();
            let after = roxmltree::Document::parse(after).unwrap();
            let attributes = |svg: roxmltree::Node<'_, '_>| {
                svg.attributes()
                    .filter(|attribute| attribute.name() != "style")
                    .map(|attribute| (attribute.name().to_owned(), attribute.value().to_owned()))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                attributes(before.root_element()),
                attributes(after.root_element())
            );
        }
    }

    #[test]
    fn aligned_math_keeps_equation_references() {
        let site = r#"
#import "@tola/web:0.0.0": math-svg
#show math.equation: math-svg
#set text(size: 18pt)
#document("index.html")[
  Inline $1 / x_2$ and $y^2$.
  #set math.equation(numbering: "(1)", alt: "Squared x")
  $ x^2 $ <numbered>
  #link(<numbered>)[See equation]
  #context [Count: #counter(math.equation).final().first()]
]
"#;
        let (_directory, realized, cancellation) = crate::compiler::tests::realized_site(site);
        let path = typst::syntax::VirtualPath::new("index.html").unwrap();
        let inventory = realized
            .compilation
            .document(&path)
            .unwrap()
            .html_inventory(&cancellation)
            .unwrap()
            .unwrap();
        assert!(
            inventory
                .fragments()
                .iter()
                .any(|fragment| fragment.value() == "numbered")
        );
        let outputs = realized
            .compilation
            .export_entries(&tola_typst::BundleOptions::default(), &cancellation, None)
            .unwrap();
        let html = std::str::from_utf8(outputs.get(&path).unwrap().bytes().as_slice()).unwrap();
        assert!(html.contains("vertical-align: -"), "{html}");
        assert!(html.contains("href=\"#numbered\""), "{html}");
        assert!(html.contains("Count: 1"), "{html}");
        assert!(html.contains("aria-label=\"Squared x\""), "{html}");
    }
}
