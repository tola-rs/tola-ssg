//! Supported rich text for syndication summaries and entry bodies.

use crate::cancellation::{BuildCancellation, BuildCancelled};

#[derive(Debug, thiserror::Error)]
pub(super) enum FeedContentError {
    #[error(transparent)]
    Cancelled(#[from] BuildCancelled),
    #[error("at `{path}`: {violation}")]
    Invalid {
        path: String,
        #[source]
        violation: FeedContentViolation,
    },
}

#[derive(Debug, thiserror::Error)]
pub(in crate::seo) enum FeedContentViolation {
    #[error("expected {expected}, found {actual}")]
    Type {
        expected: &'static str,
        actual: String,
    },
    #[error("{message}")]
    Value {
        message: &'static str,
        help: &'static str,
    },
    #[error("unsupported content element `{element}`")]
    UnsupportedContent { element: String },
    #[error("this field of content element `{element}` is not supported in feeds")]
    ContentField { element: String },
}

impl FeedContentViolation {
    /// The next action this violation asks of the author.
    pub(in crate::seo) fn help(&self) -> String {
        match self {
            Self::Type { expected, .. } => format!("Give this field a {expected} value"),
            Self::Value { help, .. } => (*help).to_string(),
            Self::UnsupportedContent { element } => {
                format!("Remove `{element}`, or select the document's HTML with `(document:, id:)`")
            }
            Self::ContentField { element } => format!("Remove this field from `{element}`"),
        }
    }
}

fn type_error(
    path: &str,
    expected: &'static str,
    value: &typst::foundations::Value,
) -> FeedContentError {
    FeedContentError::Invalid {
        path: path.into(),
        violation: FeedContentViolation::Type {
            expected,
            actual: value.ty().short_name().into(),
        },
    }
}

use typst::foundations::{Content, Dict, SequenceElem, SymbolElem, Value};
use typst::model::{Destination, EmphElem, LinkElem, LinkTarget, ParbreakElem, StrongElem};
use typst::text::{LinebreakElem, SmartQuoteElem, SpaceElem, StrikeElem, TextElem};

use crate::html::{escape, escape_attr};
use crate::seo::declaration::{field_path, index_path};

/// Style-free rich text for feed export.
///
/// Source `set` and `show` rules are excluded. Styled, contextual, and realized
/// nodes are rejected before HTML serialization. Allowed fields match Typst 0.15.1;
/// review them when upgrading Typst.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FeedContent {
    paragraphs: Vec<FeedParagraph>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FeedParagraph {
    inlines: Vec<FeedInline>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FeedInline {
    Text(String),
    Space,
    LineBreak,
    Strong(Vec<Self>),
    Emphasis(Vec<Self>),
    Strikethrough(Vec<Self>),
    Link {
        destination: String,
        body: Vec<Self>,
    },
}

impl FeedContent {
    pub(super) fn from_typst(
        content: Content,
        path: &str,
        cancellation: &BuildCancellation,
    ) -> Result<Self, FeedContentError> {
        Self::from_typst_inside(content, path, false, cancellation)
    }

    fn from_typst_inside(
        content: Content,
        path: &str,
        inside_link: bool,
        cancellation: &BuildCancellation,
    ) -> Result<Self, FeedContentError> {
        cancellation.ensure_active()?;
        validate_lifecycle(&content, path)?;

        if let Some(element) = content.to_packed::<SequenceElem>() {
            ensure_fields(&content, path, &["children"])?;
            let children = element
                .children
                .iter()
                .enumerate()
                .map(|(index, child)| {
                    Self::from_typst_inside(
                        child.clone(),
                        &index_path(&field_path(path, "children"), index),
                        inside_link,
                        cancellation,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Self::sequence(children));
        }

        if let Some(element) = content.to_packed::<TextElem>() {
            ensure_fields(&content, path, &["text"])?;
            return Ok(Self::text(element.text.as_str()));
        }

        if let Some(element) = content.to_packed::<SymbolElem>() {
            ensure_fields(&content, path, &["text"])?;
            return Ok(Self::text(element.text.as_str()));
        }

        if content.is::<SpaceElem>() {
            ensure_fields(&content, path, &[])?;
            return Ok(Self::inline(FeedInline::Space));
        }

        if content.is::<LinebreakElem>() {
            ensure_fields(&content, path, &[])?;
            return Ok(Self::inline(FeedInline::LineBreak));
        }

        if content.is::<ParbreakElem>() {
            ensure_fields(&content, path, &[])?;
            return Ok(Self::paragraph_break());
        }

        if content.is::<SmartQuoteElem>() {
            let fields = ensure_fields(&content, path, &["double"])?;
            let double = match fields.get("double") {
                Ok(Value::Bool(double)) => *double,
                Ok(value) => return Err(type_error(&field_path(path, "double"), "boolean", value)),
                Err(_) => true,
            };
            // Quote shaping needs a language and surrounding layout context.
            // Preserve the character as written without capturing either.
            return Ok(Self::text(if double { "\"" } else { "'" }));
        }

        if let Some(element) = content.to_packed::<StrongElem>() {
            ensure_fields(&content, path, &["body"])?;
            return Self::from_typst_inside(
                element.body.clone(),
                &field_path(path, "body"),
                inside_link,
                cancellation,
            )
            .map(|content| content.wrap(FeedInline::Strong));
        }

        if let Some(element) = content.to_packed::<EmphElem>() {
            ensure_fields(&content, path, &["body"])?;
            return Self::from_typst_inside(
                element.body.clone(),
                &field_path(path, "body"),
                inside_link,
                cancellation,
            )
            .map(|content| content.wrap(FeedInline::Emphasis));
        }

        if let Some(element) = content.to_packed::<StrikeElem>() {
            ensure_fields(&content, path, &["body"])?;
            return Self::from_typst_inside(
                element.body.clone(),
                &field_path(path, "body"),
                inside_link,
                cancellation,
            )
            .map(|content| content.wrap(FeedInline::Strikethrough));
        }

        if let Some(element) = content.to_packed::<LinkElem>() {
            if inside_link {
                return Err(FeedContentError::Invalid {
                    path: path.to_string(),
                    violation: FeedContentViolation::Value {
                        message: "feed content links cannot be nested",
                        help: "Remove the inner link",
                    },
                });
            }
            ensure_fields(&content, path, &["dest", "body"])?;
            let LinkTarget::Dest(Destination::Url(destination)) = &element.dest else {
                return Err(FeedContentError::Invalid {
                    path: field_path(path, "dest"),
                    violation: FeedContentViolation::Value {
                        message: "feed content links must use URL destinations",
                        help: "Use a URL, not a label or location",
                    },
                });
            };
            let Some(destination) = normalize_link_destination(destination.as_str()) else {
                return Err(FeedContentError::Invalid {
                    path: field_path(path, "dest"),
                    violation: FeedContentViolation::Value {
                        message: "feed content links must be relative, HTTP(S), mailto, or tel URLs",
                        help: "Write a relative or `http`/`https`/`mailto`/`tel` URL",
                    },
                });
            };
            let body = Self::from_typst_inside(
                element.body.clone(),
                &field_path(path, "body"),
                true,
                cancellation,
            )?;
            return Ok(body.wrap(|body| FeedInline::Link {
                destination: destination.to_string(),
                body,
            }));
        }

        Err(FeedContentError::Invalid {
            path: path.to_string(),
            violation: FeedContentViolation::UnsupportedContent {
                element: content.func().name().to_string(),
            },
        })
    }

    pub(super) fn to_plain_text(
        &self,
        cancellation: &BuildCancellation,
    ) -> Result<String, BuildCancelled> {
        let mut output = String::new();
        for (index, paragraph) in self.paragraphs.iter().enumerate() {
            cancellation.ensure_active()?;
            if index > 0 {
                output.push_str("\n\n");
            }
            paragraph.write_plain_text(&mut output, cancellation)?;
        }
        Ok(output)
    }

    pub(super) fn to_html_with_links(
        &self,
        mut map_destination: impl FnMut(&str) -> String,
        cancellation: &BuildCancellation,
    ) -> Result<String, BuildCancelled> {
        cancellation.ensure_active()?;
        let mut output = String::new();
        if self.paragraphs.len() == 1 {
            self.paragraphs[0].write_html(&mut output, &mut map_destination, cancellation)?;
        } else {
            for paragraph in &self.paragraphs {
                cancellation.ensure_active()?;
                output.push_str("<p>");
                paragraph.write_html(&mut output, &mut map_destination, cancellation)?;
                output.push_str("</p>");
            }
        }
        Ok(output)
    }

    fn sequence(children: Vec<Self>) -> Self {
        let mut normalized = Self::empty();
        for child in children {
            normalized.append(child);
        }
        normalized
    }

    fn empty() -> Self {
        Self {
            paragraphs: vec![FeedParagraph::default()],
        }
    }

    fn text(value: &str) -> Self {
        Self::inline(FeedInline::Text(value.to_string()))
    }

    fn inline(inline: FeedInline) -> Self {
        Self {
            paragraphs: vec![FeedParagraph {
                inlines: vec![inline],
            }],
        }
    }

    fn paragraph_break() -> Self {
        Self {
            paragraphs: vec![FeedParagraph::default(), FeedParagraph::default()],
        }
    }

    fn append(&mut self, child: Self) {
        let mut paragraphs = child.paragraphs.into_iter();
        if let Some(first) = paragraphs.next() {
            let current = self
                .paragraphs
                .last_mut()
                .expect("feed content always has a paragraph");
            for inline in first.inlines {
                current.push(inline);
            }
        }
        self.paragraphs.extend(paragraphs);
    }

    fn wrap(self, wrap: impl Fn(Vec<FeedInline>) -> FeedInline) -> Self {
        Self {
            paragraphs: self
                .paragraphs
                .into_iter()
                .map(|paragraph| FeedParagraph {
                    inlines: vec![wrap(paragraph.inlines)],
                })
                .collect(),
        }
    }
}

impl FeedParagraph {
    fn push(&mut self, inline: FeedInline) {
        if let FeedInline::Text(value) = inline {
            if value.is_empty() {
                return;
            }
            if let Some(FeedInline::Text(previous)) = self.inlines.last_mut() {
                previous.push_str(&value);
            } else {
                self.inlines.push(FeedInline::Text(value));
            }
        } else {
            self.inlines.push(inline);
        }
    }

    fn write_plain_text(
        &self,
        output: &mut String,
        cancellation: &BuildCancellation,
    ) -> Result<(), BuildCancelled> {
        for inline in &self.inlines {
            inline.write_plain_text(output, cancellation)?;
        }
        Ok(())
    }

    fn write_html(
        &self,
        output: &mut String,
        map_destination: &mut impl FnMut(&str) -> String,
        cancellation: &BuildCancellation,
    ) -> Result<(), BuildCancelled> {
        for inline in &self.inlines {
            inline.write_html(output, map_destination, cancellation)?;
        }
        Ok(())
    }
}

impl FeedInline {
    fn write_plain_text(
        &self,
        output: &mut String,
        cancellation: &BuildCancellation,
    ) -> Result<(), BuildCancelled> {
        cancellation.ensure_active()?;
        match self {
            Self::Text(value) => output.push_str(value),
            Self::Space => output.push(' '),
            Self::LineBreak => output.push('\n'),
            Self::Strong(body)
            | Self::Emphasis(body)
            | Self::Strikethrough(body)
            | Self::Link { body, .. } => {
                for inline in body {
                    inline.write_plain_text(output, cancellation)?;
                }
            }
        }
        Ok(())
    }

    fn write_html(
        &self,
        output: &mut String,
        map_destination: &mut impl FnMut(&str) -> String,
        cancellation: &BuildCancellation,
    ) -> Result<(), BuildCancelled> {
        cancellation.ensure_active()?;
        match self {
            Self::Text(value) => output.push_str(&escape(value)),
            Self::Space => output.push(' '),
            Self::LineBreak => output.push_str("<br>"),
            Self::Strong(body) => {
                write_element("strong", body, output, map_destination, cancellation)?
            }
            Self::Emphasis(body) => {
                write_element("em", body, output, map_destination, cancellation)?
            }
            Self::Strikethrough(body) => {
                write_element("s", body, output, map_destination, cancellation)?
            }
            Self::Link { destination, body } => {
                output.push_str("<a href=\"");
                output.push_str(&escape_attr(&map_destination(destination)));
                output.push_str("\">");
                for inline in body {
                    inline.write_html(output, map_destination, cancellation)?;
                }
                output.push_str("</a>");
            }
        }
        Ok(())
    }
}

fn validate_lifecycle(content: &Content, path: &str) -> Result<(), FeedContentError> {
    if content.label().is_some() {
        return Err(FeedContentError::Invalid {
            path: path.to_string(),
            violation: FeedContentViolation::Value {
                message: "feed content cannot have a label",
                help: "Remove the label",
            },
        });
    }
    if content.location().is_some() {
        return Err(FeedContentError::Invalid {
            path: path.to_string(),
            violation: FeedContentViolation::Value {
                message: "feed content cannot have a document location",
                help: "Write the text in the feed declaration",
            },
        });
    }
    if content.is_prepared() {
        return Err(FeedContentError::Invalid {
            path: path.to_string(),
            violation: FeedContentViolation::Value {
                message: "feed content must be authored content, not content already processed for a document",
                help: "Write the text in the feed declaration",
            },
        });
    }
    Ok(())
}

fn ensure_fields(
    content: &Content,
    path: &str,
    allowed: &[&str],
) -> Result<Dict, FeedContentError> {
    let fields = content.fields();
    if let Some((field, _)) = fields
        .iter()
        .find(|(field, _)| !allowed.contains(&field.as_str()))
    {
        return Err(FeedContentError::Invalid {
            path: field_path(path, field.as_str()),
            violation: FeedContentViolation::ContentField {
                element: content.func().name().to_string(),
            },
        });
    }
    Ok(fields)
}

fn normalize_link_destination(destination: &str) -> Option<&str> {
    if destination
        .chars()
        .any(|character| character.is_ascii_control() || character == '\\')
    {
        return None;
    }
    let destination = destination.trim_matches(|character: char| character.is_ascii_whitespace());
    if destination.is_empty() || destination.starts_with("//") {
        return None;
    }
    let Some(colon) = destination.find(':') else {
        return Some(destination);
    };
    let scheme = &destination[..colon];
    let is_scheme = scheme
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic())
        && scheme.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        });
    if is_scheme && url::Url::parse(destination).is_err() {
        return None;
    }
    (!is_scheme
        || matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "mailto" | "tel"
        ))
    .then_some(destination)
}

