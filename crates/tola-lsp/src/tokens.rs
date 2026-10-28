//! The semantic tokens of one source, as the protocol has them.

use lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens, SemanticTokensDelta,
    SemanticTokensEdit, SemanticTokensLegend,
};
use tola_typst_syntax::imports::NameGraph;
use tola_typst_syntax::tokens::{Token, TokenClass};
use tola_typst_syntax::typst_syntax::Source;

/// The token types, in the order the protocol legend has them.
const TYPES: [SemanticTokenType; 21] = [
    SemanticTokenType::new("text"),
    SemanticTokenType::new("heading"),
    SemanticTokenType::new("marker"),
    SemanticTokenType::new("term"),
    SemanticTokenType::new("label"),
    SemanticTokenType::new("ref"),
    SemanticTokenType::new("link"),
    SemanticTokenType::new("raw"),
    SemanticTokenType::KEYWORD,
    SemanticTokenType::new("bool"),
    SemanticTokenType::NUMBER,
    SemanticTokenType::STRING,
    SemanticTokenType::COMMENT,
    SemanticTokenType::OPERATOR,
    SemanticTokenType::new("punct"),
    SemanticTokenType::new("pol"),
    SemanticTokenType::new("escape"),
    SemanticTokenType::new("delim"),
    SemanticTokenType::new("error"),
    SemanticTokenType::FUNCTION,
    SemanticTokenType::NAMESPACE,
];

/// The styles a client may receive, in the order the legend has them.
const MODIFIERS: [SemanticTokenModifier; 3] = [
    SemanticTokenModifier::new("math"),
    SemanticTokenModifier::new("strong"),
    SemanticTokenModifier::new("emph"),
];

pub(super) fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TYPES.to_vec(),
        token_modifiers: MODIFIERS.to_vec(),
    }
}

/// Full or range tokens without a result identifier; the connection commits their delta baseline.
pub(crate) fn tokens(
    source: &Source,
    names: &NameGraph,
    range: Option<lsp_types::Range>,
    cancellation: &tola_build::cancellation::BuildCancellation,
) -> anyhow::Result<SemanticTokens> {
    let bytes = range.map(|range| {
        let start = crate::position::byte_offset(source.lines(), range.start).unwrap_or(0);
        let end =
            crate::position::byte_offset(source.lines(), range.end).unwrap_or(source.text().len());
        start..end
    });
    Ok(encoded_with_check(
        source,
        names,
        |token| {
            bytes
                .as_ref()
                .is_none_or(|range| token.range.end > range.start && token.range.start < range.end)
        },
        || cancellation.ensure_active(),
    )?)
}

fn encoded_with_check<E>(
    source: &Source,
    names: &NameGraph,
    keep: impl Fn(&Token) -> bool,
    check: impl Fn() -> Result<(), E>,
) -> Result<SemanticTokens, E> {
    check()?;
    let mut encoder = Encoder {
        source,
        first_break: crate::position::first_break(source.text()),
        tokens: Vec::new(),
        previous: None,
    };
    for token in tola_typst_syntax::tokens::tokens_with_check(source, names, &check)? {
        check()?;
        if keep(&token) {
            encoder.push(token, &check)?;
        }
    }
    check()?;
    Ok(SemanticTokens {
        result_id: None,
        data: encoder.tokens,
    })
}

/// The index one token class has in [`TYPES`].
fn class_index(class: TokenClass) -> u32 {
    match class {
        TokenClass::Text => 0,
        TokenClass::Heading => 1,
        TokenClass::Marker => 2,
        TokenClass::Term => 3,
        TokenClass::Label => 4,
        TokenClass::Ref => 5,
        TokenClass::Link => 6,
        TokenClass::Raw => 7,
        TokenClass::Keyword => 8,
        TokenClass::Bool => 9,
        TokenClass::Number => 10,
        TokenClass::String => 11,
        TokenClass::Comment => 12,
        TokenClass::Operator => 13,
        TokenClass::Punctuation => 14,
        TokenClass::Interpolated => 15,
        TokenClass::Escape => 16,
        TokenClass::Delimiter => 17,
        TokenClass::Error => 18,
        TokenClass::Function => 19,
        TokenClass::Namespace => 20,
    }
}

/// How many numbers one token contributes to the stream `start` and `delete_count` index.
const NUMBERS_PER_TOKEN: u32 = 5;

