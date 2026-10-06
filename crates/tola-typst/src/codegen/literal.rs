//! Parsers for explicit Typst literal strings.
//!
//! Content reconstruction applies them only when the parameter accepts the parsed
//! type; native JSON serialization uses explicit type tags.

use typst::foundations::Value;
use typst::layout::{Abs, Angle, Em, Length, Ratio};
use typst::visualize::Color;

/// Parse a Typst literal, returning `None` for plain strings.
///
/// `auto`, `none`, `true`, and `false` are the only special spellings; anything
/// else is attempted as a length, then angle, then ratio, then colour.
pub fn parse_typst_literal(s: &str) -> Option<Value> {
    let s = s.trim();

    match s {
        "auto" => return Some(Value::Auto),
        "none" => return Some(Value::None),
        "true" => return Some(Value::Bool(true)),
        "false" => return Some(Value::Bool(false)),
        _ => {}
    }

    if let Some(length) = parse_length(s) {
        return Some(Value::Length(length));
    }

    if let Some(angle) = parse_angle(s) {
        return Some(Value::Angle(angle));
    }

    if let Some(ratio) = parse_ratio(s) {
        return Some(Value::Ratio(ratio));
    }

    if let Some(color) = parse_color(s) {
        return Some(Value::Color(color));
    }

    None
}

/// Absolute units `pt`, `mm`, `cm`, and `in`, plus relative `em`.
pub fn parse_length(s: &str) -> Option<Length> {
    let s = s.trim();

    for (suffix, factor) in [
        ("pt", 1.0),
        ("mm", 2.834_645_669_291_339),
        ("cm", 28.346_456_692_913_39),
        ("in", 72.0),
    ] {
        if let Some(num_str) = s.strip_suffix(suffix)
            && let Ok(n) = num_str.trim().parse::<f64>()
        {
            return Some(Abs::pt(n * factor).into());
        }
    }

    if let Some(num_str) = s.strip_suffix("em")
        && let Ok(n) = num_str.trim().parse::<f64>()
    {
        return Some(Em::new(n).into());
    }

    None
}

/// `deg`, `rad`, and `turn`.
pub fn parse_angle(s: &str) -> Option<Angle> {
    let s = s.trim();

    if let Some(num_str) = s.strip_suffix("deg")
        && let Ok(n) = num_str.trim().parse::<f64>()
    {
        return Some(Angle::deg(n));
    }

    if let Some(num_str) = s.strip_suffix("rad")
        && let Ok(n) = num_str.trim().parse::<f64>()
    {
        return Some(Angle::rad(n));
    }

    if let Some(num_str) = s.strip_suffix("turn")
        && let Ok(n) = num_str.trim().parse::<f64>()
    {
        return Some(Angle::deg(n * 360.0));
    }

    None
}

/// A percentage suffix over a plain number.
pub fn parse_ratio(s: &str) -> Option<Ratio> {
    let s = s.trim();

    if let Some(num_str) = s.strip_suffix('%')
        && let Ok(n) = num_str.trim().parse::<f64>()
    {
        return Some(Ratio::new(n / 100.0));
    }

    None
}

/// Hex colours: `#rgb`, `#rrggbb`, and `#rrggbbaa`.
pub fn parse_color(s: &str) -> Option<Color> {
    let hex = s.trim().strip_prefix('#')?.as_bytes();

    match hex {
        [r, g, b] => Some(Color::from_u8(
            parse_hex_digit(*r)? * 17,
            parse_hex_digit(*g)? * 17,
            parse_hex_digit(*b)? * 17,
            255,
        )),
        [r1, r2, g1, g2, b1, b2] => Some(Color::from_u8(
            parse_hex_byte(*r1, *r2)?,
            parse_hex_byte(*g1, *g2)?,
            parse_hex_byte(*b1, *b2)?,
            255,
        )),
        [r1, r2, g1, g2, b1, b2, a1, a2] => Some(Color::from_u8(
            parse_hex_byte(*r1, *r2)?,
            parse_hex_byte(*g1, *g2)?,
            parse_hex_byte(*b1, *b2)?,
            parse_hex_byte(*a1, *a2)?,
        )),
        _ => None,
    }
}

fn parse_hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_hex_byte(high: u8, low: u8) -> Option<u8> {
    Some(parse_hex_digit(high)? * 16 + parse_hex_digit(low)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_literals_convert_to_typst_units() {
        assert_eq!(parse_length("12pt"), Some(Abs::pt(12.0).into()));
        assert_eq!(parse_length("1.5em"), Some(Em::new(1.5).into()));

        let Length { abs, em: _ } = parse_length("10mm").unwrap();
        assert!((abs.to_pt() - 28.346_456_692_913_39).abs() < 0.001);

        assert_eq!(parse_angle("90deg"), Some(Angle::deg(90.0)));
        let radians = parse_angle("3.14159rad").unwrap();
        assert!((radians.to_rad() - std::f64::consts::PI).abs() < 0.0001);

        assert_eq!(parse_ratio("50%"), Some(Ratio::new(0.5)));
    }

    #[test]
    fn color_literals_accept_every_width() {
        for (literal, expected) in [
            ("#f00", Color::from_u8(255, 0, 0, 255)),
            ("#ff0000", Color::from_u8(255, 0, 0, 255)),
            ("#ff000080", Color::from_u8(255, 0, 0, 128)),
        ] {
            assert_eq!(parse_color(literal), Some(expected), "{literal}");
        }
    }

    #[test]
    fn invalid_color_literals_are_rejected() {
        for input in ["#éa", "#aéabc", "#aéabcde", "#+f0000", "#中123"] {
            assert_eq!(parse_color(input), None, "{input}");
        }
    }

    #[test]
    fn only_special_spellings_are_typed() {
        assert_eq!(parse_typst_literal("auto"), Some(Value::Auto));
        assert_eq!(parse_typst_literal("none"), Some(Value::None));
        assert_eq!(parse_typst_literal("true"), Some(Value::Bool(true)));
        assert_eq!(parse_typst_literal("false"), Some(Value::Bool(false)));

        for plain in ["hello", "12", "just some text"] {
            assert!(parse_typst_literal(plain).is_none(), "{plain}");
        }
    }
}
