//! A generic table with the row detail shown over it.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{
    Block, Cell, Clear, Paragraph, Row as GridRow, Table as Grid, TableState, Wrap,
};
use unicode_width::UnicodeWidthStr;

use super::stepped_index;
use crate::terminal::style::Palette;
/// The detail of one row, drawn over the table.
struct Detail {
    title: String,
    lines: Vec<String>,
    /// The first visible detail line.
    offset: usize,
    height: usize,
    line_count: usize,
}

/// The table surface: headers, rendered rows, and the row the reader has selected.
pub(crate) struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    selected: usize,
    /// The selection and scroll position, as the table keeps them.
    state: TableState,
    /// The rows the last frame had room for.
    height: usize,
    detail: Option<Detail>,
}

impl Table {
    /// The table over `headers` and `rows`, with the first row selected.
    pub(crate) fn new(headers: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        Self {
            headers,
            rows,
            selected: 0,
            state: TableState::default(),
            height: 0,
            detail: None,
        }
    }

    /// The selected row's index.
    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    /// Reformatting cells preserves the selection and the open detail's scroll position.
    pub(crate) fn set_rows(&mut self, rows: Vec<Vec<String>>) {
        self.rows = rows;
        self.select(self.selected);
    }

    /// Whether a row detail is open.
    pub(crate) fn detail_is_open(&self) -> bool {
        self.detail.is_some()
    }

    /// Selects the row at `row`, stopping at either end.
    pub(crate) fn select(&mut self, row: usize) {
        self.selected = row.min(self.rows.len().saturating_sub(1));
    }

    /// Moves the selection one row, stopping at either end.
    pub(crate) fn select_by(&mut self, rows: isize) {
        self.selected = stepped_index(self.selected, rows, self.rows.len());
    }

    /// Whether moving the selection `rows` changes it.
    pub(crate) fn can_select_by(&self, rows: isize) -> bool {
        stepped_index(self.selected, rows, self.rows.len()) != self.selected
    }

    /// Moves the selection one page, by the rows the last frame showed.
    pub(crate) fn page_by(&mut self, pages: isize) {
        let rows = self.page_rows(pages);
        self.select_by(rows);
    }

    /// Whether paging `pages` changes the selection; a table that fits shows no paging key.
    pub(crate) fn can_page_by(&self, pages: isize) -> bool {
        self.overflows_viewport() && self.can_select_by(self.page_rows(pages))
    }

    /// The rows one page of `pages` covers: the rows the last frame showed.
    fn page_rows(&self, pages: isize) -> isize {
        (self.height.max(1) as isize) * pages
    }

    /// Whether the rows are taller than the last drawn viewport.
    fn overflows_viewport(&self) -> bool {
        self.rows.len() > self.height.max(1)
    }

    /// Selects the first row.
    pub(crate) fn select_first(&mut self) {
        self.selected = 0;
    }

    /// Whether selecting the first row changes the selection.
    pub(crate) fn can_select_first(&self) -> bool {
        self.selected != 0
    }

    /// Selects the last row.
    pub(crate) fn select_last(&mut self) {
        self.selected = self.rows.len().saturating_sub(1);
    }

    /// Whether selecting the last row changes the selection.
    pub(crate) fn can_select_last(&self) -> bool {
        self.selected != self.rows.len().saturating_sub(1)
    }

    /// Opens `lines` under `title` over the table, at their first line.
    pub(crate) fn open_detail(&mut self, title: String, lines: Vec<String>) {
        self.detail = Some(Detail {
            title,
            lines,
            offset: 0,
            height: 0,
            line_count: 0,
        });
    }

    /// Closes the open detail; whether one was open.
    pub(crate) fn close_detail(&mut self) -> bool {
        self.detail.take().is_some()
    }

    /// Scrolls the open detail by `lines`, stopping at either end.
    pub(crate) fn scroll_detail(&mut self, lines: isize) {
        if let Some(detail) = &mut self.detail {
            detail.offset = scrolled_offset(detail.offset, lines, detail.line_count, detail.height);
        }
    }

    /// Whether scrolling the open detail by `lines` moves it.
    pub(crate) fn can_scroll_detail(&self, lines: isize) -> bool {
        let Some(detail) = &self.detail else {
            return false;
        };
        scrolled_offset(detail.offset, lines, detail.line_count, detail.height) != detail.offset
    }

    pub(crate) fn page_detail_by(&mut self, pages: isize) {
        let lines = self.detail_page_rows(pages);
        self.scroll_detail(lines);
    }

