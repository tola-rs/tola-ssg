//! Completion insertion text at the Typst and LSP boundary.

use std::fmt::Write as _;

pub(super) fn typst_snippet(text: &str) -> String {
    let mut snippet = String::with_capacity(text.len());
    let mut remaining = text;
    let mut next = 1;
    while let Some(start) = remaining.find("${") {
        escape(&remaining[..start], &mut snippet);
        let placeholder = &remaining[start + 2..];
        let Some(end) = placeholder.find('}') else {
            escape(&remaining[start..], &mut snippet);
            return snippet;
        };
        let default = &placeholder[..end];
        if let Some((number, default)) = default.split_once(':')
            && let Ok(number) = number.parse::<u32>()
        {
            write!(&mut snippet, "${{{number}:").unwrap();
            escape(default, &mut snippet);
            snippet.push('}');
            next = next.max(number.saturating_add(1));
        } else {
            write!(&mut snippet, "${{{next}").unwrap();
            if !default.is_empty() {
                snippet.push(':');
                escape(default, &mut snippet);
            }
            snippet.push('}');
            next += 1;
        }
        remaining = &placeholder[end + 1..];
    }
    escape(remaining, &mut snippet);
    snippet
}

pub(super) fn escape(text: &str, snippet: &mut String) {
    for ch in text.chars() {
        if matches!(ch, '$' | '}' | '\\') {
            snippet.push('\\');
        }
        snippet.push(ch);
    }
}

pub(super) fn plain_text(snippet: &str) -> String {
    let mut text = String::with_capacity(snippet.len());
    expand(&mut snippet.chars().peekable(), &mut text, false);
    text
}

fn expand(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    text: &mut String,
    placeholder: bool,
) {
    while let Some(ch) = chars.next() {
        match ch {
            '}' if placeholder => return,
            '\\' if chars
                .peek()
                .is_some_and(|ch| matches!(ch, '$' | '}' | '\\' | ',' | '|')) =>
            {
                text.push(chars.next().unwrap());
            }
            '$' if chars.peek() == Some(&'{') => {
                chars.next();
                let mut name = String::new();
                while chars
                    .peek()
                    .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                {
                    name.push(chars.next().unwrap());
                }
                match chars.next() {
                    Some(':') => expand(chars, text, true),
                    Some('|') => {
                        let mut first = true;
                        while let Some(ch) = chars.next() {
                            match ch {
                                '\\' => {
                                    if let Some(ch) = chars.next()
                                        && first
                                    {
                                        text.push(ch);
                                    }
                                }
                                ',' => first = false,
                                '|' if chars.peek() == Some(&'}') => {
                                    chars.next();
                                    break;
                                }
                                ch if first => text.push(ch),
                                _ => {}
                            }
                        }
                    }
                    Some('}') => {
                        if !name.bytes().all(|ch| ch.is_ascii_digit()) {
                            text.push_str(&name);
                        }
                    }
                    Some(ch) => {
                        text.push_str("${");
                        text.push_str(&name);
                        text.push(ch);
                    }
                    None => {
                        text.push_str("${");
                        text.push_str(&name);
                    }
                }
            }
            '$' if chars.peek().is_some_and(char::is_ascii_digit) => {
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    chars.next();
                }
            }
            ch => text.push(ch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_become_numbered_stops() {
        for (typst, lsp) in [
            ("repr(${})", "repr(${1})"),
            ("${x}^${2:2}", "${1:x}^${2:2}"),
            ("let ${name} = ${value}", "let ${1:name} = ${2:value}"),
        ] {
            assert_eq!(typst_snippet(typst), lsp);
        }
    }

    #[test]
    fn snippet_literals_are_escaped() {
        assert_eq!(typst_snippet("$${x}$ \\ }"), "\\$${1:x}\\$ \\\\ \\}");
    }

    #[test]
    fn plain_clients_insert_placeholder_text() {
        for (snippet, text) in [
            ("repr(${1})", "repr()"),
            ("${1:outer ${2:inner}}$0", "outer inner"),
            ("${1|red,green,blue|}", "red"),
            ("${1:a\\}b}", "a}b"),
        ] {
            assert_eq!(plain_text(snippet), text);
        }
    }
}
