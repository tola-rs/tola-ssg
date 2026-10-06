//! The language-level runs of a fenced code block, shared by both renderers.

use std::ops::Range;

use crate::terminal::documentation::{Style, StyledText};

/// What one run of code text is, in the vocabulary both renderers colour by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodeKind {
    Keyword,
    String,
    Number,
    Comment,
    Literal,
    Emphasis,
    Link,
}

/// The styled runs of `source`, a fenced block written in `language`.
///
/// The runs are byte ranges into `source`, in source order, and never overlap. An unknown or
/// absent language has no runs: the whole block reads plain.
pub(crate) fn spans(source: &str, language: &str) -> Vec<(Range<usize>, CodeKind)> {
    match language {
        "typ" | "typst" | "typc" | "typst-code" => typst_spans(source, language == "typst-code"),
        "toml" => toml_spans(source),
        "sh" | "bash" | "zsh" | "shell" | "console" => shell_spans(source),
        "just" => just_spans(source),
        "rust" => rust_spans(source),
        _ => Vec::new(),
    }
}

/// The runs [`spans`] finds, each with the style of its kind.
pub(crate) fn styled(source: &str, language: &str) -> StyledText {
    StyledText {
        text: source.to_owned(),
        spans: spans(source, language)
            .into_iter()
            .map(|(span, kind)| (span, style_of(kind)))
            .collect(),
    }
}

/// The style one run of code is drawn in.
fn style_of(kind: CodeKind) -> Style {
    match kind {
        CodeKind::Keyword => Style::Keyword,
        CodeKind::String => Style::String,
        CodeKind::Number => Style::Number,
        CodeKind::Comment => Style::Comment,
        CodeKind::Literal => Style::Literal,
        CodeKind::Emphasis => Style::Emphasis,
        CodeKind::Link => Style::Link,
    }
}

/// One run of `kind`, merged into the run before it when they touch. Plain text is not a run.
fn push(
    runs: &mut Vec<(Range<usize>, CodeKind)>,
    start: usize,
    end: usize,
    kind: Option<CodeKind>,
) {
    if end <= start {
        return;
    }
    let Some(kind) = kind else {
        return;
    };
    if let Some((last, last_kind)) = runs.last_mut()
        && *last_kind == kind
        && last.end == start
    {
        last.end = end;
        return;
    }
    runs.push((start..end, kind));
}

/// The runs of a Typst fence: the compiler's own highlight leaves.
fn typst_spans(source: &str, code: bool) -> Vec<(Range<usize>, CodeKind)> {
    use typst::syntax::{LinkedNode, parse, parse_code};
    let root = if code {
        parse_code(source)
    } else {
        parse(source)
    };
    let mut runs = Vec::new();
    let mut offset = 0;
    typst_leaves(&LinkedNode::new(&root), None, &mut offset, &mut runs);
    runs
}

fn typst_leaves(
    node: &typst::syntax::LinkedNode<'_>,
    inherited: Option<CodeKind>,
    offset: &mut usize,
    runs: &mut Vec<(Range<usize>, CodeKind)>,
) {
    use typst::syntax::{Tag as Highlight, highlight};
    let kind = match highlight(node) {
        Some(Highlight::Keyword | Highlight::Operator) => Some(CodeKind::Keyword),
        Some(Highlight::String) => Some(CodeKind::String),
        Some(Highlight::Number) => Some(CodeKind::Number),
        Some(Highlight::Comment) => Some(CodeKind::Comment),
        Some(Highlight::Function | Highlight::Interpolated | Highlight::Label | Highlight::Ref) => {
            Some(CodeKind::Literal)
        }
        Some(Highlight::Heading | Highlight::Strong | Highlight::Emph) => Some(CodeKind::Emphasis),
        Some(Highlight::Link) => Some(CodeKind::Link),
        _ => inherited,
    };
    let leaf = node.leaf_text();
    if !leaf.is_empty() {
        let start = *offset;
        *offset += leaf.len();
        push(runs, start, *offset, kind);
        return;
    }
    for child in node.children() {
        typst_leaves(&child, kind, offset, runs);
    }
}

