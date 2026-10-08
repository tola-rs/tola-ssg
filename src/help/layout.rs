//! Width-dependent lines and navigation coordinates derived from one help document.

use std::ops::Range;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::model::{Anchor, Block, HeadingRole, HelpDocument, Inline, LinkTarget, PageId};
use crate::terminal::code::CodeKind;
use crate::terminal::documentation::{column_minimum, fitted_widths};
use crate::terminal::style::Palette;
use crate::terminal::ui::pager::Pager;
use crate::terminal::wrap;

/// A leaf's ordinal and byte offset in its text remain unchanged by terminal reflow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct DocumentPosition {
    leaf: usize,
    /// `None` is the separator before the leaf, distinct from its first text byte.
    offset: Option<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct LinkSpan {
    /// All fragments of one Markdown link share this identity.
    pub(super) id: usize,
    pub(super) line: usize,
    pub(super) column: usize,
    pub(super) width: usize,
    pub(super) target: LinkTarget,
}

pub(super) struct PageLayout {
    pub(super) pager: Pager,
    pub(super) width: usize,
    pub(super) links: Vec<LinkSpan>,
    headings: Vec<HeadingLine>,
    positions: Vec<DocumentPosition>,
}

struct HeadingLine {
    anchor: Anchor,
    line: usize,
    role: HeadingRole,
}

impl PageLayout {
    pub(super) fn new(document: &HelpDocument, width: usize, palette: Palette) -> Self {
        let width = width.max(1);
        let numbered = matches!(document.id, PageId::DemoFile { .. });
        let mut writer = DocumentLines::new(palette, numbered);
        writer.blocks(&document.blocks, width, "");
        Self {
            pager: Pager::new(writer.lines),
            width,
            links: writer.links,
            headings: writer.headings,
            positions: writer.positions,
        }
    }

    pub(super) fn position(&self, line: usize) -> DocumentPosition {
        self.positions
            .get(line)
            .copied()
            .or_else(|| self.positions.last().copied())
            .unwrap_or_default()
    }

    pub(super) fn line_at(&self, position: DocumentPosition) -> usize {
        self.positions
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                candidate.leaf == position.leaf
                    && match (candidate.offset, position.offset) {
                        (None, None) => true,
                        (Some(candidate), Some(offset)) => candidate <= offset,
                        _ => false,
                    }
            })
            .map(|(line, _)| line)
            .next_back()
            .unwrap_or_else(|| {
                self.positions
                    .iter()
                    .position(|candidate| candidate.leaf >= position.leaf)
                    .unwrap_or_else(|| self.positions.len().saturating_sub(1))
            })
    }

    pub(super) fn anchor_line(&self, anchor: &Anchor) -> Option<usize> {
        self.headings
            .iter()
            .find(|heading| &heading.anchor == anchor)
            .map(|heading| heading.line)
    }

    pub(super) fn next_section(&self, top: usize) -> Option<&Anchor> {
        self.headings
            .iter()
            .find(|heading| heading.role == HeadingRole::Export && heading.line > top)
            .map(|heading| &heading.anchor)
    }

    pub(super) fn previous_section(&self, top: usize) -> Option<&Anchor> {
        self.headings
            .iter()
            .rev()
            .find(|heading| heading.role == HeadingRole::Export && heading.line < top)
            .map(|heading| &heading.anchor)
    }
}

struct DocumentLines {
    lines: Vec<Line<'static>>,
    positions: Vec<DocumentPosition>,
    links: Vec<LinkSpan>,
    headings: Vec<HeadingLine>,
    leaf: usize,
    next_link: usize,
    palette: Palette,
    numbered_code: bool,
}

impl DocumentLines {
    fn new(palette: Palette, numbered_code: bool) -> Self {
        Self {
            lines: Vec::new(),
            positions: Vec::new(),
            links: Vec::new(),
            headings: Vec::new(),
            leaf: 0,
            next_link: 0,
            palette,
            numbered_code,
        }
    }