/// The edit that turns `previous`'s tokens into `current`'s.
///
/// Only the span between the two lists' longest common prefix and suffix is sent, so the tokens
/// they share at either end stay on the client. [`SemanticTokensDelta::result_id`] is left empty:
/// the caller stamps the revision its reply reports.
pub(super) fn delta(previous: &SemanticTokens, current: &SemanticTokens) -> SemanticTokensDelta {
    let from = previous.data.as_slice();
    let to = current.data.as_slice();

    let prefix = from
        .iter()
        .zip(to.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = from[prefix..]
        .iter()
        .rev()
        .zip(to[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();

    let removed = &from[prefix..from.len() - suffix];
    let inserted = &to[prefix..to.len() - suffix];
    let edits = if removed.is_empty() && inserted.is_empty() {
        Vec::new()
    } else {
        vec![SemanticTokensEdit {
            start: NUMBERS_PER_TOKEN * prefix as u32,
            delete_count: NUMBERS_PER_TOKEN * removed.len() as u32,
            data: Some(inserted.to_vec()),
        }]
    };
    SemanticTokensDelta {
        result_id: None,
        edits,
    }
}

/// Emits tokens in document order, each encoded relative to the one before it.
struct Encoder<'a> {
    source: &'a Source,
    /// Where the compiler's line model first parts from the protocol's, found once per document.
    first_break: Option<usize>,
    tokens: Vec<SemanticToken>,
    /// The line and column of the last token, which a delta is measured from.
    previous: Option<(u32, u32)>,
}

impl Encoder<'_> {
    /// Record one token.
    ///
    /// A token never crosses a line break, and its line start and length count UTF-16 code units,
    /// which is the position encoding this server negotiates.
    fn push<E>(&mut self, token: Token, check: &impl Fn() -> Result<(), E>) -> Result<(), E> {
        check()?;
        let range = token.range;
        let lines = self.source.lines();
        let (Some(first), Some(last)) = (
            lines.byte_to_line(range.start),
            lines.byte_to_line(range.end.saturating_sub(1)),
        ) else {
            return Ok(());
        };
        for line in first..=last {
            check()?;
            // A line without a following newline is the source's last line, whose end is the
            // text's own length; `line_to_byte` has no entry past it.
            let Some(line_start) = lines.line_to_byte(line) else {
                continue;
            };
            let line_end = lines
                .line_to_byte(line + 1)
                .unwrap_or_else(|| self.source.text().len());
            let end = range.end.min(line_end).max(line_start);
            let Some(text) = self.source.text().get(line_start..end) else {
                continue;
            };
            let end = line_start
                + text
                    .trim_end_matches(tola_typst_syntax::typst_syntax::is_newline)
                    .len();
            let start = range.start.max(line_start);
            // The segment lies inside one line, so its two ends are the protocol positions that
            // bound it and the text between them is its length.
            let Some(line_range) =
                crate::position::utf16_range_indexed(lines, start..end, self.first_break)
            else {
                continue;
            };
            if line_range.start.line != line_range.end.line
                || line_range.end.character <= line_range.start.character
            {
                continue;
            }
            let (line, column, length) = (
                line_range.start.line,
                line_range.start.character,
                line_range.end.character - line_range.start.character,
            );
            let (delta_line, delta_start) = match self.previous {
                Some((previous_line, previous_column)) if previous_line == line => {
                    (0, column.saturating_sub(previous_column))
                }
                Some((previous_line, _)) => (line - previous_line, column),
                None => (line, column),
            };
            self.tokens.push(SemanticToken {
                delta_line,
                delta_start,
                length,
                token_type: class_index(token.class),
                token_modifiers_bitset: token.modifiers.bits(),
            });
            self.previous = Some((line, column));
        }
        check()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tola_typst_syntax::tokens::Modifiers;

    /// One recorded token, with the absolute position its delta encoded.
    struct Recorded {
        line: u32,
        start: u32,
        length: u32,
        token_type: String,
        modifiers: u32,
    }

    fn decode(text: &str) -> Vec<Recorded> {
        let source = Source::detached(text);
        let names = NameGraph::new(
            [std::sync::Arc::new(
                tola_typst_syntax::names::SourceNames::new(source.clone()),
            )],
            |_| None,
            Some(&tola_typst::GLOBAL_LIBRARY),
            || Ok::<(), std::convert::Infallible>(()),
        )
        .unwrap();
        let mut line = 0;
        let mut start = 0;
        tokens(
            &source,
            &names,
            None,
            &tola_build::cancellation::BuildCancellation::new(),
        )
        .unwrap()
        .data
        .into_iter()
        .map(|token| {
            line += token.delta_line;
            start = if token.delta_line == 0 {
                start + token.delta_start
            } else {
                token.delta_start
            };
            Recorded {
                line,
                start,
                length: token.length,
                token_type: TYPES[token.token_type as usize].as_str().to_owned(),
                modifiers: token.token_modifiers_bitset,
            }
        })
        .collect()
    }

    /// The token that begins at the first occurrence of `needle`.
    fn token_at(text: &str, needle: &str) -> Recorded {
        let source = Source::detached(text);
        let at = source
            .text()
            .find(needle)
            .expect("the source holds the needle");
        let at =
            crate::position::utf16_range(source.lines(), at..at).expect("a position in the text");
        let (line, column) = (at.start.line, at.start.character);
        let mut recorded = decode(text);
        let index = recorded
            .iter()
            .position(|token| token.line == line && token.start == column)
            .expect("a token begins at the needle");
        recorded.remove(index)
    }

    /// Tokens stay ordered without overlap, and no token is empty.
    #[test]
    fn tokens_stay_ordered_without_overlap() {
        let text = "#\"first\n\nsecond\"\n\n*strong* _emph_ $a_1 + b$\n";
        let recorded = decode(text);
        assert!(
            recorded.iter().all(|token| token.length > 0),
            "a token is empty"
        );
        for pair in recorded.windows(2) {
            let (left, right) = (&pair[0], &pair[1]);
            if left.line == right.line {
                assert!(
                    left.start + left.length <= right.start,
                    "tokens overlap on line {}: {} then {}",
                    left.line,
                    left.start,
                    right.start
                );
            }
        }
    }

    #[test]
    fn constructs_have_their_token_types() {
        let markup = "= Heading\n\n- item\n\n<label> @label\n\nhttps://example.com\n\n`raw`\n";
        let term = "/ *term*: a `raw` description\n";
        let imports =
            "#import \"@tola/site:0.0.0\" as site\n#import \"path.typ\": renamed as other\n";
        let code = "#let factor = 2\n#let twice(x) = x * factor\n#let (done, _) = (1, 2)\n#twice(3)\n// note\n#\"text\"\n";
        for (text, needle, expected) in [
            (markup, "= ", "heading"),
            (markup, "- item", "marker"),
            (markup, "<label>", "label"),
            (markup, "@label", "ref"),
            (markup, "https", "link"),
            (markup, "`raw`", "raw"),
            (term, "term", "term"),
            (term, ": a", "punct"),
            (imports, "site\n", "namespace"),
            (imports, "other", "pol"),
            (code, "let factor", "keyword"),
            (code, "2\n", "number"),
            (code, "x * factor", "pol"),
            (code, "* factor", "operator"),
            (code, "twice(3)", "function"),
            (code, "// note", "comment"),
            (code, "_", "pol"),
            (code, "\"text\"", "string"),
        ] {
            assert_eq!(token_at(text, needle).token_type, expected, "{needle}");
        }
        assert_eq!(token_at(term, "term").modifiers, Modifiers::STRONG.bits());
    }
    #[test]
    fn token_columns_count_utf16_code_units() {
        let text = "= 标题\n";
        let heading = decode(text);
        assert_eq!(
            heading[0].length, 4,
            "the marker, a space, and two ideographs"
        );
    }
    #[test]
    fn wildcard_import_classifies_its_names() {
        let text = "#import math: *\n#let text = 1\n#text\n#sin\n#calc.pi";
        assert_eq!(token_at(text, "sin").token_type, "function");
        assert_eq!(token_at(text, "calc").token_type, "namespace");
        assert_eq!(token_at(text, "text\n").token_type, "pol");
    }

    #[test]
    fn math_scope_excludes_the_standard_library() {
        let text = "$figure$\n$table$\n$math.eq$\n$sin x$\n$image$\n$std$\n#figure([x])\n";
        for (needle, token_type) in [
            ("figure$", "pol"),
            ("table$", "pol"),
            ("math.eq", "pol"),
            ("sin", "function"),
            ("image", "pol"),
            ("std", "namespace"),
            ("figure([x])", "function"),
        ] {
            assert_eq!(token_at(text, needle).token_type, token_type, "{needle}");
        }
        assert_eq!(token_at(text, "sin").modifiers, Modifiers::MATH.bits());
    }
}
