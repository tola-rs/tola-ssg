//! Markdown to VDOM conversion using pulldown-cmark.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use smallvec::SmallVec;
use tola_vdom::prelude::*;

use crate::compiler::family::{Code, Math, TolaSite};
use crate::page::PageMeta;

/// Options for markdown conversion
#[derive(Debug, Clone, Default)]
pub struct MarkdownOptions {
    /// Enable tables extension
    pub tables: bool,
    /// Enable footnotes extension
    pub footnotes: bool,
    /// Enable strikethrough extension
    pub strikethrough: bool,
    /// Enable task lists extension
    pub task_lists: bool,
    /// Enable heading attributes extension (e.g., `# Heading {#custom-id}`)
    pub heading_attributes: bool,
}

impl MarkdownOptions {
    /// Create options with all extensions enabled
    pub fn all() -> Self {
        Self {
            tables: true,
            footnotes: true,
            strikethrough: true,
            task_lists: true,
            heading_attributes: true,
        }
    }

    /// Convert to pulldown-cmark Options
    fn to_pulldown_options(&self) -> Options {
        let mut opts = Options::empty();
        if self.tables {
            opts.insert(Options::ENABLE_TABLES);
        }
        if self.footnotes {
            opts.insert(Options::ENABLE_FOOTNOTES);
        }
        if self.strikethrough {
            opts.insert(Options::ENABLE_STRIKETHROUGH);
        }
        if self.task_lists {
            opts.insert(Options::ENABLE_TASKLISTS);
        }
        if self.heading_attributes {
            opts.insert(Options::ENABLE_HEADING_ATTRIBUTES);
        }
        opts
    }
}

/// Stack frame for tracking nested elements
struct StackFrame {
    element: Element<TolaSite::Raw>,
}

/// Markdown to VDOM converter
struct MarkdownConverter {
    /// Stack of open elements (for nested structures)
    stack: Vec<StackFrame>,
    /// Root children (collected when stack is empty)
    root_children: Vec<Node<TolaSite::Raw>>,
}

impl MarkdownConverter {
    fn new() -> Self {
        Self {
            stack: Vec::new(),
            root_children: Vec::new(),
        }
    }

    /// Convert markdown string to Document<TolaSite::Raw>
    fn convert(mut self, markdown: &str, options: &MarkdownOptions) -> Document<TolaSite::Raw> {
        let parser = Parser::new_ext(markdown, options.to_pulldown_options());

        for event in parser {
            self.handle_event(event);
        }

        // Build the document
        let root = self.build_root();
        Document::new(root)
    }

