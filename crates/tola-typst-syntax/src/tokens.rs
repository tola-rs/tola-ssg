//! Token classes and inherited styles, with names supplied by the source/import index.

use std::ops::Range;

use typst_syntax::ast::{AstNode, MathDelimited};
use typst_syntax::{LinkedNode, Source, SyntaxKind};

use crate::imports::{NameClass, NameGraph};

/// Grammar and name-identity categories, independent of an editor's numeric token legend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenClass {
    /// Ordinary markup leaves, including math atoms that are not identifier lookups.
    Text,
    /// Heading syntax carries its marker and contents as one category.
    Heading,
    /// The marker of a list, an enum, or a term.
    Marker,
    /// The text before a term-list colon, preserving nested emphasis modifiers.
    Term,
    /// A source label declaration, including its angle delimiters.
    Label,
    /// The reference marker preceding a label or citation target.
    Ref,
    /// An automatically recognized URL in markup.
    Link,
    /// Raw markup spans retain one class regardless of their declared code language.
    Raw,
    /// Control words and literal sentinels such as `none` and `auto`.
    Keyword,
    /// Boolean literals remain distinct from other language keywords.
    Bool,
    /// Numeric literals, including values with units.
    Number,
    /// Quoted strings, including import paths and their delimiters.
    String,
    /// Line and block comments are not traversed as embedded code.
    Comment,
    /// Operators include field-access dots and math attachment markers.
    Operator,
    /// Code delimiters and separators, distinct from paired math boundaries.
    Punctuation,
    /// An identifier or pattern placeholder without a more specific class.
    Interpolated,
    /// An escape, a line break, or a shorthand.
    Escape,
    /// Equation and paired math boundaries; the enclosed expressions keep their own tokens.
    Delimiter,
    /// Text the parser could not read.
    Error,
    /// Includes function-valued references, even when they are not called.
    Function,
    /// Known modules remain namespaces even when written in a syntactic callee position.
    Namespace,
}

/// The styles a construct applies to everything inside it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers(u32);

impl Modifiers {
    /// Empty style set; enclosing markup can add modifiers while walking children.
    pub const NONE: Self = Self(0);
    /// Mathematical notation, from an equation or a math block.
    pub const MATH: Self = Self(1 << 0);
    /// Strong markup applies this style throughout its nested expressions.
    pub const STRONG: Self = Self(1 << 1);
    /// Emphasis combines with inherited strong and math styles instead of replacing them.
    pub const EMPH: Self = Self(1 << 2);

    /// Whether every style in `other` applies here.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The styles as bits, in the order a caller's own legend carries them.
    pub const fn bits(self) -> u32 {
        self.0
    }

