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
use crate::terminal::text;
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
            state: TableState::default().with_selected(Some(0)),
            height: 0,
            detail: None,
        }
    }

    /// The selected row's index.
    pub(crate) fn selected(&self) -> usize {
        self.state.selected().unwrap_or(0)
    }

    /// Reformatting cells preserves the selection and the open detail's scroll position.
    pub(crate) fn set_rows(&mut self, rows: Vec<Vec<String>>) {
        self.rows = rows;
        self.select(self.selected());
    }

    /// Whether a row detail is open.
    pub(crate) fn detail_is_open(&self) -> bool {
        self.detail.is_some()
    }

    /// Selects the row at `row`, stopping at either end.
    pub(crate) fn select(&mut self, row: usize) {
        self.state
            .select(Some(row.min(self.rows.len().saturating_sub(1))));
    }

    /// Moves the selection one row, stopping at either end.
    pub(crate) fn select_by(&mut self, rows: isize) {
        self.select(self.selected().saturating_add_signed(rows));
    }

    /// Whether moving the selection `rows` changes it.
    pub(crate) fn can_select_by(&self, rows: isize) -> bool {
        stepped_index(self.selected(), rows, self.rows.len()) != self.selected()
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
        self.select(0);
    }

    /// Whether selecting the first row changes the selection.
    pub(crate) fn can_select_first(&self) -> bool {
        self.selected() != 0
    }

    /// Selects the last row.
    pub(crate) fn select_last(&mut self) {
        self.select(usize::MAX);
    }

    /// Whether selecting the last row changes the selection.
    pub(crate) fn can_select_last(&self) -> bool {
        self.selected() != self.rows.len().saturating_sub(1)
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
            self.height = 0;
            if let Some(detail) = &mut self.detail {
                detail.height = 0;
            }
            return;
        }
        let header_is_visible = area.height > 1;
        self.height = usize::from(area.height - u16::from(header_is_visible));
        let widths = self.column_widths(usize::from(area.width));
        let constraints = widths
            .iter()
            .map(|width| Constraint::Length(*width as u16))
            .collect::<Vec<_>>();
        let header = GridRow::new(
            self.headers
                .iter()
                .take(widths.len())
                .enumerate()
                .map(|(column, cell)| Cell::from(self.cell(cell, column, widths[column])))
                .collect::<Vec<_>>(),
        )
        .style(palette.accent_style());
        let rows = self.rows.iter().map(|row| {
            GridRow::new(
                row.iter()
                    .take(widths.len())
                    .enumerate()
                    .map(|(column, cell)| Cell::from(self.cell(cell, column, widths[column])))
                    .collect::<Vec<_>>(),
            )
        });
        let table = Grid::new(rows, constraints)
            .column_spacing(1)
            .row_highlight_style(palette.selected_style());
        let table = if header_is_visible {
            table.header(header)
        } else {
            table
        };
        frame.render_stateful_widget(table, area, &mut self.state);
        self.draw_detail(frame, area, palette);
    }

    /// One cell's text: the separator before the next column rides in this cell.
    fn cell(&self, source: &str, column: usize, width: usize) -> String {
        if column + 1 == self.headers.len() || width < 3 {
            text::window(source, width, 0).text.to_owned()
        } else {
            let text = text::window(source, width - 2, 0).text;
            format!("{text} |")
        }
    }

    fn column_widths(&self, columns: usize) -> Vec<usize> {
        let mut widths = self
            .headers
            .iter()
            .take(columns.div_ceil(2))
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
        let desired = widths
            .iter()
            .enumerate()
            .map(|(column, width)| if column == last { *width } else { width + 2 })
            .collect::<Vec<_>>();
        let available = columns.saturating_sub(desired.len().saturating_sub(1));
        let share = available / desired.len().max(1);
        let mut widths = desired
            .iter()
            .map(|width| (*width).min(share))
            .collect::<Vec<_>>();
        let mut remaining = available - widths.iter().sum::<usize>();
        for (width, desired) in widths.iter_mut().zip(desired) {
            let extra = (desired - *width).min(remaining);
            *width += extra;
            remaining -= extra;
        }
        widths
    }

    fn draw_detail(&mut self, frame: &mut Frame, area: Rect, palette: Palette) {
        let Some(detail) = &mut self.detail else {
            return;
        };
        let overlay = overlay_area(area);
        frame.render_widget(Clear, overlay);
        let block = if overlay.width >= 3 && overlay.height >= 3 {
            Block::bordered().title(detail.title.clone())
        } else {
            Block::default()
        }
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

fn overlay_area(area: Rect) -> Rect {
    let horizontal_margin = u16::from(area.width > 4);
    let vertical_margin = u16::from(area.height > 4);
    Rect::new(
        area.x + horizontal_margin,
        area.y + vertical_margin,
        area.width - horizontal_margin * 2,
        area.height - vertical_margin * 2,
    )
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
        for (steps, selected) in [(-3, 0), (100, 9), (-100, 0)] {
            table.select_by(steps);
            assert_eq!(table.selected(), selected);
            drawn(&mut table, Rect::new(0, 0, 24, 4));
            assert_eq!(table.selected(), selected);
        }
    }

    #[test]
    fn replacement_clamps_selection_before_draw() {
        let mut table = table();
        table.select_last();
        drawn(&mut table, Rect::new(0, 0, 24, 4));
        table.set_rows((0..3).map(|row| vec![format!("row {row}")]).collect());
        assert_eq!(table.selected(), 2);
        assert!(!table.can_select_last());
        drawn(&mut table, Rect::new(0, 0, 24, 4));
        assert_eq!(table.selected(), 2);

        table.set_rows(Vec::new());
        table.select_last();
        table.select_by(1);
        assert_eq!(table.selected(), 0);
        assert!(!table.can_select_by(1));
        assert!(!table.can_select_first());
        drawn(&mut table, Rect::new(0, 0, 24, 4));
        assert_eq!(table.selected(), 0);
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
    fn narrow_table_keeps_later_columns() {
        let mut table = Table::new(
            vec!["properties.description".to_owned(), "name".to_owned()],
            vec![vec!["a long description".to_owned(), "tail".to_owned()]],
        );
        let rows = drawn(&mut table, Rect::new(0, 0, 12, 3));
        assert!(rows.iter().any(|row| row.contains("tail")), "{rows:?}");
        let rows = drawn(&mut table, Rect::new(0, 0, 60, 3));
        assert!(
            rows.iter().any(|row| row.contains("a long description")),
            "{rows:?}"
        );
    }

    #[test]
    fn many_columns_keep_row_visible() {
        let mut table = Table::new(
            (0..12).map(|column| format!("column {column}")).collect(),
            vec![(0..12).map(|column| format!("value {column}")).collect()],
        );
        let rows = drawn(&mut table, Rect::new(0, 0, 3, 1));
        assert!(rows[0].contains('v'), "{rows:?}");
    }

    #[test]
    fn small_detail_stays_inside_viewport() {
        let mut table = table();
        table.open_detail("row".to_owned(), vec!["detail".to_owned()]);
        for width in 1..=4 {
            for height in 1..=4 {
                let area = Rect::new(0, 0, width, height);
                let overlay = overlay_area(area);
                assert!(area.contains((overlay.x, overlay.y).into()));
                assert!(overlay.right() <= area.right());
                assert!(overlay.bottom() <= area.bottom());
                let rows = drawn(&mut table, area);
                assert!(rows.iter().any(|row| row.contains('d')), "{rows:?}");
            }
        }
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
    fn movement_hints_follow_viewport_and_selection() {
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