/// The runs of a TOML fence: every table header's name, then the keys and values toml_edit can
/// point at.
///
/// A dotted header (`[[build.hooks.before-build]]`) is one run: its leading segments name no key
/// of their own, so the key walk below never reaches them.
fn toml_spans(source: &str) -> Vec<(Range<usize>, CodeKind)> {
    let Ok(document) = toml_edit::Document::parse(source) else {
        return Vec::new();
    };
    let mut runs = Vec::new();
    toml_headers(document.as_table(), source, &mut runs);
    let headers = runs.len();
    toml_table(document.as_table(), &mut runs);
    let mut keys = runs.split_off(headers);
    keys.retain(|(span, _)| {
        !runs
            .iter()
            .any(|(header, _)| header.start <= span.start && span.end <= header.end)
    });
    runs.extend(keys);
    runs.sort_by_key(|(span, _)| span.start);
    runs
}

/// The name of every table header, brackets excluded: each non-implicit table owns the header
/// line that declares it, and its span covers the whole bracketed name.
fn toml_headers(table: &toml_edit::Table, source: &str, runs: &mut Vec<(Range<usize>, CodeKind)>) {
    for (_, value) in table {
        match value {
            toml_edit::Item::Table(table) => {
                push_header(table, source, runs);
                toml_headers(table, source, runs);
            }
            toml_edit::Item::ArrayOfTables(tables) => {
                for table in tables {
                    push_header(table, source, runs);
                    toml_headers(table, source, runs);
                }
            }
            toml_edit::Item::Value(_) | toml_edit::Item::None => {}
        }
    }
}

fn push_header(table: &toml_edit::Table, source: &str, runs: &mut Vec<(Range<usize>, CodeKind)>) {
    if table.is_implicit() {
        return;
    }
    let Some(span) = table.span() else {
        return;
    };
    let declared = &source[span.clone()];
    let start = span.start + declared.len() - declared.trim_start_matches('[').len();
    let end = span.end - (declared.len() - declared.trim_end_matches(']').len());
    if start < end {
        runs.push((start..end, CodeKind::Literal));
    }
}

fn toml_table(table: &toml_edit::Table, runs: &mut Vec<(Range<usize>, CodeKind)>) {
    for (name, value) in table {
        if let Some(span) = table.key(name).and_then(toml_edit::Key::span) {
            runs.push((span, CodeKind::Literal));
        }
        match value {
            toml_edit::Item::Value(value) => toml_value(value, runs),
            toml_edit::Item::Table(table) => toml_table(table, runs),
            toml_edit::Item::ArrayOfTables(tables) => {
                for table in tables {
                    toml_table(table, runs);
                }
            }
            toml_edit::Item::None => {}
        }
    }
}

fn toml_value(value: &toml_edit::Value, runs: &mut Vec<(Range<usize>, CodeKind)>) {
    use toml_edit::Value;
    match value {
        Value::Array(array) => {
            for value in array {
                toml_value(value, runs);
            }
        }
        Value::InlineTable(table) => {
            for (name, value) in table {
                if let Some(span) = table.key(name).and_then(toml_edit::Key::span) {
                    runs.push((span, CodeKind::Literal));
                }
                toml_value(value, runs);
            }
        }
        value => {
            let kind = match value {
                Value::String(_) => CodeKind::String,
                Value::Boolean(_) => CodeKind::Keyword,
                _ => CodeKind::Number,
            };
            if let Some(span) = value.span() {
                runs.push((span, kind));
            }
        }
    }
}

/// The runs of a shell fence: its comment, its command word, and its double-quoted words.
fn shell_spans(source: &str) -> Vec<(Range<usize>, CodeKind)> {
    let mut runs = Vec::new();
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let body = line.strip_suffix('\n').unwrap_or(line);
        let commented = body.find('#');
        let code = match commented {
            Some(comment) => &body[..comment],
            None => body,
        };
        if let Some((word, word_start)) = command_word(code) {
            push(
                &mut runs,
                start + word_start,
                start + word_start + word.len(),
                Some(CodeKind::Keyword),
            );
        }
        for quoted in quoted_words(code) {
            push(
                &mut runs,
                start + quoted.start,
                start + quoted.end,
                Some(CodeKind::String),
            );
        }
        if let Some(comment) = commented {
            push(
                &mut runs,
                start + comment,
                start + body.len(),
                Some(CodeKind::Comment),
            );
        }
    }
    runs
}

