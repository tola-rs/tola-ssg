//! Presentation property and SVG attribute validation and local-URL prefixing.

use std::{
    collections::BTreeMap,
    fmt::{self, Write},
    str::FromStr,
    sync::Arc,
};

use cssparser::{Delimiter, Parser, ParserInput, Token};

use super::InvalidSvg;
use crate::svg::{geometry::parse_view_box, MAX_SVG_ID_BYTES};

const MAX_CSS_VALUE_DEPTH: usize = 32;

#[derive(Debug)]
pub(super) struct StyleDeclaration {
    pub(super) value: String,
    important: bool,
}

pub(super) fn parse_style(
    el: &str,
    src: &str,
) -> Result<BTreeMap<String, StyleDeclaration>, InvalidSvg> {
    let invalid = || invalid_value(el, "style");
    let mut input = ParserInput::new(src);
    let mut parser = Parser::new(&mut input);
    let mut decls: BTreeMap<String, StyleDeclaration> = BTreeMap::new();
    while !parser.is_exhausted() {
        if parser.try_parse(|p| p.expect_semicolon()).is_ok() {
            continue;
        }
        let name = parser
            .expect_ident_cloned()
            .map_err(|_| invalid())?
            .to_ascii_lowercase();
        if !is_presentation_property(&name) {
            return Err(InvalidSvg::Attribute {
                element: el.to_owned(),
                attribute: name,
            });
        }
        parser.expect_colon().map_err(|_| invalid())?;
        let mut decl = parser
            .parse_until_before(Delimiter::Semicolon, |p| {
                let value = p.parse_until_before(Delimiter::Bang, |p| {
                    let start = p.position();
                    consume_value(p, 0)?;
                    Ok(p.slice_from(start).trim().to_owned())
                })?;
                let important = p.try_parse(cssparser::parse_important).is_ok();
                p.expect_exhausted()?;
                Ok::<_, cssparser::ParseError<'_, ()>>(StyleDeclaration { value, important })
            })
            .map_err(|_| invalid())?;
        decl.value = normalize_property(&name, &decl.value);
        validate_attribute(el, &name, &decl.value)?;
        if decls
            .get(&name)
            .is_none_or(|prev| decl.important || !prev.important)
        {
            decls.insert(name, decl);
        }
        let _ = parser.try_parse(|p| p.expect_semicolon());
    }
    Ok(decls)
}

