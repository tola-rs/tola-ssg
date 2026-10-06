//! Tola's typed adapter over Typst's synthesized code highlighting.
//!
//! The adapter reads the `raw` element Typst already synthesized and replaces each line's body
//! with runs carrying the resolved styles of up to two appearances as local CSS custom
//! properties. Text, line numbers, and line spans stay exactly as Typst synthesized them, and the
//! optional dark appearance is a clone of the same element synthesized through the public
//! `Synthesize` capability, never a second classification.

use std::collections::BTreeMap;

use typst::comemo::Tracked;
use typst::diag::{At, EcoString, SourceResult, bail};
use typst::engine::Engine;
use typst::foundations::{
    Content, Context, Dict, NativeElement, Packed, SequenceElem, Smart, Str, StyleChain,
    StyledElem, Synthesize, func,
};
use typst::layout::BlockElem;
use typst::model::{EmphElem, StrongElem};
use typst::syntax::Span;
use typst::text::{LinebreakElem, RawElem, RawLine, TextElem, UnderlineElem};
use typst_html::{HtmlAttr, HtmlAttrs, HtmlElem, attr, tag};

/// The class every rendered code element carries.
const CODE_CLASS: &str = "tola-code";

/// The attribute carrying the author's language tag.
const LANG_ATTRIBUTE: &str = "data-lang";

/// The custom properties one run carries, primary appearance first.
const COLOR_PROPERTY: &str = "--tola-code-color";
const WEIGHT_PROPERTY: &str = "--tola-code-weight";
const STYLE_PROPERTY: &str = "--tola-code-style";
const DECORATION_PROPERTY: &str = "--tola-code-decoration";
const DARK_COLOR_PROPERTY: &str = "--tola-code-dark-color";
const DARK_WEIGHT_PROPERTY: &str = "--tola-code-dark-weight";
const DARK_STYLE_PROPERTY: &str = "--tola-code-dark-style";
const DARK_DECORATION_PROPERTY: &str = "--tola-code-dark-decoration";

/// The token an appearance declares for a style it does not resolve.
///
/// `none` is a valid custom-property token stream but invalid for `color`, `font-weight`, and
/// `font-style`: `var()` substitution makes those declarations invalid at computed-value time, so
/// they compute to the inherited value, and `text-decoration-line` accepts it to clear a
/// decoration. A CSS-wide keyword such as `inherit` cannot serve here — it would act on the custom
/// property itself, and the consumer's fallback would resolve the other appearance's value.
const OFF_TOKEN: &str = "none";

/// The `render-code` host binding: render one synthesized `raw` element as Tola's code markup.
#[func(contextual)]
pub(super) fn tola_render_code(
    engine: &mut Engine,
    context: Tracked<Context>,
    /// The raw element the author's show rule replaced.
    raw: Content,
    /// The theme carrier for the site's dark appearance, if one is configured.
    dark: Option<Content>,
    /// HTML attributes for the element Tola renders code into.
    attrs: Dict,
) -> SourceResult<Content> {
    let span = raw.span();
    let styles = context.styles().at(span)?;
    let element = raw
        .to_packed::<RawElem>()
        .ok_or("`render-code` needs the raw element the show rule replaced")
        .at(span)?;

    // `raw.theme: none` disables highlighting entirely, the dark appearance included, so the
    // element goes back to Typst's own display rule unchanged.
    if matches!(element.theme.get_ref(styles), Smart::Custom(None)) {
        return Ok(raw);
    }

    let lines = element.lines.as_deref().unwrap_or_default();
    let dark = match dark {
        Some(carrier) => Some(synthesize_dark(engine, &raw, &carrier, styles, span)?),
        None => None,
    };
    let dark_lines = dark
        .as_ref()
        .and_then(|content| content.to_packed::<RawElem>())
        .and_then(|element| element.lines.as_deref());

    let mut rendered = Vec::with_capacity(2 * lines.len());
    for (index, line) in lines.iter().enumerate() {
        if index != 0 {
            rendered.push(LinebreakElem::shared().clone());
        }
        rendered.push(render_line(
            line,
            dark_lines.and_then(|lines| lines.get(index)),
            span,
        )?);
    }

    container(
        element.block.get(styles),
        element.lang.get_cloned(styles),
        code_attributes(attrs, span)?,
        Content::sequence(rendered),
        span,
    )
}

