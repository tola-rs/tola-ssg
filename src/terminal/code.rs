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
        "json" => json_spans(source),
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
/// The runs of a JSON fence: each member name, string, number, and keyword.
///
/// A string whose next non-whitespace byte is `:` is a member name, the role a TOML key takes;
/// every other string is a value. A `\` escape pair never ends a string.
fn json_spans(source: &str) -> Vec<(Range<usize>, CodeKind)> {
    let mut runs = Vec::new();
    let mut index = 0;
    while index < source.len() {
        let character = source[index..]
            .chars()
            .next()
            .expect("index is a char boundary");
        match character {
            '"' => {
                let end = json_string_end(source, index);
                let kind = if source[end..].trim_start().starts_with(':') {
                    CodeKind::Literal
                } else {
                    CodeKind::String
                };
                push(&mut runs, index, end, Some(kind));
                index = end;
            }
            '0'..='9' | '-' => {
                let end = source[index..]
                    .find(|character: char| {
                        !(character.is_ascii_digit()
                            || matches!(character, '.' | 'e' | 'E' | '+' | '-'))
                    })
                    .map(|end| index + end)
                    .unwrap_or(source.len());
                push(&mut runs, index, end, Some(CodeKind::Number));
                index = end;
            }
            character if character.is_ascii_alphabetic() => {
                let end = source[index..]
                    .find(|character: char| !character.is_ascii_alphabetic())
                    .map(|end| index + end)
                    .unwrap_or(source.len());
                if matches!(&source[index..end], "true" | "false" | "null") {
                    push(&mut runs, index, end, Some(CodeKind::Keyword));
                }
                index = end;
            }
            _ => index += character.len_utf8(),
        }
    }
    runs
}

/// The byte after the JSON string that starts at `start`: its closing quote, or the source's end
/// when it never closes. A `\` escape pair never ends the string.
fn json_string_end(source: &str, start: usize) -> usize {
    let mut escaped = false;
    for (offset, character) in source[start + 1..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' => return start + 1 + offset + 1,
            _ => {}
        }
    }
    source.len()
}

/// Every occurrence of `character` in `line` that stands outside a quoted word, with whether it
/// also starts a word.
///
/// A single-quoted word closes at its next `'`, the way the shell reads one and just's raw
/// strings spell one; a `\` escape pair inside a double-quoted word does not end it, and a `\`
/// outside one escapes the character that follows.
fn outside_quotes(line: &str, character: char) -> Vec<(usize, bool)> {
    let mut found = Vec::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut word_start = true;
    for (index, current) in line.char_indices() {
        if escaped {
            escaped = false;
            word_start = false;
            continue;
        }
        if let Some(open) = quote {
            match current {
                '\\' if open == '"' => escaped = true,
                _ if current == open => quote = None,
                _ => {}
            }
            word_start = false;
            continue;
        }
        match current {
            '\\' => escaped = true,
            '"' | '\'' => {
                quote = Some(current);
                word_start = false;
            }
            _ if current == character => {
                found.push((index, word_start));
                word_start = false;
            }
            _ => {
                word_start = current.is_whitespace()
                    || matches!(current, ';' | '|' | '&' | '(' | ')' | '<' | '>');
            }
        }
    }
    found
}

/// The byte index a shell line's comment starts at, if it has one: a `#` outside a quoted word
/// and at the start of a word, so `echo a#b` keeps its word while `echo a # b` comments it out.
fn shell_comment_start(line: &str) -> Option<usize> {
    outside_quotes(line, '#')
        .into_iter()
        .find(|(_, word_start)| *word_start)
        .map(|(index, _)| index)
}

/// The byte index one justfile line's own comment starts at, if it has one: the first `#`
/// outside a quoted word, so `x := "#1"` keeps its value while `x := b#c` comments it out.
fn just_comment_start(line: &str) -> Option<usize> {
    outside_quotes(line, '#').first().map(|(index, _)| *index)
}

