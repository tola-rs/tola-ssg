//! The scrollable view of one page's lines.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::terminal::style::Palette;

/// The open search over the page's lines.
struct Search {
    hits: Vec<usize>,
    current: usize,
}

/// The scrollable view of a page's lines.
pub(crate) struct Pager {
    lines: Vec<Line<'static>>,
    /// The first line the viewport shows.
    top: usize,
    /// Reconciled before a screen derives targets from the visible lines.
    height: usize,
    search: Option<Search>,
}

impl Pager {
    pub(crate) fn new(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            top: 0,
            height: 0,
            search: None,
        }
    }

    /// The first line the viewport shows.
    pub(crate) fn top(&self) -> usize {
        self.top
    }

    /// The lines the viewport shows, as indices into the page.
    pub(crate) fn visible(&self) -> std::ops::Range<usize> {
        self.top..(self.top + self.height).min(self.lines.len())
    }

    /// How many lines the page lays out to.
    pub(crate) fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The plain text of one page line, as the reader shows it.
    pub(crate) fn text(&self, line: usize) -> Option<String> {
        self.lines.get(line).map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
    }

    /// Reconciles the viewport before the screen derives interaction targets from it.
    pub(crate) fn resize(&mut self, height: usize) {
        self.height = height;
        self.top = self.top.min(self.top_max());
    }

    /// Scrolls the viewport so that `line` shows, keeping the top where it can.
    pub(crate) fn move_to(&mut self, line: usize) {
        let line = line.min(self.top_max());
        if line < self.top {
            self.top = line;
        } else {
            self.top = self
                .top
                .max(line.saturating_sub(self.height.saturating_sub(1)));
        }
    }

    /// Keeps `line` at the top even when the remaining page cannot fill the viewport.
    pub(crate) fn set_top(&mut self, line: usize) {
        self.top = line.min(self.top_max());
    }

    /// Scrolls the viewport by `lines`, stopping at either end of the page.
    pub(crate) fn move_by(&mut self, lines: isize) {
        self.top = self.scrolled_top(lines);
    }

    /// Whether scrolling by `lines` moves the viewport.
    pub(crate) fn can_move_by(&self, lines: isize) -> bool {
        self.scrolled_top(lines) != self.top
    }

    /// Scrolls the viewport by `pages` of the lines the last frame showed.
    pub(crate) fn page_by(&mut self, pages: isize) {
        self.move_by(self.page_rows(pages));
    }

    /// Whether paging `pages` moves the viewport.
    pub(crate) fn can_page_by(&self, pages: isize) -> bool {
        self.can_move_by(self.page_rows(pages))
    }

    /// The lines one page of `pages` covers: the lines the last frame showed.
    fn page_rows(&self, pages: isize) -> isize {
        (self.height.max(1) as isize) * pages
    }

    fn scrolled_top(&self, lines: isize) -> usize {
        // Anchors may lie below the last screenful; scrolling must not reverse or skip rows.
        let top_max = self.last_top().max(self.top);
        self.top.saturating_add_signed(lines).min(top_max)
    }

    /// Scrolls the viewport by half a page: the lines the last frame showed, halved.
    pub(crate) fn half_page_by(&mut self, pages: isize) {
        let lines = (self.height.max(1) / 2).max(1) as isize;
        self.move_by(lines * pages);
    }

    /// Whether half a page of scrolling moves the viewport.
    pub(crate) fn can_half_page_by(&self, pages: isize) -> bool {
        let lines = (self.height.max(1) / 2).max(1) as isize;
        self.can_move_by(lines * pages)
    }

    /// Scrolls the viewport to the first line.
    pub(crate) fn first(&mut self) {
        self.top = 0;
    }

    /// Whether scrolling to the first line moves the viewport.
    pub(crate) fn can_first(&self) -> bool {
        self.top != 0
    }

    /// Scrolls the viewport to the last screenful.
    pub(crate) fn last(&mut self) {
        self.top = self.last_top();
    }

    /// Whether scrolling to the last screenful moves the viewport.
    pub(crate) fn can_last(&self) -> bool {
        self.top != self.last_top()
    }

    /// The top of the last screenful.
    fn last_top(&self) -> usize {
        self.lines.len().saturating_sub(self.height.max(1))
    }

    /// Searches the page for `query`, case-insensitively; an empty query closes the search.
    pub(crate) fn search(&mut self, query: &str) {
        if query.is_empty() {
            self.search = None;
            return;
        }
        let query = query.to_lowercase();
        let hits = self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line_text(line).to_lowercase().contains(&query))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        self.search = Some(Search { hits, current: 0 });
        self.move_to_hit();
    }

    /// Whether the open search has hits to move between.
    pub(crate) fn has_hits(&self) -> bool {
        self.search
            .as_ref()
            .is_some_and(|search| !search.hits.is_empty())
    }

    pub(crate) fn current_hit(&self) -> Option<usize> {
        let search = self.search.as_ref()?;
        search.hits.get(search.current).copied()
    }

    /// The one-based match ordinal and total matching lines; no matches has ordinal zero.
    pub(crate) fn search_position(&self) -> Option<(usize, usize)> {
        let search = self.search.as_ref()?;
        let total = search.hits.len();
        Some((if total == 0 { 0 } else { search.current + 1 }, total))
    }

    /// Focuses the nearest matching line without changing the restored viewport.
    pub(crate) fn select_hit(&mut self, line: usize) {
        if let Some(search) = &mut self.search
            && let Some(current) = search
                .hits
                .iter()
                .enumerate()
                .min_by_key(|(_, hit)| hit.abs_diff(line))
                .map(|(current, _)| current)
        {
            search.current = current;
        }
    }

    /// Moves the viewport to the next hit, wrapping around the page.
    pub(crate) fn next_hit(&mut self) {
        self.step_hit(1);
    }

    /// Moves the viewport to the previous hit, wrapping around the page.
    pub(crate) fn previous_hit(&mut self) {
        self.step_hit(-1);
    }

    /// Draws the page scrolled to its first visible line.
    pub(crate) fn draw(&mut self, frame: &mut Frame, area: Rect, palette: Palette) {
        self.resize(usize::from(area.height));
        if area.height == 0 || area.width == 0 {
            return;
        }
        let lines = self
            .visible()
            .map(|index| {
                let line = &self.lines[index];
                Line {
                    spans: line
                        .spans
                        .iter()
                        .map(|span| Span::styled(span.content.as_ref(), span.style))
                        .collect(),
                    style: self.hit_style(index, palette).unwrap_or(line.style),
                    alignment: line.alignment,
                }
            })
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(Text::from(lines)), area);
    }

    /// The style one line's search hit draws with, when it holds one.
    fn hit_style(&self, index: usize, palette: Palette) -> Option<ratatui::style::Style> {
        let search = self.search.as_ref()?;
        let position = search.hits.binary_search(&index).ok()?;
        Some(if position == search.current {
            palette.hit_style()
        } else {
            palette.notice_style()
        })
    }

    fn step_hit(&mut self, step: isize) {
        if let Some(search) = &mut self.search {
            if search.hits.is_empty() {
                return;
            }
            let last = search.hits.len() - 1;
            search.current = if step < 0 {
                search.current.checked_sub(1).unwrap_or(last)
            } else if search.current >= last {
                0
            } else {
                search.current + 1
            };
        }
        self.move_to_hit();
    }

    fn move_to_hit(&mut self) {
        if let Some(search) = &self.search
            && let Some(hit) = search.hits.get(search.current)
        {
            self.move_to(*hit);
        }
    }

    /// Every content line can begin a viewport; blank rows may follow the page.
    fn top_max(&self) -> usize {
        self.lines.len().saturating_sub(1)
    }
}