/// Synthesize the dark appearance from a clone of `raw` that carries `carrier`'s loaded theme.
///
/// Every other field — text, syntaxes, tab size, align, block, lang — stays the primary's, so
/// both appearances always describe the same text.
fn synthesize_dark(
    engine: &mut Engine,
    raw: &Content,
    carrier: &Content,
    styles: StyleChain,
    span: Span,
) -> SourceResult<Content> {
    let carrier = carrier
        .to_packed::<RawElem>()
        .ok_or("`dark-theme` must name a theme file")
        .at(span)?;
    let theme = carrier.theme.get_cloned(styles);
    if !matches!(theme, Smart::Custom(Some(_))) {
        bail!(
            span,
            "`dark-theme` must name a theme file";
            hint: "pass a value from `code-themes` or `path(\"/…\")`"
        );
    }

    let mut dark = raw.clone();
    dark.to_packed_mut::<RawElem>()
        .expect("the clone of a raw element is a raw element")
        .theme
        .set(theme);
    dark.with_mut::<dyn Synthesize>()
        .expect("a raw element is synthesized through the public capability")
        .synthesize(engine, styles)?;
    Ok(dark)
}

/// One line as Typst's own `RawLine`, with the merged runs as its body.
///
/// The rebuilt line keeps the synthesized line's span, and each run the offset of its first byte
/// within the line, so a span inside the new body still names the same source text.
fn render_line(
    line: &Packed<RawLine>,
    dark_line: Option<&Packed<RawLine>>,
    element_span: Span,
) -> SourceResult<Content> {
    let mut primary = Vec::new();
    if collect_ranges(&line.body, ResolvedStyle::default(), &mut primary).is_err() {
        return unrenderable(element_span);
    }
    let dark = match dark_line {
        Some(dark_line) => {
            if dark_line.text != line.text {
                return unrenderable(element_span);
            }
            let mut ranges = Vec::new();
            if collect_ranges(&dark_line.body, ResolvedStyle::default(), &mut ranges).is_err() {
                return unrenderable(element_span);
            }
            Some(ranges)
        }
        None => None,
    };
    let merged = match merge_ranges(&primary, dark.as_deref(), line.text.len()) {
        Ok(merged) => merged,
        Err(AppearanceMismatch) => return unrenderable(element_span),
    };

    let span = line.span();
    let text = line.text.as_str();
    let mut runs = Vec::with_capacity(merged.len());
    let mut start = 0;
    for range in &merged {
        let end = start + range.len;
        let mut piece = TextElem::packed(&text[start..end]).spanned(span);
        if start > 0 {
            piece = piece.set(TextElem::span_offset, start);
        }
        start = end;
        runs.push(match style_declarations(range) {
            Some(declarations) => HtmlElem::new(tag::span)
                .with_attr(attr::style, declarations)
                .with_body(Some(piece))
                .pack()
                .spanned(span),
            None => piece,
        });
    }

    Ok(Packed::new(RawLine::new(
        line.number,
        line.count,
        line.text.clone(),
        Content::sequence(runs),
    ))
    .spanned(span)
    .pack())
}

