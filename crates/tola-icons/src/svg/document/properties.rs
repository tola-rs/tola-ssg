//! Presentation properties read from an element's style attribute and CSS declarations.

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
    element: &str,
    source: &str,
) -> Result<BTreeMap<String, StyleDeclaration>, InvalidSvg> {
    let invalid = || InvalidSvg::Value {
        element: element.to_owned(),
        attribute: "style".to_owned(),
    };
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    let mut declarations: BTreeMap<String, StyleDeclaration> = BTreeMap::new();
    while !parser.is_exhausted() {
        if parser.try_parse(|parser| parser.expect_semicolon()).is_ok() {
            continue;
        }
        let name = parser
            .expect_ident_cloned()
            .map_err(|_| invalid())?
            .to_ascii_lowercase();
        if !is_presentation_property(&name) {
            return Err(InvalidSvg::Attribute {
                element: element.to_owned(),
                attribute: name,
            });
        }
        parser.expect_colon().map_err(|_| invalid())?;
        let mut declaration = parser
            .parse_until_before(Delimiter::Semicolon, |parser| {
                let value = parser.parse_until_before(Delimiter::Bang, |parser| {
                    let start = parser.position();
                    consume_value(parser, 0)?;
                    Ok(parser.slice_from(start).trim().to_owned())
                })?;
                let important = parser.try_parse(cssparser::parse_important).is_ok();
                parser.expect_exhausted()?;
                Ok::<_, cssparser::ParseError<'_, ()>>(StyleDeclaration { value, important })
            })
            .map_err(|_| invalid())?;
        declaration.value = normalize_property(&name, &declaration.value);
        validate_attribute(element, &name, &declaration.value)?;
        if declarations
            .get(&name)
            .is_none_or(|previous| declaration.important || !previous.important)
        {
            declarations.insert(name, declaration);
        }
        let _ = parser.try_parse(|parser| parser.expect_semicolon());
    }
    Ok(declarations)
}

