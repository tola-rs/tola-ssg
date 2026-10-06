//! Text of Typst content, following Typst's own reading rules.

use typst::diag::{SourceResult, bail, warning};
use typst::ecow::EcoString;
use typst::engine::Engine;
use typst::foundations::{Content, PlainText, Str, Value, func};
use typst::introspection::MetadataElem;
use typst::syntax::{Span, Spanned};

#[func]
pub(super) fn tola_plain_text(
    engine: &mut Engine,
    /// A string or Typst content.
    value: Spanned<Value>,
) -> SourceResult<Str> {
    match value.v {
        Value::Str(text) => Ok(text),
        Value::Content(content) => {
            let projection = TextProjection::of(&content);
            if !projection.unreadable.is_empty() {
                engine.sink.warn(warning!(
                    projection.span().unwrap_or(value.span),
                    "{}",
                    unreadable(&projection.unreadable);
                    hint: "write the text you want in its place"
                ));
            }
            Ok(projection.text.into())
        }
        other => bail!(
            value.span,
            "expected string or content, found {}",
            other.ty().short_name()
        ),
    }
}

#[derive(Default)]
struct TextProjection {
    text: EcoString,
    unreadable: Vec<(EcoString, Span)>,
}

impl TextProjection {
    fn of(content: &Content) -> Self {
        let mut projection = Self::default();
        projection.write(content);
        projection
            .unreadable
            .sort_by(|left, right| left.0.cmp(&right.0));
        projection
            .unreadable
            .dedup_by(|left, right| left.0 == right.0);
        projection
    }

    /// Where the first omitted element was written, when that is known.
    ///
    /// An element built by a helper has no place of its own, so the call site is the
    /// only location the author can act on.
    fn span(&self) -> Option<Span> {
        self.unreadable
            .iter()
            .map(|(_, span)| *span)
            .find(|span| !span.is_detached())
    }

    fn write(&mut self, content: &Content) {
        // Typst's own reading decides which elements carry text. Everything it does
        // not read contributes through its content instead.
        if let Some(readable) = content.with::<dyn PlainText>() {
            readable.plain_text(&mut self.text);
            return;
        }

        // Metadata is not display content: its payload never becomes text.
        if content.is::<MetadataElem>() {
            return;
        }

        let mut carries_content = false;
        for (_, value) in content.fields() {
            self.write_value(value, &mut carries_content);
        }
        if !carries_content {
            self.unreadable
                .push((content.func().name().into(), content.span()));
        }
    }

    fn write_value(&mut self, value: Value, carries_content: &mut bool) {
        match value {
            Value::Content(content) => {
                *carries_content = true;
                self.write(&content);
            }
            Value::Array(array) => {
                for value in array {
                    self.write_value(value, carries_content);
                }
            }
            _ => {}
        }
    }
}

/// State what the text left out, naming at most three elements like other Tola
/// diagnostics that list an unbounded set.
fn unreadable(unreadable: &[(EcoString, Span)]) -> String {
    let shown = unreadable
        .iter()
        .take(3)
        .map(|(name, _)| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let shown = match unreadable.len().checked_sub(3) {
        Some(extra) if extra > 0 => format!("{shown}, and {extra} more"),
        _ => shown,
    };
    if unreadable.len() == 1 {
        format!("{shown} has no text, so the text leaves it out")
    } else {
        format!("{shown} have no text, so the text leaves them out")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::foundations::{Label, NativeElement, SymbolElem};
    use typst::math::EquationElem;
    use typst::model::{Destination, EmphElem, LinkElem, LinkTarget, RefElem, StrongElem, Url};
    use typst::text::{RawContent, RawElem, SmartQuoteElem, SpaceElem, TextElem};
    use typst::visualize::Color;

    fn project(content: &Content) -> TextProjection {
        TextProjection::of(content)
    }

    fn text(content: &Content) -> String {
        project(content).text.to_string()
    }

    fn unreadable_names(content: &Content) -> Vec<String> {
        project(content)
            .unreadable
            .iter()
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// One row per element kind: what Typst reads contributes its own text, and everything else
    /// contributes through its content.
    #[test]
    fn projection_follows_typst_reads() {
        let readable = Content::sequence([
            TextElem::packed("A"),
            SymbolElem::packed("+"),
            SpaceElem::shared().clone(),
            RawElem::new(RawContent::Text("code()".into())).pack(),
            SpaceElem::shared().clone(),
            SmartQuoteElem::new().with_double(true).pack(),
            TextElem::packed("quoted"),
            SmartQuoteElem::new().with_double(false).pack(),
        ]);
        assert_eq!(text(&readable), "A+ code() \"quoted'");

        let containers = StrongElem::new(Content::sequence([
            TextElem::packed("bold"),
            EmphElem::new(TextElem::packed("italic")).pack(),
            LinkElem::new(
                LinkTarget::Dest(Destination::Url(Url::new("/ignored-target/").unwrap())),
                TextElem::packed("linked"),
            )
            .pack(),
        ]))
        .pack()
        .set(TextElem::fill, Color::RED.into());
        assert_eq!(text(&containers), "bolditaliclinked");

        let mathematics = Content::sequence([
            TextElem::packed("before"),
            EquationElem::new(TextElem::packed("x")).pack(),
            TextElem::packed("after"),
        ]);
        assert_eq!(text(&mathematics), "beforexafter");

        let metadata = Content::sequence([
            TextElem::packed("before"),
            MetadataElem::new(Value::Content(
                EquationElem::new(TextElem::packed("hidden equation")).pack(),
            ))
            .pack(),
            StrongElem::new(Content::sequence([
                MetadataElem::new(Value::Str("hidden string".into())).pack(),
                TextElem::packed("after"),
            ]))
            .pack(),
        ]);
        assert_eq!(text(&metadata), "beforeafter");
    }

    #[test]
    fn unreadable_elements_are_named() {
        let reference = |label: &str| {
            RefElem::new(Label::construct(label.into()).unwrap())
                .pack()
                .spanned(Span::detached())
        };
        let source = Content::sequence([
            TextElem::packed("see "),
            reference("figure"),
            TextElem::packed(" and "),
            reference("table"),
        ]);
        assert_eq!(text(&source), "see  and ");
        assert_eq!(unreadable_names(&source), ["ref"]);
        assert!(unreadable_names(&Content::sequence([TextElem::packed("see ")])).is_empty());
    }

    #[test]
    fn unreadable_summary_names_at_most_three() {
        let summarised = |names: &[&str]| {
            unreadable(
                &names
                    .iter()
                    .map(|name| (EcoString::from(*name), Span::detached()))
                    .collect::<Vec<_>>(),
            )
        };
        fn named(summary: &str) -> Vec<&str> {
            summary.split('`').skip(1).step_by(2).collect()
        }

        assert_eq!(named(&summarised(&["image"])), ["image"]);
        assert_eq!(named(&summarised(&["a", "b", "c"])), ["a", "b", "c"]);
        assert_eq!(
            named(&summarised(&["a", "b", "c", "d", "e"])),
            ["a", "b", "c"]
        );
        assert!(summarised(&["a", "b", "c", "d", "e"]).contains("and 2 more"));
    }
}