/// The runs of a justfile fence: each recipe header, its body commands, and its comments.
fn just_spans(source: &str) -> Vec<(Range<usize>, CodeKind)> {
    let mut runs = Vec::new();
    let mut offset = 0;
    let mut in_recipe = false;
    for line in source.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let body = line.strip_suffix('\n').unwrap_or(line);
        if body.trim().is_empty() {
            in_recipe = false;
            continue;
        }
        let indented = body.starts_with(char::is_whitespace);
        let commented = body.find('#');
        let code = match commented {
            Some(comment) => &body[..comment],
            None => body,
        };
        let (word, kind) = if indented {
            (
                in_recipe.then(|| command_word(code)).flatten(),
                CodeKind::Keyword,
            )
        } else {
            in_recipe = true;
            (setting_word(code), CodeKind::Literal)
        };
        if let Some((word, word_start)) = word {
            push(
                &mut runs,
                start + word_start,
                start + word_start + word.len(),
                Some(kind),
            );
        }
        if let Some(comment) = commented {
            push(
                &mut runs,
                start + comment,
                start + body.len(),
                Some(CodeKind::Comment),
            );
        }
    }
    runs
}

/// The first whitespace-delimited word of `line`, with the byte index it starts at.
fn command_word(line: &str) -> Option<(&str, usize)> {
    let start = line
        .char_indices()
        .find(|(_, character)| !character.is_whitespace())
        .map(|(index, _)| index)?;
    let end = line[start..]
        .find(char::is_whitespace)
        .map(|end| start + end)
        .unwrap_or(line.len());
    Some((&line[start..end], start))
}

/// The leading identifier of a justfile header line, up to its `:` or first space.
fn setting_word(line: &str) -> Option<(&str, usize)> {
    let (word, start) = command_word(line)?;
    let end = word.find([':', '=']).unwrap_or(word.len());
    Some((&word[..end], start))
}

/// The runs of a Rust fence: line comments, then the keywords the language reserves.
///
/// The scanner is deliberately shallow: it skips string and character literals so a keyword
/// inside one stays plain, but it colours no type, function, or macro name.
fn rust_spans(source: &str) -> Vec<(Range<usize>, CodeKind)> {
    const KEYWORDS: &[&str] = &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true",
        "type", "unsafe", "use", "where", "while",
    ];
    let mut runs = Vec::new();
    let mut index = 0;
    let bytes = source.as_bytes();
    while index < bytes.len() {
        if source[index..].starts_with("//") {
            let end = source[index..]
                .find('\n')
                .map(|end| index + end)
                .unwrap_or(source.len());
            push(&mut runs, index, end, Some(CodeKind::Comment));
            index = end;
            continue;
        }
        let character = source[index..]
            .chars()
            .next()
            .expect("index is a char boundary");
        if character == '"' || character == '\'' {
            index = skip_rust_literal(source, index, character);
            continue;
        }
        if character.is_alphabetic() || character == '_' {
            let end = source[index..]
                .find(|character: char| !(character.is_alphanumeric() || character == '_'))
                .map(|end| index + end)
                .unwrap_or(source.len());
            let word = &source[index..end];
            if KEYWORDS.contains(&word) {
                push(&mut runs, index, end, Some(CodeKind::Keyword));
            }
            index = end;
            continue;
        }
        index += character.len_utf8();
    }
    runs
}

/// The byte after a Rust string or character literal that starts at `start`.
fn skip_rust_literal(source: &str, start: usize, quote: char) -> usize {
    if quote == '\''
        && matches!(
            source[start + 1..].chars().next(),
            Some('a'..='z' | 'A'..='Z')
        )
    {
        // A lifetime such as `'a`, not a character literal, so only the apostrophe is consumed.
        return start + 1;
    }
    let mut escaped = false;
    for (index, character) in source[start + quote.len_utf8()..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            _ if character == quote => {
                return start + quote.len_utf8() + index + character.len_utf8();
            }
            _ => {}
        }
    }
    source.len()
}

