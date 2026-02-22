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

        #[test]
        fn text() {
            assert_eq!(extract(&json!({"func": "text", "text": "Hello"})), "Hello");
        }

        #[test]
        fn space() {
            assert_eq!(extract(&json!({"func": "space"})), " ");
        }

        #[test]
        fn sequence() {
            let json = json!({
                "func": "sequence",
                "children": [
                    {"func": "text", "text": "Hello"},
                    {"func": "space"},
                    {"func": "text", "text": "World"}
                ]
            });
            assert_eq!(extract(&json), "Hello World");
        }

        #[test]
        fn strong() {
            let json = json!({"func": "strong", "body": {"func": "text", "text": "bold"}});
            assert_eq!(extract(&json), "bold");
        }

        #[test]
        fn link() {
            let json = json!({
                "func": "link",
                "dest": "https://example.com",
                "body": {"func": "text", "text": "click"}
            });
            assert_eq!(extract(&json), r#"<a href="https://example.com">click</a>"#);
        }

        #[test]
        fn html_elem() {
            let json = json!({
                "func": "elem",
                "tag": "span",
                "attrs": {"class": "test"},
                "body": {"func": "text", "text": "content"}
            });
            assert_eq!(extract(&json), r#"<span class="test">content</span>"#);
        }

        #[test]
        fn html_elem_void() {
            let json =
                json!({"func": "elem", "tag": "br", "body": {"func": "sequence", "children": []}});
            assert_eq!(extract(&json), "<br/>");
        }

        #[test]
        fn raw_html() {
            let json = json!({"func": "raw", "text": "<b>bold</b>", "lang": "html"});
            assert_eq!(extract(&json), "<b>bold</b>");
        }

        #[test]
        fn raw_code() {
            let json = json!({"func": "raw", "text": "let x = 1;", "lang": "rust"});
            assert_eq!(extract(&json), "let x = 1;");
        }

        #[test]
        fn styled() {
            let json = json!({"func": "styled", "child": {"func": "text", "text": "styled"}});
            assert_eq!(extract(&json), "styled");
        }

        #[test]
        fn escaping() {
            let json = json!({"func": "text", "text": "<script>alert(1)</script>"});
            assert_eq!(extract(&json), "&lt;script&gt;alert(1)&lt;/script&gt;");
        }

        #[test]
        fn complex() {
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
            assert_eq!(
                extract(&json),
                r#"Hello <strong>bold</strong> <a href="https://example.com">link</a>"#
            );
        }

        #[test]
        fn unknown_element_returns_empty() {
            // Unknown element with no text/body/child/children -> empty string
            let json = json!({"func": "unknown_element", "data": 123});
            assert_eq!(extract(&json), "");
        }
    }

    /// End-to-end tests: Typst source -> compile -> query -> JSON -> extract -> HTML
    mod e2e {
        use super::*;
        use tempfile::TempDir;

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
            let source = r#"
#set page(width: 100pt, height: auto)
#metadata((
  title: "Test",
  summary: [Hello world],
)) <tola-meta>
"#;
            assert_eq!(compile_and_extract(source), Some("Hello world".into()));
        }

        #[test]
        fn bold_text() {
            let source = r#"
#set page(width: 100pt, height: auto)
#metadata((
  title: "Test",
  summary: [This is *bold* text],
)) <tola-meta>
"#;
            assert_eq!(
                compile_and_extract(source),
                Some("This is bold text".into())
            );
        }

        #[test]
        fn italic_text() {
            let source = r#"
#set page(width: 100pt, height: auto)
#metadata((
  title: "Test",
  summary: [This is _italic_ text],
)) <tola-meta>
"#;
            assert_eq!(
                compile_and_extract(source),
                Some("This is italic text".into())
            );
        }

        #[test]
        fn link() {
            let source = r#"
#set page(width: 100pt, height: auto)
#metadata((
  title: "Test",
  summary: [Click #link("https://example.com")[here]],
)) <tola-meta>
"#;
            assert_eq!(
                compile_and_extract(source),
                Some(r#"Click <a href="https://example.com">here</a>"#.into())
            );
        }

        #[test]
        fn mixed_content() {
            let source = r#"
#set page(width: 100pt, height: auto)
#metadata((
  title: "Test",
  summary: [Hello *world* and _italic_ with #link("url")[link]],
)) <tola-meta>
"#;
            assert_eq!(
                compile_and_extract(source),
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