    const fn or(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// A byte span that may cross lines; protocol adapters split it before encoding positions.
pub struct Token {
    /// UTF-8 offsets in the supplied source, not line/column or UTF-16 positions.
    pub range: Range<usize>,
    /// Grammar classification, refined by a matching source-name identity when available.
    pub class: TokenClass,
    /// Combined styles from all enclosing markup and mathematical constructs.
    pub modifiers: Modifiers,
}

/// Produces tokens in source order; `names` must describe the same source version.
pub fn tokens(source: &Source, names: &NameGraph) -> Vec<Token> {
    let classify = |range: &Range<usize>| {
        names.class_at(source.id(), range).map(|class| match class {
            NameClass::Function => TokenClass::Function,
            NameClass::Namespace => TokenClass::Namespace,
            NameClass::Value => TokenClass::Interpolated,
        })
    };
    let mut tokens = Vec::new();
    walk_text(
        &LinkedNode::new(source.root()),
        Modifiers::NONE,
        None,
        &classify,
        &mut tokens,
    );
    tokens
}

/// Walk one node, with `text` naming the class plain text takes inside the enclosing construct.
fn walk_text(
    node: &LinkedNode<'_>,
    modifiers: Modifiers,
    text: Option<TokenClass>,
    classify: &impl Fn(&Range<usize>) -> Option<TokenClass>,
    tokens: &mut Vec<Token>,
) {
    let inherited = modifiers.or(style(node.kind()));
    if let Some(group) = node.cast::<MathDelimited>() {
        for child in node.children() {
            if child.span() == group.open().span() || child.span() == group.close().span() {
                tokens.push(Token {
                    range: child.range(),
                    class: TokenClass::Delimiter,
                    modifiers: inherited,
                });
            } else {
                walk_text(&child, inherited, text, classify, tokens);
            }
        }
        return;
    }
    if node.kind() == SyntaxKind::TermItem {
        // The term of `/ term: details` is the text between the marker and the colon.
        let marker_end = node
            .children()
            .next()
            .map_or(node.range().start, |marker| marker.range().end);
        let colon_start = node
            .children()
            .find(|child| child.kind() == SyntaxKind::Colon)
            .map_or(node.range().end, |colon| colon.range().start);
        for child in node.children() {
            let is_term = child.range().start >= marker_end && child.range().end <= colon_start;
            walk_text(
                &child,
                inherited,
                is_term.then_some(TokenClass::Term),
                classify,
                tokens,
            );
        }
        return;
    }
    match token_class(node, classify) {
        Some(TokenClass::Text) => {
            let class = text.unwrap_or(TokenClass::Text);
            tokens.push(Token {
                range: node.range(),
                class,
                modifiers: inherited,
            });
        }
        Some(class) => tokens.push(Token {
            range: node.range(),
            class,
            modifiers: inherited,
        }),
        None => {
            for child in node.children() {
                walk_text(&child, inherited, text, classify, tokens);
            }
        }
    }
}

fn style(kind: SyntaxKind) -> Modifiers {
    match kind {
        SyntaxKind::Strong => Modifiers::STRONG,
        SyntaxKind::Emph => Modifiers::EMPH,
        SyntaxKind::Math | SyntaxKind::Equation => Modifiers::MATH,
        _ => Modifiers::NONE,
    }
}

/// The class of a node, or `None` when its children carry their own.
fn token_class(
    node: &LinkedNode<'_>,
    classify: &impl Fn(&Range<usize>) -> Option<TokenClass>,
) -> Option<TokenClass> {
    use SyntaxKind::*;

    Some(match node.kind() {
        Star if node.parent_kind() == Some(Strong) => TokenClass::Punctuation,
        Star if node.parent_kind() == Some(ModuleImport) => TokenClass::Operator,
        Underscore if node.parent_kind() == Some(Emph) => TokenClass::Punctuation,
        Underscore if node.parent_kind() == Some(MathAttach) => TokenClass::Operator,
        Star => TokenClass::Operator,
        Ident | MathIdent => classify(&node.range()).unwrap_or_else(|| syntactic_name_class(node)),
        // A name `_` discards in a pattern; emphasis and attachment hold their own cases above.
        Underscore => TokenClass::Interpolated,
        Hash => hashtag_class(node, classify),
        LeftBrace | RightBrace | LeftBracket | RightBracket | LeftParen | RightParen | Comma
        | Semicolon | Colon => TokenClass::Punctuation,
        Linebreak | Escape | Shorthand => TokenClass::Escape,
        Link => TokenClass::Link,
        Raw => TokenClass::Raw,
        Label => TokenClass::Label,
        RefMarker => TokenClass::Ref,
        Heading | HeadingMarker => TokenClass::Heading,
        ListMarker | EnumMarker | TermMarker => TokenClass::Marker,
        Not | And | Or => TokenClass::Keyword,
        MathAlignPoint | Plus | Minus | Slash | Hat | Dot | Eq | EqEq | ExclEq | Lt | LtEq | Gt
        | GtEq | PlusEq | HyphEq | StarEq | SlashEq | Dots | Arrow => TokenClass::Operator,
        Dollar => TokenClass::Delimiter,
        None | Auto | Let | Show | If | Else | For | In | While | Break | Continue | Return
        | Import | Include | As | Set | Context => TokenClass::Keyword,
        Bool => TokenClass::Bool,
        Int | Float | Numeric => TokenClass::Number,
        Str => TokenClass::String,
        LineComment | BlockComment => TokenClass::Comment,
        Error => TokenClass::Error,
        _ if node.children().next().is_none() => TokenClass::Text,
        _ => return Option::None,
    })
}

/// A hashtag writes the class of the expression it introduces, so `#let` reads as its keyword and
/// `#(1)` as its opening parenthesis. A hashtag the parser could attach no expression to, as in
/// markup that writes one alone, stays plain text.
fn hashtag_class(
    node: &LinkedNode<'_>,
    classify: &impl Fn(&Range<usize>) -> Option<TokenClass>,
) -> TokenClass {
    node.next_sibling()
        .and_then(|next| next.leftmost_leaf())
        .and_then(|leaf| token_class(&leaf, classify))
        .unwrap_or(TokenClass::Text)
}

fn syntactic_name_class(node: &LinkedNode<'_>) -> TokenClass {
    // `#import "site.typ" as site` binds a module. A renamed item of a specific import belongs to
    // the `ImportItem` node, so the parent separates the two.
    if node.parent_kind() == Some(SyntaxKind::ModuleImport)
        && node
            .prev_leaf()
            .is_some_and(|previous| previous.kind() == SyntaxKind::As)
    {
        return TokenClass::Namespace;
    }
    let next = node.next_leaf();
    let adjacent = next
        .as_ref()
        .is_some_and(|next| next.range().start == node.range().end);
    match next.map(|next| next.kind()) {
        Some(SyntaxKind::LeftParen | SyntaxKind::LeftBracket) if adjacent => TokenClass::Function,
        _ => TokenClass::Interpolated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every token of a source, as the text it covers and the class it carries.
    fn classified(text: &str) -> Vec<(String, TokenClass, Modifiers)> {
        let source = Source::detached(text.to_owned());
        let names = NameGraph::new(
            [std::sync::Arc::new(crate::names::SourceNames::new(
                source.clone(),
            ))],
            |_| None,
            None,
            || Ok::<(), std::convert::Infallible>(()),
        )
        .unwrap();
        tokens(&source, &names)
            .into_iter()
            .map(|token| (text[token.range].to_owned(), token.class, token.modifiers))
            .collect()
    }

    #[test]
    fn code_constructs_carry_their_class() {
        let tokens = classified("#let x = 1\n");
        assert!(tokens.contains(&("let".to_owned(), TokenClass::Keyword, Modifiers::NONE)));
        assert!(tokens.contains(&("1".to_owned(), TokenClass::Number, Modifiers::NONE)));
    }

    #[test]
    fn construct_style_reaches_its_contents() {
        let tokens = classified("*a* _b_\n");
        assert!(
            tokens.iter().any(|(text, class, modifiers)| text == "a"
                && *class == TokenClass::Text
                && modifiers.contains(Modifiers::STRONG)),
            "{tokens:?}"
        );
        assert!(
            tokens
                .iter()
                .any(|(text, _, modifiers)| text == "b" && modifiers.contains(Modifiers::EMPH)),
            "{tokens:?}"
        );
    }

    #[test]
    fn imported_name_inherits_namespace_class() {
        let tokens = classified("#import \"site.typ\" as site\n#site()\n");
        assert!(tokens.contains(&("site".to_owned(), TokenClass::Namespace, Modifiers::NONE)));
    }

    #[test]
    fn hashtag_inherits_expression_class() {
        let tokens = classified("#(1)\n#let x = 1\nnested #\n");
        assert_eq!(
            tokens[0],
            ("#".to_owned(), TokenClass::Punctuation, Modifiers::NONE)
        );
        assert!(
            tokens
                .iter()
                .any(|(text, class, _)| text == "#" && *class == TokenClass::Keyword),
            "{tokens:?}"
        );
        assert!(
            tokens
                .iter()
                .any(|(text, class, _)| text == "#" && *class == TokenClass::Text),
            "{tokens:?}"
        );
    }

    #[test]
    fn math_groups_keep_inner_tokens() {
        for text in ["$(alpha + beta)$", "$[alpha + beta]$", "$(alpha / beta)$"] {
            let tokens = classified(text);
            assert!(
                tokens.iter().any(|(text, class, modifiers)| text == "alpha"
                    && *class == TokenClass::Interpolated
                    && modifiers.contains(Modifiers::MATH)),
                "{tokens:?}"
            );
            assert!(
                tokens
                    .iter()
                    .all(|(text, _, _)| !text.contains("alpha + beta")),
                "{tokens:?}"
            );
        }
    }
}
