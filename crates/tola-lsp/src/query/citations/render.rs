//! Rendering one entry as markdown, and the note naming files no reader understood.

use std::sync::LazyLock;

use hayagriva::citationberg::{Display, FontStyle, FontWeight, Locale};
use hayagriva::{ElemChild, ElemChildren, Formatted};
use tola_typst::typst::syntax::FileId;

use super::sources::{Declared, StyleLocale};

/// The CSL locale files a style reads its terms from, loaded once.
static LOCALES: LazyLock<Vec<Locale>> = LazyLock::new(hayagriva::archive::locales);

/// What one entry answers with.
///
/// The page prints a citation in the style its own `bibliography(…)` call names, in the language
/// that call is written in, so the entry renders here the way the call that declares it renders
/// it. An entry whose call the site did not compile has no rendering to read, and the entry's own
/// fields answer instead.
pub(super) fn rendered(declared: &Declared<'_>) -> String {
    declared
        .rendering
        .and_then(|rendering| cited(rendering, declared.entry))
        .unwrap_or_else(|| declared.described.to_owned())
}

/// The note a key no entry reaches has when a bibliography file could not be read.
pub(super) fn unread_note(files: &[FileId]) -> String {
    let quoted: Vec<String> = files
        .iter()
        .map(|file| format!("`{}`", file.vpath().get_without_slash()))
        .collect();
    let (listed, remaining) = crate::sentence::listed(
        quoted.iter().map(String::as_str),
        crate::routes::NAMED_PAGES,
    );
    let listed = if remaining > 0 {
        format!("{listed} and {remaining} more")
    } else {
        listed
    };
    format!("Tola could not read {listed}")
}

/// One entry as the style renders the reference to it, in markdown.
///
/// The bibliography item, not the citation: a citation renders as the number the page happens to
/// give it, and an author hovering a key asks which work it names.
fn cited(rendering: &StyleLocale, entry: &hayagriva::Entry) -> Option<String> {
    let item = rendered_item(rendering, entry)?;
    let mut rendered = String::new();
    write_markdown(&item, &mut rendered);
    Some(rendered)
}

/// The bibliography item one entry renders to under its call's style and language.
fn rendered_item(rendering: &StyleLocale, entry: &hayagriva::Entry) -> Option<ElemChildren> {
    let style = rendering.style.get();
    let mut driver = hayagriva::BibliographyDriver::new();
    driver.citation(hayagriva::CitationRequest::from_items(
        vec![hayagriva::CitationItem::with_entry(entry)],
        style,
        &LOCALES,
    ));
    let rendered = driver.finish(hayagriva::BibliographyRequest::new(
        style,
        Some(rendering.locale.clone()),
        &LOCALES,
    ));
    Some(rendered.bibliography?.items.first()?.content.clone())
}

/// The item as CommonMark: emphasis, strong, and links, with every character markdown would read
/// escaped.
///
/// A hover is markdown, and CommonMark is the one markup every client that asks for markdown
/// renders: the styles a site names emphasize titles and journal names and link DOIs, and both
/// survive here. Underline, small caps, and sub/superscript have no CommonMark form, so their text
/// stays as the author wrote it.
fn write_markdown(children: &ElemChildren, out: &mut String) {
    for child in &children.0 {
        match child {
            ElemChild::Text(text) => write_run(text, out),
            ElemChild::Elem(elem) => {
                if elem.display == Some(Display::Block) {
                    out.push_str("\n\n");
                }
                write_markdown(&elem.children, out);
            }
            ElemChild::Markup(markup) => write_escaped(markup, out),
            ElemChild::Link { text, url } => {
                out.push('[');
                write_run(text, out);
                // A destination with whitespace or parentheses only parses inside `<…>`.
                if url.contains([' ', '(', ')']) {
                    out.push_str("](<");
                    write_escaped(url, out);
                    out.push_str(">)");
                } else {
                    out.push_str("](");
                    out.push_str(url);
                    out.push(')');
                }
            }
            ElemChild::Transparent { .. } => {}
        }
    }
}