    /// Whether paging the open detail by `pages` moves it.
    pub(crate) fn can_page_detail_by(&self, pages: isize) -> bool {
        self.detail.is_some() && self.can_scroll_detail(self.detail_page_rows(pages))
    }

    /// The detail lines one page of `pages` covers: the rows the detail shows.
    fn detail_page_rows(&self, pages: isize) -> isize {
        let rows = self
            .detail
            .as_ref()
            .map_or(0, |detail| detail.height.max(1));
        rows as isize * pages
    }

    /// Draws the table, and the open row detail over it.
    pub(crate) fn draw(&mut self, frame: &mut Frame, area: Rect, palette: Palette) {
        if area.height == 0 || area.width == 0 {
            if let Some(detail) = &mut self.detail {
                detail.height = 0;
            }
            return;
        }
        self.height = usize::from(area.height.saturating_sub(1));
        let widths = self
            .column_widths()
            .into_iter()
            .enumerate()
            .map(|(column, width)| {
                if column + 1 == self.headers.len() {
                    Constraint::Fill(1)
                } else {
                    Constraint::Length(width as u16)
                }
            })
            .collect::<Vec<_>>();
        let header = GridRow::new(
            self.headers
                .iter()
                .enumerate()
                .map(|(column, cell)| Cell::from(self.cell(cell, column)))
                .collect::<Vec<_>>(),
        )
        .style(palette.accent_style());
        let rows = self.rows.iter().map(|row| {
            GridRow::new(
                row.iter()
                    .enumerate()
                    .map(|(column, cell)| Cell::from(self.cell(cell, column)))
                    .collect::<Vec<_>>(),
            )
        });
        let table = Grid::new(rows, widths)
            .header(header)
            .column_spacing(1)
            .row_highlight_style(palette.selected_style());
        self.state.select(Some(self.selected));
        frame.render_stateful_widget(table, area, &mut self.state);
        self.draw_detail(frame, area, palette);
    }

    /// One cell's text: the separator before the next column rides in this cell.
    fn cell(&self, text: &str, column: usize) -> String {
        if column + 1 == self.headers.len() {
            text.to_owned()
        } else {
            format!("{text} |")
        }
    }

    /// The width each column needs: its longest text, and the separator for every column but the
    /// last.
    fn column_widths(&self) -> Vec<usize> {
        let mut widths = self
            .headers
            .iter()
            .map(|text| text.width())
            .collect::<Vec<_>>();
        for row in &self.rows {
            for (column, cell) in row.iter().enumerate() {
                if column < widths.len() {
                    widths[column] = widths[column].max(cell.width());
                }
            }
        }
        let last = widths.len().saturating_sub(1);
        widths
            .iter()
            .enumerate()
            .map(|(column, width)| if column == last { *width } else { width + 2 })
            .collect()
    }

    fn draw_detail(&mut self, frame: &mut Frame, area: Rect, palette: Palette) {
        let Some(detail) = &mut self.detail else {
            return;
        };
        let overlay = overlay_area(area);
        frame.render_widget(Clear, overlay);
        let block = Block::bordered()
            .title(detail.title.clone())
            .title_style(palette.accent_style())
            .border_style(palette.dim_style());
        let inner = block.inner(overlay);
        detail.height = usize::from(inner.height);
        let lines = detail.lines.iter().map(|line| Line::from(line.as_str()));
        let paragraph =
            Paragraph::new(Text::from(lines.collect::<Vec<_>>())).wrap(Wrap { trim: false });
        detail.line_count = paragraph.line_count(inner.width);
        detail.offset = detail
            .offset
            .min(detail.line_count.saturating_sub(detail.height.max(1)));
        frame.render_widget(
            paragraph
                .scroll((u16::try_from(detail.offset).unwrap_or(u16::MAX), 0))
                .block(block),
            overlay,
        );
    }
}

/// The offset `lines` away from `offset`, inside a detail `line_count` lines tall and `height`
/// rows of it visible.
fn scrolled_offset(offset: usize, lines: isize, line_count: usize, height: usize) -> usize {
    offset
        .saturating_add_signed(lines)
        .min(line_count.saturating_sub(height.max(1)))
}

