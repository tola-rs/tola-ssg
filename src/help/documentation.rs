use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag};
use unicode_width::UnicodeWidthStr;

use super::model::{Node, plain_nodes};
use crate::terminal::text::{Style, StyledText};
use crate::terminal::wrap;

pub(crate) struct Documentation {
    columns: Option<usize>,
    color: bool,
}

impl Documentation {
    /// Columns assumed when the caller does not size the output.
    pub(crate) const DEFAULT_COLUMNS: usize = 88;

    pub(crate) fn new(columns: Option<usize>, color: bool) -> Self {
        Self { columns, color }
    }

    pub(crate) fn render(&self, markdown: &str) -> String {
        let nodes =
            plain_nodes(&mut Parser::new_ext(markdown, Options::ENABLE_TABLES).into_offset_iter());
        let mut rendered = self.blocks(&nodes, "");
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered
    }

    fn columns(&self) -> usize {
        self.columns.unwrap_or(Self::DEFAULT_COLUMNS).max(1)
    }

    fn blocks(&self, nodes: &[Node<'_>], indent: &str) -> String {
        let mut blocks = Vec::new();
        let mut inline_start = 0;
        for (position, node) in nodes.iter().enumerate() {
            if !is_block(node) {
                continue;
            }
            if inline_start < position {
                blocks.push(self.paragraph(&nodes[inline_start..position], indent, Style::Plain));
            }
            let rendered = match node {
                Node::Branch(Tag::Paragraph, children) => {
                    let style = if is_lead_in(nodes, position, children) {
                        Style::Lead
                    } else {
                        Style::Plain
                    };
                    self.paragraph(children, indent, style)
                }
                Node::Heading {
                    level, children, ..
                } => {
                    let style = match level {
                        pulldown_cmark::HeadingLevel::H1 => Style::Heading1,
                        pulldown_cmark::HeadingLevel::H2 => Style::Heading2,
                        pulldown_cmark::HeadingLevel::H3 => Style::Heading3,
                        _ => Style::Heading4,
                    };
                    self.paragraph(children, indent, style)
                }
                Node::Branch(Tag::CodeBlock(kind), children) => self.code(kind, children, indent),
                Node::Branch(Tag::List(start), children) => self.list(*start, children, indent),
                Node::Branch(Tag::Table(alignments), children) => {
                    self.table(alignments, children, indent)
                }
                Node::Branch(Tag::BlockQuote(_), children) => {
                    self.blocks(children, &format!("{indent}> "))
                }
                Node::Branch(_, children) => self.blocks(children, indent),
                Node::Leaf(Event::Rule) => {
                    let width = self.columns().saturating_sub(indent.width()).max(1);
                    format!("{indent}{}", "-".repeat(width))
                }
                Node::Leaf(_) => unreachable!(),
            };
            if !rendered.is_empty() {
                blocks.push(rendered);
            }
            inline_start = position + 1;
        }
        if inline_start < nodes.len() {
            blocks.push(self.paragraph(&nodes[inline_start..], indent, Style::Plain));
        }
        blocks.join("\n\n")
    }

    fn paragraph(&self, nodes: &[Node<'_>], indent: &str, style: Style) -> String {
        let mut text = StyledText::default();
        inline_text(nodes, style, &mut text);
        self.wrapped(&text, indent)
    }

    fn wrapped(&self, text: &StyledText, indent: &str) -> String {
        wrap::line_ranges(
            &text.text,
            self.columns().saturating_sub(indent.width()).max(1),
        )
        .into_iter()
        .map(|range| format!("{indent}{}", text.paint(range, self.color)))
        .collect::<Vec<_>>()
        .join("\n")
    }

    fn list(&self, start: Option<u64>, children: &[Node<'_>], indent: &str) -> String {
        children
            .iter()
            .filter_map(|node| match node {
                Node::Branch(Tag::Item, children) => Some(children),
                _ => None,
            })
            .enumerate()
            .map(|(position, children)| {
                let marker = start.map_or_else(
                    || "- ".to_owned(),
                    |start| format!("{}. ", start + position as u64),
                );
                let continuation = format!("{indent}{}", " ".repeat(marker.width()));
                let body = self.blocks(children, &continuation);
                format!(
                    "{indent}{}{}",
                    Style::Emphasis.paint(&marker, self.color),
                    body.strip_prefix(&continuation).unwrap_or(&body)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn code(&self, kind: &CodeBlockKind<'_>, children: &[Node<'_>], indent: &str) -> String {
        let source: String = children
            .iter()
            .filter_map(|node| match node {
                Node::Leaf(Event::Text(text)) => Some(text.as_ref()),
                _ => None,
            })
            .collect();
        let language = match kind {
            CodeBlockKind::Fenced(language) => match language.split_whitespace().next() {
                // Package examples tagged typc are compiled as complete Typst documents.
                Some("typc") => "typ",
                language => language.unwrap_or_default(),
            },
            CodeBlockKind::Indented => "",
        };
        let theme = if self.color {
            tola_highlight::ThemeChoice::Dark
        } else {
            tola_highlight::ThemeChoice::Plain
        };
        let text = tola_highlight::highlight(&source, language, theme);
        // Code is never reflowed: the parser's source bytes remain copyable after indentation.
        let mut rendered = String::new();
        let mut offset = 0;
        for line in source.split_inclusive('\n') {
            let end = offset + line.strip_suffix('\n').unwrap_or(line).len();
            rendered.push_str(indent);
            rendered.push_str("  ");
            text.paint(offset..end, &mut rendered);
            if line.ends_with('\n') {
                rendered.push('\n');
            }
            offset += line.len();
        }
        if rendered.ends_with('\n') {
            rendered.pop();
        }
        rendered
    }

    fn table(&self, alignments: &[Alignment], children: &[Node<'_>], indent: &str) -> String {
        let rows: Vec<Vec<StyledText>> = children
            .iter()
            .filter_map(|node| match node {
                Node::Branch(Tag::TableHead | Tag::TableRow, cells) => Some(cells),
                _ => None,
            })
            .map(|cells| {
                cells
                    .iter()
                    .filter_map(|node| match node {
                        Node::Branch(Tag::TableCell, children) => {
                            let mut text = StyledText::default();
                            inline_text(children, Style::Plain, &mut text);
                            Some(text)
                        }
                        _ => None,
                    })
                    .collect()
            })
            .collect();
        let Some(headers) = rows.first() else {
            return String::new();
        };
        let column_count = headers.len();
        if column_count == 0 {
            return String::new();
        }
        let available = self.columns().saturating_sub(indent.width());
        let minimum: Vec<usize> = (0..column_count)
            .map(|column| {
                column_minimum(
                    rows.iter()
                        .filter_map(|row| row.get(column))
                        .map(|cell| cell.text.as_str()),
                )
            })
            .collect();
        let natural: Vec<usize> = (0..column_count)
            .map(|column| {
                rows.iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| cell.text.width())
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let separators = (column_count - 1) * 3;
        let Some(widths) = fitted_widths(&minimum, &natural, separators, available) else {
            return rows
                .iter()
                .skip(1)
                .map(|row| {
                    headers
                        .iter()
                        .zip(row)
                        .map(|(header, cell)| {
                            let mut text = StyledText::default();
                            text.push(&header.text, Style::Emphasis);
                            text.push(": ", Style::Plain);
                            text.append(cell);
                            self.wrapped(&text, indent)
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .collect::<Vec<_>>()
                .join("\n\n");
        };
        let mut lines = Vec::new();
        for (row_number, row) in rows.iter().enumerate() {
            let cell_lines: Vec<Vec<Range<usize>>> = row
                .iter()
                .zip(&widths)
                .map(|(cell, width)| wrap::line_ranges(&cell.text, *width))
                .collect();
            let height = cell_lines.iter().map(Vec::len).max().unwrap_or(1);
            for line_number in 0..height {
                let mut line = indent.to_owned();
                for (column, width) in widths.iter().enumerate() {
                    if column > 0 {
                        line.push_str(" | ");
                    }
                    let range = cell_lines[column].get(line_number).cloned().unwrap_or(0..0);
                    let cell = &row[column];
                    let padding = width.saturating_sub(cell.text[range.clone()].width());
                    let left = match alignments.get(column) {
                        Some(Alignment::Right) => padding,
                        Some(Alignment::Center) => padding / 2,
                        _ => 0,
                    };
                    line.push_str(&" ".repeat(left));
                    let painted = cell.paint(range, self.color);
                    if row_number == 0 {
                        line.push_str(&Style::Emphasis.paint(&painted, self.color));
                    } else {
                        line.push_str(&painted);
                    }
                    if column + 1 < column_count {
                        line.push_str(&" ".repeat(padding - left));
                    }
                }
                lines.push(line);
            }
            if row_number == 0 {
                lines.push(format!(
                    "{indent}{}",
                    widths
                        .iter()
                        .map(|width| "-".repeat(*width))
                        .collect::<Vec<_>>()
                        .join("-+-")
                ));
            }
        }
        lines.join("\n")
    }
}

fn is_block(node: &Node<'_>) -> bool {
    matches!(
        node,
        Node::Heading { .. }
            | Node::Branch(
                Tag::Paragraph
                    | Tag::CodeBlock(_)
                    | Tag::List(_)
                    | Tag::Table(_)
                    | Tag::BlockQuote(_)
                    | Tag::HtmlBlock,
                _
            )
            | Node::Leaf(Event::Rule)
    )
}

/// Whether a paragraph introduces the code block that follows it.
///
/// A lead-in closes with a colon and labels the block it introduces, such as `Example - …:`; it
/// reads as a heading for the example rather than as body prose. Body paragraphs never take this
/// style because they do not end at a colon.
fn is_lead_in(nodes: &[Node<'_>], position: usize, children: &[Node<'_>]) -> bool {
    if !matches!(
        nodes.get(position + 1),
        Some(Node::Branch(Tag::CodeBlock(_), _))
    ) {
        return false;
    }
    let mut text = StyledText::default();
    inline_text(children, Style::Plain, &mut text);
    text.text.trim_end().ends_with([':', '：'])
}

fn inline_text(nodes: &[Node<'_>], style: Style, text: &mut StyledText) {
    for node in nodes {
        match node {
            Node::Branch(Tag::Emphasis | Tag::Strong, children) => {
                inline_text(children, Style::Emphasis, text);
            }
            Node::Branch(Tag::Link { dest_url, .. }, children) => {
                let start = text.text.len();
                inline_text(children, Style::Link, text);
                // A cross-reference between help pages is a link for a view, not an address the
                // reader can use: its target stays invisible.
                if &text.text[start..] != dest_url.as_ref()
                    && !dest_url.starts_with(super::model::CROSS_REFERENCE_SCHEME)
                {
                    text.push(" (", Style::Plain);
                    text.push(dest_url, Style::Link);
                    text.push(")", Style::Plain);
                }
            }
            Node::Branch(_, children) | Node::Heading { children, .. } => {
                inline_text(children, style, text)
            }
            Node::Leaf(Event::Text(source) | Event::Html(source) | Event::InlineHtml(source)) => {
                text.push(source, style);
            }
            Node::Leaf(Event::Code(source)) => {
                text.push("`", Style::Literal);
                text.push(source, Style::Literal);
                text.push("`", Style::Literal);
            }
            Node::Leaf(Event::SoftBreak) => text.push(" ", style),
            Node::Leaf(Event::HardBreak) => text.push("\n", style),
            Node::Leaf(_) => {}
        }
    }
}

/// The narrowest width one column may take: the widest word its cells hold, kept within the
/// bounds a name stays whole at.
///
/// The plain renderer and the help reader size their table columns by the same rules.
pub(crate) fn column_minimum<'a>(cells: impl Iterator<Item = &'a str>) -> usize {
    cells
        .flat_map(str::split_whitespace)
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0)
        .clamp(12, 20)
}

/// The width each column takes inside `available`, or `None` when even the narrowest columns
/// do not fit; `separators` is the room the gaps between the columns need.
///
/// The plain renderer and the help reader size their table columns by the same rules.
pub(crate) fn fitted_widths(
    minimum: &[usize],
    natural: &[usize],
    separators: usize,
    available: usize,
) -> Option<Vec<usize>> {
    if minimum.iter().sum::<usize>() + separators > available {
        return None;
    }
    let mut widths = minimum
        .iter()
        .zip(natural)
        .map(|(minimum, natural)| (*natural).max(*minimum))
        .collect::<Vec<_>>();
    while widths.iter().sum::<usize>() + separators > available {
        let column = (0..widths.len())
            .filter(|&column| widths[column] > minimum[column])
            .max_by_key(|&column| widths[column])
            .expect("the minimum widths fit");
        widths[column] -= 1;
    }
    Some(widths)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text one painted string draws, with its escape sequences removed.
    fn unpainted(text: &str) -> String {
        let mut plain = String::new();
        let mut characters = text.chars();
        while let Some(character) = characters.next() {
            if character != '\x1b' {
                plain.push(character);
                continue;
            }
            for character in characters.by_ref() {
                if character == 'm' {
                    break;
                }
            }
        }
        plain
    }

    #[test]
    fn prose_wraps_whole_graphemes() {
        let markdown =
            "界界界 👩‍💻 e\u{301}e\u{301}e\u{301} abcdefghijk\n\n- nested words wrap\n  - 界界界界\n";
        let rendered = Documentation::new(Some(9), false).render(markdown);
        for line in rendered.lines() {
            assert!(line.width() <= 9, "{line:?}");
            assert!(!line.starts_with('\u{301}'));
        }
        assert!(rendered.contains("👩‍💻"));
        assert!(rendered.contains("  - 界界"));
    }

    #[test]
    fn narrow_tables_stack_fields() {
        let markdown = "| Parameter | Description |\n| --- | --- |\n| `path` | Site asset path |\n| `size` | Image width |\n";
        let narrow = Documentation::new(Some(20), false).render(markdown);
        assert!(narrow.contains("Parameter: `path`"));
        assert!(narrow.contains("Description: Site"));
        assert!(narrow.lines().all(|line| line.width() <= 20));
        let wide = Documentation::new(Some(72), false).render(markdown);
        assert!(wide.contains("Site asset path"));
    }

    #[test]
    fn color_preserves_plain_layout() {
        let markdown = "# API `image`\n\n**Render** `path` with [Typst](https://typst.app).\n\n```typst\n#let f(x) = x + 1\n#f(2)\n```\n\n```typst-code\nf(path: str, size: int)\n```\n\n```toml\n[build]\noutput = \"public\"\nminify = true\nsize = 42\noptions = { mode = \"auto\" }\n```\n\n| Name | Type |\n| --- | --- |\n| `path` | `str` |\n\n```unknown\nx = 3\n```\n";
        for columns in [None, Some(24), Some(80)] {
            let plain = Documentation::new(columns, false).render(markdown);
            let colored = Documentation::new(columns, true).render(markdown);
            assert!(!plain.contains('\x1b'));
            assert!(colored.contains('\x1b'));
            assert_eq!(unpainted(&colored), plain);
        }
    }

    #[test]
    fn typc_fences_read_as_typst_code() {
        let typst = Documentation::new(Some(80), true).render("```typst\n#let x = 1\n```\n");
        let typc = Documentation::new(Some(80), true).render("```typc\n#let x = 1\n```\n");
        let plain = Documentation::new(Some(80), false).render("```typc\n#let x = 1\n```\n");
        assert_eq!(typc, typst);
        assert!(!plain.contains('\x1b'));
    }

    #[test]
    fn cross_reference_targets_stay_invisible() {
        // The builder links the code spans it already writes, so the label renders as the span
        // did and only the reserved target is hidden.
        for color in [false, true] {
            let linked = "See [`@tola/schema`](tola-help://packages/schema) and\n[`tola help \"[site]\"`](tola-help://config/site).\n\nExternals keep their address: [Typst](https://typst.app).\n";
            let unlinked = "See `@tola/schema` and\n`tola help \"[site]\"`.\n\nExternals keep their address: [Typst](https://typst.app).\n";
            let documentation = Documentation::new(Some(80), color);
            assert_eq!(
                documentation.render(linked),
                documentation.render(unlinked),
                "color={color}"
            );
            let rendered = documentation.render(linked);
            assert!(!rendered.contains("tola-help://"), "color={color}");
            // An external address stays visible in either mode once escapes are stripped.
            assert!(
                unpainted(&rendered).contains("(https://typst.app)"),
                "color={color}: {rendered:?}"
            );
        }
    }
}
