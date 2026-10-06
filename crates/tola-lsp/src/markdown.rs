//! What the markdown payloads hold, as the plain text a client asks for and the docs a hover
//! reads.

/// The text a markdown payload reads as, for a client that asked for plaintext.
pub(crate) fn plain(markdown: &str) -> String {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let mut text = String::with_capacity(markdown.len());
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Text(value)
            | Event::Code(value)
            | Event::InlineMath(value)
            | Event::DisplayMath(value)
            | Event::FootnoteReference(value)
            | Event::Html(value)
            | Event::InlineHtml(value) => text.push_str(&value),
            Event::SoftBreak | Event::HardBreak => text.push('\n'),
            Event::Start(Tag::Item) => {
                line_breaks(&mut text, 1);
                text.push_str("- ");
            }
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::List(_),
            )
            | Event::Rule => line_breaks(&mut text, 2),
            Event::End(TagEnd::Item | TagEnd::TableHead | TagEnd::TableRow) => {
                line_breaks(&mut text, 1)
            }
            Event::End(TagEnd::TableCell) => text.push('\t'),
            Event::TaskListMarker(checked) => text.push_str(if checked { "[x] " } else { "[ ] " }),
            _ => {}
        }
    }
    text.truncate(text.trim_end().len());
    text
}

fn line_breaks(text: &mut String, count: usize) {
    if text.is_empty() {
        return;
    }
    let existing = text.len() - text.trim_end_matches('\n').len();
    for _ in existing..count {
        text.push('\n');
    }
}

/// A docs body as one hover section has it: no blank edges, and every sample left as the
/// documentation wrote it.
///
/// Typst's own sources fence their samples as `example`, which names no grammar an editor has;
/// the fence an author reads in the hover is the `typ` the documentation sites write, and every
/// other language reaches the editor untouched.
pub(crate) fn docs(documentation: &str) -> String {
    let mut text = String::with_capacity(documentation.len());
    let mut fence: Option<char> = None;
    for (index, line) in documentation.trim().lines().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        let indent = line.len() - line.trim_start().len();
        let rest = &line[indent..];
        let marker = rest
            .chars()
            .take_while(|ch| matches!(ch, '`' | '~'))
            .count();
        if marker >= 3 {
            let character = rest.chars().next().unwrap();
            if fence == Some(character) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(character);
                let info = &rest[marker..];
                let end = info.find(char::is_whitespace).unwrap_or(info.len());
                if &info[..end] == "example" {
                    text.push_str(&line[..indent + marker]);
                    text.push_str("typ");
                    text.push_str(&rest[marker + end..]);
                    continue;
                }
            }
            // The other fence character inside an open one is the sample's own text.
            text.push_str(line);
            continue;
        }
        text.push_str(line);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_preserves_visible_markdown() {
        let markdown = "A [link](https://example.test) and `snake_name`.\n\n- **first**\n- second\n\n```typst\n#let value = [x]\n```";
        assert_eq!(
            plain(markdown),
            "A link and snake_name.\n\n- first\n- second\n\n#let value = [x]"
        );
    }

    /// The `example` fence Typst's own docs write becomes the `typ` an editor knows; every other
    /// fence reaches the hover as the documentation wrote it.
    #[test]
    fn example_fences_become_typ() {
        for (documentation, expected) in [
            (
                "A sample:\n\n```example\n#let value = 1\n```\n",
                "A sample:\n\n```typ\n#let value = 1\n```",
            ),
            (
                "```typst\n#let value = 1\n```",
                "```typst\n#let value = 1\n```",
            ),
        ] {
            assert_eq!(docs(documentation), expected, "{documentation:?}");
        }
    }
}