    fn blocks(&mut self, blocks: &[Block], width: usize, indent: &str) {
        for (index, block) in blocks.iter().enumerate() {
            if index > 0 {
                self.push_line(Line::default(), None);
            }
            let leaf = self.leaf;
            match block {
                Block::Heading {
                    level,
                    spans,
                    anchor,
                    role,
                } => {
                    self.headings.push(HeadingLine {
                        anchor: anchor.clone(),
                        line: self.lines.len(),
                        role: *role,
                    });
                    self.inline(spans, width, indent, self.palette.heading_style(*level));
                    self.leaf += 1;
                }
                Block::Paragraph { spans, lead } => {
                    let style = if *lead {
                        self.palette.emphasis_style()
                    } else {
                        Style::default()
                    };
                    self.inline(spans, width, indent, style);
                    self.leaf += 1;
                }
                Block::List { start, items } => {
                    for (index, blocks) in items.iter().enumerate() {
                        let marker = start.map_or_else(
                            || "- ".to_owned(),
                            |start| format!("{}. ", start.saturating_add(index as u64)),
                        );
                        let prefix = format!("{indent}{marker}");
                        let hanging = format!("{indent}{}", " ".repeat(marker.width()));
                        self.item(blocks, width, &prefix, &hanging);
                    }
                }
                Block::Code { source, runs } => {
                    self.code(source, runs, width, indent);
                    self.leaf += 1;
                }
                Block::Table { headers, rows } => {
                    if !self.grid(headers, rows, width, indent) {
                        for row in rows {
                            let mut spans = Vec::new();
                            for (column, (header, cell)) in headers.iter().zip(row).enumerate() {
                                if column > 0 {
                                    spans.push(Inline::Text("  ".to_owned()));
                                }
                                spans.extend(header.iter().cloned());
                                spans.push(Inline::Text(": ".to_owned()));
                                spans.extend(cell.iter().cloned());
                            }
                            self.inline(&spans, width, indent, Style::default());
                            self.leaf += 1;
                        }
                    }
                }
                Block::Quote { blocks } => {
                    self.blocks(blocks, width, &format!("{indent}  "));
                }
                Block::Rule => {
                    let prefix = clipped_prefix(indent, width);
                    self.push_line(
                        Line::styled(
                            format!(
                                "{prefix}{}",
                                "-".repeat(width.saturating_sub(prefix.width()))
                            ),
                            self.palette.dim_style(),
                        ),
                        Some(0),
                    );
                    self.leaf += 1;
                }
            }
            // Empty containers still separate two document locations.
            if self.leaf == leaf {
                self.leaf += 1;
            }
        }
    }

    fn item(&mut self, blocks: &[Block], width: usize, prefix: &str, hanging: &str) {
        let base = self.lines.len();
        self.blocks(blocks, width, hanging);
        if let Some(line) = self.lines.get_mut(base) {
            let old = clipped_prefix(hanging, width);
            if let Some(span) = line.spans.first_mut()
                && let Some(suffix) = span.content.strip_prefix(old)
            {
                span.content = format!("{}{suffix}", clipped_prefix(prefix, width)).into();
            }
        }
    }

    fn inline(&mut self, spans: &[Inline], width: usize, prefix: &str, style: Style) {
        let mut text = InlineText::default();
        text.collect(spans, style, None, self.palette, &mut self.next_link);
        let prefix = clipped_prefix(prefix, width);
        let available = width.saturating_sub(prefix.width()).max(1);
        for range in wrap::line_ranges(&text.text, available) {
            self.write_runs(&text, range, prefix, width);
        }
    }

    fn code(
        &mut self,
        source: &str,
        runs: &[(Range<usize>, CodeKind)],
        width: usize,
        indent: &str,
    ) {
        let mut text = InlineText {
            text: source.to_owned(),
            runs: Vec::new(),
        };
        let mut cursor = 0;
        for (range, kind) in runs {
            if range.start > cursor {
                text.runs.push(InlineRun {
                    range: cursor..range.start,
                    style: Style::default(),
                    link: None,
                });
            }
            cursor = range.end;
            text.runs.push(InlineRun {
                range: range.clone(),
                style: code_style(*kind, self.palette),
                link: None,
            });
        }
        if cursor < source.len() {
            text.runs.push(InlineRun {
                range: cursor..source.len(),
                style: Style::default(),
                link: None,
            });
        }
        let digits = source.lines().count().max(1).to_string().len();
        let continuation = if self.numbered_code {
            format!("{indent}{}", " ".repeat(digits + 3))
        } else {
            format!("{indent}  ")
        };
        let mut offset = 0;
        for (number, line) in source.split_inclusive('\n').enumerate() {
            let first = if self.numbered_code {
                format!("{indent}{:>digits$} │ ", number + 1)
            } else {
                continuation.clone()
            };
            let available = width
                .saturating_sub(clipped_prefix(&first, width).width())
                .max(1);
            let end = offset + line.strip_suffix('\n').unwrap_or(line).len();
            for (part, range) in grapheme_ranges(&source[offset..end], available)
                .into_iter()
                .enumerate()
            {
                let prefix = if part == 0 { &first } else { &continuation };
                self.write_runs(
                    &text,
                    offset + range.start..offset + range.end,
                    clipped_prefix(prefix, width),
                    width,
                );
            }
            offset += line.len();
        }
    }