/// Collect one synthesized line body's ranges, in order, as byte lengths of the line's text.
///
/// The HTML synthesis colors a piece by wrapping it in an `HtmlElem` whose css carries `color`,
/// marks bold, italic, and underline with the text elements Typst's own highlighter applies, and
/// wraps every piece but the first in a `StyledElem` carrying its byte offset. A body node or a css
/// property beyond those refuses the block instead of rendering it with styles silently dropped.
fn collect_ranges(
    body: &Content,
    style: ResolvedStyle,
    ranges: &mut Vec<StyledRange>,
) -> Result<(), UnexpectedBody> {
    if let Some(sequence) = body.to_packed::<SequenceElem>() {
        for child in &sequence.children {
            collect_ranges(child, style.clone(), ranges)?;
        }
    } else if let Some(html) = body.to_packed::<HtmlElem>() {
        let mut style = style;
        if let Some(properties) = html.css.as_option() {
            for property in properties.iter() {
                if property.name == "color" {
                    style.color = Some(property.value.to_string());
                } else {
                    return Err(UnexpectedBody);
                }
            }
        }
        if let Some(Some(child)) = html.body.as_option() {
            collect_ranges(child, style, ranges)?;
        }
    } else if let Some(strong) = body.to_packed::<StrongElem>() {
        let mut style = style;
        style.bold = true;
        collect_ranges(&strong.body, style, ranges)?;
    } else if let Some(emph) = body.to_packed::<EmphElem>() {
        let mut style = style;
        style.italic = true;
        collect_ranges(&emph.body, style, ranges)?;
    } else if let Some(underline) = body.to_packed::<UnderlineElem>() {
        let mut style = style;
        style.underline = true;
        collect_ranges(&underline.body, style, ranges)?;
    } else if let Some(styled) = body.to_packed::<StyledElem>() {
        collect_ranges(&styled.child, style, ranges)?;
    } else if let Some(text) = body.to_packed::<TextElem>() {
        ranges.push(StyledRange {
            len: text.text.len(),
            style,
        });
    } else {
        return Err(UnexpectedBody);
    }
    Ok(())
}

/// Pair two appearances' ranges over one line, byte range by byte range.
///
/// Both lists must cover `total` bytes exactly. A disagreement is refused, never resolved by
/// truncating or padding: the published line must say what both appearances say.
fn merge_ranges(
    primary: &[StyledRange],
    dark: Option<&[StyledRange]>,
    total: usize,
) -> Result<Vec<MergedRange>, AppearanceMismatch> {
    let covered = |ranges: &[StyledRange]| ranges.iter().map(|range| range.len).sum::<usize>();
    if covered(primary) != total || dark.is_some_and(|ranges| covered(ranges) != total) {
        return Err(AppearanceMismatch);
    }

    let mut merged = Vec::new();
    let (mut primary_index, mut dark_index) = (0, 0);
    let (mut primary_start, mut dark_start) = (0, 0);
    let mut position = 0;
    while position < total {
        let primary_end = primary_start + primary[primary_index].len;
        let dark_end = match dark {
            Some(ranges) => dark_start + ranges[dark_index].len,
            None => total,
        };
        let end = primary_end.min(dark_end);
        if end > position {
            merged.push(MergedRange {
                len: end - position,
                primary: primary[primary_index].style.clone(),
                dark: dark.map(|ranges| ranges[dark_index].style.clone()),
            });
        }
        position = end;
        if position == primary_end {
            primary_index += 1;
            primary_start = primary_end;
        }
        if position == dark_end && dark.is_some() {
            dark_index += 1;
            dark_start = dark_end;
        }
    }
    Ok(merged)
}

/// The custom properties one merged range declares, or `None` when it renders as plain text.
///
/// A dark declaration is emitted only where it differs from the primary fallback: the stylesheet
/// consumes a missing dark property as the primary one, so equal appearances share declarations. A
/// style the dark appearance does not resolve is declared as [`OFF_TOKEN`], which clears the
/// primary's declaration instead of inheriting it.
fn style_declarations(range: &MergedRange) -> Option<String> {
    let mut declarations = String::new();
    let mut declare = |property: &str, value: &str| {
        declarations.push_str(property);
        declarations.push(':');
        declarations.push_str(value);
        declarations.push(';');
    };

    if let Some(color) = &range.primary.color {
        declare(COLOR_PROPERTY, color);
    }
    if range.primary.bold {
        declare(WEIGHT_PROPERTY, "bold");
    }
    if range.primary.italic {
        declare(STYLE_PROPERTY, "italic");
    }
    if range.primary.underline {
        declare(DECORATION_PROPERTY, "underline");
    }

    if let Some(dark) = &range.dark {
        if dark.color != range.primary.color {
            declare(
                DARK_COLOR_PROPERTY,
                dark.color.as_deref().unwrap_or(OFF_TOKEN),
            );
        }
        if dark.bold != range.primary.bold {
            declare(
                DARK_WEIGHT_PROPERTY,
                if dark.bold { "bold" } else { OFF_TOKEN },
            );
        }
        if dark.italic != range.primary.italic {
            declare(
                DARK_STYLE_PROPERTY,
                if dark.italic { "italic" } else { OFF_TOKEN },
            );
        }
        if dark.underline != range.primary.underline {
            declare(
                DARK_DECORATION_PROPERTY,
                if dark.underline {
                    "underline"
                } else {
                    OFF_TOKEN
                },
            );
        }
    }

    (!declarations.is_empty()).then_some(declarations)
}