/// The text of one line, as a search reads it.
fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    /// A page of twelve numbered lines, searching for "needle" on lines 2 and 8.
    fn page() -> Pager {
        let lines = (0..12)
            .map(|index| {
                let mut text = format!("line {index}");
                if index == 2 || index == 8 {
                    text.push_str(" needle");
                }
                Line::from(text)
            })
            .collect();
        Pager::new(lines)
    }

    /// The text one frame drew, line by line.
    fn drawn(pager: &mut Pager, area: Rect) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        terminal
            .draw(|frame| pager.draw(frame, area, Palette::new(false)))
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
    fn zero_height_hides_every_line() {
        let mut pager = page();
        let mut terminal = Terminal::new(TestBackend::new(20, 4)).unwrap();
        terminal
            .draw(|frame| pager.draw(frame, frame.area(), Palette::new(false)))
            .unwrap();
        terminal
            .draw(|frame| pager.draw(frame, Rect::new(0, 0, 20, 0), Palette::new(false)))
            .unwrap();
        assert!(pager.visible().is_empty());
    }

    #[test]
    fn scrolling_moves_the_viewport() {
        let mut pager = page();
        pager.move_by(9);

        assert_eq!(
            drawn(&mut pager, Rect::new(0, 0, 20, 3)),
            ["line 9", "line 10", "line 11"]
        );
    }

    #[test]
    fn moving_stops_at_both_ends() {
        let mut pager = page();
        pager.resize(4);
        pager.move_by(-4);
        assert_eq!(pager.top(), 0);
        // Scrolling stops at the last screenful, not at the last line.
        pager.move_by(100);
        assert_eq!(pager.top(), 8);
        pager.first();
        assert_eq!(pager.top(), 0);
        pager.last();
        assert_eq!(pager.top(), 8);
    }

    #[test]
    fn paging_moves_by_the_lines_the_frame_showed() {
        let mut pager = page();
        drawn(&mut pager, Rect::new(0, 0, 20, 4));
        pager.page_by(1);

        assert_eq!(pager.top(), 4);
    }

    #[test]
    fn movement_hints_follow_viewport_and_top() {
        let mut pager = page();
        pager.resize(20);
        assert!(!pager.can_page_by(1));
        assert!(!pager.can_move_by(1));
        assert!(!pager.can_last());
        pager.resize(4);
        assert!(pager.can_page_by(1));
        assert!(pager.can_half_page_by(1));
        // The last screenful is bottom-aligned, and scrolling stops there.
        pager.last();
        assert!(!pager.can_last());
        assert!(!pager.can_move_by(1));
        assert!(!pager.can_page_by(1));
        assert!(pager.can_first());
        pager.first();
        assert!(!pager.can_first());
        assert!(!pager.can_move_by(-1));
    }

    #[test]
    fn last_line_keeps_top_alignment() {
        let mut pager = page();
        pager.set_top(11);
        for height in [4, 6, 20] {
            pager.resize(height);
            assert_eq!(pager.top(), 11);
            let rows = drawn(&mut pager, Rect::new(0, 0, 20, height as u16));
            assert_eq!(rows[0], "line 11");
            assert!(rows[1..].iter().all(String::is_empty));
            pager.move_to(11);
            assert_eq!(pager.top(), 11);
            assert_eq!(pager.visible(), 11..12);
        }
        pager.resize(0);
        assert_eq!(pager.top(), 11);
        assert!(pager.visible().is_empty());
    }

    #[test]
    fn anchored_scroll_preserves_direction() {
        for height in [4, 20] {
            let mut pager = page();
            pager.resize(height);
            pager.set_top(11);
            assert!(pager.can_first());
            assert!(pager.can_move_by(-1));
            assert!(!pager.can_move_by(1));
            assert!(!pager.can_page_by(1));
            pager.move_by(1);
            assert_eq!(pager.top(), 11);
            pager.move_by(-1);
            assert_eq!(pager.top(), 10);
            pager.move_by(0);
            assert_eq!(pager.top(), 10);
            pager.page_by(-1);
            assert_eq!(pager.top(), 10usize.saturating_sub(height));
            pager.first();
            assert_eq!(pager.top(), 0);
        }
    }

    #[test]
    fn selected_hit_keeps_the_viewport() {
        let mut pager = page();
        pager.resize(3);
        pager.search("needle");
        pager.set_top(5);
        pager.select_hit(7);
        assert_eq!(pager.current_hit(), Some(8));
        assert_eq!(pager.top(), 5);
        pager.next_hit();
        assert_eq!(pager.current_hit(), Some(2));
        assert!(pager.visible().contains(&2));
        pager.search("absent");
        pager.select_hit(7);
        assert_eq!(pager.current_hit(), None);
    }

    #[test]
    fn search_cycles_the_hits() {
        let mut pager = page();
        pager.search("NEEDLE");
        assert_eq!(pager.top(), 2);
        pager.next_hit();
        assert_eq!(pager.top(), 8);
        pager.next_hit();
        assert_eq!(pager.top(), 2);
        pager.previous_hit();
        assert_eq!(pager.top(), 8);
        pager.search("");
        assert_eq!(pager.top(), 8);
    }

    #[test]
    fn query_without_hits_moves_nothing() {
        let mut pager = page();
        pager.move_by(3);
        pager.search("nothing here");
        assert_eq!(pager.top(), 3);
        pager.next_hit();
        assert_eq!(pager.top(), 3);
    }

    #[test]
    fn search_keeps_span_colors() {
        use ratatui::style::{Color, Style};

        let mut pager = Pager::new(vec![
            Line::from(vec![
                Span::raw("hit "),
                Span::styled("blue", Style::new().fg(Color::Blue)),
            ])
            .style(Style::new().fg(Color::Red)),
        ]);
        let palette = Palette::new(true);
        let area = Rect::new(3, 1, 12, 1);
        let mut terminal = Terminal::new(TestBackend::new(20, 4)).unwrap();
        pager.search("hit");
        terminal
            .draw(|frame| pager.draw(frame, area, palette))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(area.x, area.y)].fg, palette.hit_style().fg.unwrap());
        assert_eq!(buffer[(area.x + 4, area.y)].fg, Color::Blue);
        assert!(
            buffer[(area.x + 4, area.y)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
        assert!(buffer[(area.x + 8, area.y)].modifier.is_empty());
    }
}