    fn write_runs(&mut self, text: &InlineText, range: Range<usize>, prefix: &str, columns: usize) {
        let mut spans = vec![Span::raw(prefix.to_owned())];
        let mut column = prefix.width();
        let mut run_index = text
            .runs
            .partition_point(|run| run.range.end <= range.start);
        let mut styled = String::new();
        let mut style = Style::default();
        for (offset, value) in text.text[range.clone()].grapheme_indices(true) {
            let start = range.start + offset;
            while text.runs[run_index].range.end <= start {
                run_index += 1;
            }
            let run = &text.runs[run_index];
            if style != run.style && !styled.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut styled), style));
            }
            style = run.style;
            styled.push_str(value);
            let width = value.width();
            if let Some((id, target)) = &run.link
                && width > 0
                && column + width <= columns
            {
                self.record_link(*id, column, width, target.clone());
            }
            column += width;
        }
        if !styled.is_empty() {
            spans.push(Span::styled(styled, style));
        }
        self.push_line(Line::from(spans), Some(range.start));
    }

    /// Draws one block table as a grid of wrapped cells; answers whether its columns fit
    /// `width`.
    ///
    /// A table whose columns cannot shrink that far keeps the labelled form its caller draws
    /// instead.
    fn grid(
        &mut self,
        headers: &[Vec<Inline>],
        rows: &[Vec<Vec<Inline>>],
        width: usize,
        indent: &str,
    ) -> bool {
        let prefix = clipped_prefix(indent, width).to_owned();
        let available = width.saturating_sub(prefix.width());
        let Some(widths) = table_widths(headers, rows, available) else {
            return false;
        };
        let cells = TableCells::collect(headers, rows, self.palette, &mut self.next_link);
        // A grid line is its own location: the separator before the block, the header's
        // lines, the rule, and every wrapped row line are told apart, so a reader's place
        // survives a reflow.
        self.leaf += 1;
        self.grid_lines(
            &cells.headers,
            &widths,
            self.palette.emphasis_style(),
            &prefix,
        );
        let rule = widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>()
            .join(COLUMN_RULE);
        self.leaf += 1;
        self.push_line(
            Line::styled(format!("{prefix}{rule}"), self.palette.dim_style()),
            None,
        );
        for row in &cells.rows {
            self.grid_lines(row, &widths, Style::default(), &prefix);
        }
        // The separator of the block that follows needs a leaf the table has not used.
        self.leaf += 1;
        true
    }

    /// Pushes one grid row: every cell wrapped to its column width, columns separated by the
    /// gap. Every line of the row is its own location.
    fn grid_lines(
        &mut self,
        cells: &[InlineText],
        widths: &[usize],
        cell_style: Style,
        prefix: &str,
    ) {
        let wrapped = cells
            .iter()
            .zip(widths)
            .map(|(cell, width)| wrap::line_ranges(&cell.text, *width))
            .collect::<Vec<_>>();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
        for line in 0..height {
            self.leaf += 1;
            let mut spans = vec![Span::raw(prefix.to_owned())];
            let mut column = prefix.width();
            for (index, ((cell, width), ranges)) in
                cells.iter().zip(widths).zip(&wrapped).enumerate()
            {
                if index > 0 {
                    spans.push(Span::styled(
                        COLUMN_GAP.to_owned(),
                        self.palette.dim_style(),
                    ));
                    column += COLUMN_GAP.width();
                }
                let start = column;
                if let Some(range) = ranges.get(line) {
                    column = self.grid_cell(&mut spans, cell, range.clone(), start, cell_style);
                }
                if index + 1 < widths.len() {
                    let padding = width.saturating_sub(column - start);
                    spans.push(Span::raw(" ".repeat(padding)));
                    column += padding;
                }
            }
            self.push_line(Line::from(spans), None);
        }
    }

    /// Appends one grid cell's styled spans for `range`, recording its links from `column`;
    /// answers the column the cell's text ends at.
    fn grid_cell(
        &mut self,
        spans: &mut Vec<Span<'static>>,
        cell: &InlineText,
        range: Range<usize>,
        column: usize,
        cell_style: Style,
    ) -> usize {
        let mut column = column;
        let mut run_index = cell
            .runs
            .partition_point(|run| run.range.end <= range.start);
        let mut styled = String::new();
        let mut style = Style::default();
        for (offset, value) in cell.text[range.clone()].grapheme_indices(true) {
            let start = range.start + offset;
            while cell.runs[run_index].range.end <= start {
                run_index += 1;
            }
            let run = &cell.runs[run_index];
            let run_style = if run.style == Style::default() {
                cell_style
            } else {
                run.style
            };
            if style != run_style && !styled.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut styled), style));
            }
            style = run_style;
            styled.push_str(value);
            let width = value.width();
            if let Some((id, target)) = &run.link
                && width > 0
            {
                self.record_link(*id, column, width, target.clone());
            }
            column += width;
        }
        if !styled.is_empty() {
            spans.push(Span::styled(styled, style));
        }
        column
    }

    /// Records one link fragment, continuing the fragment before it when the same link
    /// reaches this column on the line being built.
    fn record_link(&mut self, id: usize, column: usize, width: usize, target: LinkTarget) {
        let line = self.lines.len();
        let continues = self.links.last().is_some_and(|previous| {
            previous.id == id && previous.line == line && previous.column + previous.width == column
        });
        if continues {
            self.links.last_mut().expect("checked above").width += width;
        } else {
            self.links.push(LinkSpan {
                id,
                line,
                column,
                width,
                target,
            });
        }
    }

    fn push_line(&mut self, line: Line<'static>, offset: Option<usize>) {
        self.lines.push(line);
        self.positions.push(DocumentPosition {
            leaf: self.leaf,
            offset,
        });
    }
}

