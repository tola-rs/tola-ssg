//! HTML escaping.

use std::borrow::Cow;

pub fn escape(s: &str) -> Cow<'_, str> {
    escape_matching(s, |character| match character {
        '<' => Some("&lt;"),
        '>' => Some("&gt;"),
        '&' => Some("&amp;"),
        _ => None,
    })
}

pub fn escape_attr(s: &str) -> Cow<'_, str> {
    escape_matching(s, |character| match character {
        '<' => Some("&lt;"),
        '>' => Some("&gt;"),
        '&' => Some("&amp;"),
        '"' => Some("&quot;"),
        '\'' => Some("&#39;"),
        _ => None,
    })
}

fn escape_matching(s: &str, escaped: impl Fn(char) -> Option<&'static str>) -> Cow<'_, str> {
    if !s.chars().any(|character| escaped(character).is_some()) {
        return Cow::Borrowed(s);
    }

    let mut output = String::with_capacity(s.len());
    for character in s.chars() {
        if let Some(replacement) = escaped(character) {
            output.push_str(replacement);
        } else {
            output.push(character);
        }
    }
    Cow::Owned(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_returns_borrowed() {
        assert!(matches!(escape("plain"), Cow::Borrowed(_)));
    }

    #[test]
    fn quotes_escape_only_in_attributes() {
        assert_eq!(escape("<a & \"b\">"), "&lt;a &amp; \"b\"&gt;");
        assert_eq!(escape_attr("<a & \"b\">"), "&lt;a &amp; &quot;b&quot;&gt;");
    }
}