/// The area an overlay covers: the middle of `area`, with a margin of one row and column.
fn overlay_area(area: Rect) -> Rect {
    let width = area.width.saturating_sub(2).max(1);
    let height = area.height.saturating_sub(2).max(1);
    Rect::new(area.x + 1, area.y + 1, width, height)
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    /// A table of ten rows.
    fn table() -> Table {
        Table::new(
            vec!["name".to_owned()],
            (0..10)
                .map(|row| vec![format!("row {row}")])
                .collect::<Vec<_>>(),
        )
    }

    /// The text one frame drew, line by line.
    fn drawn(table: &mut Table, area: Rect) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        terminal
            .draw(|frame| table.draw(frame, area, Palette::new(false)))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .chunks(usize::from(area.width))
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn selection_stops_at_both_ends() {
        let mut table = table();
        table.select_by(-3);
        assert_eq!(table.selected(), 0);
        table.select_by(100);
        assert_eq!(table.selected(), 9);
    }

    #[test]
    fn paging_moves_by_the_drawn_rows() {
        let mut table = table();
        drawn(&mut table, Rect::new(0, 0, 24, 4));
        table.page_by(1);
        assert_eq!(table.selected(), 3);
        table.page_by(-1);
        assert_eq!(table.selected(), 0);
    }

    #[test]
    fn long_table_follows_its_selection() {
        let mut table = table();
        table.select_last();
        let lines = drawn(&mut table, Rect::new(0, 0, 24, 4));
        assert_eq!(lines[0], "name");
        assert_eq!(lines[3], "row 9");
    }

    #[test]
    fn selected_row_is_marked() {
        let mut table = table();
        table.select_by(1);
        let mut terminal = Terminal::new(TestBackend::new(24, 4)).unwrap();
        terminal
            .draw(|frame| table.draw(frame, frame.area(), Palette::new(true)))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let row = |index: u16| {
            buffer
                .content()
                .chunks(24)
                .nth(usize::from(index))
                .expect("the row is drawn")[0]
                .modifier
        };
        assert!(row(2).contains(ratatui::style::Modifier::REVERSED));
        assert!(!row(1).contains(ratatui::style::Modifier::REVERSED));
    }

    #[test]
    fn detail_scrolling_stops_at_its_ends() {
        let mut table = table();
        let lines = (0..30).map(|line| format!("line {line}")).collect();
        table.open_detail("row 0".to_owned(), lines);
        drawn(&mut table, Rect::new(0, 0, 24, 10));
        assert!(!table.can_scroll_detail(-1));
        assert!(table.can_scroll_detail(1));
        table.scroll_detail(100);
        let drawn = drawn(&mut table, Rect::new(0, 0, 24, 10));
        assert!(
            drawn.iter().any(|line| line.contains("line 29")),
            "{drawn:?}"
        );
        assert!(!table.can_scroll_detail(1));
        assert!(table.can_scroll_detail(-1));
    }

    #[test]
    fn unicode_columns_leave_text_visible() {
        let mut table = Table::new(
            vec!["name".to_owned(), "value".to_owned()],
            vec![vec!["界界".to_owned(), "tail".to_owned()]],
        );
        let rows = drawn(&mut table, Rect::new(0, 0, 12, 3));
        assert!(rows.iter().any(|row| row.contains("tail")), "{rows:?}");
    }

    #[test]
    fn wrapped_detail_reaches_last_word() {
        let mut table = table();
        table.open_detail(
            "row".to_owned(),
            vec![format!("{}ending", "word ".repeat(20))],
        );
        let first = drawn(&mut table, Rect::new(0, 0, 20, 8));
        assert!(!first.iter().any(|row| row.contains("ending")));
        table.page_detail_by(100);
        let last = drawn(&mut table, Rect::new(0, 0, 20, 8));
        assert!(last.iter().any(|row| row.contains("ending")), "{last:?}");
    }

    #[test]
    fn movement_hints_follow_the_viewport_and_the_selection() {
        let mut table = table();
        drawn(&mut table, Rect::new(0, 0, 24, 20));
        assert!(!table.overflows_viewport());
        assert!(!table.can_page_by(1));
        assert!(table.can_select_by(1));
        drawn(&mut table, Rect::new(0, 0, 24, 4));
        assert!(table.can_page_by(1));
        table.select_last();
        assert!(!table.can_select_last());
        assert!(!table.can_select_by(1));
        assert!(table.can_select_first());
    }

    #[test]
    fn detail_closes_on_demand() {
        let mut table = table();
        table.open_detail("row 0".to_owned(), vec!["line".to_owned()]);
        assert!(table.close_detail());
        assert!(!table.detail_is_open());
    }
}
