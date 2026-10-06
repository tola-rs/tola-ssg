//! The colours one source writes.
//!
//! A colour is `rgb` called with a spelling Typst itself reads, so a caller can paint the swatch
//! without compiling the site. A form the reader cannot decide is left unanswered rather than
//! guessed at.

use std::ops::Range;
use std::str::FromStr;

use typst_library::visualize::Color;
use typst_syntax::ast::AstNode;
use typst_syntax::{LinkedNode, Source, SyntaxKind, ast};

/// One colour a source writes, with the bytes of the spelling that names it.
pub struct Colour {
    /// The bytes of the argument spelling that names the colour.
    pub range: Range<usize>,
    /// The colour that spelling reads as.
    pub value: Color,
}

/// Every colour the source writes, in document order.
pub fn colours(source: &Source) -> Vec<Colour> {
    let mut colours = Vec::new();
    let mut stack = vec![LinkedNode::new(source.root())];
    while let Some(node) = stack.pop() {
        if node.kind() == SyntaxKind::FuncCall
            && let Some(colour) = call_colour(&node)
        {
            colours.push(colour);
            continue;
        }
        stack.extend(node.children());
    }
    colours.sort_by_key(|colour| colour.range.start);
    colours
}

/// The forms the author can paste back for one colour, as red, green, blue, and alpha.
///
/// `rgb` accepts both spellings: the hexadecimal notation, which carries alpha as its fourth pair
/// of digits, and whole components out of 255 with a ratio for alpha.
pub fn spellings(components: [f32; 4]) -> Vec<String> {
    let [red, green, blue, alpha] = components;
    let whole = |component: f32| (component.clamp(0.0, 1.0) * 255.0).round() as u8;
    let components = format!("{}, {}, {}", whole(red), whole(green), whole(blue));
    // Alpha is the fourth component of the same call, never a second one.
    let ratio = if alpha < 1.0 {
        format!(", {alpha:.3}")
    } else {
        String::new()
    };
    vec![
        format!("rgb(\"{}\")", hex([red, green, blue, alpha])),
        format!("rgb({components}{ratio})"),
    ]
}

/// The hexadecimal notation of one colour, with alpha when it is not opaque.
pub fn hex(components: [f32; 4]) -> String {
    let [red, green, blue, alpha] = components;
    let whole = |component: f32| (component.clamp(0.0, 1.0) * 255.0).round() as u8;
    if alpha < 1.0 {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            whole(red),
            whole(green),
            whole(blue),
            whole(alpha)
        )
    } else {
        format!("#{:02x}{:02x}{:02x}", whole(red), whole(green), whole(blue))
    }
}

/// The colour one call writes, with the bytes of the spelling that names it.
fn call_colour(node: &LinkedNode<'_>) -> Option<Colour> {
    let call = node.cast::<ast::FuncCall>()?;
    if name_of(call.callee()).as_deref() != Some("rgb") {
        return None;
    }
    let mut arguments = call.args().items();
    let (Some(ast::Arg::Pos(ast::Expr::Str(text))), None) = (arguments.next(), arguments.next())
    else {
        return None;
    };
    let value = Color::from_str(text.get().as_str()).ok()?;
    let span = node.find(text.span())?;
    Some(Colour {
        range: span.range(),
        value,
    })
}

/// The name a callee writes, whether bare or as `color.<name>`.
fn name_of(callee: ast::Expr<'_>) -> Option<String> {
    match callee {
        ast::Expr::Ident(ident) => Some(ident.get().to_string()),
        ast::Expr::FieldAccess(access) => {
            let ast::Expr::Ident(target) = access.target() else {
                return None;
            };
            (target.get().as_str() == "color").then(|| access.field().get().to_string())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every colour of a source, as its spelling and its hexadecimal notation.
    fn written(text: &str) -> Vec<(String, String)> {
        let source = Source::detached(text);
        colours(&source)
            .into_iter()
            .map(|colour| {
                let components = colour.value.to_rgb().into_components();
                (
                    text[colour.range.clone()].to_owned(),
                    hex([components.0, components.1, components.2, components.3]),
                )
            })
            .collect()
    }

    #[test]
    fn hexadecimal_spelling_yields_colour() {
        assert_eq!(
            written("#let brand = rgb(\"#777\")\n"),
            [("\"#777\"".to_owned(), "#777777".to_owned())]
        );
    }

    #[test]
    fn undecidable_colour_answers_nothing() {
        assert!(written("#let brand = rgb(\"nonsense\")\n").is_empty());
        assert!(written("#let brand = not-a-colour(\"#777\")\n").is_empty());
    }

    #[test]
    fn alpha_survives_the_spellings() {
        let spellings = spellings([1.0, 0.0, 0.0, 0.5]);
        assert_eq!(spellings[0], "rgb(\"#ff000080\")");
        assert_eq!(spellings[1], "rgb(255, 0, 0, 0.500)");
    }
}