struct InlineRun {
    range: Range<usize>,
    style: Style,
    link: Option<(usize, LinkTarget)>,
}

#[derive(Default)]
struct InlineText {
    text: String,
    runs: Vec<InlineRun>,
}

impl InlineText {
    fn collect(
        &mut self,
        spans: &[Inline],
        style: Style,
        link: Option<(usize, LinkTarget)>,
        palette: Palette,
        next_link: &mut usize,
    ) {
        for span in spans {
            match span {
                Inline::Text(text) | Inline::Code(text) => {
                    let start = self.text.len();
                    self.text.push_str(text);
                    let style = if link.is_some() {
                        palette.link_style()
                    } else if matches!(span, Inline::Code(_)) {
                        palette.accent_style()
                    } else {
                        style
                    };
                    self.runs.push(InlineRun {
                        range: start..self.text.len(),
                        style,
                        link: link.clone(),
                    });
                }
                Inline::Emph(children) | Inline::Strong(children) => {
                    self.collect(
                        children,
                        palette.emphasis_style(),
                        link.clone(),
                        palette,
                        next_link,
                    );
                }
                Inline::Link { label, target } => {
                    let id = *next_link;
                    *next_link += 1;
                    self.collect(label, style, Some((id, target.clone())), palette, next_link);
                }
            }
        }
    }
}

/// What separates two grid columns.
const COLUMN_GAP: &str = " | ";

/// What joins the column rules under a grid's header.
const COLUMN_RULE: &str = "+";

/// One table's cells: the header row and the body rows, each cell keeping its own styles
/// and links.
struct TableCells {
    headers: Vec<InlineText>,
    rows: Vec<Vec<InlineText>>,
}

impl TableCells {
    fn collect(
        headers: &[Vec<Inline>],
        rows: &[Vec<Vec<Inline>>],
        palette: Palette,
        next_link: &mut usize,
    ) -> Self {
        let cell = |spans: &[Inline], next_link: &mut usize| {
            let mut text = InlineText::default();
            text.collect(spans, Style::default(), None, palette, next_link);
            text
        };
        Self {
            headers: headers.iter().map(|spans| cell(spans, next_link)).collect(),
            rows: rows
                .iter()
                .map(|row| row.iter().map(|spans| cell(spans, next_link)).collect())
                .collect(),
        }
    }
}

/// The width each of one table's columns takes inside `available`, or `None` when the table
/// keeps its labelled form instead: its rows are not as wide as its header, or its columns
/// cannot shrink to the words they hold.
fn table_widths(
    headers: &[Vec<Inline>],
    rows: &[Vec<Vec<Inline>>],
    available: usize,
) -> Option<Vec<usize>> {
    if headers.is_empty() || rows.iter().any(|row| row.len() != headers.len()) {
        return None;
    }
    let minimum = (0..headers.len())
        .map(|column| {
            let texts = column_cells(headers, rows, column)
                .map(spans_text)
                .collect::<Vec<_>>();
            column_minimum(texts.iter().map(String::as_str))
        })
        .collect::<Vec<_>>();
    let natural = (0..headers.len())
        .map(|column| {
            column_cells(headers, rows, column)
                .map(|cell| text_width(&spans_text(cell)))
                .max()
                .unwrap_or(0)
        })
        .collect::<Vec<_>>();
    let separators = (headers.len() - 1) * COLUMN_GAP.width();
    fitted_widths(&minimum, &natural, separators, available)
}

/// One table column's cells: the header's, then every row's.
fn column_cells<'a>(
    headers: &'a [Vec<Inline>],
    rows: &'a [Vec<Vec<Inline>>],
    column: usize,
) -> impl Iterator<Item = &'a [Inline]> + 'a {
    std::iter::once(headers[column].as_slice()).chain(
        rows.iter()
            .filter_map(move |row| row.get(column).map(Vec::as_slice)),
    )
}