/// One formatted run as markdown.
fn write_run(text: &Formatted, out: &mut String) {
    let mut body = String::with_capacity(text.text.len());
    write_escaped(&text.text, &mut body);
    if text.formatting.font_style == FontStyle::Italic {
        body = format!("*{body}*");
    }
    if text.formatting.font_weight == FontWeight::Bold {
        body = format!("**{body}**");
    }
    out.push_str(&body);
}

/// The text with the characters markdown reads as markup escaped.
fn write_escaped(text: &str, out: &mut String) {
    for ch in text.chars() {
        if matches!(
            ch,
            '\\' | '*' | '_' | '[' | ']' | '`' | '<' | '>' | '&' | '#' | '!' | '|'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hayagriva::archive::ArchivedStyle;
    use hayagriva::citationberg::LocaleCode;
    use tola_typst::typst::model::CslStyle;

    const BIB: &str = r#"
@article{knuth1984,
  title = {Literate Programming},
  author = {Knuth, Donald E. and Muster, Max},
  year = {1984},
  journal = {The Computer Journal},
  volume = {27},
  pages = {97--111},
  doi = {10.1093/comjnl/27.2.97},
}
@book{marks,
  title = {Stars *and* Underscores & <Angles>},
  editor = {Editor, Ed},
  year = {2001},
  publisher = {A Publisher},
  url = {https://example.test/a(b)c},
}
"#;

    /// The style and language one archived style answers with.
    fn style_locale(style: ArchivedStyle) -> StyleLocale {
        StyleLocale {
            style: CslStyle::from_archived(style),
            locale: LocaleCode("en".to_owned()),
        }
    }

    /// The markdown one entry answers with, and the plain text it must read as.
    fn rendered(style_locale: &StyleLocale, key: &str) -> (String, String) {
        let library = hayagriva::io::from_biblatex_str(BIB).expect("the bibliography parses");
        let entry = library.get(key).expect("the entry is written");
        let item = rendered_item(style_locale, entry).expect("the style renders the item");
        let mut plain = String::new();
        item.write_buf(&mut plain, hayagriva::BufWriteFormat::Plain)
            .expect("the item writes");
        (
            cited(style_locale, entry).expect("the item writes as markdown"),
            plain,
        )
    }

    /// The text with whitespace collapsed: a paragraph break is two newlines in markdown and one
    /// in the plain writer.
    fn one_line(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Whatever a style renders, the markdown reads as exactly the plain text.
    #[test]
    fn markdown_preserves_plain_text() {
        for style in [
            ArchivedStyle::InstituteOfElectricalAndElectronicsEngineers,
            ArchivedStyle::Nature,
            ArchivedStyle::AmericanPsychologicalAssociation,
        ] {
            let style_locale = style_locale(style);
            for key in ["knuth1984", "marks"] {
                let (markdown, plain) = rendered(&style_locale, key);
                assert_eq!(
                    one_line(&crate::markdown::plain(&markdown)),
                    one_line(&plain),
                    "{style:?} {key}: {markdown}"
                );
            }
        }
    }

    /// The emphasis and links a style writes survive as markdown.
    #[test]
    fn emphasis_and_links_survive() {
        let style_locale =
            style_locale(ArchivedStyle::InstituteOfElectricalAndElectronicsEngineers);
        let (markdown, _) = rendered(&style_locale, "knuth1984");
        assert!(markdown.contains("*The Computer Journal*"), "{markdown}");
        assert!(
            markdown.contains("[10.1093/comjnl/27.2.97](https://doi.org/10.1093/comjnl/27.2.97)"),
            "{markdown}"
        );
    }

    /// Text that reads as markdown markup stays the author's text.
    #[test]
    fn literal_markup_stays_text() {
        let style_locale =
            style_locale(ArchivedStyle::InstituteOfElectricalAndElectronicsEngineers);
        let (markdown, plain) = rendered(&style_locale, "marks");
        assert!(plain.contains("<Angles>"), "{plain}");
        let text = crate::markdown::plain(&markdown);
        assert!(text.contains("<Angles>"), "{markdown}");
        assert!(text.contains("*and*"), "{markdown}");
    }
}