    /// Handle a single pulldown-cmark event
    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.add_text(text.as_ref()),
            Event::Code(code) => self.add_inline_code(code.as_ref()),
            Event::Html(html) => self.add_raw_html(html.as_ref()),
            Event::InlineHtml(html) => self.add_raw_html(html.as_ref()),
            Event::SoftBreak => self.add_text("\n"),
            Event::HardBreak => self.add_element("br", vec![]),
            Event::Rule => self.add_element("hr", vec![]),
            Event::FootnoteReference(name) => self.add_footnote_ref(name.as_ref()),
            Event::TaskListMarker(checked) => self.add_task_marker(checked),
            Event::InlineMath(math) => self.add_math(math.as_ref(), false),
            Event::DisplayMath(math) => self.add_math(math.as_ref(), true),
        }
    }

    /// Start a new tag (push onto stack)
    fn start_tag(&mut self, tag: Tag) {
        let (tag_name, attrs) = tag_to_element(&tag);
        let attrs = Attrs::from_iter(attrs.into_iter().map(|(k, v)| (k.into(), v.into())));
        let element = TolaSite::element(&tag_name, attrs);

        self.stack.push(StackFrame { element });
    }

    /// End a tag (pop from stack)
    fn end_tag(&mut self, _tag: TagEnd) {
        if let Some(frame) = self.stack.pop() {
            self.add_node(Node::Element(Box::new(frame.element)));
        }
    }

    /// Add text content
    fn add_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.add_node(Node::Text(Text::new(text.to_string())));
    }

    /// Add inline code using type-safe Code family
    fn add_inline_code(&mut self, code: &str) {
        let data = Code::inline(code);
        let elem = TolaSite::element_with_ext("code", TolaSite::RawExt::Code(data), Attrs::new());
        self.add_node(Node::Element(Box::new(elem)));
    }

    /// Add raw HTML - parse with tl and convert to VDOM elements
    fn add_raw_html(&mut self, html: &str) {
        // Use tl to parse the HTML fragment
        let Ok(dom) = tl::parse(html, tl::ParserOptions::default()) else {
            // Parse failed, store as raw text
            self.add_node(Node::Text(Text::new(html.to_string())));
            return;
        };

        // Convert tl nodes to our VDOM
        let parser = dom.parser();
        for handle in dom.children() {
            if let Some(node) = self.tl_node_to_vdom(*handle, parser) {
                self.add_node(node);
            }
        }
    }

    /// Convert a tl node handle to our VDOM node
    fn tl_node_to_vdom(
        &self,
        handle: tl::NodeHandle,
        parser: &tl::Parser,
    ) -> Option<Node<TolaSite::Raw>> {
        let node = handle.get(parser)?;

        match node {
            tl::Node::Tag(tag) => {
                let tag_name = tag.name().as_utf8_str().to_lowercase();

                // Collect attributes
                let tag_attrs = tag.attributes();
                let mut attrs = Attrs::new();
                for (key, value) in tag_attrs.iter() {
                    let key_str: &str = key.as_ref();
                    let value_str = value.map(|v| v.to_string()).unwrap_or_default();
                    attrs.set(key_str, &value_str);
                }

                let mut elem = TolaSite::element(&tag_name, attrs);

                // Recursively process children
                for child_handle in tag.children().top().iter() {
                    if let Some(child_node) = self.tl_node_to_vdom(*child_handle, parser) {
                        match child_node {
                            Node::Element(e) => elem.children.push(Node::Element(e)),
                            Node::Text(t) => elem.children.push(Node::Text(t)),
                        }
                    }
                }

                Some(Node::Element(Box::new(elem)))
            }
            tl::Node::Raw(bytes) => {
                let text = bytes.as_utf8_str().to_string();
                // Skip whitespace-only text
                if text.trim().is_empty() {
                    None
                } else {
                    Some(Node::Text(Text::new(text)))
                }
            }
            tl::Node::Comment(_) => None, // Skip comments
        }
    }

    /// Add a simple self-closing element
    fn add_element(&mut self, tag: &str, attrs: Vec<(String, String)>) {
        let attrs = Attrs::from_iter(attrs.into_iter().map(|(k, v)| (k.into(), v.into())));
        let elem = TolaSite::element(tag, attrs);
        self.add_node(Node::Element(Box::new(elem)));
    }

    /// Add footnote reference
    fn add_footnote_ref(&mut self, name: &str) {
        let mut elem = TolaSite::element("sup", Attrs::from([("class", "footnote-ref")]));

        let href = format!("#fn-{}", name);
        let id = format!("fnref-{}", name);
        let mut link = TolaSite::element(
            "a",
            Attrs::from([("href", href.as_str()), ("id", id.as_str())]),
        );
        link.children = SmallVec::from_vec(vec![Node::Text(Text::new(format!("[{}]", name)))]);

        elem.children = SmallVec::from_vec(vec![Node::Element(Box::new(link))]);
        self.add_node(Node::Element(Box::new(elem)));
    }

    /// Add task list marker
    fn add_task_marker(&mut self, checked: bool) {
        let mut attrs = Attrs::from([("type", "checkbox"), ("disabled", "")]);
        if checked {
            attrs.set("checked", "");
        }
        let elem = TolaSite::element("input", attrs);
        self.add_node(Node::Element(Box::new(elem)));
    }

    /// Add math content using type-safe Math family
    fn add_math(&mut self, formula: &str, display: bool) {
        let data = if display {
            Math::display(formula)
        } else {
            Math::inline(formula)
        };
        let tag = if display { "div" } else { "span" };
        let elem = TolaSite::element_with_ext(tag, TolaSite::RawExt::Math(data), Attrs::new());
        self.add_node(Node::Element(Box::new(elem)));
    }

    /// Add a node to current context (top of stack or root)
    fn add_node(&mut self, node: Node<TolaSite::Raw>) {
        if let Some(frame) = self.stack.last_mut() {
            frame.element.children.push(node);
        } else {
            self.root_children.push(node);
        }
    }

    /// Build the root element from collected children
    fn build_root(self) -> Element<TolaSite::Raw> {
        let mut root = TolaSite::element("article", Attrs::new());
        root.children = SmallVec::from_vec(self.root_children);
        root
    }
}