fn consume_value<'i>(
    parser: &mut Parser<'i, '_>,
    depth: usize,
) -> Result<(), cssparser::ParseError<'i, ()>> {
    if depth > MAX_CSS_VALUE_DEPTH {
        return Err(parser.new_custom_error(()));
    }
    while !parser.is_exhausted() {
        let tok = parser.next()?.clone();
        if tok.is_parse_error() {
            return Err(parser.new_custom_error(()));
        }
        match tok {
            Token::Function(_) | Token::ParenthesisBlock | Token::SquareBracketBlock => {
                parser.parse_nested_block(|nested| consume_value(nested, depth + 1))?;
            }
            Token::CurlyBracketBlock => return Err(parser.new_custom_error(())),
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn normalize_property(name: &str, val: &str) -> String {
    if is_presentation_property(name)
        && !matches!(
            name,
            "fill" | "stroke" | "color" | "stop-color" | "flood-color" | "lighting-color"
        )
    {
        let mut input = ParserInput::new(val);
        let mut parser = Parser::new(&mut input);
        if let Ok(kw) = parser.expect_ident_cloned() {
            if parser.is_exhausted() {
                return kw.to_ascii_lowercase();
            }
        }
    }
    val.to_owned()
}

fn is_presentation_property(name: &str) -> bool {
    matches!(
        name,
        "fill"
            | "stroke"
            | "color"
            | "opacity"
            | "fill-opacity"
            | "stroke-opacity"
            | "fill-rule"
            | "clip-rule"
            | "stroke-width"
            | "stroke-linecap"
            | "stroke-linejoin"
            | "stroke-miterlimit"
            | "stroke-dasharray"
            | "stroke-dashoffset"
            | "clip-path"
            | "mask"
            | "mask-type"
            | "filter"
            | "stop-color"
            | "stop-opacity"
            | "flood-color"
            | "flood-opacity"
            | "lighting-color"
            | "display"
            | "visibility"
            | "overflow"
            | "vector-effect"
            | "paint-order"
            | "shape-rendering"
            | "color-interpolation"
            | "color-interpolation-filters"
            | "isolation"
            | "mix-blend-mode"
    )
}

fn invalid_value(el: &str, attr: &str) -> InvalidSvg {
    InvalidSvg::Value {
        element: el.to_owned(),
        attribute: attr.to_owned(),
    }
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_SVG_ID_BYTES && !id.chars().any(char::is_whitespace)
}

const CSS_WIDE_KEYWORDS: &[&str] = &["inherit", "initial", "unset"];

const KEYWORD_VALUES: &[(&str, &[&str])] = &[
    ("fill-rule", RULE_KEYWORDS),
    ("clip-rule", RULE_KEYWORDS),
    ("stroke-linecap", &["butt", "round", "square", "inherit"]),
    ("stroke-linejoin", STROKE_JOIN_KEYWORDS),
    ("gradientUnits", OBJECT_UNITS),
    ("patternUnits", OBJECT_UNITS),
    ("patternContentUnits", OBJECT_UNITS),
    ("clipPathUnits", OBJECT_UNITS),
    ("maskUnits", OBJECT_UNITS),
    ("maskContentUnits", OBJECT_UNITS),
    ("filterUnits", OBJECT_UNITS),
    ("primitiveUnits", OBJECT_UNITS),
    ("spreadMethod", &["pad", "reflect", "repeat"]),
    ("mask-type", &["luminance", "alpha"]),
    ("display", &["none", "inline", "block", "inherit"]),
    ("visibility", VISIBILITY_KEYWORDS),
    ("overflow", OVERFLOW_KEYWORDS),
    ("vector-effect", &["none", "non-scaling-stroke"]),
    ("color-interpolation", COLOR_KEYWORDS),
    ("color-interpolation-filters", COLOR_KEYWORDS),
    ("shape-rendering", RENDERING_KEYWORDS),
    ("isolation", &["auto", "isolate"]),
];

const RULE_KEYWORDS: &[&str] = &["nonzero", "evenodd", "inherit"];
const OBJECT_UNITS: &[&str] = &["userSpaceOnUse", "objectBoundingBox"];
const STROKE_JOIN_KEYWORDS: &[&str] = &["miter", "miter-clip", "round", "bevel", "arcs", "inherit"];
const VISIBILITY_KEYWORDS: &[&str] = &["visible", "hidden", "collapse", "inherit"];
const OVERFLOW_KEYWORDS: &[&str] = &["visible", "hidden", "scroll", "auto", "inherit"];
const COLOR_KEYWORDS: &[&str] = &["auto", "srgb", "linearrgb", "inherit"];
const RENDERING_KEYWORDS: &[&str] = &["auto", "optimizespeed", "crispedges", "geometricprecision"];

fn keyword_values(attr: &str) -> Option<&'static [&'static str]> {
    KEYWORD_VALUES
        .iter()
        .find_map(|(name, kws)| (*name == attr).then_some(*kws))
}

fn validate_kw(val: &str, kws: &[&str], el: &str, attr: &str) -> Result<(), InvalidSvg> {
    kws.contains(&val.trim())
        .then_some(())
        .ok_or_else(|| invalid_value(el, attr))
}

fn validate_f64(val: &str, nonnegative: bool, el: &str, attr: &str) -> Result<(), InvalidSvg> {
    let n = val
        .trim()
        .parse::<f64>()
        .map_err(|_| invalid_value(el, attr))?;
    (n.is_finite() && (!nonnegative || n >= 0.0))
        .then_some(())
        .ok_or_else(|| invalid_value(el, attr))
}

fn validate_length(val: &str, nonnegative: bool, el: &str, attr: &str) -> Result<(), InvalidSvg> {
    let len = svgtypes::Length::from_str(val.trim()).map_err(|_| invalid_value(el, attr))?;
    (len.number.is_finite() && (!nonnegative || len.number >= 0.0))
        .then_some(())
        .ok_or_else(|| invalid_value(el, attr))
}

pub(super) fn validate_attribute(el: &str, attr: &str, val: &str) -> Result<(), InvalidSvg> {
    let invalid = || invalid_value(el, attr);
    if matches!(
        attr,
        "class" | "role" | "style" | "xml:space" | "xml:lang" | "version"
    ) || attr.starts_with("aria-")
        || attr.starts_with("data-")
    {
        return Ok(());
    }
    if is_presentation_property(attr) && CSS_WIDE_KEYWORDS.contains(&val.trim()) {
        return Ok(());
    }
    if let Some(kws) = keyword_values(attr) {
        return validate_kw(val, kws, el, attr);
    }
    match attr {
        "id" => is_valid_id(val).then_some(()).ok_or_else(invalid),
        "d" if el == "path" => validate_path(val),
        "viewBox" => parse_view_box(val).map(|_| ()),
        "preserveAspectRatio" => svgtypes::AspectRatio::from_str(val)
            .map(|_| ())
            .map_err(|_| invalid()),
        "href" if matches!(el, "use" | "linearGradient" | "radialGradient" | "pattern") => {
            local_reference(val).map(|_| ())
        }
        "transform" | "gradientTransform" | "patternTransform" => {
            let t = svgtypes::Transform::from_str(val).map_err(|_| invalid())?;
            [t.a, t.b, t.c, t.d, t.e, t.f]
                .iter()
                .all(|n| n.is_finite())
                .then_some(())
                .ok_or_else(invalid)
        }
        "fill" | "stroke" => parse_paint(val).map(|_| ()).map_err(|_| invalid()),
        "color" | "stop-color" | "flood-color" | "lighting-color" => {
            parse_color(val).map(|_| ()).map_err(|_| invalid())
        }
        "clip-path" | "mask" | "filter" => {
            if val.trim().eq_ignore_ascii_case("none") {
                return Ok(());
            }
            if local_urls(val)?.len() != 1 {
                return Err(invalid());
            }
            let mut input = ParserInput::new(val);
            let mut parser = Parser::new(&mut input);
            parser.expect_url().map_err(|_| invalid())?;
            parser.expect_exhausted().map_err(|_| invalid())
        }
        "x" | "y" | "x1" | "y1" | "x2" | "y2" | "cx" | "cy" | "fx" | "fy" | "width" | "height"
        | "r" | "rx" | "ry" | "fr" | "stroke-width" | "stroke-dashoffset" => {
            let nonneg = matches!(
                attr,
                "width" | "height" | "r" | "rx" | "ry" | "fr" | "stroke-width"
            );
            validate_length(val, nonneg, el, attr)
        }
        "points" => {
            let count = validate_number_list(val).map_err(|_| invalid())?;
            (count % 2 == 0).then_some(()).ok_or_else(invalid)
        }
        "stroke-dasharray" => {
            if val.trim() == "none" {
                return Ok(());
            }
            for len in svgtypes::LengthListParser::from(val) {
                let len = len.map_err(|_| invalid())?;
                if !len.number.is_finite() || len.number < 0.0 {
                    return Err(invalid());
                }
            }
            Ok(())
        }
        "opacity" | "fill-opacity" | "stroke-opacity" | "stop-opacity" | "flood-opacity"
        | "offset" => {
            let num = val.trim().strip_suffix('%').unwrap_or(val.trim());
            validate_f64(num, false, el, attr)
        }
        "pathLength" | "stroke-miterlimit" => validate_f64(val, true, el, attr),
        "paint-order" => validate_paint_order(val).map_err(|_| invalid()),
        "mix-blend-mode" => {
            if val == "plus-lighter" {
                return Ok(());
            }
            validate_kw(val, BLEND_MODES, el, attr)
        }
        "in" | "in2" | "result" if el.starts_with("fe") => {
            let mut input = ParserInput::new(val);
            let mut parser = Parser::new(&mut input);
            let ident = parser.expect_ident().map_err(|_| invalid())?;
            if matches!(
                ident.to_ascii_lowercase().as_str(),
                "initial" | "inherit" | "unset" | "revert" | "revert-layer" | "default"
            ) {
                return Err(invalid());
            }
            parser.expect_exhausted().map_err(|_| invalid())
        }
        "type" if el == "feColorMatrix" => validate_kw(
            val,
            &["matrix", "saturate", "hueRotate", "luminanceToAlpha"],
            el,
            attr,
        ),
        "type" if matches!(el, "feFuncA" | "feFuncR" | "feFuncG" | "feFuncB") => validate_kw(
            val,
            &["identity", "table", "discrete", "linear", "gamma"],
            el,
            attr,
        ),
        "type" if el == "feTurbulence" => {
            validate_kw(val, &["fractalNoise", "turbulence"], el, attr)
        }
        "mode" if el == "feBlend" => validate_kw(val, BLEND_MODES, el, attr),
        "operator" if el == "feComposite" => validate_kw(
            val,
            &["over", "in", "out", "atop", "xor", "arithmetic", "lighter"],
            el,
            attr,
        ),
        "operator" if el == "feMorphology" => validate_kw(val, &["erode", "dilate"], el, attr),
        "edgeMode" if el == "feConvolveMatrix" => {
            validate_kw(val, &["duplicate", "wrap", "none"], el, attr)
        }
        "stitchTiles" if el == "feTurbulence" => {
            validate_kw(val, &["stitch", "noStitch"], el, attr)
        }
        "preserveAlpha" if el == "feConvolveMatrix" => {
            validate_kw(val, &["true", "false"], el, attr)
        }
        "values" | "stdDeviation" | "dx" | "dy" | "k1" | "k2" | "k3" | "k4" | "slope"
        | "intercept" | "amplitude" | "exponent" | "tableValues" | "order" | "kernelMatrix"
        | "divisor" | "bias" | "targetX" | "targetY" | "kernelUnitLength" | "surfaceScale"
        | "diffuseConstant" | "specularConstant" | "specularExponent" | "scale" | "radius"
        | "azimuth" | "elevation" | "z" | "pointsAtX" | "pointsAtY" | "pointsAtZ"
        | "limitingConeAngle" | "baseFrequency" | "numOctaves" | "seed"
            if el.starts_with("fe") =>
        {
            validate_number_list(val).map(|_| ()).map_err(|_| invalid())
        }
        "xChannelSelector" | "yChannelSelector" if el == "feDisplacementMap" => {
            validate_kw(val, &["R", "G", "B", "A"], el, attr)
        }
        _ => Err(InvalidSvg::Attribute {
            element: el.to_owned(),
            attribute: attr.to_owned(),
        }),
    }
}

const BLEND_MODES: &[&str] = &[
    "normal",
    "multiply",
    "screen",
    "overlay",
    "darken",
    "lighten",
    "color-dodge",
    "color-burn",
    "hard-light",
    "soft-light",
    "difference",
    "exclusion",
    "hue",
    "saturation",
    "color",
    "luminosity",
];

fn validate_paint_order(val: &str) -> Result<(), ()> {
    let mut input = ParserInput::new(val);
    let mut parser = Parser::new(&mut input);
    let mut seen = 0_u8;
    while !parser.is_exhausted() {
        let kw = parser.expect_ident_cloned().map_err(|_| ())?;
        let bit = match kw.to_ascii_lowercase().as_str() {
            "normal" if seen == 0 && parser.is_exhausted() => return Ok(()),
            "fill" => 1,
            "stroke" => 2,
            "markers" => 4,
            _ => return Err(()),
        };
        if seen & bit != 0 {
            return Err(());
        }
        seen |= bit;
    }
    (seen != 0).then_some(()).ok_or(())
}

fn validate_number_list(val: &str) -> Result<usize, ()> {
    let mut count = 0;
    for n in svgtypes::NumberListParser::from(val) {
        if !n.map_err(|_| ())?.is_finite() {
            return Err(());
        }
        count += 1;
    }
    Ok(count)
}

fn validate_path(val: &str) -> Result<(), InvalidSvg> {
    use svgtypes::PathSegment;
    for seg in svgtypes::PathParser::from(val) {
        let seg = seg.map_err(|_| InvalidSvg::Path)?;
        let finite = match seg {
            PathSegment::MoveTo { x, y, .. }
            | PathSegment::LineTo { x, y, .. }
            | PathSegment::SmoothQuadratic { x, y, .. } => [x, y].iter().all(|n| n.is_finite()),
            PathSegment::HorizontalLineTo { x, .. } => x.is_finite(),
            PathSegment::VerticalLineTo { y, .. } => y.is_finite(),
            PathSegment::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
                ..
            } => [x1, y1, x2, y2, x, y].iter().all(|n| n.is_finite()),
            PathSegment::SmoothCurveTo { x2, y2, x, y, .. } => {
                [x2, y2, x, y].iter().all(|n| n.is_finite())
            }
            PathSegment::Quadratic { x1, y1, x, y, .. } => {
                [x1, y1, x, y].iter().all(|n| n.is_finite())
            }
            PathSegment::EllipticalArc {
                rx,
                ry,
                x_axis_rotation,
                x,
                y,
                ..
            } => [rx, ry, x_axis_rotation, x, y]
                .iter()
                .all(|n| n.is_finite()),
            PathSegment::ClosePath { .. } => true,
        };
        if !finite {
            return Err(InvalidSvg::Path);
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub(super) enum PaintValue {
    None,
    Inherit,
    Initial,
    Unset,
    CurrentColor,
    Fixed,
    Transparent,
    Resource(Arc<str>),
}

fn parse_color(val: &str) -> Result<PaintValue, ()> {
    let mut input = ParserInput::new(val);
    let mut parser = Parser::new(&mut input);
    if let Ok(kw) = parser.try_parse(|p| p.expect_ident_cloned()) {
        if parser.is_exhausted() {
            match kw.to_ascii_lowercase().as_str() {
                "inherit" => return Ok(PaintValue::Inherit),
                "initial" => return Ok(PaintValue::Initial),
                "unset" => return Ok(PaintValue::Unset),
                _ => {}
            }
        }
    }
    let mut input = ParserInput::new(val);
    let mut parser = Parser::new(&mut input);
    let color = cssparser_color::Color::parse(&mut parser).map_err(|_| ())?;
    parser.expect_exhausted().map_err(|_| ())?;
    use cssparser_color::Color;
    let alpha = match color {
        Color::CurrentColor => return Ok(PaintValue::CurrentColor),
        Color::Rgba(c) => Some(c.alpha),
        Color::Hsl(c) => c.alpha,
        Color::Hwb(c) => c.alpha,
        Color::Lab(c) => c.alpha,
        Color::Lch(c) => c.alpha,
        Color::Oklab(c) => c.alpha,
        Color::Oklch(c) => c.alpha,
        Color::ColorFunction(c) => c.alpha,
    };
    // Distinguish zero-alpha color from `none` because SVG gradients interpolate non-premultiplied RGB.
    Ok(if alpha.unwrap_or(0.0) == 0.0 {
        PaintValue::Transparent
    } else {
        PaintValue::Fixed
    })
}

pub(super) fn parse_paint(val: &str) -> Result<PaintValue, ()> {
    let mut input = ParserInput::new(val);
    let mut parser = Parser::new(&mut input);
    if let Ok(kw) = parser.try_parse(|p| p.expect_ident_cloned()) {
        if parser.is_exhausted() {
            match kw.to_ascii_lowercase().as_str() {
                "none" => return Ok(PaintValue::None),
                "inherit" => return Ok(PaintValue::Inherit),
                "initial" => return Ok(PaintValue::Initial),
                "unset" => return Ok(PaintValue::Unset),
                "currentcolor" => return Ok(PaintValue::CurrentColor),
                _ => {}
            }
        }
    }
    let mut input = ParserInput::new(val);
    let mut parser = Parser::new(&mut input);
    if let Ok(url) = parser.try_parse(|p| p.expect_url()) {
        let id = local_reference(&url).map_err(|_| ())?;
        if !parser.is_exhausted()
            && parser
                .try_parse(|p| p.expect_ident_matching("none"))
                .is_err()
        {
            cssparser_color::Color::parse(&mut parser).map_err(|_| ())?;
        }
        parser.expect_exhausted().map_err(|_| ())?;
        return Ok(PaintValue::Resource(id.into()));
    }
    parse_color(val)
}

pub(super) fn local_reference(val: &str) -> Result<String, InvalidSvg> {
    let frag = val
        .trim()
        .strip_prefix('#')
        .filter(|s| !s.is_empty())
        .ok_or(InvalidSvg::ExternalReference)?;
    let id = percent_encoding::percent_decode_str(frag)
        .decode_utf8()
        .map_err(|_| InvalidSvg::ExternalReference)?;
    if !is_valid_id(&id) {
        return Err(InvalidSvg::ExternalReference);
    }
    Ok(id.into_owned())
}

pub(super) fn local_urls(src: &str) -> Result<Vec<String>, InvalidSvg> {
    let mut input = ParserInput::new(src);
    let mut parser = Parser::new(&mut input);
    let mut refs = Vec::new();
    while !parser.is_exhausted() {
        if let Ok(url) = parser.try_parse(|p| p.expect_url()) {
            refs.push(local_reference(&url)?);
            continue;
        }
        let tok = parser
            .next()
            .map_err(|_| InvalidSvg::ExternalReference)?
            .clone();
        if tok.is_parse_error() {
            return Err(InvalidSvg::ExternalReference);
        }
        if matches!(
            tok,
            Token::Function(_)
                | Token::ParenthesisBlock
                | Token::SquareBracketBlock
                | Token::CurlyBracketBlock
        ) {
            parser
                .parse_nested_block(|nested| consume_value(nested, 0))
                .map_err(|_| InvalidSvg::ExternalReference)?;
        }
    }
    Ok(refs)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ReferenceKind {
    Id,
    Fragment,
    IdRefs,
    Urls,
}

impl ReferenceKind {
    pub(super) fn has_references(self, val: &str) -> bool {
        match self {
            Self::Id | Self::Fragment | Self::IdRefs => true,
            Self::Urls => !local_urls(val)
                .expect("validated local SVG URLs")
                .is_empty(),
        }
    }
}

pub(super) fn reference_attribute(attr: &str) -> Option<ReferenceKind> {
    match attr {
        "id" => Some(ReferenceKind::Id),
        "href" => Some(ReferenceKind::Fragment),
        "aria-activedescendant"
        | "aria-controls"
        | "aria-describedby"
        | "aria-details"
        | "aria-errormessage"
        | "aria-flowto"
        | "aria-labelledby"
        | "aria-owns" => Some(ReferenceKind::IdRefs),
        "fill" | "stroke" | "clip-path" | "mask" | "filter" | "style" => Some(ReferenceKind::Urls),
        _ => None,
    }
}

pub(super) fn prefix_references(
    kind: ReferenceKind,
    src: &str,
    prefix: &str,
    out: &mut impl Write,
) -> fmt::Result {
    match kind {
        ReferenceKind::Id => write!(out, "{prefix}{src}"),
        ReferenceKind::Fragment => out.write_str(&prefixed_fragment(
            prefix,
            &local_reference(src).expect("validated local href"),
        )),
        ReferenceKind::IdRefs => {
            for (idx, id) in src.split_ascii_whitespace().enumerate() {
                if idx != 0 {
                    out.write_char(' ')?;
                }
                out.write_str(prefix)?;
                out.write_str(id)?;
            }
            Ok(())
        }
        ReferenceKind::Urls => prefix_urls(src, prefix, out),
    }
}

fn prefix_urls(src: &str, prefix: &str, out: &mut impl Write) -> fmt::Result {
    let mut input = ParserInput::new(src);
    let mut parser = Parser::new(&mut input);
    let mut pos = 0;
    while !parser.is_exhausted() {
        parser.skip_whitespace();
        let start = parser.position().byte_index();
        if let Ok(url) = parser.try_parse(|p| p.expect_url()) {
            let id = local_reference(&url).expect("validated local URL");
            out.write_str(&src[pos..start])?;
            out.write_str("url(")?;
            cssparser::serialize_string(&prefixed_fragment(prefix, &id), out)?;
            out.write_char(')')?;
            pos = parser.position().byte_index();
        } else if parser.next_including_whitespace_and_comments().is_err() {
            break;
        }
    }
    out.write_str(&src[pos..])
}

fn prefixed_fragment(prefix: &str, id: &str) -> String {
    const FRAGMENT_ENCODE_SET: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
        .add(b' ')
        .add(b'#')
        .add(b'%')
        .add(b'"')
        .add(b'\'')
        .add(b'<')
        .add(b'>')
        .add(b'`')
        .add(b'\\')
        .add(b'{')
        .add(b'}')
        .add(b'[')
        .add(b']')
        .add(b'^')
        .add(b'|');
    format!(
        "#{}",
        percent_encoding::utf8_percent_encode(&format!("{prefix}{id}"), FRAGMENT_ENCODE_SET)
    )
}

#[cfg(test)]
mod tests {
    use crate::SvgIcon;

    #[test]
    fn property_grammar() {
        for (prop, accepted, rejected) in [
            ("mix-blend-mode", "plus-lighter", "banana"),
            ("shape-rendering", "geometricPrecision", "banana"),
            ("isolation", "isolate", "normal"),
            ("paint-order", "stroke fill markers", "fill stroke fill"),
            ("paint-order", "markers", "normal fill"),
        ] {
            let svg = |v| format!(r#"<svg viewBox="0 0 24 24"><path style="{prop}:{v}"/></svg>"#);
            assert!(SvgIcon::parse(svg(accepted)).is_ok(), "{prop}:{accepted}");
            assert!(SvgIcon::parse(svg(rejected)).is_err(), "{prop}:{rejected}");
            for kw in ["inherit", "initial", "unset"] {
                assert!(SvgIcon::parse(svg(kw)).is_ok(), "{prop}:{kw}");
            }
        }
    }

    #[test]
    fn attrs_accept_inherit() {
        for attr in [
            "stroke-linejoin",
            "overflow",
            "color-interpolation",
            "color-interpolation-filters",
            "visibility",
        ] {
            let svg = format!(r#"<svg viewBox="0 0 24 24"><path {attr}="inherit"/></svg>"#);
            assert!(SvgIcon::parse(svg).is_ok(), "{attr}");
        }
    }
}