fn consume_value<'i>(
    parser: &mut Parser<'i, '_>,
    depth: usize,
) -> Result<(), cssparser::ParseError<'i, ()>> {
    if depth > MAX_CSS_VALUE_DEPTH {
        return Err(parser.new_custom_error(()));
    }
    while !parser.is_exhausted() {
        let token = parser.next()?.clone();
        if token.is_parse_error() {
            return Err(parser.new_custom_error(()));
        }
        match token {
            Token::Function(_) | Token::ParenthesisBlock | Token::SquareBracketBlock => {
                parser.parse_nested_block(|nested| consume_value(nested, depth + 1))?;
            }
            Token::CurlyBracketBlock => return Err(parser.new_custom_error(())),
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn normalize_property(name: &str, value: &str) -> String {
    if is_presentation_property(name)
        && !matches!(
            name,
            "fill" | "stroke" | "color" | "stop-color" | "flood-color" | "lighting-color"
        )
    {
        let mut input = ParserInput::new(value);
        let mut parser = Parser::new(&mut input);
        if let Ok(keyword) = parser.expect_ident_cloned() {
            if parser.is_exhausted() {
                return keyword.to_ascii_lowercase();
            }
        }
    }
    value.to_owned()
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

fn invalid_value(element: &str, attribute: &str) -> InvalidSvg {
    InvalidSvg::Value {
        element: element.to_owned(),
        attribute: attribute.to_owned(),
    }
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_SVG_ID_BYTES && !id.chars().any(char::is_whitespace)
}

pub(super) fn validate_attribute(
    element: &str,
    attribute: &str,
    value: &str,
) -> Result<(), InvalidSvg> {
    let invalid = || invalid_value(element, attribute);
    if matches!(
        attribute,
        "class" | "role" | "style" | "xml:space" | "xml:lang" | "version"
    ) || attribute.starts_with("aria-")
        || attribute.starts_with("data-")
    {
        return Ok(());
    }
    if matches!(
        attribute,
        "opacity"
            | "fill-opacity"
            | "stroke-opacity"
            | "stop-opacity"
            | "flood-opacity"
            | "stroke-width"
            | "visibility"
            | "display"
            | "mask-type"
            | "paint-order"
            | "shape-rendering"
            | "isolation"
            | "mix-blend-mode"
    ) && matches!(value.trim(), "inherit" | "initial" | "unset")
    {
        return Ok(());
    }
    match attribute {
        "id" => is_valid_id(value).then_some(()).ok_or_else(invalid),
        "d" if element == "path" => validate_path(value),
        "viewBox" => parse_view_box(value).map(|_| ()),
        "preserveAspectRatio" => svgtypes::AspectRatio::from_str(value)
            .map(|_| ())
            .map_err(|_| invalid()),
        "href"
            if matches!(
                element,
                "use" | "linearGradient" | "radialGradient" | "pattern"
            ) =>
        {
            local_reference(value).map(|_| ())
        }
        "transform" | "gradientTransform" | "patternTransform" => {
            let transform = svgtypes::Transform::from_str(value).map_err(|_| invalid())?;
            [
                transform.a,
                transform.b,
                transform.c,
                transform.d,
                transform.e,
                transform.f,
            ]
            .iter()
            .all(|number| number.is_finite())
            .then_some(())
            .ok_or_else(invalid)
        }
        "fill" | "stroke" => parse_paint(value).map(|_| ()).map_err(|_| invalid()),
        "color" | "stop-color" | "flood-color" | "lighting-color" => {
            parse_color(value).map(|_| ()).map_err(|_| invalid())
        }
        "clip-path" | "mask" | "filter" => {
            if value.trim().eq_ignore_ascii_case("none") {
                return Ok(());
            }
            let references = local_urls(value)?;
            if references.len() != 1 {
                return Err(invalid());
            }
            let mut input = ParserInput::new(value);
            let mut parser = Parser::new(&mut input);
            parser.expect_url().map_err(|_| invalid())?;
            parser.expect_exhausted().map_err(|_| invalid())
        }
        "x" | "y" | "x1" | "y1" | "x2" | "y2" | "cx" | "cy" | "fx" | "fy" | "width" | "height"
        | "r" | "rx" | "ry" | "fr" | "stroke-width" | "stroke-dashoffset" => {
            let length = svgtypes::Length::from_str(value.trim()).map_err(|_| invalid())?;
            let nonnegative = matches!(
                attribute,
                "width" | "height" | "r" | "rx" | "ry" | "fr" | "stroke-width"
            );
            if !length.number.is_finite() || (nonnegative && length.number < 0.0) {
                return Err(invalid());
            }
            Ok(())
        }
        "points" => {
            let count = validate_number_list(value).map_err(|_| invalid())?;
            (count % 2 == 0).then_some(()).ok_or_else(invalid)
        }
        "stroke-dasharray" => {
            if value.trim() == "none" {
                return Ok(());
            }
            for length in svgtypes::LengthListParser::from(value) {
                let length = length.map_err(|_| invalid())?;
                if !length.number.is_finite() || length.number < 0.0 {
                    return Err(invalid());
                }
            }
            Ok(())
        }
        "opacity" | "fill-opacity" | "stroke-opacity" | "stop-opacity" | "flood-opacity"
        | "offset" => {
            let source = value.trim().strip_suffix('%').unwrap_or(value.trim());
            let number = source.parse::<f64>().map_err(|_| invalid())?;
            number.is_finite().then_some(()).ok_or_else(invalid)
        }
        "pathLength" | "stroke-miterlimit" => {
            let number = value.trim().parse::<f64>().map_err(|_| invalid())?;
            (number.is_finite() && number >= 0.0)
                .then_some(())
                .ok_or_else(invalid)
        }
        "fill-rule" | "clip-rule" => {
            validate_keyword(value, &["nonzero", "evenodd", "inherit"]).map_err(|_| invalid())
        }
        "stroke-linecap" => {
            validate_keyword(value, &["butt", "round", "square", "inherit"]).map_err(|_| invalid())
        }
        "stroke-linejoin" => validate_keyword(
            value,
            &["miter", "miter-clip", "round", "bevel", "arcs", "inherit"],
        )
        .map_err(|_| invalid()),
        "gradientUnits"
        | "patternUnits"
        | "patternContentUnits"
        | "clipPathUnits"
        | "maskUnits"
        | "maskContentUnits"
        | "filterUnits"
        | "primitiveUnits" => {
            validate_keyword(value, &["userSpaceOnUse", "objectBoundingBox"]).map_err(|_| invalid())
        }
        "spreadMethod" => {
            validate_keyword(value, &["pad", "reflect", "repeat"]).map_err(|_| invalid())
        }
        "mask-type" => validate_keyword(value, &["luminance", "alpha"]).map_err(|_| invalid()),
        "display" => {
            validate_keyword(value, &["none", "inline", "block", "inherit"]).map_err(|_| invalid())
        }
        "visibility" => validate_keyword(value, &["visible", "hidden", "collapse", "inherit"])
            .map_err(|_| invalid()),
        "overflow" => validate_keyword(value, &["visible", "hidden", "scroll", "auto", "inherit"])
            .map_err(|_| invalid()),
        "vector-effect" => {
            validate_keyword(value, &["none", "non-scaling-stroke"]).map_err(|_| invalid())
        }
        "color-interpolation" | "color-interpolation-filters" => {
            validate_keyword(value, &["auto", "srgb", "linearrgb", "inherit"])
                .map_err(|_| invalid())
        }
        "paint-order" => validate_paint_order(value).map_err(|_| invalid()),
        "shape-rendering" => validate_keyword(
            value,
            &["auto", "optimizespeed", "crispedges", "geometricprecision"],
        )
        .map_err(|_| invalid()),
        "isolation" => validate_keyword(value, &["auto", "isolate"]).map_err(|_| invalid()),
        "mix-blend-mode" => {
            if value == "plus-lighter" {
                return Ok(());
            }
            validate_keyword(value, BLEND_MODES).map_err(|_| invalid())
        }
        "in" | "in2" | "result" if element.starts_with("fe") => {
            let mut input = ParserInput::new(value);
            let mut parser = Parser::new(&mut input);
            let identifier = parser.expect_ident().map_err(|_| invalid())?;
            if matches!(
                identifier.to_ascii_lowercase().as_str(),
                "initial" | "inherit" | "unset" | "revert" | "revert-layer" | "default"
            ) {
                return Err(invalid());
            }
            parser.expect_exhausted().map_err(|_| invalid())
        }
        "type" if element == "feColorMatrix" => validate_keyword(
            value,
            &["matrix", "saturate", "hueRotate", "luminanceToAlpha"],
        )
        .map_err(|_| invalid()),
        "type" if matches!(element, "feFuncA" | "feFuncR" | "feFuncG" | "feFuncB") => {
            validate_keyword(value, &["identity", "table", "discrete", "linear", "gamma"])
                .map_err(|_| invalid())
        }
        "type" if element == "feTurbulence" => {
            validate_keyword(value, &["fractalNoise", "turbulence"]).map_err(|_| invalid())
        }
        "mode" if element == "feBlend" => {
            validate_keyword(value, BLEND_MODES).map_err(|_| invalid())
        }
        "operator" if element == "feComposite" => validate_keyword(
            value,
            &["over", "in", "out", "atop", "xor", "arithmetic", "lighter"],
        )
        .map_err(|_| invalid()),
        "operator" if element == "feMorphology" => {
            validate_keyword(value, &["erode", "dilate"]).map_err(|_| invalid())
        }
        "edgeMode" if element == "feConvolveMatrix" => {
            validate_keyword(value, &["duplicate", "wrap", "none"]).map_err(|_| invalid())
        }
        "stitchTiles" if element == "feTurbulence" => {
            validate_keyword(value, &["stitch", "noStitch"]).map_err(|_| invalid())
        }
        "preserveAlpha" if element == "feConvolveMatrix" => {
            validate_keyword(value, &["true", "false"]).map_err(|_| invalid())
        }
        "values" | "stdDeviation" | "dx" | "dy" | "k1" | "k2" | "k3" | "k4" | "slope"
        | "intercept" | "amplitude" | "exponent" | "tableValues" | "order" | "kernelMatrix"
        | "divisor" | "bias" | "targetX" | "targetY" | "kernelUnitLength" | "surfaceScale"
        | "diffuseConstant" | "specularConstant" | "specularExponent" | "scale" | "radius"
        | "azimuth" | "elevation" | "z" | "pointsAtX" | "pointsAtY" | "pointsAtZ"
        | "limitingConeAngle" | "baseFrequency" | "numOctaves" | "seed"
            if element.starts_with("fe") =>
        {
            validate_number_list(value)
                .map(|_| ())
                .map_err(|_| invalid())
        }
        "xChannelSelector" | "yChannelSelector" if element == "feDisplacementMap" => {
            validate_keyword(value, &["R", "G", "B", "A"]).map_err(|_| invalid())
        }
        _ => Err(InvalidSvg::Attribute {
            element: element.to_owned(),
            attribute: attribute.to_owned(),
        }),
    }
}

fn validate_keyword(value: &str, keywords: &[&str]) -> Result<(), ()> {
    keywords.contains(&value.trim()).then_some(()).ok_or(())
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

fn validate_paint_order(value: &str) -> Result<(), ()> {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let mut seen = 0_u8;
    while !parser.is_exhausted() {
        let keyword = parser.expect_ident_cloned().map_err(|_| ())?;
        let part = match keyword.to_ascii_lowercase().as_str() {
            "normal" if seen == 0 && parser.is_exhausted() => return Ok(()),
            "fill" => 1,
            "stroke" => 2,
            "markers" => 4,
            _ => return Err(()),
        };
        if seen & part != 0 {
            return Err(());
        }
        seen |= part;
    }
    (seen != 0).then_some(()).ok_or(())
}

fn validate_number_list(value: &str) -> Result<usize, ()> {
    let mut count = 0;
    for number in svgtypes::NumberListParser::from(value) {
        if !number.map_err(|_| ())?.is_finite() {
            return Err(());
        }
        count += 1;
    }
    Ok(count)
}

fn validate_path(value: &str) -> Result<(), InvalidSvg> {
    use svgtypes::PathSegment;
    for segment in svgtypes::PathParser::from(value) {
        let segment = segment.map_err(|_| InvalidSvg::Path)?;
        let finite = match segment {
            PathSegment::MoveTo { x, y, .. }
            | PathSegment::LineTo { x, y, .. }
            | PathSegment::SmoothQuadratic { x, y, .. } => {
                [x, y].iter().all(|number| number.is_finite())
            }
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
            } => [x1, y1, x2, y2, x, y]
                .iter()
                .all(|number| number.is_finite()),
            PathSegment::SmoothCurveTo { x2, y2, x, y, .. } => {
                [x2, y2, x, y].iter().all(|number| number.is_finite())
            }
            PathSegment::Quadratic { x1, y1, x, y, .. } => {
                [x1, y1, x, y].iter().all(|number| number.is_finite())
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
                .all(|number| number.is_finite()),
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

fn parse_color(value: &str) -> Result<PaintValue, ()> {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    if let Ok(keyword) = parser.try_parse(|parser| parser.expect_ident_cloned()) {
        if parser.is_exhausted() {
            match keyword.to_ascii_lowercase().as_str() {
                "inherit" => return Ok(PaintValue::Inherit),
                "initial" => return Ok(PaintValue::Initial),
                "unset" => return Ok(PaintValue::Unset),
                _ => {}
            }
        }
    }
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let color = cssparser_color::Color::parse(&mut parser).map_err(|_| ())?;
    parser.expect_exhausted().map_err(|_| ())?;
    use cssparser_color::Color;
    let alpha = match color {
        Color::CurrentColor => return Ok(PaintValue::CurrentColor),
        Color::Rgba(color) => Some(color.alpha),
        Color::Hsl(color) => color.alpha,
        Color::Hwb(color) => color.alpha,
        Color::Lab(color) => color.alpha,
        Color::Lch(color) => color.alpha,
        Color::Oklab(color) => color.alpha,
        Color::Oklch(color) => color.alpha,
        Color::ColorFunction(color) => color.alpha,
    };
    // Keep zero-alpha color distinct from no paint: SVG gradient interpolation still uses RGB.
    // A missing component is zero when the stop color is resolved outside CSS interpolation.
    Ok(if alpha.unwrap_or(0.0) == 0.0 {
        PaintValue::Transparent
    } else {
        PaintValue::Fixed
    })
}

pub(super) fn parse_paint(value: &str) -> Result<PaintValue, ()> {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    if let Ok(keyword) = parser.try_parse(|parser| parser.expect_ident_cloned()) {
        if parser.is_exhausted() {
            if keyword.eq_ignore_ascii_case("none") {
                return Ok(PaintValue::None);
            }
            if keyword.eq_ignore_ascii_case("inherit") {
                return Ok(PaintValue::Inherit);
            }
            if keyword.eq_ignore_ascii_case("initial") {
                return Ok(PaintValue::Initial);
            }
            if keyword.eq_ignore_ascii_case("unset") {
                return Ok(PaintValue::Unset);
            }
            if keyword.eq_ignore_ascii_case("currentcolor") {
                return Ok(PaintValue::CurrentColor);
            }
        }
    }
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    if let Ok(url) = parser.try_parse(|parser| parser.expect_url()) {
        let id = local_reference(&url).map_err(|_| ())?;
        if !parser.is_exhausted() {
            // A validated local paint server resolves; still validate the syntactic fallback.
            if parser
                .try_parse(|parser| parser.expect_ident_matching("none"))
                .is_err()
            {
                cssparser_color::Color::parse(&mut parser).map_err(|_| ())?;
            }
        }
        parser.expect_exhausted().map_err(|_| ())?;
        return Ok(PaintValue::Resource(id.into()));
    }
    parse_color(value)
}

pub(super) fn local_reference(value: &str) -> Result<String, InvalidSvg> {
    let fragment = value
        .trim()
        .strip_prefix('#')
        .filter(|fragment| !fragment.is_empty())
        .ok_or(InvalidSvg::ExternalReference)?;
    let id = percent_encoding::percent_decode_str(fragment)
        .decode_utf8()
        .map_err(|_| InvalidSvg::ExternalReference)?;
    if !is_valid_id(&id) {
        return Err(InvalidSvg::ExternalReference);
    }
    Ok(id.into_owned())
}

pub(super) fn local_urls(source: &str) -> Result<Vec<String>, InvalidSvg> {
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    let mut references = Vec::new();
    while !parser.is_exhausted() {
        if let Ok(url) = parser.try_parse(|parser| parser.expect_url()) {
            references.push(local_reference(&url)?);
            continue;
        }
        let token = parser
            .next()
            .map_err(|_| InvalidSvg::ExternalReference)?
            .clone();
        if token.is_parse_error() {
            return Err(InvalidSvg::ExternalReference);
        }
        if matches!(
            token,
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
    Ok(references)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ReferenceKind {
    Id,
    Fragment,
    IdRefs,
    Urls,
}

impl ReferenceKind {
    pub(super) fn has_references(self, value: &str) -> bool {
        match self {
            Self::Id | Self::Fragment | Self::IdRefs => true,
            Self::Urls => !local_urls(value)
                .expect("validated local SVG URLs")
                .is_empty(),
        }
    }
}

pub(super) fn reference_attribute(attribute: &str) -> Option<ReferenceKind> {
    match attribute {
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

// `prefix` is the already length-delimited instance component, shared by every reference.
pub(super) fn prefix_references(
    kind: ReferenceKind,
    source: &str,
    prefix: &str,
    output: &mut impl Write,
) -> fmt::Result {
    match kind {
        ReferenceKind::Id => write!(output, "{prefix}{source}"),
        ReferenceKind::Fragment => output.write_str(&prefixed_fragment(
            prefix,
            &local_reference(source).expect("validated local href"),
        )),
        ReferenceKind::IdRefs => {
            for (index, id) in source.split_ascii_whitespace().enumerate() {
                if index != 0 {
                    output.write_char(' ')?;
                }
                output.write_str(prefix)?;
                output.write_str(id)?;
            }
            Ok(())
        }
        ReferenceKind::Urls => prefix_urls(source, prefix, output),
    }
}

fn prefix_urls(source: &str, prefix: &str, output: &mut impl Write) -> fmt::Result {
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    let mut copied_until = 0;
    while !parser.is_exhausted() {
        parser.skip_whitespace();
        let start = parser.position().byte_index();
        if let Ok(url) = parser.try_parse(|parser| parser.expect_url()) {
            let id = local_reference(&url).expect("validated local URL");
            output.write_str(&source[copied_until..start])?;
            output.write_str("url(")?;
            cssparser::serialize_string(&prefixed_fragment(prefix, &id), output)?;
            output.write_char(')')?;
            copied_until = parser.position().byte_index();
        } else if parser.next_including_whitespace_and_comments().is_err() {
            break;
        }
    }
    output.write_str(&source[copied_until..])
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
    fn values_follow_their_property_grammar() {
        for (property, accepted, rejected) in [
            ("mix-blend-mode", "plus-lighter", "banana"),
            ("shape-rendering", "geometricPrecision", "banana"),
            ("isolation", "isolate", "normal"),
            ("paint-order", "stroke fill markers", "fill stroke fill"),
            ("paint-order", "markers", "normal fill"),
        ] {
            let svg = |value| {
                format!(r#"<svg viewBox="0 0 24 24"><path style="{property}:{value}"/></svg>"#)
            };
            assert!(
                SvgIcon::parse(svg(accepted)).is_ok(),
                "{property}:{accepted}"
            );
            assert!(
                SvgIcon::parse(svg(rejected)).is_err(),
                "{property}:{rejected}"
            );
            for keyword in ["inherit", "initial", "unset"] {
                assert!(SvgIcon::parse(svg(keyword)).is_ok(), "{property}:{keyword}");
            }
        }
    }

    #[test]
    fn filter_keywords_are_primitive_specific() {
        for (element, attribute, accepted, rejected) in [
            ("feColorMatrix", "type", "hueRotate", "gamma"),
            ("feFuncR", "type", "gamma", "hueRotate"),
            ("feTurbulence", "type", "fractalNoise", "matrix"),
            ("feMorphology", "operator", "dilate", "over"),
            ("feComposite", "operator", "over", "dilate"),
            ("feBlend", "mode", "multiply", "plus-lighter"),
            ("feConvolveMatrix", "preserveAlpha", "true", "banana"),
        ] {
            let svg = |value| {
                format!(
                    r#"<svg viewBox="0 0 24 24"><filter><{element} {attribute}="{value}"/></filter></svg>"#
                )
            };
            assert!(SvgIcon::parse(svg(accepted)).is_ok());
            assert!(SvgIcon::parse(svg(rejected)).is_err());
        }
    }
}
