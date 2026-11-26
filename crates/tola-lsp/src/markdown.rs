//! Markdown-to-plaintext conversion and Typst doc fence normalization.

/// Convert markdown to plain text for clients without markdown support.
pub(crate) fn plain(md: &str) -> String {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let mut out = String::with_capacity(md.len());
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for ev in Parser::new_ext(md, opts) {
        match ev {
            Event::Text(val)
            | Event::Code(val)
            | Event::InlineMath(val)
            | Event::DisplayMath(val)
            | Event::FootnoteReference(val)
            | Event::Html(val)
            | Event::InlineHtml(val) => out.push_str(&val),
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            Event::Start(Tag::Item) => {
                line_breaks(&mut out, 1);
                out.push_str("- ");
            }
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::List(_),
            )
            | Event::Rule => line_breaks(&mut out, 2),
            Event::End(TagEnd::Item | TagEnd::TableHead | TagEnd::TableRow) => {
                line_breaks(&mut out, 1)
            }
            Event::End(TagEnd::TableCell) => out.push('\t'),
            Event::TaskListMarker(checked) => out.push_str(if checked { "[x] " } else { "[ ] " }),
            _ => {}
        }
    }
    out.truncate(out.trim_end().len());
    out
}

fn line_breaks(out: &mut String, count: usize) {
    if out.is_empty() {
        return;
    }
    let existing = out.len() - out.trim_end_matches('\n').len();
    for _ in existing..count {
        out.push('\n');
    }
}

/// Normalize doc markdown: trim blank edges and rewrite Typst's `example` fence to `typ`.
pub(crate) fn docs(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut fence: Option<char> = None;
    for (idx, line) in raw.trim().lines().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        let indent = line.len() - line.trim_start().len();
        let rest = &line[indent..];
        let marker = rest
            .chars()
            .take_while(|ch| matches!(ch, '`' | '~'))
            .count();
        if marker >= 3 {
            let ch = rest.chars().next().unwrap();
            if fence == Some(ch) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(ch);
                let info = &rest[marker..];
                let end = info.find(char::is_whitespace).unwrap_or(info.len());
                if &info[..end] == "example" {
                    out.push_str(&line[..indent + marker]);
                    out.push_str("typ");
                    out.push_str(&rest[marker + end..]);
                    continue;
                }
            }
        }
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_visible_markdown() {
        let md = "A [link](https://example.test) and `snake_name`.\n\n- **first**\n- second\n\n```typst\n#let value = [x]\n```";
        assert_eq!(
            plain(md),
            "A link and snake_name.\n\n- first\n- second\n\n#let value = [x]"
        );
    }

    #[test]
    fn example_fences_become_typ() {
        for (raw, expected) in [
            (
                "A sample:\n\n```example\n#let value = 1\n```\n",
                "A sample:\n\n```typ\n#let value = 1\n```",
            ),
            (
                "```typst\n#let value = 1\n```",
                "```typst\n#let value = 1\n```",
            ),
        ] {
            assert_eq!(docs(raw), expected, "{raw:?}");
        }
    }
}
