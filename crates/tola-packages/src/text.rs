//! Plain-text extraction over Typst content following Typst's own reading rules.

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
            let buf = PlainTextBuf::of(&content);
            if !buf.unreadable.is_empty() {
                engine.sink.warn(warning!(
                    buf.span().unwrap_or(value.span),
                    "{}",
                    unreadable_msg(&buf.unreadable);
                    hint: "write the text you want in its place"
                ));
            }
            Ok(buf.text.into())
        }
        other => bail!(
            value.span,
            "expected string or content, found {}",
            other.ty().short_name()
        ),
    }
}

#[derive(Default)]
struct PlainTextBuf {
    text: EcoString,
    unreadable: Vec<(EcoString, Span)>,
}

impl PlainTextBuf {
    fn of(content: &Content) -> Self {
        let mut buf = Self::default();
        buf.write(content);
        buf.unreadable.sort_by(|a, b| a.0.cmp(&b.0));
        buf.unreadable.dedup_by(|a, b| a.0 == b.0);
        buf
    }

    fn span(&self) -> Option<Span> {
        self.unreadable
            .iter()
            .map(|(_, span)| *span)
            .find(|span| !span.is_detached())
    }

    fn write(&mut self, content: &Content) {
        if let Some(readable) = content.with::<dyn PlainText>() {
            readable.plain_text(&mut self.text);
            return;
        }
        // Metadata is non-visual payload, never rendered text.
        if content.is::<MetadataElem>() {
            return;
        }
        let mut has_child = false;
        for (_, val) in content.fields() {
            self.write_val(val, &mut has_child);
        }
        if !has_child {
            self.unreadable
                .push((content.func().name().into(), content.span()));
        }
    }

    fn write_val(&mut self, val: Value, has_child: &mut bool) {
        match val {
            Value::Content(content) => {
                *has_child = true;
                self.write(&content);
            }
            Value::Array(arr) => {
                for item in arr {
                    self.write_val(item, has_child);
                }
            }
            _ => {}
        }
    }
}

fn unreadable_msg(unreadable: &[(EcoString, Span)]) -> String {
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
    let verb = if unreadable.len() == 1 {
        "has no text, so the text leaves it out"
    } else {
        "have no text, so the text leaves them out"
    };
    format!("{shown} {verb}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::foundations::{Label, NativeElement, SymbolElem};
    use typst::math::EquationElem;
    use typst::model::{Destination, EmphElem, LinkElem, LinkTarget, RefElem, StrongElem, Url};
    use typst::text::{RawContent, RawElem, SmartQuoteElem, SpaceElem, TextElem};
    use typst::visualize::Color;

    fn text(content: &Content) -> String {
        PlainTextBuf::of(content).text.to_string()
    }

    fn unreadable_names(content: &Content) -> Vec<String> {
        PlainTextBuf::of(content)
            .unreadable
            .iter()
            .map(|(name, _)| name.to_string())
            .collect()
    }

    #[test]
    fn plain_text_elements() {
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

        let math = Content::sequence([
            TextElem::packed("before"),
            EquationElem::new(TextElem::packed("x")).pack(),
            TextElem::packed("after"),
        ]);
        assert_eq!(text(&math), "beforexafter");

        let meta = Content::sequence([
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
        assert_eq!(text(&meta), "beforeafter");
    }

    #[test]
    fn plain_text_unreadable_warned() {
        let mk_ref = |label: &str| {
            RefElem::new(Label::construct(label.into()).unwrap())
                .pack()
                .spanned(Span::detached())
        };
        let src = Content::sequence([
            TextElem::packed("see "),
            mk_ref("figure"),
            TextElem::packed(" and "),
            mk_ref("table"),
        ]);
        assert_eq!(text(&src), "see  and ");
        assert_eq!(unreadable_names(&src), ["ref"]);
        assert!(unreadable_names(&Content::sequence([TextElem::packed("see ")])).is_empty());
    }
}