/// Convert pulldown-cmark Tag to (tag_name, attributes)
fn tag_to_element(tag: &Tag) -> (String, Vec<(String, String)>) {
    match tag {
        // Block elements
        Tag::Paragraph => ("p".to_string(), vec![]),
        Tag::Heading { level, id, .. } => {
            let tag_name = heading_level_to_tag(*level);
            let attrs = id
                .as_ref()
                .map(|id| vec![("id".to_string(), id.to_string())])
                .unwrap_or_default();
            (tag_name, attrs)
        }
        Tag::BlockQuote(_) => ("blockquote".to_string(), vec![]),
        Tag::CodeBlock(kind) => {
            let attrs = match kind {
                pulldown_cmark::CodeBlockKind::Indented => vec![],
                pulldown_cmark::CodeBlockKind::Fenced(lang) => {
                    if lang.is_empty() {
                        vec![]
                    } else {
                        vec![("class".to_string(), format!("language-{}", lang))]
                    }
                }
            };
            // Wrap in <pre><code>
            ("pre".to_string(), attrs)
        }
        Tag::List(start) => {
            if let Some(start_num) = start {
                let attrs = if *start_num != 1 {
                    vec![("start".to_string(), start_num.to_string())]
                } else {
                    vec![]
                };
                ("ol".to_string(), attrs)
            } else {
                ("ul".to_string(), vec![])
            }
        }
        Tag::Item => ("li".to_string(), vec![]),
        Tag::FootnoteDefinition(name) => (
            "div".to_string(),
            vec![
                ("class".to_string(), "footnote".to_string()),
                ("id".to_string(), format!("fn-{}", name)),
            ],
        ),

        // Table elements
        Tag::Table(alignments) => {
            // Store alignments as data attribute for later processing
            let align_str: String = alignments
                .iter()
                .map(|a| match a {
                    pulldown_cmark::Alignment::None => 'n',
                    pulldown_cmark::Alignment::Left => 'l',
                    pulldown_cmark::Alignment::Center => 'c',
                    pulldown_cmark::Alignment::Right => 'r',
                })
                .collect();
            (
                "table".to_string(),
                vec![("data-align".to_string(), align_str)],
            )
        }
        Tag::TableHead => ("thead".to_string(), vec![]),
        Tag::TableRow => ("tr".to_string(), vec![]),
        Tag::TableCell => ("td".to_string(), vec![]),

        // Inline elements
        Tag::Emphasis => ("em".to_string(), vec![]),
        Tag::Strong => ("strong".to_string(), vec![]),
        Tag::Strikethrough => ("del".to_string(), vec![]),
        Tag::Link {
            dest_url, title, ..
        } => {
            let mut attrs = vec![("href".to_string(), dest_url.to_string())];
            if !title.is_empty() {
                attrs.push(("title".to_string(), title.to_string()));
            }
            ("a".to_string(), attrs)
        }
        Tag::Image {
            dest_url, title, ..
        } => {
            let mut attrs = vec![("src".to_string(), dest_url.to_string())];
            if !title.is_empty() {
                attrs.push(("title".to_string(), title.to_string()));
            }
            // alt text will be added as children (text content)
            ("img".to_string(), attrs)
        }

        // Metadata (typically not rendered)
        Tag::MetadataBlock(_) => ("__metadata".to_string(), vec![]),

        // HTML block
        Tag::HtmlBlock => ("__html_block".to_string(), vec![]),

        // Definition list (extended syntax)
        Tag::DefinitionList => ("dl".to_string(), vec![]),
        Tag::DefinitionListTitle => ("dt".to_string(), vec![]),
        Tag::DefinitionListDefinition => ("dd".to_string(), vec![]),

        // Extended inline elements
        Tag::Superscript => ("sup".to_string(), vec![]),
        Tag::Subscript => ("sub".to_string(), vec![]),
    }
}

/// Convert heading level to tag name
fn heading_level_to_tag(level: HeadingLevel) -> String {
    match level {
        HeadingLevel::H1 => "h1",
        HeadingLevel::H2 => "h2",
        HeadingLevel::H3 => "h3",
        HeadingLevel::H4 => "h4",
        HeadingLevel::H5 => "h5",
        HeadingLevel::H6 => "h6",
    }
    .to_string()
}