/// The container's HTML attributes: `tola-code` extended by the author's.
fn code_attributes(attrs: Dict, span: Span) -> SourceResult<BTreeMap<String, String>> {
    let mut attributes = BTreeMap::new();
    attributes.insert("class".to_owned(), CODE_CLASS.to_owned());
    for (name, value) in attrs {
        let name = if name.as_str().eq_ignore_ascii_case("class") {
            Str::from("class")
        } else if name.as_str().eq_ignore_ascii_case("style") {
            Str::from("style")
        } else {
            name
        };
        if name.as_str().eq_ignore_ascii_case(LANG_ATTRIBUTE) {
            bail!(
                span,
                "`{name}` belongs to the rendered code";
                hint: "set the language tag on the raw element"
            );
        }
        let value = value.cast::<Str>().at(span)?.to_string();
        let value = match (name.as_str(), attributes.get(name.as_str())) {
            ("class", Some(source)) => format!("{source} {value}"),
            ("style", Some(source)) => format!("{};{value}", source.trim_end_matches(';')),
            _ => value,
        };
        attributes.insert(name.to_string(), value);
    }
    Ok(attributes)
}

/// The element Tola renders code into, around the rendered lines.
fn container(
    block: bool,
    lang: Option<EcoString>,
    attributes: BTreeMap<String, String>,
    body: Content,
    span: Span,
) -> SourceResult<Content> {
    let mut html_attributes = HtmlAttrs::new();
    for (name, value) in attributes {
        html_attributes.push(HtmlAttr::intern(&name).at(span)?, value);
    }
    let language = HtmlAttr::constant(LANG_ATTRIBUTE);
    Ok(if block {
        let code = HtmlElem::new(tag::code)
            .with_optional_attr(language, lang)
            .with_body(Some(body))
            .pack()
            .spanned(span);
        BlockElem::packed(
            HtmlElem::new(tag::pre)
                .with_attrs(html_attributes)
                .with_body(Some(code))
                .pack()
                .spanned(span),
        )
    } else {
        HtmlElem::new(tag::code)
            .with_attrs(html_attributes)
            .with_optional_attr(language, lang)
            .with_body(Some(body))
            .pack()
            .spanned(span)
    })
}

/// The refusal for a block whose synthesized appearances Tola cannot reproduce.
fn unrenderable<T>(span: Span) -> SourceResult<T> {
    bail!(
        span,
        "Tola could not render this code block";
        hint: "report this at https://github.com/tola-rs/tola-ssg/issues"
    )
}

/// The style one appearance resolves for a byte range.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ResolvedStyle {
    color: Option<String>,
    bold: bool,
    italic: bool,
    underline: bool,
}

/// One byte range of one appearance's resolved style.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StyledRange {
    len: usize,
    style: ResolvedStyle,
}

/// One byte range with both appearances' resolved styles.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MergedRange {
    len: usize,
    primary: ResolvedStyle,
    dark: Option<ResolvedStyle>,
}

/// The two appearances disagreed about the text of one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AppearanceMismatch;

/// A line body held something other than the text, the style elements, and the color the adapter
/// reproduces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UnexpectedBody;

#[cfg(test)]
mod tests {
    use typst::visualize::Color;
    use typst_html::html_span_filled;

    use super::*;

    /// One appearance range of `len` bytes carrying `style`.
    fn range(len: usize, style: ResolvedStyle) -> StyledRange {
        StyledRange { len, style }
    }

