//! Typst Content -> HTML extraction.
//!
//! Converts Typst's serialized content JSON to HTML strings for RSS feeds.

use crate::utils::html::{escape, escape_attr, is_void_element};
use serde_json::{Map, Value};

/// Extract HTML from Typst content JSON (with trimmed whitespace)
pub fn extract(value: &Value) -> String {
    Extractor::new(value).run().trim().to_string()
}

struct Extractor<'a> {
    value: &'a Value,
}

impl<'a> Extractor<'a> {
    fn new(value: &'a Value) -> Self {
        Self { value }
    }

    fn run(self) -> String {
        self.extract(self.value)
    }

    fn extract(&self, value: &Value) -> String {
        match value {
            Value::String(s) => escape(s).into_owned(),
            Value::Array(arr) => arr.iter().map(|v| self.extract(v)).collect(),
            Value::Object(obj) => self.extract_element(obj),
            _ => String::new(),
        }
    }

    fn extract_element(&self, obj: &Map<String, Value>) -> String {
        let func = obj.get("func").and_then(Value::as_str).unwrap_or("");

        match func {
            "space" | "linebreak" | "parbreak" => " ".into(),
            "raw" => self.extract_raw(obj),
            "elem" => self.extract_html_elem(obj),
            "frame" => self.extract_frame(obj),
            "link" => self.extract_link(obj),
            _ => self.extract_generic(obj),
        }
    }

    fn extract_raw(&self, obj: &Map<String, Value>) -> String {
        let text = obj.get("text").and_then(Value::as_str).unwrap_or("");
        let lang = obj.get("lang").and_then(Value::as_str).unwrap_or("");

        if lang == "html" {
            text.into() // Raw HTML: no escaping
        } else {
            escape(text).into_owned()
        }
    }

    fn extract_html_elem(&self, obj: &Map<String, Value>) -> String {
        let tag = obj.get("tag").and_then(Value::as_str).unwrap_or("span");
        let attrs = self.build_attrs(obj.get("attrs"));
        let body = obj.get("body").map(|v| self.extract(v)).unwrap_or_default();

        if is_void_element(tag) {
            format!("<{tag}{attrs}/>")
        } else {
            format!("<{tag}{attrs}>{body}</{tag}>")
        }
    }

