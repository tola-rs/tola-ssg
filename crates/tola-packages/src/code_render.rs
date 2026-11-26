//! Typed adapter over Typst's synthesized code highlighting.
//!
//! Replaces each synthesized `RawLine` body with runs carrying primary and optional dark styles as
//! local CSS custom properties, preserving text, line numbers, and `span_offset` invariants.

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

const CODE_CLASS: &str = "tola-code";
const LANG_ATTR: &str = "data-lang";

const COLOR_PROP: &str = "--tola-code-color";
const WEIGHT_PROP: &str = "--tola-code-weight";
const STYLE_PROP: &str = "--tola-code-style";
const DECORATION_PROP: &str = "--tola-code-decoration";
const DARK_COLOR_PROP: &str = "--tola-code-dark-color";
const DARK_WEIGHT_PROP: &str = "--tola-code-dark-weight";
const DARK_STYLE_PROP: &str = "--tola-code-dark-style";
const DARK_DECORATION_PROP: &str = "--tola-code-dark-decoration";

/// Clears an inherited primary custom property without triggering CSS-wide keyword behavior.
const OFF_TOKEN: &str = "none";

#[func(contextual)]
pub(super) fn tola_render_code(
    engine: &mut Engine,
    context: Tracked<Context>,
    /// The raw element the author's show rule replaced.
    raw: Content,
    /// The element naming the site's dark theme, if one is configured.
    dark: Option<Content>,
    /// HTML attributes for the element Tola renders code into.
    attrs: Dict,
) -> SourceResult<Content> {
    let span = raw.span();
    let styles = context.styles().at(span)?;
    let elem = raw
        .to_packed::<RawElem>()
        .ok_or("`render-code` needs the raw element the show rule replaced")
        .at(span)?;

    // `raw.theme: none` disables highlighting entirely, including the dark appearance.
    if matches!(elem.theme.get_ref(styles), Smart::Custom(None)) {
        return Ok(raw);
    }

    let raw = super::code_syntaxes::with_bundled_syntax(elem, engine, styles)?
        .map(Packed::pack)
        .unwrap_or(raw);
    let elem = raw.to_packed::<RawElem>().expect("a raw element");
    let lines = elem.lines.as_deref().unwrap_or_default();
    let dark = dark
        .as_ref()
        .map(|el| synthesize_dark(engine, &raw, el, styles, span))
        .transpose()?;
    let dark_lines = dark
        .as_ref()
        .and_then(|c| c.to_packed::<RawElem>())
        .and_then(|el| el.lines.as_deref());

    let mut out = Vec::with_capacity(2 * lines.len());
    for (idx, line) in lines.iter().enumerate() {
        if idx != 0 {
            out.push(LinebreakElem::shared().clone());
        }
        out.push(render_line(
            line,
            dark_lines.and_then(|ls| ls.get(idx)),
            span,
        )?);
    }

    container(
        elem.block.get(styles),
        elem.lang.get_cloned(styles),
        code_attrs(attrs, span)?,
        Content::sequence(out),
        span,
    )
}

fn synthesize_dark(
    engine: &mut Engine,
    raw: &Content,
    dark_el: &Content,
    styles: StyleChain,
    span: Span,
) -> SourceResult<Content> {
    let dark_el = dark_el
        .to_packed::<RawElem>()
        .ok_or("`dark-theme` must name a theme file")
        .at(span)?;
    let theme = dark_el.theme.get_cloned(styles);
    if !matches!(theme, Smart::Custom(Some(_))) {
        bail!(
            span,
            "`dark-theme` must name a theme file";
            hint: "pass a value from `code-themes` or `path(\"/…\")`"
        );
    }

    let mut dark = raw.clone();
    dark.to_packed_mut::<RawElem>()
        .expect("cloned raw element")
        .theme
        .set(theme);
    dark.with_mut::<dyn Synthesize>()
        .expect("raw element implements Synthesize")
        .synthesize(engine, styles)?;
    Ok(dark)
}