/// Convert markdown string to Raw VDOM Document
pub fn from_markdown(markdown: &str, options: &MarkdownOptions) -> Document<TolaSite::Raw> {
    MarkdownConverter::new().convert(markdown, options)
}

/// Convert markdown with default options
#[allow(dead_code)]
pub fn from_markdown_default(markdown: &str) -> Document<TolaSite::Raw> {
    from_markdown(markdown, &MarkdownOptions::default())
}

/// Convert markdown with all extensions enabled
#[allow(dead_code)]
pub fn from_markdown_full(markdown: &str) -> Document<TolaSite::Raw> {
    from_markdown(markdown, &MarkdownOptions::all())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_paragraph() {
        let doc = from_markdown_default("Hello world");
        assert_eq!(doc.root.tag, "article");
        assert_eq!(doc.root.children.len(), 1);
    }

    #[test]
    fn test_heading() {
        let doc = from_markdown_default("# Title");
        let first_child = &doc.root.children[0];
        if let Node::Element(elem) = first_child {
            assert_eq!(elem.tag, "h1");
        } else {
            panic!("Expected element");
        }
    }

    #[test]
    fn test_link() {
        let doc = from_markdown_default("[Link](https://example.com)");
        // Navigate to p > a
        let p = &doc.root.children[0];
        if let Node::Element(p_elem) = p
            && let Some(Node::Element(a_elem)) = p_elem.children.first()
        {
            assert_eq!(a_elem.tag, "a");
            assert!(
                a_elem
                    .attrs
                    .iter()
                    .any(|(k, v)| k == "href" && v == "https://example.com")
            );
        }
    }

    #[test]
    fn test_nested_list() {
        let md = "- Item 1\n  - Nested\n- Item 2";
        let doc = from_markdown_default(md);
        // Should create ul > li structure
        if let Node::Element(ul) = &doc.root.children[0] {
            assert_eq!(ul.tag, "ul");
        }
    }
}

use anyhow::Result;

/// Markdown metadata extractor from YAML (`---`) or TOML (`+++`) frontmatter
pub struct MarkdownMetaExtractor;

impl MarkdownMetaExtractor {
    /// Extract frontmatter and return (metadata, body).
    ///
    /// This is the unified API for pre-compile metadata extraction.
    pub fn extract_frontmatter<'a>(&self, content: &'a str) -> Result<Option<(PageMeta, &'a str)>> {
        match Self::detect_frontmatter(content) {
            Some((fm, body, is_toml)) => {
                let meta = if is_toml {
                    Self::parse_toml(fm)?
                } else {
                    Self::parse_yaml_like(fm)
                };
                Ok(Some((meta, body)))
            }
            None => Ok(None),
        }
    }

    /// Parse simple YAML-like frontmatter (key: value).
    ///
    /// Supports standard fields (title, date, etc.) and custom fields in `extra`.
    fn parse_yaml_like(content: &str) -> PageMeta {
        let mut meta = PageMeta::default();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some((key, value)) = line.split_once(':') {
                let key_lower = key.trim().to_lowercase();
                let value = value.trim();

                match key_lower.as_str() {
                    "title" => meta.title = Some(value.to_string()),
                    "date" => meta.date = Some(value.to_string()),
                    "update" => meta.update = Some(value.to_string()),
                    "author" => meta.author = Some(value.to_string()),
                    "summary" => meta.summary = Some(serde_json::Value::String(value.to_string())),
                    "draft" => meta.draft = value.eq_ignore_ascii_case("true"),
                    "tags" => {
                        meta.tags = value
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                    _ => {
                        // Custom field -> extra (preserve original key case)
                        let key = key.trim().to_string();
                        let json_value = parse_yaml_value(value);
                        meta.extra.insert(key, json_value);
                    }
                }
            }
        }

        meta
    }

    /// Parse TOML frontmatter.
    fn parse_toml(content: &str) -> Result<PageMeta> {
        toml::from_str(content).map_err(|e| anyhow::anyhow!("Invalid TOML frontmatter: {}", e))
    }

    /// Detect and extract frontmatter.
    /// Returns `(frontmatter, body, is_toml)` if found.
    fn detect_frontmatter(content: &str) -> Option<(&str, &str, bool)> {
        let trimmed = content.trim_start();

        // YAML: ---...---
        if trimmed.starts_with("---")
            && let Some(end) = trimmed[3..].find("\n---")
        {
            let fm = trimmed[3..3 + end].trim();
            let body = trimmed[3 + end + 4..].trim_start_matches('\n');
            return Some((fm, body, false));
        }

        // TOML: +++...+++
        if trimmed.starts_with("+++")
            && let Some(end) = trimmed[3..].find("\n+++")
        {
            let fm = trimmed[3..3 + end].trim();
            let body = trimmed[3 + end + 4..].trim_start_matches('\n');
            return Some((fm, body, true));
        }

        None
    }
}