/// One cell's text, as the grid lays it out.
fn spans_text(spans: &[Inline]) -> String {
    let mut text = String::new();
    for span in spans {
        match span {
            Inline::Text(value) | Inline::Code(value) => text.push_str(value),
            Inline::Emph(children)
            | Inline::Strong(children)
            | Inline::Link {
                label: children, ..
            } => text.push_str(&spans_text(children)),
        }
    }
    text
}

/// The columns one text occupies; a grapheme such as a combining mark or a joined emoji
/// counts as the one cluster it is.
fn text_width(text: &str) -> usize {
    text.graphemes(true).map(UnicodeWidthStr::width).sum()
}

/// The text one line shows between two display columns; a grapheme counts as the one cluster
/// it is, so a selection never splits one.
pub(super) fn slice_columns(text: &str, from: usize, to: usize) -> String {
    let mut selected = String::new();
    let mut column = 0;
    for grapheme in text.graphemes(true) {
        if column >= from && column < to {
            selected.push_str(grapheme);
        }
        column += grapheme.width();
    }
    selected
}

fn code_style(kind: CodeKind, palette: Palette) -> Style {
    match kind {
        CodeKind::Keyword => palette.keyword_style(),
        CodeKind::String => palette.string_style(),
        CodeKind::Number => palette.number_style(),
        CodeKind::Comment => palette.comment_style(),
        CodeKind::Literal => palette.literal_style(),
        CodeKind::Emphasis => palette.emphasis_style(),
        CodeKind::Link => palette.link_style(),
    }
}

fn clipped_prefix(prefix: &str, width: usize) -> &str {
    let available = width.saturating_sub(1);
    let mut used = 0;
    for (offset, grapheme) in prefix.grapheme_indices(true) {
        used += grapheme.width();
        if used > available {
            return &prefix[..offset];
        }
    }
    prefix
}

