//! The colours one source writes.

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
pub fn colours(src: &Source) -> Vec<Colour> {
    let mut found = Vec::new();
    let mut stack = vec![LinkedNode::new(src.root())];
    while let Some(node) = stack.pop() {
        if node.kind() == SyntaxKind::FuncCall
            && let Some(colour) = call_colour(&node)
        {
            found.push(colour);
            continue;
        }
        stack.extend(node.children());
    }
    found.sort_by_key(|colour| colour.range.start);
    found
}

/// The forms the author can paste back for one colour, as red, green, blue, and alpha.
pub fn spellings(components: [f32; 4]) -> Vec<String> {
    let [r, g, b, a] = components;
    let rgb = format!("{}, {}, {}", whole(r), whole(g), whole(b));
    let ratio = if a < 1.0 {
        format!(", {a:.3}")
    } else {
        String::new()
    };
    vec![
        format!("rgb(\"{}\")", hex([r, g, b, a])),
        format!("rgb({rgb}{ratio})"),
    ]
}

/// The hexadecimal notation of one colour, with alpha when it is not opaque.
pub fn hex(components: [f32; 4]) -> String {
    let [r, g, b, a] = components;
    if a < 1.0 {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            whole(r),
            whole(g),
            whole(b),
            whole(a)
        )
    } else {
        format!("#{:02x}{:02x}{:02x}", whole(r), whole(g), whole(b))
    }
}

fn whole(component: f32) -> u8 {
    (component.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn call_colour(node: &LinkedNode<'_>) -> Option<Colour> {
    let call = node.cast::<ast::FuncCall>()?;
    let rgb = match call.callee() {
        ast::Expr::Ident(name) => name.get().as_str() == "rgb",
        ast::Expr::FieldAccess(field) => {
            matches!(field.target(), ast::Expr::Ident(name) if name.get().as_str() == "color")
                && field.field().get().as_str() == "rgb"
        }
        _ => false,
    };
    if !rgb {
        return None;
    }
    let mut args = call.args().items();
    let (Some(ast::Arg::Pos(ast::Expr::Str(text))), None) = (args.next(), args.next()) else {
        return None;
    };
    let val = Color::from_str(text.get().as_str()).ok()?;
    let span = node.find(text.span())?;
    Some(Colour {
        range: span.range(),
        value: val,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_hex_rgb() {
        let text = "#rgb(\"nonsense\") #other.rgb(\"#777\") #color.rgb(\"#ff000080\")";
        let found = colours(&Source::detached(text));
        assert_eq!(found.len(), 1);
        assert_eq!(&text[found[0].range.clone()], "\"#ff000080\"");
        assert_eq!(found[0].value, Color::from_str("#ff000080").unwrap());
    }
}