    /// Every text piece one body carries, in order.
    fn text_pieces(body: &Content) -> Vec<String> {
        if let Some(sequence) = body.to_packed::<SequenceElem>() {
            sequence.children.iter().flat_map(text_pieces).collect()
        } else if let Some(html) = body.to_packed::<HtmlElem>() {
            match html.body.as_option() {
                Some(Some(child)) => text_pieces(child),
                _ => Vec::new(),
            }
        } else if let Some(strong) = body.to_packed::<StrongElem>() {
            text_pieces(&strong.body)
        } else if let Some(emph) = body.to_packed::<EmphElem>() {
            text_pieces(&emph.body)
        } else if let Some(underline) = body.to_packed::<UnderlineElem>() {
            text_pieces(&underline.body)
        } else if let Some(styled) = body.to_packed::<StyledElem>() {
            text_pieces(&styled.child)
        } else if let Some(text) = body.to_packed::<TextElem>() {
            vec![text.text.to_string()]
        } else {
            Vec::new()
        }
    }

    #[test]
    fn merged_ranges_align_both_appearances() {
        let primary = [
            range(
                5,
                ResolvedStyle {
                    color: Some("#aa0000".into()),
                    ..Default::default()
                },
            ),
            range(
                6,
                ResolvedStyle {
                    bold: true,
                    ..Default::default()
                },
            ),
        ];
        let dark = [
            range(3, ResolvedStyle::default()),
            range(
                8,
                ResolvedStyle {
                    color: Some("#008800".into()),
                    italic: true,
                    ..Default::default()
                },
            ),
        ];

        let merged = merge_ranges(&primary, Some(&dark), 11).unwrap();

        assert_eq!(
            merged.iter().map(|range| range.len).collect::<Vec<_>>(),
            [3, 2, 6]
        );
        assert_eq!(merged[0].primary.color.as_deref(), Some("#aa0000"));
        assert_eq!(merged[0].dark.as_ref().unwrap().color, None);
        assert_eq!(
            merged[1].dark.as_ref().unwrap().color.as_deref(),
            Some("#008800")
        );
        assert!(merged[2].primary.bold);
        assert!(merged[2].dark.as_ref().unwrap().italic);
    }

    #[test]
    fn merge_refuses_disagreeing_ranges() {
        let line = [range(5, ResolvedStyle::default())];
        let short = [range(4, ResolvedStyle::default())];

        assert_eq!(
            merge_ranges(&line, Some(&short), 5),
            Err(AppearanceMismatch)
        );
        assert_eq!(merge_ranges(&line, None, 4), Err(AppearanceMismatch));
    }

