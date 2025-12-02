use serde::{Deserialize, Serialize};

/// Represents parsed Typst content elements
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "func", rename_all = "lowercase")]
pub enum TypstElement {
    Space,
    Linebreak,
    Text {
        text: String,
    },
    Strike {
        text: String,
    },
    Link {
        dest: String,
        body: Box<TypstElement>,
    },
    Sequence {
        children: Vec<TypstElement>,
    },
    #[serde(other)]
    Unknown,
}

impl TypstElement {
    /// Convert Typst element to HTML string
    pub fn to_html(&self, base_url: &str) -> String {
        match self {
            Self::Space => " ".into(),
            Self::Linebreak => "<br/>".into(),
            Self::Text { text } => html_escape(text),
            Self::Strike { text } => format!("<s>{}</s>", html_escape(text)),
            Self::Link { dest, body } => {
                let href = normalize_link(dest, base_url);
                format!("<a href=\"{}\">{}</a>", href, body.to_html(base_url))
            }
            Self::Sequence { children } => {
                let mut result = String::new();
                for child in children {
                    result.push_str(&child.to_html(base_url));
                }
                result
            }
            Self::Unknown => String::new(),
        }
    }
}

/// Escape HTML special characters
#[inline]
fn html_escape(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '&' => result.push_str("&amp;"),
            '"' => result.push_str("&quot;"),
            _ => result.push(c),
        }
    }
    result
}

/// Normalize relative links to absolute URLs
#[inline]
fn normalize_link(dest: &str, base_url: &str) -> String {
    if dest.starts_with(['.', '/']) {
        let path = dest.trim_start_matches(['.', '/']);
        format!("{}/{}", base_url.trim_end_matches('/'), path)
    } else {
        dest.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_typst_element_text() {
        let json = r#"{ "func": "text", "text": "Hello World" }"#;
        let elem: TypstElement = serde_json::from_str(json).unwrap();
        assert!(matches!(elem, TypstElement::Text { text } if text == "Hello World"));
    }

    #[test]
    fn test_typst_element_space() {
        let json = r#"{ "func": "space" }"#;
        let elem: TypstElement = serde_json::from_str(json).unwrap();
        assert!(matches!(elem, TypstElement::Space));
    }

    #[test]
    fn test_typst_element_linebreak() {
        let json = r#"{ "func": "linebreak" }"#;
        let elem: TypstElement = serde_json::from_str(json).unwrap();
        assert!(matches!(elem, TypstElement::Linebreak));
    }

    #[test]
    fn test_typst_element_strike() {
        let json = r#"{ "func": "strike", "text": "strikethrough" }"#;
        let elem: TypstElement = serde_json::from_str(json).unwrap();
        assert!(matches!(elem, TypstElement::Strike { text } if text == "strikethrough"));
    }

    #[test]
    fn test_typst_element_link() {
        let json = r#"{ "func": "link", "dest": "https://example.com", "body": { "func": "text", "text": "link text" } }"#;
        let elem: TypstElement = serde_json::from_str(json).unwrap();

        if let TypstElement::Link { dest, body } = elem {
            assert_eq!(dest, "https://example.com");
            assert!(matches!(*body, TypstElement::Text { text } if text == "link text"));
        } else {
            panic!("Expected Link element");
        }
    }

    #[test]
    fn test_typst_element_unknown_ignored() {
        let json = r#"{ "func": "custom_unknown_func" }"#;
        let elem: TypstElement = serde_json::from_str(json).unwrap();
        assert!(matches!(elem, TypstElement::Unknown));
    }

    #[test]
    fn test_typst_element_sequence() {
        let json = r#"{
        "func": "sequence",
        "children": [
            { "func": "text", "text": "Hello" },
            { "func": "space" },
            { "func": "text", "text": "World" }
        ]
    }"#;
        let elem: TypstElement = serde_json::from_str(json).unwrap();

        if let TypstElement::Sequence { children } = elem {
            assert_eq!(children.len(), 3);
            assert!(matches!(&children[0], TypstElement::Text { text } if text == "Hello"));
            assert!(matches!(&children[1], TypstElement::Space));
            assert!(matches!(&children[2], TypstElement::Text { text } if text == "World"));
        } else {
            panic!("Expected Sequence element");
        }
    }

    #[test]
    fn test_parse_element_from_typst_sequence() {
        let json_str = r#"
    {
        "func": "sequence",
        "children": [
            { "func": "space" },
            { "func": "text", "text": "This is a simple, fluent, and flexible typing scheme" },
            { "func": "space" },
            { "func": "linebreak" },
            { "func": "space" },
            { "func": "link", "dest": "https://example.com", "body": { "func": "text", "text": "Learn more" } },
            { "func": "text", "text": "Suitable for those who want to improve typing speed without too much effort" },
            { "func": "space" },
            { "func": "unknown_func" }
        ]
    }
    "#;

        let result: TypstElement = serde_json::from_str(json_str).unwrap();
        assert_eq!(
            result,
            TypstElement::Sequence {
                children: vec![
                    TypstElement::Space,
                    TypstElement::Text {
                        text: "This is a simple, fluent, and flexible typing scheme".to_string()
                    },
                    TypstElement::Space,
                    TypstElement::Linebreak,
                    TypstElement::Space,
                    TypstElement::Link {
                        dest: "https://example.com".to_string(),
                        body: Box::new(TypstElement::Text {
                            text: "Learn more".to_string()
                        }),
                    },
                    TypstElement::Text {
                        text: "Suitable for those who want to improve typing speed without too much effort"
                            .to_string()
                    },
                    TypstElement::Space,
                    TypstElement::Unknown,
                ]
            }
        );
    }
}