/// The runs of a shell fence: its comment, its command word, and its quoted words.
fn shell_spans(source: &str) -> Vec<(Range<usize>, CodeKind)> {
    let mut runs = Vec::new();
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let body = line.strip_suffix('\n').unwrap_or(line);
        let comment = shell_comment_start(body);
        let code = match comment {
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
        if let Some(comment) = comment {
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

/// The runs of a justfile fence: each recipe's name and dependencies, the command each body
/// line runs, the words its lines quote, and its comments.
///
/// A recipe body is shell text — just passes each line to the shell — so a body line reads the
/// way a shell line does, while one `{{ … }}` interpolation splices a just value into it. A
/// blank line belongs to the body above it: `body : INDENT line+ DEDENT`.
fn just_spans(source: &str) -> Vec<(Range<usize>, CodeKind)> {
    let mut runs = Vec::new();
    let mut offset = 0;
    let mut in_recipe = false;
    for line in source.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let body = line.strip_suffix('\n').unwrap_or(line);
        if body.trim().is_empty() {
            continue;
        }
        let indented = body.starts_with(char::is_whitespace);
        let comment = if indented {
            shell_comment_start(body)
        } else {
            just_comment_start(body)
        };
        let code = match comment {
            Some(comment) => &body[..comment],
            None => body,
        };
        if indented {
            if in_recipe {
                just_body(code, start, &mut runs);
            }
        } else {
            in_recipe = true;
            just_header(code, start, &mut runs);
        }
        if let Some(comment) = comment {
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

/// The runs of one recipe head or statement line: the name it declares, the words its
/// parameters or value quote, and — after a recipe's `:` — the dependencies it runs.
fn just_header(code: &str, start: usize, runs: &mut Vec<(Range<usize>, CodeKind)>) {
    if let Some((word, word_start)) = setting_word(code) {
        push(
            runs,
            start + word_start,
            start + word_start + word.len(),
            Some(CodeKind::Literal),
        );
    }
    let (parameters, dependencies) = match separating_colon(code) {
        Some(colon) => (&code[..colon], Some(colon + 1)),
        None => (code, None),
    };
    for quoted in quoted_words(parameters) {
        push(
            runs,
            start + quoted.start,
            start + quoted.end,
            Some(CodeKind::String),
        );
    }
    if let Some(from) = dependencies {
        just_dependencies(code, from, start, runs);
    }
}

/// The runs of the dependencies one recipe head names: each target, and each word it quotes.
fn just_dependencies(
    code: &str,
    from: usize,
    start: usize,
    runs: &mut Vec<(Range<usize>, CodeKind)>,
) {
    let dependencies = &code[from..];
    let quoted = quoted_words(dependencies);
    let mut index = 0;
    while index < dependencies.len() {
        if let Some(span) = quoted.iter().find(|span| span.start == index) {
            push(
                runs,
                start + from + span.start,
                start + from + span.end,
                Some(CodeKind::String),
            );
            index = span.end;
            continue;
        }
        let character = dependencies[index..]
            .chars()
            .next()
            .expect("index is a char boundary");
        if character.is_alphabetic() || character == '_' {
            let end = dependencies[index..]
                .find(|character: char| {
                    !(character.is_alphanumeric() || character == '_' || character == '-')
                })
                .map(|end| index + end)
                .unwrap_or(dependencies.len());
            push(
                runs,
                start + from + index,
                start + from + end,
                Some(CodeKind::Literal),
            );
            index = end;
            continue;
        }
        index += character.len_utf8();
    }
}

/// The runs of one recipe body line: the command it runs, the words it quotes, and the just
/// values it splices into them.
fn just_body(code: &str, start: usize, runs: &mut Vec<(Range<usize>, CodeKind)>) {
    let splices = interpolations(code);
    let mut pieces = Vec::new();
    if let Some((word, word_start)) = command_word(code) {
        let span = word_start..word_start + word.len();
        if !splices
            .iter()
            .any(|hole| hole.start < span.end && span.start < hole.end)
        {
            pieces.push((span, CodeKind::Keyword));
        }
    }
    for quoted in quoted_words(code) {
        pieces.extend(
            remaining(quoted, &splices)
                .into_iter()
                .map(|span| (span, CodeKind::String)),
        );
    }
    pieces.extend(splices.into_iter().map(|span| (span, CodeKind::Literal)));
    pieces.sort_by_key(|(span, _)| span.start);
    for (span, kind) in pieces {
        push(runs, start + span.start, start + span.end, Some(kind));
    }
}

/// The `{{ … }}` spans of `code`: each splices a just value into the line it stands in.
///
/// An opening `{{` without a closing `}}` ends the scan: nothing after it closes either.
fn interpolations(code: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut index = 0;
    while let Some(open) = code[index..].find("{{") {
        let open = index + open;
        let Some(close) = code[open + 2..].find("}}") else {
            break;
        };
        let close = open + 2 + close + 2;
        spans.push(open..close);
        index = close;
    }
    spans
}

/// The parts of `span` that remain when the spans of `holes` — given in source order — are cut
/// out, in order.
fn remaining(span: Range<usize>, holes: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut parts = Vec::new();
    let mut start = span.start;
    for hole in holes {
        if hole.end <= start || span.end <= hole.start {
            continue;
        }
        if start < hole.start {
            parts.push(start..hole.start);
        }
        start = hole.end;
    }
    if start < span.end {
        parts.push(start..span.end);
    }
    parts
}

/// The byte index of the `:` that separates one recipe head's parameters from its dependencies:
/// the first colon outside a quoted word that opens no `:=`.
fn separating_colon(code: &str) -> Option<usize> {
    outside_quotes(code, ':')
        .into_iter()
        .map(|(index, _)| index)
        .find(|index| !code[index + 1..].starts_with('='))
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

/// The spans of the quoted words in `source`: each opening quote through its closing one, or to
/// the source's end when it never closes. A `\` escape pair does not end a double-quoted word,
/// and a single-quoted word ends at its next `'`, as a shell word and just's raw string do.
fn quoted_words(source: &str) -> Vec<Range<usize>> {
    let mut words = Vec::new();
    let mut index = 0;
    while index < source.len() {
        let quote = source[index..]
            .chars()
            .next()
            .expect("index is a char boundary");
        if quote != '"' && quote != '\'' {
            index += quote.len_utf8();
            continue;
        }
        let mut escaped = false;
        let mut end = source.len();
        for (offset, character) in source[index + quote.len_utf8()..].char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            match character {
                '\\' if quote == '"' => escaped = true,
                _ if character == quote => {
                    end = index + quote.len_utf8() + offset + quote.len_utf8();
                    break;
                }
                _ => {}
            }
        }
        words.push(index..end);
        index = end;
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

    /// A JSON fence colours each member name apart from the string it maps to.
    #[test]
    fn json_fence_styles_keys_and_values() {
        let source = "{\"jobs\": 4, \"on\": true, \"note\": null, \"title\": \"a\\\"b\"}\n";
        let runs = runs_of(source, "json");

        assert_eq!(
            runs,
            [
                ("\"jobs\"", CodeKind::Literal),
                ("4", CodeKind::Number),
                ("\"on\"", CodeKind::Literal),
                ("true", CodeKind::Keyword),
                ("\"note\"", CodeKind::Literal),
                ("null", CodeKind::Keyword),
                ("\"title\"", CodeKind::Literal),
                ("\"a\\\"b\"", CodeKind::String),
            ]
        );
    }

    /// A shell fence colours its comment, the command it runs, and each quoted word.
    #[test]
    fn shell_fence_styles_command_and_comment() {
        let source =
            "tola vendor          # commit the result\nls \"$TOLA_HOOK_TEMP_DIR\"\necho 'a # b'\n";
        let runs = runs_of(source, "sh");

        assert_eq!(runs[0], ("tola", CodeKind::Keyword));
        assert_eq!(runs[1], ("# commit the result", CodeKind::Comment));
        assert_eq!(runs[2], ("ls", CodeKind::Keyword));
        assert_eq!(runs[3], ("\"$TOLA_HOOK_TEMP_DIR\"", CodeKind::String));
        assert_eq!(runs[4], ("echo", CodeKind::Keyword));
        assert_eq!(runs[5], ("'a # b'", CodeKind::String));
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

    /// A justfile fence colours the dependencies a recipe head names after its `:`.
    #[test]
    fn just_fence_styles_recipe_dependencies() {
        let runs = runs_of("build target: fmt test\n", "just");

        assert_eq!(
            runs,
            [
                ("build", CodeKind::Literal),
                ("fmt", CodeKind::Literal),
                ("test", CodeKind::Literal),
            ]
        );
    }

    /// A recipe body continues across a blank line: `body : INDENT line+ DEDENT`.
    #[test]
    fn recipe_body_continues_after_blank_line() {
        let runs = runs_of("fmt:\n    cargo fmt\n\n    cargo clippy\n", "just");

        assert_eq!(
            runs,
            [
                ("fmt", CodeKind::Literal),
                ("cargo", CodeKind::Keyword),
                ("cargo", CodeKind::Keyword),
            ]
        );
    }

    /// A `#` inside a quoted word opens no comment.
    #[test]
    fn quoted_hash_opens_no_comment() {
        let runs = runs_of("x := \"#1\"\n", "just");

        assert_eq!(
            runs,
            [("x", CodeKind::Literal), ("\"#1\"", CodeKind::String)]
        );
    }

    /// In shell text, a `#` inside a word opens no comment.
    #[test]
    fn word_hash_opens_no_comment() {
        let runs = runs_of("run:\n    echo a#b\n", "just");

        assert_eq!(
            runs,
            [("run", CodeKind::Literal), ("echo", CodeKind::Keyword)]
        );
    }

    /// An interpolation keeps its own run inside the quoted word it stands in.
    #[test]
    fn interpolation_has_its_own_run() {
        let runs = runs_of("fmt:\n    echo \"{{target}}\"\n", "just");

        assert_eq!(
            runs,
            [
                ("fmt", CodeKind::Literal),
                ("echo", CodeKind::Keyword),
                ("\"", CodeKind::String),
                ("{{target}}", CodeKind::Literal),
                ("\"", CodeKind::String),
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