fn render_line(
    line: &Packed<RawLine>,
    dark_line: Option<&Packed<RawLine>>,
    elem_span: Span,
) -> SourceResult<Content> {
    let Ok(primary) = collect_line_ranges(&line.body) else {
        return unrenderable(elem_span);
    };
    let dark = match dark_line {
        Some(dl) if dl.text == line.text => match collect_line_ranges(&dl.body) {
            Ok(ranges) => Some(ranges),
            Err(UnexpectedBody) => return unrenderable(elem_span),
        },
        Some(_) => return unrenderable(elem_span),
        None => None,
    };
    let Ok(merged) = merge_ranges(&primary, dark.as_deref(), line.text.len()) else {
        return unrenderable(elem_span);
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
            Some(decls) => HtmlElem::new(tag::span)
                .with_attr(attr::style, decls)
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

fn collect_line_ranges(body: &Content) -> Result<Vec<StyledRange>, UnexpectedBody> {
    let mut ranges = Vec::new();
    collect_ranges(body, TextStyle::default(), &mut ranges)?;
    Ok(ranges)
}

fn collect_ranges(
    body: &Content,
    style: TextStyle,
    ranges: &mut Vec<StyledRange>,
) -> Result<(), UnexpectedBody> {
    if let Some(seq) = body.to_packed::<SequenceElem>() {
        for child in &seq.children {
            collect_ranges(child, style.clone(), ranges)?;
        }
    } else if let Some(html) = body.to_packed::<HtmlElem>() {
        let mut style = style;
        if let Some(props) = html.css.as_option() {
            for prop in props.iter() {
                if prop.name == "color" {
                    style.color = Some(prop.value.to_string());
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

fn merge_ranges(
    primary: &[StyledRange],
    dark: Option<&[StyledRange]>,
    total: usize,
) -> Result<Vec<MergedRange>, AppearanceMismatch> {
    let covered = |rs: &[StyledRange]| rs.iter().map(|r| r.len).sum::<usize>();
    if covered(primary) != total || dark.is_some_and(|rs| covered(rs) != total) {
        return Err(AppearanceMismatch);
    }

    let mut merged = Vec::new();
    let (mut p_idx, mut d_idx) = (0, 0);
    let (mut p_start, mut d_start) = (0, 0);
    let mut pos = 0;
    while pos < total {
        let p_end = p_start + primary[p_idx].len;
        let d_end = dark.map_or(total, |rs| d_start + rs[d_idx].len);
        let end = p_end.min(d_end);
        if end > pos {
            merged.push(MergedRange {
                len: end - pos,
                primary: primary[p_idx].style.clone(),
                dark: dark.map(|rs| rs[d_idx].style.clone()),
            });
        }
        pos = end;
        if pos == p_end {
            p_idx += 1;
            p_start = p_end;
        }
        if pos == d_end && dark.is_some() {
            d_idx += 1;
            d_start = d_end;
        }
    }
    Ok(merged)
}

fn style_declarations(range: &MergedRange) -> Option<String> {
    let mut out = String::new();
    let mut push = |prop: &str, val: &str| {
        out.push_str(prop);
        out.push(':');
        out.push_str(val);
        out.push(';');
    };

    if let Some(color) = &range.primary.color {
        push(COLOR_PROP, color);
    }
    let flags = [
        (
            WEIGHT_PROP,
            DARK_WEIGHT_PROP,
            "bold",
            range.primary.bold,
            range.dark.as_ref().map(|d| d.bold),
        ),
        (
            STYLE_PROP,
            DARK_STYLE_PROP,
            "italic",
            range.primary.italic,
            range.dark.as_ref().map(|d| d.italic),
        ),
        (
            DECORATION_PROP,
            DARK_DECORATION_PROP,
            "underline",
            range.primary.underline,
            range.dark.as_ref().map(|d| d.underline),
        ),
    ];
    for (prop, _, token, p_on, _) in flags {
        if p_on {
            push(prop, token);
        }
    }
    if let Some(dark) = &range.dark {
        if dark.color != range.primary.color {
            push(DARK_COLOR_PROP, dark.color.as_deref().unwrap_or(OFF_TOKEN));
        }
        for (_, dark_prop, token, p_on, d_on) in flags {
            if let Some(d_on) = d_on
                && d_on != p_on
            {
                push(dark_prop, if d_on { token } else { OFF_TOKEN });
            }
        }
    }

    (!out.is_empty()).then_some(out)
}

fn code_attrs(attrs: Dict, span: Span) -> SourceResult<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    out.insert("class".to_owned(), CODE_CLASS.to_owned());
    for (name, val) in attrs {
        let name = crate::attr_key(&name);
        if name.eq_ignore_ascii_case(LANG_ATTR) {
            bail!(
                span,
                "`{name}` belongs to the rendered code";
                hint: "set the language tag on the raw element"
            );
        }
        let val = val.cast::<Str>().at(span)?;
        crate::merge_attr(&mut out, name, val.as_str());
    }
    Ok(out)
}

fn container(
    block: bool,
    lang: Option<EcoString>,
    attrs: BTreeMap<String, String>,
    body: Content,
    span: Span,
) -> SourceResult<Content> {
    let mut html_attrs = HtmlAttrs::new();
    for (name, val) in attrs {
        html_attrs.push(HtmlAttr::intern(&name).at(span)?, val);
    }
    let lang_attr = HtmlAttr::constant(LANG_ATTR);
    Ok(if block {
        let code = HtmlElem::new(tag::code)
            .with_optional_attr(lang_attr, lang)
            .with_body(Some(body))
            .pack()
            .spanned(span);
        BlockElem::packed(
            HtmlElem::new(tag::pre)
                .with_attrs(html_attrs)
                .with_body(Some(code))
                .pack()
                .spanned(span),
        )
    } else {
        HtmlElem::new(tag::code)
            .with_attrs(html_attrs)
            .with_optional_attr(lang_attr, lang)
            .with_body(Some(body))
            .pack()
            .spanned(span)
    })
}

fn unrenderable<T>(span: Span) -> SourceResult<T> {
    bail!(
        span,
        "Tola could not render this code block";
        hint: "report this at https://github.com/tola-rs/tola-ssg/issues"
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TextStyle {
    color: Option<String>,
    bold: bool,
    italic: bool,
    underline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StyledRange {
    len: usize,
    style: TextStyle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MergedRange {
    len: usize,
    primary: TextStyle,
    dark: Option<TextStyle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AppearanceMismatch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UnexpectedBody;

#[cfg(test)]
mod tests {
    use typst::visualize::Color;
    use typst_html::html_span_filled;

    use super::*;

    fn range(len: usize, style: TextStyle) -> StyledRange {
        StyledRange { len, style }
    }

    fn text_pieces(body: &Content) -> Vec<String> {
        if let Some(seq) = body.to_packed::<SequenceElem>() {
            seq.children.iter().flat_map(text_pieces).collect()
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
    fn merge_ranges_mismatch() {
        let line = [range(5, TextStyle::default())];
        let short = [range(4, TextStyle::default())];

        assert_eq!(
            merge_ranges(&line, Some(&short), 5),
            Err(AppearanceMismatch)
        );
        assert_eq!(merge_ranges(&line, None, 4), Err(AppearanceMismatch));
    }

    #[test]
    fn render_line_utf8_text() {
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