fn write_element(
    tag: &str,
    body: &[FeedInline],
    output: &mut String,
    map_destination: &mut impl FnMut(&str) -> String,
    cancellation: &BuildCancellation,
) -> Result<(), BuildCancelled> {
    output.push('<');
    output.push_str(tag);
    output.push('>');
    for inline in body {
        inline.write_html(output, map_destination, cancellation)?;
    }
    output.push_str("</");
    output.push_str(tag);
    output.push('>');
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::foundations::{Label, NativeElement};
    use typst::introspection::Location;
    use typst::text::UnderlineElem;
    use typst::visualize::Color;

    fn url(value: &str) -> Destination {
        Destination::Url(typst::model::Url::new(value).unwrap())
    }

    #[test]
    fn link_mapping_observes_cancellation() {
        let canceller = crate::cancellation::BuildCanceller::default();
        let cancellation = canceller.token();
        let source = LinkElem::new(
            LinkTarget::Dest(url("/document/")),
            TextElem::packed("Document"),
        )
        .pack();
        let content = FeedContent::from_typst(source, "/content", &cancellation).unwrap();

        let rendered = content.to_html_with_links(
            |destination| {
                canceller.cancel();
                destination.to_owned()
            },
            &cancellation,
        );

        assert!(matches!(rendered, Err(BuildCancelled)));
    }

    #[test]
    fn semantic_markup_becomes_html() {
        let source = Content::sequence([
            TextElem::packed("这是 "),
            StrikeElem::new(TextElem::packed("旧内容")).pack(),
            TextElem::packed("、"),
            StrongElem::new(TextElem::packed("重点")).pack(),
            TextElem::packed("和"),
            EmphElem::new(TextElem::packed("补充")).pack(),
            TextElem::packed("；请看 "),
            LinkElem::new(LinkTarget::Dest(url("/new/")), TextElem::packed("新方案")).pack(),
        ]);

        let content =
            FeedContent::from_typst(source, "/summary", &BuildCancellation::default()).unwrap();

        assert_eq!(
            content
                .to_html_with_links(str::to_owned, &BuildCancellation::default())
                .unwrap(),
            "这是 <s>旧内容</s>、<strong>重点</strong>和<em>补充</em>；请看 <a href=\"/new/\">新方案</a>"
        );
        assert_eq!(
            content
                .to_plain_text(&BuildCancellation::default())
                .unwrap(),
            "这是 旧内容、重点和补充；请看 新方案"
        );
    }

    #[test]
    fn html_escaping_is_complete() {
        let source = LinkElem::new(
            LinkTarget::Dest(url("/search?q=\"rust\"&kind=<post>")),
            TextElem::packed("<Rust> & Typst"),
        )
        .pack();

        let content =
            FeedContent::from_typst(source, "/card/action", &BuildCancellation::default()).unwrap();

        assert_eq!(
            content
                .to_html_with_links(str::to_owned, &BuildCancellation::default())
                .unwrap(),
            "<a href=\"/search?q=&quot;rust&quot;&amp;kind=&lt;post&gt;\">&lt;Rust&gt; &amp; Typst</a>"
        );
    }

    #[test]
    fn whitespace_becomes_portable_markup() {
        let source = Content::sequence([
            TextElem::packed("A"),
            SymbolElem::packed("+"),
            SpaceElem::shared().clone(),
            TextElem::packed("B"),
            LinebreakElem::shared().clone(),
            TextElem::packed("C"),
            ParbreakElem::shared().clone(),
            SmartQuoteElem::new().with_double(false).pack(),
            TextElem::packed("quoted"),
            SmartQuoteElem::new().with_double(false).pack(),
        ]);

        let content =
            FeedContent::from_typst(source, "/summary", &BuildCancellation::default()).unwrap();

        assert_eq!(
            content
                .to_html_with_links(str::to_owned, &BuildCancellation::default())
                .unwrap(),
            "<p>A+ B<br>C</p><p>'quoted'</p>"
        );
        assert_eq!(
            content
                .to_plain_text(&BuildCancellation::default())
                .unwrap(),
            "A+ B\nC\n\n'quoted'"
        );
    }

    #[test]
    fn splits_inline_markup_across_paragraphs() {
        let source = StrongElem::new(Content::sequence([
            TextElem::packed("first"),
            ParbreakElem::shared().clone(),
            TextElem::packed("second"),
        ]))
        .pack();

        let content =
            FeedContent::from_typst(source, "/summary", &BuildCancellation::default()).unwrap();

        assert_eq!(
            content
                .to_html_with_links(str::to_owned, &BuildCancellation::default())
                .unwrap(),
            "<p><strong>first</strong></p><p><strong>second</strong></p>"
        );
        assert_eq!(
            content
                .to_plain_text(&BuildCancellation::default())
                .unwrap(),
            "first\n\nsecond"
        );
    }

    /// Values a feed reader cannot receive faithfully: styled or realized nodes,
    /// lifecycle attributes, unlisted elements, explicit visual fields, and nested links.
    #[test]
    fn unrepresentable_content_is_rejected() {
        for (label, source, path) in [
            (
                "styled text",
                TextElem::packed("styled").set(TextElem::fill, Color::RED.into()),
                "",
            ),
            (
                "strong delta",
                StrongElem::new(TextElem::packed("strong"))
                    .with_delta(300)
                    .pack(),
                "/summary/delta",
            ),
            (
                "strike background",
                StrikeElem::new(TextElem::packed("strike"))
                    .with_background(false)
                    .pack(),
                "/summary/background",
            ),
            (
                "justified linebreak",
                LinebreakElem::new().with_justify(false).pack(),
                "/summary/justify",
            ),
            (
                "labelled text",
                TextElem::packed("labelled")
                    .labelled(Label::construct("text-label".into()).unwrap()),
                "",
            ),
            (
                "located text",
                TextElem::packed("located").located(Location::new(1)),
                "",
            ),
            (
                "prepared text",
                {
                    let mut prepared = TextElem::packed("prepared");
                    prepared.mark_prepared();
                    prepared
                },
                "",
            ),
            (
                "underline",
                UnderlineElem::new(TextElem::packed("underline")).pack(),
                "",
            ),
            (
                "inner link",
                LinkElem::new(
                    LinkTarget::Dest(url("/outer/")),
                    LinkElem::new(LinkTarget::Dest(url("/inner/")), TextElem::packed("inner"))
                        .pack(),
                )
                .pack(),
                "/summary/body",
            ),
        ] {
            let error = FeedContent::from_typst(source, "/summary", &BuildCancellation::default())
                .unwrap_err();
            let FeedContentError::Invalid { path: actual, .. } = &error else {
                panic!("{label}: unexpected cancellation");
            };
            assert_eq!(
                actual,
                if path.is_empty() { "/summary" } else { path },
                "{label}"
            );
        }

        for destination in [
            "javascript:alert(1)",
            " data:text/html,evil",
            "vbscript:evil",
            "java\tscript:alert(1)",
            "java\nscript:alert(1)",
            "java\rscript:alert(1)",
            "/\\evil.example/path",
            "//evil.example/path",
            "   ",
        ] {
            let source = LinkElem::new(
                LinkTarget::Dest(url(destination)),
                TextElem::packed("unsafe"),
            )
            .pack();
            let error = FeedContent::from_typst(source, "/summary", &BuildCancellation::default())
                .unwrap_err();
            assert!(matches!(
                error,
                FeedContentError::Invalid { path, violation: FeedContentViolation::Value { .. } }
                    if path == "/summary/dest"
            ));
        }
    }
}