/// Parse a YAML-like value string to JSON value
///
/// Supports:
/// - Booleans: `true`, `false`
/// - Numbers: `123`, `3.14`
/// - Arrays: `a, b, c` -> `["a", "b", "c"]`
/// - Strings: everything else
fn parse_yaml_value(s: &str) -> serde_json::Value {
    use serde_json::Value;

    // Boolean
    if s.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if s.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }

    // Null
    if s.eq_ignore_ascii_case("null") || s == "~" {
        return Value::Null;
    }

    // Number (integer)
    if let Ok(n) = s.parse::<i64>() {
        return Value::Number(n.into());
    }

    // Number (float)
    if let Ok(n) = s.parse::<f64>()
        && let Some(num) = serde_json::Number::from_f64(n)
    {
        return Value::Number(num);
    }

    // Comma-separated array (if contains comma)
    if s.contains(',') {
        let arr: Vec<Value> = s
            .split(',')
            .map(|item| Value::String(item.trim().to_string()))
            .filter(|v| !matches!(v, Value::String(s) if s.is_empty()))
            .collect();
        return Value::Array(arr);
    }

    // Default: string
    Value::String(s.to_string())
}

#[cfg(test)]
mod extractor_tests {
    use super::*;

    #[test]
    fn test_yaml_frontmatter() {
        let content = "---\ntitle: Hello\ndate: 2024-01-01\ntags: a, b\n---\n\n# Body";
        let extractor = MarkdownMetaExtractor;
        let result = extractor.extract_frontmatter(content).unwrap().unwrap();

        assert_eq!(result.0.title, Some("Hello".to_string()));
        assert_eq!(result.0.date, Some("2024-01-01".to_string()));
        assert_eq!(result.0.tags, vec!["a", "b"]);
        assert!(result.1.starts_with("# Body"));
    }

    #[test]
    fn test_toml_frontmatter() {
        let content = "+++\ntitle = \"Hello\"\ntags = [\"a\", \"b\"]\n+++\n\n# Body";
        let extractor = MarkdownMetaExtractor;
        let result = extractor.extract_frontmatter(content).unwrap().unwrap();

        assert_eq!(result.0.title, Some("Hello".to_string()));
        assert_eq!(result.0.tags, vec!["a", "b"]);
    }

    #[test]
    fn test_no_frontmatter() {
        let content = "# Just content";
        let extractor = MarkdownMetaExtractor;
        let result = extractor.extract_frontmatter(content).unwrap();

        assert!(result.is_none());
    }

    #[test]
    fn test_yaml_extra_fields() {
        let content =
            "---\ntitle: Hello\ncustom: world\ncount: 42\nflag: true\nitems: x, y, z\n---\n";
        let extractor = MarkdownMetaExtractor;
        let result = extractor.extract_frontmatter(content).unwrap().unwrap();

        assert_eq!(result.0.title, Some("Hello".to_string()));
        assert_eq!(
            result.0.extra.get("custom"),
            Some(&serde_json::json!("world"))
        );
        assert_eq!(result.0.extra.get("count"), Some(&serde_json::json!(42)));
        assert_eq!(result.0.extra.get("flag"), Some(&serde_json::json!(true)));
        assert_eq!(
            result.0.extra.get("items"),
            Some(&serde_json::json!(["x", "y", "z"]))
        );
    }

    #[test]
    fn test_toml_extra_fields() {
        let content = "+++\ntitle = \"Hello\"\ncustom = \"world\"\ncount = 42\n+++\n";
        let extractor = MarkdownMetaExtractor;
        let result = extractor.extract_frontmatter(content).unwrap().unwrap();

        assert_eq!(result.0.title, Some("Hello".to_string()));
        assert_eq!(
            result.0.extra.get("custom"),
            Some(&serde_json::json!("world"))
        );
        assert_eq!(result.0.extra.get("count"), Some(&serde_json::json!(42)));
    }
}