fn grapheme_ranges(text: &str, width: usize) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut used = 0;
    for (offset, grapheme) in text.grapheme_indices(true) {
        if used + grapheme.width() > width && offset > start {
            ranges.push(start..offset);
            start = offset;
            used = 0;
        }
        used += grapheme.width();
    }
    ranges.push(start..text.len());
    ranges
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::help::model::{HelpPage, PageId, anchor};

    #[test]
    fn styled_boundaries_keep_adjacent_text() {
        let mut page = page(
            "`tola help package` [`web`](tola-help://packages/web) a**b**c, `x`!\n",
            120,
        );
        let rendered = rows(&mut page)[0].clone();
        assert_eq!(rendered, "tola help package web abc, x!");
        assert_eq!(page.links.len(), 1);
        let link = &page.links[0];
        assert_eq!(link.column, rendered.find("web").unwrap());
        assert_eq!(link.width, "web".width());
        assert_eq!(
            link.target,
            LinkTarget::Page(PageId::Package { name: "web".into() })
        );
    }

    #[test]
    fn full_width_words_stay_together() {
        let mut page = page("alpha beta gamma delta", 16);
        assert_eq!(rows(&mut page), ["alpha beta gamma", "delta"]);
    }

    #[test]
    fn chinese_prose_fills_full_line() {
        let mut page = page("或按下 Tab 进行基于键盘的快速导航。", 20);
        let drawn = rows(&mut page);
        assert_eq!(drawn.len(), 2);
        assert_eq!(
            drawn[0].split_whitespace().collect::<String>(),
            "或按下Tab进行基于"
        );
        assert_eq!(
            drawn[1].split_whitespace().collect::<String>(),
            "键盘的快速导航。"
        );
    }

    #[test]
    fn wrapped_links_share_occurrence() {
        let mut page = page("[one **two** three four](#target) [one](#target)\n", 8);
        let drawn = rows(&mut page);
        assert_eq!(
            drawn.iter().map(String::as_str).collect::<Vec<_>>(),
            ["one two", "three", "four one"]
        );
        assert_eq!(
            page.links
                .iter()
                .map(|span| span.id)
                .collect::<BTreeSet<_>>()
                .len(),
            2
        );
        let first = page
            .links
            .iter()
            .filter(|span| span.id == 0)
            .collect::<Vec<_>>();
        assert_eq!(first.len(), 3);
        assert_eq!((first[0].column, first[0].width), (0, 7));
        assert_eq!((first[2].column, first[2].width), (0, 4));
    }

    #[test]
    fn clipped_graphemes_have_no_targets() {
        for (markdown, width) in [("[名](#target)\n", 1), ("- [名](#target)\n", 3)] {
            let mut page = page(markdown, width);
            assert!(page.links.is_empty());
            assert!(!rows(&mut page).join("").contains('名'));
        }
        let mut page = page("[名x](#target)\n", 1);
        assert_eq!(page.links.len(), 1);
        assert_eq!(
            (
                page.links[0].line,
                page.links[0].column,
                page.links[0].width
            ),
            (1, 0, 1)
        );
        assert_eq!(rows(&mut page), ["", "x"]);
        assert_eq!(
            PageLayout::new(&document("text"), 0, Palette::new(false)).width,
            1
        );
    }

    #[test]
    fn nested_passages_survive_reflow() {
        let document = document(
            "# Title\n\n- first alpha beta gamma delta epsilon zeta eta theta iota\n\n  > quoted alpha beta gamma delta epsilon zeta eta theta iota\n\n  ```text\n  code_alpha_beta_gamma_delta_epsilon_zeta_eta_theta_iota\n  ```\n",
        );
        let narrow = PageLayout::new(&document, 16, Palette::new(false));
        let wide = PageLayout::new(&document, 35, Palette::new(false));
        let mut leaves = BTreeSet::new();
        for position in &narrow.positions {
            leaves.insert(position.leaf);
            let line = wide.line_at(*position);
            let translated = wide.position(line);
            assert_eq!(translated.leaf, position.leaf);
            assert!(translated.offset <= position.offset);
            if let Some(next) = wide.positions.get(line + 1)
                && next.leaf == position.leaf
            {
                assert!(next.offset > position.offset);
            }
            let returned = narrow.position(narrow.line_at(translated));
            assert_eq!(returned.leaf, position.leaf);
        }
        assert_eq!(leaves.len(), 4);
        assert!(
            narrow
                .positions
                .iter()
                .filter(|position| position.leaf == 3)
                .any(|position| position.offset > Some(30))
        );
    }

    #[test]
    fn document_rows_round_trip() {
        let document = document(
            "# Title\n\nfirst alpha beta gamma delta epsilon\n\n- list alpha beta gamma\n\n  > quoted alpha beta gamma\n\n```text\n\ncode_alpha_beta_gamma_delta\n\n```\n\n| Empty |\n| --- |\n\nlast alpha beta gamma\n\n---\n",
        );
        for width in [12, 40, 120] {
            let page = PageLayout::new(&document, width, Palette::new(false));
            for line in 0..page.pager.line_count() {
                assert_eq!(page.line_at(page.position(line)), line, "{width}: {line}");
            }
        }
    }

    #[test]
    fn separators_survive_reflow() {
        let document = document(
            "# Title\n\nfirst alpha beta gamma delta epsilon\n\n> quoted alpha beta gamma delta epsilon\n\n| Empty |\n| --- |\n\nlast alpha beta gamma delta epsilon\n",
        );
        let narrow = PageLayout::new(&document, 12, Palette::new(false));
        let wide = PageLayout::new(&document, 40, Palette::new(false));
        let mut separators = 0;
        for position in narrow
            .positions
            .iter()
            .filter(|position| position.offset.is_none())
        {
            separators += 1;
            let translated = wide.line_at(*position);
            assert_eq!(wide.position(translated), *position);
            assert_eq!(
                narrow.line_at(wide.position(translated)),
                narrow.line_at(*position)
            );
        }
        // Four block separators, plus the header and the rule of the empty table.
        assert_eq!(separators, 6);
    }

    #[test]
    fn headings_preserve_nested_targets() {
        let mut page = page(
            "# [Same](#same-2)\n\n> ## [Same](#same)\n\n- ### [Same](#same-2)\n",
            30,
        );
        assert_eq!(page.links.len(), 3);
        for name in ["same", "same-2", "same-3"] {
            assert!(page.anchor_line(&anchor(name)).is_some());
        }
        let drawn = rows(&mut page);
        assert_eq!(
            drawn[page.anchor_line(&anchor("same-3")).unwrap()],
            "- Same"
        );
    }

    #[test]
    fn primary_navigation_ignores_prose() {
        let mut page = HelpPage::new(
            PageId::Overview,
            "# Package\n\n## callable - function\n\nOverview example.\n".to_owned(),
        );
        page.push_export(
            "\n## callable - function\n\n### Parameters\n\nparameter prose alpha beta gamma delta epsilon\n\n> ## callable - function\n\n## misleading - value\n\n```typ\n## fenced - function\n```\n"
                .to_owned(),
        );
        page.push_export("\n---\n\n## constant - value\n\nThe value.\n".to_owned());
        let document = HelpDocument::parse(page);
        let callable = anchor("callable-function");
        let constant = anchor("constant-value");
        for width in [12, 28, 80] {
            let page = PageLayout::new(&document, width, Palette::new(false));
            let first = page.anchor_line(&callable).unwrap();
            let second = page.anchor_line(&constant).unwrap();
            assert_eq!(page.next_section(0), Some(&callable));
            assert_eq!(page.next_section(first), Some(&constant));
            assert_eq!(page.previous_section(first), None);
            assert_eq!(page.previous_section(first + 1), Some(&callable));
            assert_eq!(page.previous_section(second), Some(&callable));
            assert_eq!(page.previous_section(second + 1), Some(&constant));
            assert_eq!(page.next_section(second), None);
            assert_eq!(page.next_section(usize::MAX), None);
            assert_eq!(page.previous_section(0), None);
        }
    }

    #[test]
    fn tables_keep_clickable_cells() {
        let page = page(
            "| Topic |\n| --- |\n| [the **whole** label](#target) |\n",
            12,
        );
        assert!(
            page.links.len() > 1,
            "every fragment of a wrapped link stays a target"
        );
        assert!(page.links.iter().all(|span| span.id == 0));
        assert_eq!(
            page.links[0].column, 0,
            "the cell's link starts where its column does"
        );
        assert!(
            page.links.iter().any(|span| span.line > page.links[0].line),
            "the fragment on the wrapped line is clickable too"
        );
    }

    #[test]
    fn list_markers_keep_starting_ordinal() {
        let mut page = page("7. alpha beta gamma delta\n8. next\n", 16);
        let drawn = rows(&mut page);
        assert!(drawn[0].starts_with("7. "));
        assert!(drawn.last().unwrap().starts_with("8. "));
    }

    #[test]
    fn code_keeps_graphemes_across_wraps() {
        for language in ["text", "typst", "toml"] {
            let source = "名称e\u{301}👨‍👩‍👧‍👦longlonglong\nnext\n";
            let mut page = page(&format!("```{language}\n{source}```\n"), 12);
            let drawn = rows(&mut page);
            assert_eq!(
                drawn
                    .iter()
                    .flat_map(|row| row.split_whitespace())
                    .collect::<String>(),
                source.split_whitespace().collect::<String>()
            );
            assert!(
                page.positions
                    .iter()
                    .any(|position| position.offset > Some("名称e\u{301}".len()))
            );
        }
    }

    #[test]
    fn code_tokens_keep_wrapped_styles() {
        let palette = Palette::new(true);
        for (language, source, expected) in [
            (
                "toml",
                "title = \"zzzzzzzzzzzzzzzzzzzz\"",
                palette.string_style(),
            ),
            (
                "typst-code",
                "let title = \"zzzzzzzzzzzzzzzzzzzz\"",
                palette.string_style(),
            ),
            (
                "typst",
                "Let *zzzzzzzzzzzzzzzzzzzz* be #emph[other].",
                palette.emphasis_style(),
            ),
        ] {
            let mut page = PageLayout::new(
                &document(&format!("```{language}\n{source}\n```\n")),
                12,
                palette,
            );
            let terminal = rendered(&mut page);
            let mut marked_rows = BTreeSet::new();
            let foreground = expected.fg.unwrap_or(ratatui::style::Color::Reset);
            for (index, cell) in terminal.backend().buffer().content().iter().enumerate() {
                if cell.symbol() == "z" {
                    marked_rows.insert(index / 12);
                    assert_eq!(cell.style().fg, Some(foreground), "{language}");
                    assert_eq!(
                        cell.style().add_modifier,
                        expected.add_modifier,
                        "{language}"
                    );
                }
            }
            assert!(marked_rows.len() > 1, "{language}");
        }
    }

    #[test]
    fn chinese_prose_keeps_every_character() {
        use crate::i18n::HelpLanguage;

        let translations =
            crate::i18n::package(HelpLanguage::SimplifiedChinese, "address").unwrap();
        let document = document(&format!(
            "{}\n\n{}\n",
            translations.overview().unwrap(),
            translations.parameter("output-to-url", "output").unwrap(),
        ));
        let mut whole = PageLayout::new(&document, 4096, Palette::new(false));
        let source = rows(&mut whole)
            .join("")
            .split_whitespace()
            .collect::<String>();
        for width in [12, 24, 40, 64] {
            let mut page = PageLayout::new(&document, width, Palette::new(false));
            let drawn = rows(&mut page);
            assert_eq!(
                drawn.join("").split_whitespace().collect::<String>(),
                source,
                "{width}"
            );
        }
    }

    fn document(markdown: &str) -> HelpDocument {
        HelpDocument::parse(HelpPage::new(PageId::Overview, markdown.to_owned()))
    }

    fn page(markdown: &str, width: usize) -> PageLayout {
        PageLayout::new(&document(markdown), width, Palette::new(false))
    }

    fn rows(page: &mut PageLayout) -> Vec<String> {
        let terminal = rendered(page);
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(page.width)
            .map(|cells| {
                cells
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    fn rendered(page: &mut PageLayout) -> Terminal<TestBackend> {
        let width = page.width as u16;
        let height = page.pager.line_count().max(1) as u16;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| page.pager.draw(frame, frame.area(), Palette::new(false)))
            .unwrap();
        terminal
    }

    #[test]
    fn file_numbers_preserve_source_positions() {
        let source =
            "first_long_source_line_with_a_value\nsecond_名称_source_line_with_another_value\n";
        let document = HelpDocument::parse(HelpPage::new(
            PageId::DemoFile {
                id: "backlinks".into(),
                path: "site/page.typ".into(),
            },
            format!("```text\n{source}```\n"),
        ));
        let mut narrow = PageLayout::new(&document, 16, Palette::new(false));
        let wide = PageLayout::new(&document, 80, Palette::new(false));
        let numbers = rows(&mut narrow)
            .iter()
            .filter_map(|row| row.split_once('│')?.0.trim().parse::<usize>().ok())
            .collect::<Vec<_>>();
        assert_eq!(numbers, [1, 2]);
        for position in &narrow.positions {
            let row = wide.line_at(*position);
            let translated = wide.position(row);
            assert_eq!(translated.leaf, position.leaf);
            assert!(translated.offset <= position.offset);
            if let Some(next) = wide.positions.get(row + 1) {
                assert!(next.offset > position.offset);
            }
        }
        assert!(
            matches!(&document.blocks[0], Block::Code { source: original, .. } if original == source)
        );
    }

    #[test]
    fn tables_align_columns_by_display_width() {
        let mut page = page(
            "| 命令 | 状态 |\n| --- | --- |\n| `tola build` | 运行 |\n",
            40,
        );
        let drawn = rows(&mut page);
        assert_eq!(drawn.len(), 3, "a header, its rule, and one row");
        let terminal = rendered(&mut page);
        let buffer = terminal.backend().buffer();
        let column_of = |row: u16, symbol: &str| {
            (0..page.width as u16)
                .find(|column| buffer[(*column, row)].symbol() == symbol)
                .unwrap_or_else(|| panic!("row {row} holds no {symbol:?}"))
        };
        assert_eq!(
            column_of(0, "|"),
            column_of(2, "|"),
            "a double-width header keeps the rows' columns together: {drawn:?}"
        );
        assert_eq!(
            buffer[(column_of(0, "|") - 1, 1)].symbol(),
            "+",
            "the rule spans the first column: {drawn:?}"
        );
    }

    #[test]
    fn wrapped_cell_keeps_the_grid_aligned() {
        let mut page = page(
            "| 命令 | 状态 |\n| --- | --- |\n| `tola preview build` | 运行 |\n",
            30,
        );
        let drawn = rows(&mut page);
        assert_eq!(
            drawn.len(),
            4,
            "a header, the rule, and the row's two wrapped lines: {drawn:?}"
        );
        let terminal = rendered(&mut page);
        let buffer = terminal.backend().buffer();
        let column_of = |row: u16, symbol: &str| {
            (0..page.width as u16)
                .find(|column| buffer[(*column, row)].symbol() == symbol)
                .unwrap_or_else(|| panic!("row {row} holds no {symbol:?}"))
        };
        assert!(drawn[2].starts_with("tola preview"), "{drawn:?}");
        assert!(drawn[3].starts_with("build"), "{drawn:?}");
        assert_eq!(
            column_of(0, "|"),
            column_of(2, "|"),
            "the wrapped lines stay in their columns: {drawn:?}"
        );
    }

    #[test]
    fn table_too_wide_for_the_reader_keeps_labelled_fields() {
        let mut page = page(
            "| 命令 | 状态 |\n| --- | --- |\n| `tola build` | 运行 |\n",
            8,
        );
        let drawn = rows(&mut page);
        assert!(
            drawn.iter().all(|row| !row.contains('+')),
            "no grid fits eight columns: {drawn:?}"
        );
        assert!(
            drawn[0].contains(':'),
            "the labelled form names each header beside its cell: {drawn:?}"
        );
    }

    #[test]
    fn cell_wraps_before_the_grid_gives_way() {
        let mut page = page(
            "| Field | Note |\n| --- | --- |\n| alpha beta gamma delta epsilon | 运行 |\n",
            30,
        );
        let drawn = rows(&mut page);
        assert!(
            drawn.iter().any(|row| row.contains('+')),
            "columns shrink to their minimum before the grid gives way: {drawn:?}"
        );
        assert!(drawn[0].starts_with("Field"), "{drawn:?}");
        assert!(
            drawn.iter().any(|row| row.starts_with("alpha beta")),
            "the long cell wraps into its column: {drawn:?}"
        );
    }
}