/// The spans of the double-quoted words in `source`.
fn quoted_words(source: &str) -> Vec<Range<usize>> {
    let mut words = Vec::new();
    let mut index = 0;
    while let Some(open) = source[index..].find('"') {
        let open = index + open;
        let Some(close) = source[open + 1..].find('"') else {
            break;
        };
        let close = open + 1 + close + 1;
        words.push(open..close);
        index = close;
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_keys_and_values_are_different_runs() {
        let runs = spans("title = \"Tola\"\n", "toml");

        assert_eq!(
            runs.iter()
                .map(|(span, kind)| (&"title = \"Tola\"\n"[span.clone()], *kind))
                .collect::<Vec<_>>(),
            [("title", CodeKind::Literal), ("\"Tola\"", CodeKind::String)]
        );
    }

    #[test]
    fn dotted_header_covers_every_segment() {
        let source = "[[build.hooks.before-build]]\nenable = true\n";
        let runs = spans(source, "toml");

        assert_eq!(
            runs.iter()
                .map(|(span, kind)| (&source[span.clone()], *kind))
                .collect::<Vec<_>>(),
            [
                ("build.hooks.before-build", CodeKind::Literal),
                ("enable", CodeKind::Literal),
                ("true", CodeKind::Keyword),
            ]
        );
    }

    #[test]
    fn typst_markup_and_code_fences_share_their_runs() {
        for language in ["typ", "typst", "typc"] {
            let runs = spans("Let *this* be #emph[that].\n", language);
            assert!(!runs.is_empty(), "{language}");
            assert!(
                runs.iter().any(|(_, kind)| *kind == CodeKind::Emphasis),
                "{language}: {runs:?}"
            );
        }
        let code = spans("let x = 1\n", "typst-code");
        assert!(
            code.iter().any(|(_, kind)| *kind == CodeKind::Keyword),
            "{code:?}"
        );
    }

    #[test]
    fn unknown_language_has_no_runs() {
        assert!(spans("anything at all\n", "text").is_empty());
        assert!(spans("anything at all\n", "").is_empty());
    }

    #[test]
    fn styled_runs_style_each_kind() {
        let text = styled("title = \"Tola\"\njobs = 4\n", "toml");
        let runs = text
            .spans
            .iter()
            .map(|(span, style)| (&text.text[span.clone()], *style))
            .collect::<Vec<_>>();
        assert_eq!(
            runs,
            [
                ("title", Style::Literal),
                ("\"Tola\"", Style::String),
                ("jobs", Style::Literal),
                ("4", Style::Number),
            ]
        );
    }

    /// A shell fence colours its comment, the command it runs, and each quoted word.
    #[test]
    fn shell_fence_styles_command_and_comment() {
        let source = "tola vendor          # commit the result\nls \"$TOLA_HOOK_TEMP_DIR\"\n";
        let runs = runs_of(source, "sh");

        assert_eq!(runs[0], ("tola", CodeKind::Keyword));
        assert_eq!(runs[1], ("# commit the result", CodeKind::Comment));
        assert_eq!(runs[2], ("ls", CodeKind::Keyword));
        assert_eq!(runs[3], ("\"$TOLA_HOOK_TEMP_DIR\"", CodeKind::String));
    }

    /// A justfile fence colours each recipe header and every command in its body.
    #[test]
    fn just_fence_styles_recipe_header_and_body() {
        let source = "search:\n    pagefind --site x\n    echo done\n";
        let runs = runs_of(source, "just");

        assert_eq!(
            runs,
            [
                ("search", CodeKind::Literal),
                ("pagefind", CodeKind::Keyword),
                ("echo", CodeKind::Keyword),
            ]
        );
    }

    /// A Rust fence colours the keywords the language reserves, and skips string literals.
    #[test]
    fn rust_fence_styles_keywords_outside_literals() {
        let source = "let answer = \"let\";\n// let\n";
        let runs = runs_of(source, "rust");

        assert_eq!(runs[0], ("let", CodeKind::Keyword));
        assert_eq!(runs[1], ("// let", CodeKind::Comment));
    }

    /// Every run of `source` under `language`, with the text it covers.
    fn runs_of<'a>(source: &'a str, language: &str) -> Vec<(&'a str, CodeKind)> {
        spans(source, language)
            .into_iter()
            .map(|(span, kind)| (&source[span], kind))
            .collect()
    }
}
