//! Semantic tokens for Typst sources in LSP delta encoding.

use lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens, SemanticTokensDelta,
    SemanticTokensEdit, SemanticTokensLegend,
};
use tola_typst_syntax::imports::NameGraph;
use tola_typst_syntax::tokens::{Token, TokenClass};
use tola_typst_syntax::typst_syntax::Source;

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

/// Full or range tokens without `result_id`; the connection commits the delta baseline.
pub(crate) fn tokens(
    src: &Source,
    names: &NameGraph,
    range: Option<lsp_types::Range>,
    cancel: &tola_build::cancellation::BuildCancellation,
) -> Result<SemanticTokens, tola_build::cancellation::BuildCancelled> {
    let bytes = range.map(|r| {
        let start = crate::position::byte_offset(src.lines(), r.start).unwrap_or(0);
        let end = crate::position::byte_offset(src.lines(), r.end).unwrap_or(src.text().len());
        start..end
    });
    encoded_with_check(
        src,
        names,
        |tok| {
            bytes
                .as_ref()
                .is_none_or(|r| tok.range.end > r.start && tok.range.start < r.end)
        },
        || cancel.ensure_active(),
    )
}

fn encoded_with_check<E>(
    src: &Source,
    names: &NameGraph,
    keep: impl Fn(&Token) -> bool,
    check: impl Fn() -> Result<(), E>,
) -> Result<SemanticTokens, E> {
    check()?;
    let mut enc = Encoder {
        src,
        first_break: crate::position::first_break(src.text()),
        tokens: Vec::new(),
        prev: None,
    };
    for tok in tola_typst_syntax::tokens::tokens_with_check(src, names, &check)? {
        check()?;
        if keep(&tok) {
            enc.push(tok, &check)?;
        }
    }
    check()?;
    Ok(SemanticTokens {
        result_id: None,
        data: enc.tokens,
    })
}

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

const NUMBERS_PER_TOKEN: u32 = 5;

