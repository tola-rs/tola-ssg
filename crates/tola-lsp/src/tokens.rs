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

/// Every token of the source, in the protocol's relative encoding.
pub(super) fn tokens(source: &Source, names: &NameGraph) -> SemanticTokens {
    encoded(source, names, |_| true)
}

/// The tokens the requested range covers, encoded from the first of them.
///
/// A range reply has no identifier: only a full reply is a revision a delta is measured from.
pub(super) fn in_range(
    source: &Source,
    names: &NameGraph,
    range: lsp_types::Range,
) -> SemanticTokens {
    let lines = source.lines();
    let start = crate::position::byte_offset(lines, range.start).unwrap_or(0);
    let end = crate::position::byte_offset(lines, range.end).unwrap_or(source.text().len());
    encoded(source, names, |token| {
        token.range.end > start && token.range.start < end
    })
}

/// The tokens this predicate keeps, in the protocol's relative encoding.
fn encoded(source: &Source, names: &NameGraph, keep: impl Fn(&Token) -> bool) -> SemanticTokens {
    let mut encoder = Encoder {
        source,
        tokens: Vec::new(),
        previous: None,
    };
    for token in tola_typst_syntax::tokens::tokens(source, names) {
        if keep(&token) {
            encoder.push(token);
        }
    }
    SemanticTokens {
        result_id: None,
        data: encoder.tokens,
    }
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
    tokens: Vec<SemanticToken>,
    /// The line and column of the last token, which a delta is measured from.
    previous: Option<(u32, u32)>,
}

impl Encoder<'_> {
    /// Record one token.
    ///
    /// A token never crosses a line break, and its line start and length count UTF-16 code units,
    /// which is the position encoding this server negotiates.
    fn push(&mut self, token: Token) {
        let range = token.range;
        let lines = self.source.lines();
        let (Some(first), Some(last)) = (
            lines.byte_to_line(range.start),
            lines.byte_to_line(range.end.saturating_sub(1)),
        ) else {
            return;
        };
        for line in first..=last {
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
            let Some(line_range) = crate::position::utf16_range(lines, start..end) else {
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
        tokens(&source, &names)
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
    fn styles_survive_inside_constructs() {
        let text = "*bold* _italic_ $sin x$\n";
        let bold = token_at(text, "bold");
        assert_eq!(bold.token_type.as_str(), "text");
        assert_eq!(bold.modifiers, Modifiers::STRONG.bits());
        let italic = token_at(text, "italic");
        assert_eq!(italic.token_type.as_str(), "text");
        assert_eq!(italic.modifiers, Modifiers::EMPH.bits());
        let math = token_at(text, "sin");
        assert_eq!(math.token_type.as_str(), "function");
        assert_eq!(math.modifiers, Modifiers::MATH.bits());
        assert_eq!(token_at(text, "x$").token_type, "text");
        assert_eq!(token_at(text, "$").token_type, "delim");
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
    fn cross_line_construct_splits_per_line() {
        let text = "// one\n// two\n";
        let comment = decode(text);
        assert_eq!(comment.len(), 2, "each line has its own token");
        assert_eq!(comment[0].line, 0);
        assert_eq!(comment[1].line, 1);
        assert!(comment.iter().all(|token| token.length == 6));
        assert!(comment.iter().all(|token| token.token_type == "comment"));
    }

    #[test]
    fn tokens_exclude_source_line_breaks() {
        // A break the client counts ends the token's line; one only the compiler counts keeps the
        // token on the client's line, past the character the break holds.
        for (newline, expected) in [
            ("\n", [(0, 0, 1), (0, 1, 3), (1, 0, 2)]),
            ("\r\n", [(0, 0, 1), (0, 1, 3), (1, 0, 2)]),
            ("\r", [(0, 0, 1), (0, 1, 3), (1, 0, 2)]),
            ("\u{000B}", [(0, 0, 1), (0, 1, 3), (0, 5, 2)]),
            ("\u{000C}", [(0, 0, 1), (0, 1, 3), (0, 5, 2)]),
            ("\u{0085}", [(0, 0, 1), (0, 1, 3), (0, 5, 2)]),
            ("\u{2028}", [(0, 0, 1), (0, 1, 3), (0, 5, 2)]),
            ("\u{2029}", [(0, 0, 1), (0, 1, 3), (0, 5, 2)]),
        ] {
            let text = format!("#\"🦊{newline}b\"");
            let recorded = decode(&text);
            let spans: Vec<_> = recorded
                .iter()
                .map(|token| (token.line, token.start, token.length))
                .collect();
            assert_eq!(spans, expected, "{newline:?}");
        }
    }

    /// One token, spelled as the five numbers a client decodes.
    fn token(encoded: [u32; 5]) -> SemanticToken {
        let [
            delta_line,
            delta_start,
            length,
            token_type,
            token_modifiers_bitset,
        ] = encoded;
        SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type,
            token_modifiers_bitset,
        }
    }

    #[test]
    fn unchanged_tokens_need_no_edit() {
        let previous = SemanticTokens {
            data: vec![token([0, 0, 3, 1, 0])],
            ..SemanticTokens::default()
        };
        let reply = delta(&previous, &previous);
        assert_eq!(reply.result_id, None);
        assert!(reply.edits.is_empty());
    }

    #[test]
    fn appended_token_inserts_at_the_end() {
        let head = token([0, 0, 3, 1, 0]);
        let appended = token([2, 0, 4, 0, 0]);
        let reply = delta(
            &SemanticTokens {
                data: vec![head],
                ..SemanticTokens::default()
            },
            &SemanticTokens {
                data: vec![head, appended],
                ..SemanticTokens::default()
            },
        );
        assert_eq!(reply.edits.len(), 1);
        assert_eq!(
            reply.edits[0].start, 5,
            "the insertion follows the one token before it"
        );
        assert_eq!(
            reply.edits[0].delete_count, 0,
            "an insertion removes nothing"
        );
        assert_eq!(reply.edits[0].data.as_deref(), Some(&[appended][..]));
    }

    #[test]
    fn replaced_tokens_delete_their_numbers() {
        let head = token([0, 0, 3, 1, 0]);
        let tail = token([2, 0, 4, 0, 0]);
        let replaced = [token([0, 1, 2, 11, 0]), token([0, 9, 1, 14, 0])];
        let replacement = token([0, 1, 8, 11, 0]);
        let reply = delta(
            &SemanticTokens {
                data: vec![head, replaced[0], replaced[1], tail],
                ..SemanticTokens::default()
            },
            &SemanticTokens {
                data: vec![head, replacement, tail],
                ..SemanticTokens::default()
            },
        );
        assert_eq!(
            reply.edits.len(),
            1,
            "the shared head and tail stay untouched"
        );
        assert_eq!(reply.edits[0].start, 5);
        assert_eq!(
            reply.edits[0].delete_count, 10,
            "both replaced tokens' numbers are removed"
        );
        assert_eq!(reply.edits[0].data.as_deref(), Some(&[replacement][..]));
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