    #[test]
    fn dark_declarations_match_the_resolved_styles() {
        let color = |value: &str| ResolvedStyle {
            color: Some(value.into()),
            ..Default::default()
        };
        let bold = || ResolvedStyle {
            bold: true,
            ..Default::default()
        };
        let italic = || ResolvedStyle {
            italic: true,
            ..Default::default()
        };
        let underline = || ResolvedStyle {
            underline: true,
            ..Default::default()
        };

        // Every property's four states, plus a block with no dark appearance at all.
        for (primary, dark, expected) in [
            // The dark appearance resolves a value the primary does not.
            (
                ResolvedStyle::default(),
                Some(color("#0000aa")),
                Some("--tola-code-dark-color:#0000aa;"),
            ),
            (
                ResolvedStyle::default(),
                Some(bold()),
                Some("--tola-code-dark-weight:bold;"),
            ),
            (
                ResolvedStyle::default(),
                Some(italic()),
                Some("--tola-code-dark-style:italic;"),
            ),
            (
                ResolvedStyle::default(),
                Some(underline()),
                Some("--tola-code-dark-decoration:underline;"),
            ),
            // The dark appearance resolves nothing: the primary's declaration is cleared, not
            // inherited.
            (
                color("#aa0000"),
                Some(ResolvedStyle::default()),
                Some("--tola-code-color:#aa0000;--tola-code-dark-color:none;"),
            ),
            (
                bold(),
                Some(ResolvedStyle::default()),
                Some("--tola-code-weight:bold;--tola-code-dark-weight:none;"),
            ),
            (
                italic(),
                Some(ResolvedStyle::default()),
                Some("--tola-code-style:italic;--tola-code-dark-style:none;"),
            ),
            (
                underline(),
                Some(ResolvedStyle::default()),
                Some("--tola-code-decoration:underline;--tola-code-dark-decoration:none;"),
            ),
            // Equal appearances share the primary's declaration.
            (
                color("#aa0000"),
                Some(color("#aa0000")),
                Some("--tola-code-color:#aa0000;"),
            ),
            (bold(), Some(bold()), Some("--tola-code-weight:bold;")),
            (italic(), Some(italic()), Some("--tola-code-style:italic;")),
            (
                underline(),
                Some(underline()),
                Some("--tola-code-decoration:underline;"),
            ),
            // Neither appearance resolves anything.
            (
                ResolvedStyle::default(),
                Some(ResolvedStyle::default()),
                None,
            ),
            // No dark appearance: the primary's declarations only.
            (
                ResolvedStyle {
                    color: Some("#aa0000".into()),
                    bold: true,
                    italic: true,
                    underline: true,
                },
                None,
                Some(concat!(
                    "--tola-code-color:#aa0000;",
                    "--tola-code-weight:bold;",
                    "--tola-code-style:italic;",
                    "--tola-code-decoration:underline;",
                )),
            ),
        ] {
            let merged = MergedRange {
                len: 3,
                primary,
                dark,
            };
            let declarations = style_declarations(&merged).unwrap_or_default();

            assert_eq!(
                declarations.as_str(),
                expected.unwrap_or_default(),
                "{merged:?}"
            );
            // A CSS-wide keyword would act on the custom property itself, so the consumer's
            // fallback would resolve the other appearance's value.
            for keyword in ["inherit", "initial", "unset", "revert"] {
                assert!(
                    !declarations.contains(keyword),
                    "{keyword} in {declarations}"
                );
            }
        }
    }

    #[test]
    fn ranges_follow_the_synthesized_styles() {
        let body = Content::sequence(vec![
            TextElem::packed("let"),
            html_span_filled(TextElem::packed(" x"), Color::from_u8(0xaa, 0, 0, 255)),
            TextElem::packed(" = 1")
                .set(TextElem::span_offset, 4)
                .strong()
                .emph()
                .underlined(),
        ]);

        let mut ranges = Vec::new();
        collect_ranges(&body, ResolvedStyle::default(), &mut ranges).unwrap();

        assert_eq!(
            ranges.iter().map(|range| range.len).collect::<Vec<_>>(),
            [3, 2, 4]
        );
        assert_eq!(ranges[0].style, ResolvedStyle::default());
        assert_eq!(ranges[1].style.color.as_deref(), Some("#aa0000"));
        assert!(ranges[2].style.bold && ranges[2].style.italic && ranges[2].style.underline);
    }

    #[test]
    fn unknown_body_nodes_are_refused() {
        let mut ranges = Vec::new();

        assert_eq!(
            collect_ranges(
                &LinebreakElem::shared().clone(),
                ResolvedStyle::default(),
                &mut ranges
            ),
            Err(UnexpectedBody)
        );
    }

    #[test]
    fn rendered_line_keeps_the_synthesized_text() {
        let body = Content::sequence(vec![
            html_span_filled(TextElem::packed("日本"), Color::from_u8(0xaa, 0, 0, 255)),
            TextElem::packed("語"),
        ]);
        let line = Packed::new(RawLine::new(2, 3, "日本語".into(), body));
        let dark = Packed::new(RawLine::new(
            2,
            3,
            "日本語".into(),
            TextElem::packed("日本語"),
        ));

        let rendered = render_line(&line, Some(&dark), Span::detached()).unwrap();
        let rendered = rendered.to_packed::<RawLine>().unwrap();

        assert_eq!((rendered.number, rendered.count), (2, 3));
        assert_eq!(rendered.text.as_str(), "日本語");
        assert_eq!(text_pieces(&rendered.body).concat(), "日本語");
    }
}