/// Minimal delta edit from `prev` to `curr`, leaving `result_id` for the caller to stamp.
pub(super) fn delta(prev: &SemanticTokens, curr: &SemanticTokens) -> SemanticTokensDelta {
    let from = prev.data.as_slice();
    let to = curr.data.as_slice();

    let prefix = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = from[prefix..]
        .iter()
        .rev()
        .zip(to[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
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

struct Encoder<'a> {
    src: &'a Source,
    first_break: Option<usize>,
    tokens: Vec<SemanticToken>,
    prev: Option<(u32, u32)>,
}

impl Encoder<'_> {
    fn push<E>(&mut self, tok: Token, check: &impl Fn() -> Result<(), E>) -> Result<(), E> {
        check()?;
        let lines = self.src.lines();
        let (Some(first), Some(last)) = (
            lines.byte_to_line(tok.range.start),
            lines.byte_to_line(tok.range.end.saturating_sub(1)),
        ) else {
            return Ok(());
        };
        for line in first..=last {
            check()?;
            self.push_line(line, &tok);
        }
        check()?;
        Ok(())
    }

    fn push_line(&mut self, line: usize, tok: &Token) {
        let lines = self.src.lines();
        let Some(line_start) = lines.line_to_byte(line) else {
            return;
        };
        let line_end = lines
            .line_to_byte(line + 1)
            .unwrap_or_else(|| self.src.text().len());
        let end = tok.range.end.min(line_end).max(line_start);
        let Some(text) = self.src.text().get(line_start..end) else {
            return;
        };
        let end = line_start
            + text
                .trim_end_matches(tola_typst_syntax::typst_syntax::is_newline)
                .len();
        let start = tok.range.start.max(line_start);
        let Some(line_range) =
            crate::position::utf16_range_indexed(lines, start..end, self.first_break)
        else {
            return;
        };
        if line_range.start.line != line_range.end.line
            || line_range.end.character <= line_range.start.character
        {
            return;
        }
        let (ln, col, len) = (
            line_range.start.line,
            line_range.start.character,
            line_range.end.character - line_range.start.character,
        );
        let (delta_line, delta_start) = match self.prev {
            Some((prev_ln, prev_col)) if prev_ln == ln => (0, col.saturating_sub(prev_col)),
            Some((prev_ln, _)) => (ln - prev_ln, col),
            None => (ln, col),
        };
        self.tokens.push(SemanticToken {
            delta_line,
            delta_start,
            length: len,
            token_type: class_index(tok.class),
            token_modifiers_bitset: tok.modifiers.bits(),
        });
        self.prev = Some((ln, col));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tola_typst_syntax::tokens::Modifiers;

    struct Recorded {
        line: u32,
        start: u32,
        length: u32,
        token_type: String,
        modifiers: u32,
    }

    fn decode(text: &str) -> Vec<Recorded> {
        let src = Source::detached(text);
        let names = NameGraph::new(
            [std::sync::Arc::new(
                tola_typst_syntax::names::SourceNames::new(src.clone()),
            )],
            |_| None,
            Some(&tola_typst::GLOBAL_LIBRARY),
            || Ok::<(), std::convert::Infallible>(()),
        )
        .unwrap();
        let mut line = 0;
        let mut start = 0;
        tokens(
            &src,
            &names,
            None,
            &tola_build::cancellation::BuildCancellation::new(),
        )
        .unwrap()
        .data
        .into_iter()
        .map(|tok| {
            line += tok.delta_line;
            start = if tok.delta_line == 0 {
                start + tok.delta_start
            } else {
                tok.delta_start
            };
            Recorded {
                line,
                start,
                length: tok.length,
                token_type: TYPES[tok.token_type as usize].as_str().to_owned(),
                modifiers: tok.token_modifiers_bitset,
            }
        })
        .collect()
    }

    fn token_at(text: &str, needle: &str) -> Recorded {
        let src = Source::detached(text);
        let at = src
            .text()
            .find(needle)
            .expect("the source holds the needle");
        let at = crate::position::utf16_range(src.lines(), at..at).expect("a position in the text");
        let (line, col) = (at.start.line, at.start.character);
        let mut recorded = decode(text);
        let idx = recorded
            .iter()
            .position(|tok| tok.line == line && tok.start == col)
            .expect("a token begins at the needle");
        recorded.remove(idx)
    }

    #[test]
    fn tokens_ordered_non_overlapping() {
        let text = "#\"first\n\nsecond\"\n\n*strong* _emph_ $a_1 + b$\n";
        let recorded = decode(text);
        assert!(recorded.iter().all(|tok| tok.length > 0), "empty token");
        for pair in recorded.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if a.line == b.line {
                assert!(
                    a.start + a.length <= b.start,
                    "overlap on line {}: {} then {}",
                    a.line,
                    a.start,
                    b.start
                );
            }
        }
    }

    #[test]
    fn token_types() {
        let markup = "= Heading\n\n- item\n\n<label> @label\n\nhttps://example.com\n\n`raw`\n";
        let term = "/ *term*: a `raw` description\n";
        let imports =
            "#import \"@tola/site:0.0.0\" as site\n#import \"path.typ\": renamed as other\n";
        let code = "#let factor = 2\n#let twice(x) = x * factor\n#let (done, _) = (1, 2)\n#twice(3)\n// note\n#\"text\"\n";
        let wildcard = "#import math: *\n#let text = 1\n#text\n#sin\n#calc.pi";
        let math = "$figure$\n$table$\n$math.eq$\n$sin x$\n$image$\n$std$\n#figure([x])\n";
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
            (wildcard, "sin", "function"),
            (wildcard, "calc", "namespace"),
            (wildcard, "text\n", "pol"),
            (math, "figure$", "pol"),
            (math, "table$", "pol"),
            (math, "math.eq", "pol"),
            (math, "sin", "function"),
            (math, "image", "pol"),
            (math, "std", "namespace"),
            (math, "figure([x])", "function"),
        ] {
            assert_eq!(token_at(text, needle).token_type, expected, "{needle}");
        }
        assert_eq!(token_at(term, "term").modifiers, Modifiers::STRONG.bits());
        assert_eq!(token_at(math, "sin").modifiers, Modifiers::MATH.bits());
    }

    #[test]
    fn token_columns_utf16() {
        let heading = decode("= 标题\n");
        assert_eq!(heading[0].length, 4);
    }
}