    fn extract_frame(&self, obj: &Map<String, Value>) -> String {
        obj.get("body")
            .and_then(|b| b.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .into()
    }

    fn extract_link(&self, obj: &Map<String, Value>) -> String {
        let href = obj.get("dest").and_then(Value::as_str).unwrap_or("#");
        let body = obj.get("body").map(|v| self.extract(v)).unwrap_or_default();
        format!("<a href=\"{}\">{body}</a>", escape_attr(href))
    }

    fn extract_generic(&self, obj: &Map<String, Value>) -> String {
        let mut out = String::new();

        // text field (text, symbol)
        if let Some(s) = obj.get("text").and_then(Value::as_str) {
            out.push_str(&escape(s));
        }
        // body field (strong, emph, etc.)
        if let Some(v) = obj.get("body") {
            out.push_str(&self.extract(v));
        }
        // child field (styled)
        if let Some(v) = obj.get("child") {
            out.push_str(&self.extract(v));
        }
        // children field (sequence)
        if let Some(Value::Array(arr)) = obj.get("children") {
            for v in arr {
                out.push_str(&self.extract(v));
            }
        }

        out
    }

    fn build_attrs(&self, attrs: Option<&Value>) -> String {
        attrs
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .map(|(k, v)| {
                        let val = v.as_str().unwrap_or("");
                        format!(" {k}=\"{}\"", escape_attr(val))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    mod extract {
        use super::*;

        fn assert_extract(input: serde_json::Value, expected: &str) {
            assert_eq!(extract(&input), expected);
        }

        #[test]
        fn basic_nodes() {
            assert_extract(json!({"func": "text", "text": "Hello"}), "Hello");
            assert_extract(json!({"func": "space"}), "");

            let json = json!({
                "func": "sequence",
                "children": [
                    {"func": "text", "text": "Hello"},
                    {"func": "space"},
                    {"func": "text", "text": "World"}
                ]
            });
            assert_extract(json, "Hello World");
        }

        #[test]
        fn wrapped_nodes() {
            assert_extract(
                json!({"func": "strong", "body": {"func": "text", "text": "bold"}}),
                "bold",
            );
            assert_extract(
                json!({"func": "styled", "child": {"func": "text", "text": "styled"}}),
                "styled",
            );

            let json = json!({
                "func": "link",
                "dest": "https://example.com",
                "body": {"func": "text", "text": "click"}
            });
            assert_extract(json, r#"<a href="https://example.com">click</a>"#);

            let json = json!({
                "func": "elem",
                "tag": "span",
                "attrs": {"class": "test"},
                "body": {"func": "text", "text": "content"}
            });
            assert_extract(json, r#"<span class="test">content</span>"#);

            let json =
                json!({"func": "elem", "tag": "br", "body": {"func": "sequence", "children": []}});
            assert_extract(json, "<br/>");
        }

        #[test]
        fn raw_and_escaping_nodes() {
            assert_extract(
                json!({"func": "raw", "text": "<b>bold</b>", "lang": "html"}),
                "<b>bold</b>",
            );
            assert_extract(
                json!({"func": "raw", "text": "let x = 1;", "lang": "rust"}),
                "let x = 1;",
            );
            assert_extract(
                json!({"func": "text", "text": "<script>alert(1)</script>"}),
                "&lt;script&gt;alert(1)&lt;/script&gt;",
            );
        }

        #[test]
        fn complex_and_unknown_nodes() {
            let json = json!({
                "func": "sequence",
                "children": [
                    {"func": "text", "text": "Hello"},
                    {"func": "space"},
                    {"func": "elem", "tag": "strong", "body": {"func": "text", "text": "bold"}},
                    {"func": "space"},
                    {"func": "link", "dest": "https://example.com", "body": {"func": "text", "text": "link"}}
                ]
            });
            assert_extract(
                json,
                r#"Hello <strong>bold</strong> <a href="https://example.com">link</a>"#,
            );

            assert_extract(json!({"func": "unknown_element", "data": 123}), "");
        }
    }

    /// End-to-end tests: Typst source -> compile -> query -> JSON -> extract -> HTML
    mod e2e {
        use super::*;
        use tempfile::TempDir;

        fn summary_source(summary: &str) -> String {
            format!(
                r#"
#set page(width: 100pt, height: auto)
#metadata((
  title: "Test",
  summary: [{}],
)) <tola-meta>
"#,
                summary
            )
        }

        /// Compile Typst source and extract summary field as HTML.
        fn compile_and_extract(source: &str) -> Option<String> {
            let temp = TempDir::new().unwrap();
            let path = temp.path().join("test.typ");
            std::fs::write(&path, source).unwrap();

            let result = typst_batch::Compiler::new(temp.path())
                .with_path(&path)
                .compile()
                .ok()?;

            let json = result.document().query_metadata("tola-meta")?;
            let summary = json.get("summary")?;
            Some(extract(summary))
        }

        #[test]
        fn plain_text_summary() {
            assert_eq!(
                compile_and_extract(&summary_source("Hello world")),
                Some("Hello world".into())
            );
        }

        #[test]
        fn inline_markup_text_cases() {
            for (summary, expected) in [
                ("This is *bold* text", "This is bold text"),
                ("This is _italic_ text", "This is italic text"),
            ] {
                assert_eq!(
                    compile_and_extract(&summary_source(summary)),
                    Some(expected.into()),
                    "{summary:?}"
                );
            }
        }

        #[test]
        fn link() {
            assert_eq!(
                compile_and_extract(&summary_source(
                    r#"Click #link("https://example.com")[here]"#
                )),
                Some(r#"Click <a href="https://example.com">here</a>"#.into())
            );
        }

        #[test]
        fn mixed_content() {
            assert_eq!(
                compile_and_extract(&summary_source(
                    r#"Hello *world* and _italic_ with #link("url")[link]"#,
                )),
                Some(r#"Hello world and italic with <a href="url">link</a>"#.into())
            );
        }

        #[test]
        fn no_summary() {
            let source = r#"
#set page(width: 100pt, height: auto)
#metadata((
  title: "Test",
)) <tola-meta>
"#;
            assert_eq!(compile_and_extract(source), None);
        }
    }
}
