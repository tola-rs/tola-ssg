//! The help reader coordinates navigation and input over one laid-out document.

use std::fmt::Write as _;
use std::rc::Rc;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::Rect;

use super::jump::Jump;
use super::layout::{DocumentPosition, LinkSpan, PageLayout};
use super::model::{Anchor, HelpDocument, LinkTarget, PageId};
use super::navigation::{History, Visit};
use crate::terminal::style::Palette;
use crate::terminal::ui::filter::{Reply, TextLine};
use crate::terminal::ui::pager::Pager;
use crate::terminal::ui::{Action, Step, Surface, answer_key, hints, keymap};

const ACCEPTED: &[Action] = &[
    Action::Quit,
    Action::Dismiss,
    Action::Up,
    Action::Down,
    Action::PageUp,
    Action::PageDown,
    Action::HalfPageUp,
    Action::HalfPageDown,
    Action::First,
    Action::Last,
    Action::Search,
    Action::NextMatch,
    Action::PreviousMatch,
    Action::Label,
];

pub(crate) struct View<'a> {
    document: Rc<HelpDocument>,
    page: PageLayout,
    history: History,
    mode: Mode,
    query: String,
    notice: Option<String>,
    content: Rect,
    mouse: bool,
    hovered: Option<usize>,
    palette: Palette,
    load: &'a dyn Fn(&PageId) -> Result<HelpDocument>,
}

enum Mode {
    Reading,
    Search(Search),
    Jump(Jump),
}

struct Search {
    line: TextLine,
    origin: DocumentPosition,
    focus: Option<DocumentPosition>,
}

impl<'a> View<'a> {
    pub(crate) fn new(
        document: HelpDocument,
        load: &'a dyn Fn(&PageId) -> Result<HelpDocument>,
    ) -> Self {
        let palette = Palette::new(false);
        let page = PageLayout::new(&document, 80, palette);
        Self {
            document: Rc::new(document),
            page,
            history: History::default(),
            mode: Mode::Reading,
            query: String::new(),
            notice: None,
            content: Rect::default(),
            mouse: false,
            hovered: None,
            palette,
            load,
        }
    }

    pub(crate) fn set_mouse(&mut self, mouse: bool) {
        self.mouse = mouse;
        self.hovered = None;
    }

    fn position(&self) -> DocumentPosition {
        self.page.position(self.page.pager.top())
    }

    fn focus(&self) -> Option<DocumentPosition> {
        self.page
            .pager
            .current_hit()
            .map(|line| self.page.position(line))
    }

    fn visit(&self) -> Visit {
        Visit {
            document: Rc::clone(&self.document),
            position: self.position(),
            focus: self.focus(),
        }
    }

    fn active_query(&self) -> &str {
        match &self.mode {
            Mode::Search(search) => search.line.text(),
            _ => &self.query,
        }
    }

    fn restore_position(&mut self, position: DocumentPosition, focus: Option<DocumentPosition>) {
        let query = self.active_query().to_owned();
        self.page.pager.search(&query);
        if let Some(focus) = focus {
            self.page.pager.select_hit(self.page.line_at(focus));
        }
        self.page.pager.set_top(self.page.line_at(position));
    }

    fn resize(&mut self, content: Rect, palette: Palette) {
        let reflow = self.page.width != usize::from(content.width).max(1)
            || self.palette.uses_color() != palette.uses_color();
        let changed = content != self.content || reflow;
        let position = self.position();
        let focus = self.focus();
        if reflow {
            self.page = PageLayout::new(&self.document, usize::from(content.width), palette);
        }
        self.content = content;
        self.palette = palette;
        self.page.pager.resize(usize::from(content.height));
        if reflow {
            self.restore_position(position, focus);
        }
        if changed {
            self.hovered = None;
            if matches!(self.mode, Mode::Jump(_)) {
                self.begin_jump();
            }
        }
    }

    fn begin_jump(&mut self) {
        self.hovered = None;
        self.mode = match Jump::new(
            &self.page.links,
            self.page.pager.visible(),
            usize::from(self.content.width),
        ) {
            Some(jump) => Mode::Jump(jump),
            None => {
                self.notice = Some("no links on screen".to_owned());
                Mode::Reading
            }
        };
    }

    fn scroll(&mut self, scroll: impl FnOnce(&mut Pager)) {
        self.hovered = None;
        if matches!(self.mode, Mode::Jump(_)) {
            self.mode = Mode::Reading;
        }
        scroll(&mut self.page.pager);
    }

    fn follow(&mut self, target: &LinkTarget) {
        self.hovered = None;
        self.mode = Mode::Reading;
        match target {
            LinkTarget::External(url) => self.notice = Some(url.clone()),
            LinkTarget::Page(id) => self.open(id, None),
            LinkTarget::PageAnchor(id, anchor) => self.open(id, Some(anchor)),
        }
    }

    fn open(&mut self, id: &PageId, anchor: Option<&Anchor>) {
        let document = if *id == self.document.id {
            Rc::clone(&self.document)
        } else {
            match (self.load)(id) {
                Ok(document) => Rc::new(document),
                Err(error) => {
                    tracing::debug!(?error, "could not open help page");
                    self.notice = Some(format!("could not open `{}`", page_label(id)));
                    return;
                }
            }
        };
        if let Some(anchor) = anchor
            && !document.contains_anchor(anchor)
        {
            self.missing_anchor(id, anchor);
            return;
        }
        let destination_line = |page: &PageLayout| {
            anchor.map_or(0, |anchor| {
                page.anchor_line(anchor)
                    .expect("the document's anchor has a layout row")
            })
        };
        // A jump inside the current page moves within it; history keeps pages.
        if *id == self.document.id {
            self.page.pager.set_top(destination_line(&self.page));
            return;
        }
        let mut page = PageLayout::new(&document, self.page.width, self.palette);
        page.pager.resize(usize::from(self.content.height));
        page.pager.search(&self.query);
        page.pager.set_top(destination_line(&page));
        // A failed load or missing anchor commits neither the page nor its history.
        self.history.push(self.visit());
        self.document = document;
        self.page = page;
    }

    fn missing_anchor(&mut self, id: &PageId, anchor: &Anchor) {
        self.notice = Some(format!(
            "`{}` is not on `{}`",
            anchor.as_str(),
            page_label(id)
        ));
    }

    fn restore_visit(&mut self, visit: Visit) {
        self.mode = Mode::Reading;
        self.hovered = None;
        self.document = visit.document;
        self.page = PageLayout::new(&self.document, self.page.width, self.palette);
        self.page.pager.resize(usize::from(self.content.height));
        self.restore_position(visit.position, visit.focus);
    }

    fn begin_search(&mut self) {
        self.hovered = None;
        self.mode = Mode::Search(Search {
            line: TextLine::new("search"),
            origin: self.position(),
            focus: self.focus(),
        });
        self.page.pager.search("");
    }

    fn edit_search(&mut self) {
        self.hovered = None;
        let query = self.active_query().to_owned();
        self.page.pager.search(&query);
    }

    fn finish_search(&mut self, accepted: bool) {
        let Mode::Search(search) = std::mem::replace(&mut self.mode, Mode::Reading) else {
            return;
        };
        self.hovered = None;
        if accepted {
            self.query = search.line.text().to_owned();
        } else {
            self.restore_position(search.origin, search.focus);
        }
    }

    fn link_at(&self, column: u16, row: u16) -> Option<&LinkSpan> {
        if !self.content.contains((column, row).into()) {
            return None;
        }
        let line = self.page.pager.top() + usize::from(row - self.content.y);
        let column = usize::from(column - self.content.x);
        self.page.links.iter().find(|link| {
            link.line == line && (link.column..link.column + link.width).contains(&column)
        })
    }

    fn draw_hover(&self, frame: &mut Frame) {
        let Some(hovered) = self.hovered else { return };
        for link in self.page.links.iter().filter(|link| link.id == hovered) {
            if !self.page.pager.visible().contains(&link.line) {
                continue;
            }
            let y = self.content.y + (link.line - self.page.pager.top()) as u16;
            let columns = usize::from(self.content.width);
            for column in link.column..(link.column + link.width).min(columns) {
                let x = self.content.x + column as u16;
                frame.buffer_mut()[(x, y)].set_style(self.palette.link_hover_style());
            }
        }
    }

    fn status(&self) -> String {
        let mut status = format!(
            "{} - {}/{} lines",
            page_label(&self.document.id),
            (self.page.pager.top() + 1).min(self.page.pager.line_count()),
            self.page.pager.line_count()
        );
        let query = self.active_query();
        if !query.is_empty() {
            let _ = write!(status, " · /{query}");
        }
        status
    }

    /// Whether one action still scrolls the page: a page that fits, or an end already reached,
    /// earns no hint.
    fn scroll_changes(&self, action: Action) -> bool {
        let pager = &self.page.pager;
        match action {
            Action::Up => pager.can_move_by(-1),
            Action::Down => pager.can_move_by(1),
            Action::PageUp => pager.can_page_by(-1),
            Action::PageDown => pager.can_page_by(1),
            Action::HalfPageUp => pager.can_half_page_by(-1),
            Action::HalfPageDown => pager.can_half_page_by(1),
            Action::First => pager.can_first(),
            Action::Last => pager.can_last(),
            _ => true,
        }
    }
}

impl Surface for View<'_> {
    fn draw(&mut self, frame: &mut Frame, palette: Palette) {
        let (content, footer) = split_footer(frame.area());
        self.resize(content, palette);
        self.page.pager.draw(frame, content, palette);
        self.draw_hover(frame);
        if let Mode::Jump(jump) = &mut self.mode
            && !jump.draw(frame, content, self.page.pager.top(), palette)
        {
            self.mode = Mode::Reading;
            self.notice = Some("no links on screen".to_owned());
        }
        let caption = self.caption();
        let status = self.status();
        let line = self
            .notice
            .as_deref()
            .or(caption.as_deref())
            .unwrap_or(&status);
        hints::draw(frame, footer, line, &self.live(), self.bindings(), palette);
        if let Mode::Search(search) = &self.mode {
            search.line.draw(
                frame,
                Rect {
                    height: footer.height.min(1),
                    ..footer
                },
                palette,
            );
        }
    }

    fn answer(&mut self, action: Action) -> Step {
        self.notice = None;
        match action {
            Action::Quit => return Step::Done,
            Action::Dismiss => match self.mode {
                Mode::Search(_) => self.finish_search(false),
                Mode::Jump(_) => self.mode = Mode::Reading,
                Mode::Reading => match self.history.back(self.visit()) {
                    Some(visit) => self.restore_visit(visit),
                    None => return Step::Cancel,
                },
            },
            Action::Back => {
                if let Some(visit) = self.history.back(self.visit()) {
                    self.restore_visit(visit);
                }
            }
            Action::Forward => {
                if let Some(visit) = self.history.forward(self.visit()) {
                    self.restore_visit(visit);
                }
            }
            Action::NextSection | Action::PreviousSection => {
                let top = self.page.pager.top();
                let anchor = if action == Action::NextSection {
                    self.page.next_section(top)
                } else {
                    self.page.previous_section(top)
                }
                .cloned();
                if let Some(anchor) = anchor {
                    self.follow(&LinkTarget::PageAnchor(self.document.id.clone(), anchor));
                }
            }
            Action::Up => self.scroll(|pager| pager.move_by(-1)),
            Action::Down => self.scroll(|pager| pager.move_by(1)),
            Action::PageUp => self.scroll(|pager| pager.page_by(-1)),
            Action::PageDown => self.scroll(|pager| pager.page_by(1)),
            Action::HalfPageUp => self.scroll(|pager| pager.half_page_by(-1)),
            Action::HalfPageDown => self.scroll(|pager| pager.half_page_by(1)),
            Action::First => self.scroll(Pager::first),
            Action::Last => self.scroll(Pager::last),
            Action::NextMatch => self.scroll(Pager::next_hit),
            Action::PreviousMatch => self.scroll(Pager::previous_hit),
            Action::Search => self.begin_search(),
            Action::Label => match self.mode {
                Mode::Jump(_) => self.mode = Mode::Reading,
                Mode::Reading => self.begin_jump(),
                Mode::Search(_) => {}
            },
            _ => {}
        }
        Step::Continue
    }

    fn key(&mut self, key: &KeyEvent) -> Step {
        self.notice = None;
        match &mut self.mode {
            Mode::Jump(jump) => {
                if key.code == KeyCode::Esc || keymap::HELP.action(key) == Some(Action::Label) {
                    self.mode = Mode::Reading;
                } else if let KeyCode::Char(character) = key.code
                    && key.modifiers.difference(KeyModifiers::SHIFT).is_empty()
                    && let Some(target) = jump.key(character)
                {
                    self.follow(&target);
                }
                Step::Continue
            }
            Mode::Search(search) => {
                let before = search.line.text().to_owned();
                match search.line.key(key) {
                    Reply::Editing if before != search.line.text() => self.edit_search(),
                    Reply::Editing => {}
                    Reply::Accepted => self.finish_search(true),
                    Reply::Dismissed => self.finish_search(false),
                }
                Step::Continue
            }
            Mode::Reading => answer_key(self, key),
        }
    }

    fn paste(&mut self, text: &str) -> Step {
        if let Mode::Search(search) = &mut self.mode {
            search.line.paste(text);
            self.edit_search();
        }
        Step::Continue
    }

    fn pointer(&mut self, event: MouseEvent) -> Step {
        if !self.mouse || !matches!(self.mode, Mode::Reading) {
            return Step::Continue;
        }
        match event.kind {
            MouseEventKind::Moved => {
                self.hovered = self.link_at(event.column, event.row).map(|link| link.id);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(target) = self
                    .link_at(event.column, event.row)
                    .map(|link| link.target.clone())
                {
                    self.notice = None;
                    self.follow(&target);
                }
            }
            _ => {}
        }
        Step::Continue
    }

    fn bindings(&self) -> &'static keymap::Table {
        &keymap::HELP
    }

    fn title(&self) -> Option<String> {
        Some(format!("tola help - {}", page_label(&self.document.id)))
    }

    fn wants_mouse(&self) -> bool {
        self.mouse
    }

    fn live(&self) -> Vec<Action> {
        if !matches!(self.mode, Mode::Reading) {
            return vec![Action::Dismiss];
        }
        let mut live = ACCEPTED.to_vec();
        live.retain(|action| self.scroll_changes(*action));
        let top = self.page.pager.top();
        if self.page.next_section(top).is_some() {
            live.push(Action::NextSection);
        }
        if self.page.previous_section(top).is_some() {
            live.push(Action::PreviousSection);
        }
        if !self.page.pager.has_hits() {
            live.retain(|action| !matches!(action, Action::NextMatch | Action::PreviousMatch));
        }
        if self.history.has_back() {
            live.push(Action::Back);
        }
        if self.history.has_forward() {
            live.push(Action::Forward);
        }
        live
    }

    fn caption(&self) -> Option<String> {
        match &self.mode {
            Mode::Jump(jump) => Some(jump.caption()),
            _ => None,
        }
    }
}

fn page_label(id: &PageId) -> String {
    id.selector().unwrap_or_else(|| "tola help".to_owned())
}

fn split_footer(area: Rect) -> (Rect, Rect) {
    let height = area.height.min(2);
    (
        Rect {
            height: area.height - height,
            ..area
        },
        Rect {
            y: area.bottom() - height,
            height,
            ..area
        },
    )
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::{Terminal, TerminalOptions, Viewport};

    use super::*;
    use crate::help::model::{HelpPage, anchor};

    fn document(markdown: &str) -> HelpDocument {
        HelpDocument::parse(HelpPage::new(PageId::Overview, markdown.to_owned()))
    }

    fn linked() -> HelpDocument {
        document(
            "# Index\n\nRead [the whole manual](tola://package/@tola/schema).\n\n## Details\n\nMore text.\n\n## Tail\n\nEnd.\n",
        )
    }

    fn load(id: &PageId) -> Result<HelpDocument> {
        Ok(HelpDocument::parse(HelpPage::new(
            id.clone(),
            "# Destination\n\n## Details\n\nOther text.\n".to_owned(),
        )))
    }

    fn destination() -> LinkTarget {
        LinkTarget::Page(PageId::Package {
            name: "@tola/schema".to_owned(),
        })
    }

    fn frame(view: &mut View<'_>, area: Rect) -> Buffer {
        let mut terminal = Terminal::with_options(
            TestBackend::new(area.right().max(1), area.bottom().max(1)),
            TerminalOptions {
                viewport: Viewport::Fixed(area),
            },
        )
        .unwrap();
        terminal
            .draw(|frame| view.draw(frame, Palette::new(true)))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn row(buffer: &Buffer, area: Rect, row: u16) -> String {
        (area.x..area.right())
            .map(|x| buffer[(x, row)].symbol())
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    fn press(view: &mut View<'_>, code: KeyCode) -> Step {
        view.key(&KeyEvent::from(code))
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn search_page() -> HelpDocument {
        document(
            "# Title\n\nfirst needle\n\nfiller alpha beta gamma delta epsilon zeta\n\nsecond needle\n\nend alpha beta gamma delta epsilon zeta\n",
        )
    }

    fn search(view: &mut View<'_>, query: &str) {
        press(view, KeyCode::Char('/'));
        view.paste(query);
        press(view, KeyCode::Enter);
    }

    #[test]
    fn jump_keys_follow_visible_links() {
        let mut view = View::new(linked(), &load);
        let area = Rect::new(0, 0, 60, 10);
        let plain = frame(&mut view, area);
        press(&mut view, KeyCode::Tab);
        let labelled = frame(&mut view, area);
        assert_ne!(labelled, plain);
        assert!(row(&labelled, area, 2).contains("the whole manual"));
        press(&mut view, KeyCode::Char('a'));
        assert_eq!(LinkTarget::Page(view.document.id.clone()), destination());
        assert!(matches!(view.mode, Mode::Reading));
    }

    #[test]
    fn cancelling_jump_restores_the_page() {
        for cancel in [KeyCode::Esc, KeyCode::Tab] {
            let mut view = View::new(linked(), &load);
            let area = Rect::new(0, 0, 60, 10);
            let plain = frame(&mut view, area);
            press(&mut view, KeyCode::Tab);
            frame(&mut view, area);
            press(&mut view, KeyCode::Char('q'));
            assert!(matches!(view.mode, Mode::Jump(_)));
            press(&mut view, cancel);
            assert_eq!(frame(&mut view, area), plain);
        }
    }

    #[test]
    fn reflow_relabels_current_targets() {
        let mut view = View::new(
            document(
                "# A title that wraps across several lines\n\nfirst [go](tola://package/@tola/schema)\n\nmore content\n",
            ),
            &load,
        );
        frame(&mut view, Rect::new(0, 0, 80, 10));
        press(&mut view, KeyCode::Tab);
        frame(&mut view, Rect::new(0, 0, 18, 10));
        press(&mut view, KeyCode::Char('a'));
        assert_eq!(LinkTarget::Page(view.document.id.clone()), destination());
    }

    #[test]
    fn shrinking_hides_jump_targets() {
        let mut view = View::new(linked(), &load);
        frame(&mut view, Rect::new(0, 0, 60, 10));
        press(&mut view, KeyCode::Tab);
        frame(&mut view, Rect::new(0, 0, 60, 3));
        assert!(matches!(view.mode, Mode::Reading));
        press(&mut view, KeyCode::Char('a'));
        assert_eq!(view.document.id, PageId::Overview);
        frame(&mut view, Rect::new(0, 0, 60, 2));
        assert!(view.page.pager.visible().is_empty());
        press(&mut view, KeyCode::Tab);
        assert!(matches!(view.mode, Mode::Reading));
    }

    #[test]
    fn scrolling_dismisses_jump_labels() {
        let mut view = View::new(linked(), &load);
        frame(&mut view, Rect::new(0, 0, 60, 6));
        press(&mut view, KeyCode::Tab);
        view.answer(Action::Down);
        assert!(matches!(view.mode, Mode::Reading));
        assert_eq!(view.page.pager.top(), 1);
        view.answer(Action::HalfPageDown);
        assert_eq!(view.page.pager.top(), 3);
        view.answer(Action::HalfPageUp);
        assert_eq!(view.page.pager.top(), 1);
    }

    #[test]
    fn same_page_anchors_stay_out_of_history() {
        let mut view = View::new(linked(), &load);
        let area = Rect::new(0, 0, 60, 3);
        frame(&mut view, area);
        view.follow(&LinkTarget::PageAnchor(PageId::Overview, anchor("Details")));
        assert_eq!(row(&frame(&mut view, area), area, 0), "Details");
        assert!(!view.history.has_back());
        assert_eq!(view.answer(Action::Dismiss), Step::Cancel);
    }

    #[test]
    fn back_returns_to_the_page_that_was_left() {
        let mut view = View::new(linked(), &load);
        let area = Rect::new(0, 0, 60, 3);
        frame(&mut view, area);
        view.follow(&destination());
        let opened = view.document.id.clone();
        view.follow(&LinkTarget::PageAnchor(opened, anchor("Details")));
        assert_eq!(row(&frame(&mut view, area), area, 0), "Details");
        assert_eq!(view.answer(Action::Back), Step::Continue);
        assert_eq!(view.document.id, PageId::Overview);
    }

    #[test]
    fn section_keys_follow_export_boundaries() {
        let mut page = HelpPage::new(
            PageId::Overview,
            "# API\n\n## Intro - function\n\nIntroduction.\n".to_owned(),
        );
        page.push_export(format!(
            "\n## first - function\n\n{}\n## Example - function\n\nAn ordinary documentation heading.\n",
            "A paragraph of the first export.\n\n".repeat(10),
        ));
        page.push_export("\n## second - value\n\nShort description.\n".to_owned());
        let mut view = View::new(HelpDocument::parse(page), &load);
        let area = Rect::new(0, 0, 60, 8);
        frame(&mut view, area);
        press(&mut view, KeyCode::Char('f'));
        assert_eq!(row(&frame(&mut view, area), area, 0), "first - function");
        let first = view.position();

        press(&mut view, KeyCode::Char(' '));
        assert_ne!(view.position(), first);
        press(&mut view, KeyCode::Char('b'));
        assert_eq!(view.position(), first);
        press(&mut view, KeyCode::Char('b'));
        assert_eq!(view.position(), first);

        press(&mut view, KeyCode::Char('f'));
        assert_eq!(row(&frame(&mut view, area), area, 0), "second - value");
        let second = view.position();
        let resized = Rect::new(0, 0, 30, 20);
        assert_eq!(
            row(&frame(&mut view, resized), resized, 0),
            "second - value"
        );
        press(&mut view, KeyCode::Char('f'));
        assert_eq!(view.position(), second);
        assert!(!view.live().contains(&Action::NextSection));
        press(&mut view, KeyCode::Char('b'));
        assert_eq!(
            row(&frame(&mut view, resized), resized, 0),
            "first - function"
        );
    }

    #[test]
    fn tail_anchor_keeps_top_alignment() {
        let mut view = View::new(linked(), &load);
        let area = Rect::new(0, 0, 60, 20);
        frame(&mut view, area);
        view.follow(&LinkTarget::PageAnchor(PageId::Overview, anchor("Tail")));
        assert_eq!(row(&frame(&mut view, area), area, 0), "Tail");
        let resized = Rect::new(0, 0, 24, 30);
        assert_eq!(row(&frame(&mut view, resized), resized, 0), "Tail");
    }

    #[test]
    fn history_restores_source_after_reflow() {
        let words = (0..90)
            .map(|index| format!("word{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let mut view = View::new(document(&format!("# Index\n\n{words}\n\nTail.\n")), &load);
        frame(&mut view, Rect::new(0, 0, 28, 6));
        view.page.pager.set_top(8);
        let narrow = Rect::new(0, 0, 28, 6);
        let origin = row(&frame(&mut view, narrow), narrow, 0);
        let word = origin.split_whitespace().next().unwrap();
        view.follow(&destination());
        let wide = Rect::new(0, 0, 48, 6);
        frame(&mut view, wide);
        view.answer(Action::Back);
        assert!(
            row(&frame(&mut view, wide), wide, 0)
                .split_whitespace()
                .any(|text| text == word)
        );
        view.answer(Action::Forward);
        assert_eq!(LinkTarget::Page(view.document.id.clone()), destination());
    }

    #[test]
    fn new_navigation_discards_forward_history() {
        let mut view = View::new(linked(), &load);
        frame(&mut view, Rect::new(0, 0, 60, 10));
        view.follow(&destination());
        view.answer(Action::Back);
        assert!(view.history.has_forward());
        view.follow(&destination());
        assert!(!view.history.has_forward());
        assert!(view.history.has_back());
        assert_eq!(view.answer(Action::Quit), Step::Done);
    }

    #[test]
    fn missing_anchors_preserve_navigation() {
        let mut view = View::new(linked(), &load);
        frame(&mut view, Rect::new(0, 0, 60, 6));
        view.follow(&destination());
        view.answer(Action::Back);
        let origin = view.position();
        for id in [
            PageId::Overview,
            PageId::Package {
                name: "@tola/schema".to_owned(),
            },
        ] {
            view.follow(&LinkTarget::PageAnchor(id, anchor("absent")));
            assert_eq!(view.document.id, PageId::Overview);
            assert_eq!(view.position(), origin);
            assert!(view.history.has_forward());
            assert!(!view.history.has_back());
            assert!(view.notice.as_ref().unwrap().contains("absent"));
        }
    }

    #[test]
    fn failed_page_keeps_current_document() {
        let failed = |_: &PageId| anyhow::bail!("private implementation detail");
        let mut view = View::new(linked(), &failed);
        frame(&mut view, Rect::new(0, 0, 60, 6));
        view.follow(&destination());
        assert_eq!(view.document.id, PageId::Overview);
        assert!(!view.history.has_back());
        assert_eq!(
            view.notice.as_deref(),
            Some("could not open `@tola/schema`")
        );
    }

    #[test]
    fn pasted_search_updates_visible_matches() {
        let mut view = View::new(search_page(), &load);
        let area = Rect::new(0, 0, 30, 4);
        frame(&mut view, area);
        press(&mut view, KeyCode::Char('/'));
        view.paste("second needle");
        let hit = view
            .page
            .pager
            .current_hit()
            .expect("the paste finds its match");
        assert!(view.page.pager.visible().contains(&hit));
        assert!(matches!(view.mode, Mode::Search(_)));
        let shown = frame(&mut view, area);
        assert!((0..view.content.height).any(|y| row(&shown, area, y).contains("second needle")));
    }

    #[test]
    fn draft_search_survives_reflow() {
        let mut view = View::new(search_page(), &load);
        frame(&mut view, Rect::new(0, 0, 30, 4));
        search(&mut view, "first needle");
        let origin = view.position();
        let focus = view.focus();
        press(&mut view, KeyCode::Char('/'));
        for character in "second needle".chars() {
            press(&mut view, KeyCode::Char(character));
        }
        let selected = view.focus().unwrap();
        frame(&mut view, Rect::new(0, 0, 50, 4));
        assert_eq!(view.active_query(), "second needle");
        assert_eq!(view.focus(), Some(selected));
        press(&mut view, KeyCode::Esc);
        assert_eq!(view.active_query(), "first needle");
        assert_eq!(view.position(), origin);
        assert_eq!(view.focus(), focus);
    }

    #[test]
    fn reflow_preserves_selected_search_occurrence() {
        let mut view = View::new(search_page(), &load);
        frame(&mut view, Rect::new(0, 0, 30, 5));
        search(&mut view, "needle");
        let first = view.focus().unwrap();
        press(&mut view, KeyCode::Char('n'));
        let second = view.focus().unwrap();
        assert_ne!(first, second);
        frame(&mut view, Rect::new(0, 0, 60, 5));
        assert_eq!(view.focus(), Some(second));
        press(&mut view, KeyCode::Char('n'));
        assert_eq!(view.focus(), Some(first));
    }

    #[test]
    fn history_preserves_search_focus() {
        let mut view = View::new(search_page(), &load);
        frame(&mut view, Rect::new(0, 0, 80, 20));
        search(&mut view, "needle");
        let first = view.focus().unwrap();
        press(&mut view, KeyCode::Char('n'));
        let second = view.focus().unwrap();
        assert_eq!(view.page.pager.top(), 0);
        view.follow(&destination());
        view.answer(Action::Back);
        assert_eq!(view.focus(), Some(second));
        press(&mut view, KeyCode::Char('n'));
        assert_eq!(view.focus(), Some(first));
    }

    #[test]
    fn cancelled_search_restores_scroll_exactly() {
        let mut view = View::new(search_page(), &load);
        frame(&mut view, Rect::new(0, 0, 30, 5));
        search(&mut view, "needle");
        press(&mut view, KeyCode::Char('n'));
        let origin = view.position();
        let focus = view.focus();
        press(&mut view, KeyCode::Char('/'));
        view.paste("Title");
        assert_ne!(view.position(), origin);
        press(&mut view, KeyCode::Esc);
        assert_eq!(view.position(), origin);
        assert_eq!(view.focus(), focus);
    }

    #[test]
    fn cancelled_search_restores_separator_row() {
        let mut view = View::new(linked(), &load);
        frame(&mut view, Rect::new(0, 0, 60, 5));
        view.answer(Action::Down);
        let top = view.page.pager.top();
        press(&mut view, KeyCode::Char('/'));
        view.paste("Tail");
        press(&mut view, KeyCode::Esc);
        assert_eq!(view.page.pager.top(), top);
    }

    #[test]
    fn wrapped_hover_uses_content_coordinates() {
        let mut view = View::new(
            document(
                "[one two three four five six](tola://package/@tola/schema) [other](https://example.com)\n",
            ),
            &load,
        );
        view.set_mouse(true);
        let area = Rect::new(5, 3, 16, 12);
        frame(&mut view, area);
        let spans = view
            .page
            .links
            .iter()
            .filter(|link| link.id == 0)
            .map(|link| {
                (
                    link.column as u16 + area.x,
                    link.line as u16 + area.y,
                    link.width as u16,
                )
            })
            .collect::<Vec<_>>();
        assert!(spans.len() > 1);
        let (column, y, _) = *spans.last().unwrap();
        view.pointer(mouse(MouseEventKind::Moved, column, y));
        let shown = frame(&mut view, area);
        for (column, y, width) in spans {
            for x in column..column + width {
                assert_eq!(
                    shown[(x, y)].fg,
                    Palette::new(true).link_hover_style().fg.unwrap()
                );
                assert_eq!(
                    shown[(x, y)].modifier,
                    Palette::new(true).link_hover_style().add_modifier
                );
            }
        }
        let other = view.page.links.iter().find(|link| link.id != 0).unwrap();
        assert_eq!(
            shown[(other.column as u16 + area.x, other.line as u16 + area.y)].modifier,
            Palette::new(true).link_style().add_modifier
        );
        view.pointer(mouse(MouseEventKind::Down(MouseButton::Left), column, y));
        assert_eq!(LinkTarget::Page(view.document.id.clone()), destination());
    }

    #[test]
    fn search_movement_clears_hover() {
        let mut view = View::new(linked(), &load);
        view.set_mouse(true);
        frame(&mut view, Rect::new(0, 0, 60, 6));
        let link = &view.page.links[0];
        view.pointer(mouse(
            MouseEventKind::Moved,
            link.column as u16,
            link.line as u16,
        ));
        assert!(view.hovered.is_some());
        search(&mut view, "Tail");
        assert!(view.hovered.is_none());
        press(&mut view, KeyCode::Char('N'));
        assert!(view.hovered.is_none());
    }

    #[test]
    fn mouse_respects_input_mode() {
        let mut view = View::new(linked(), &load);
        frame(&mut view, Rect::new(0, 0, 60, 10));
        let link = &view.page.links[0];
        let click = mouse(
            MouseEventKind::Down(MouseButton::Left),
            link.column as u16,
            link.line as u16,
        );
        view.pointer(click);
        assert_eq!(view.document.id, PageId::Overview);
        view.set_mouse(true);
        press(&mut view, KeyCode::Char('/'));
        view.pointer(click);
        assert!(matches!(view.mode, Mode::Search(_)));
        assert_eq!(view.document.id, PageId::Overview);
        press(&mut view, KeyCode::Esc);
        view.pointer(click);
        assert_eq!(LinkTarget::Page(view.document.id.clone()), destination());
    }

    #[test]
    fn external_jump_shows_destination() {
        let mut view = View::new(
            document("[read manual](https://example.com/manual)\n"),
            &load,
        );
        let area = Rect::new(0, 0, 40, 6);
        frame(&mut view, area);
        press(&mut view, KeyCode::Tab);
        frame(&mut view, area);
        press(&mut view, KeyCode::Char('a'));
        assert_eq!(view.notice.as_deref(), Some("https://example.com/manual"));
        assert!(!view.history.has_back());
    }

    #[test]
    fn scrolling_hints_follow_the_page_and_its_ends() {
        let short = document("# Index\n\nOne line.\n");
        let mut view = View::new(short, &load);
        frame(&mut view, Rect::new(0, 0, 60, 20));
        for action in [
            Action::Up,
            Action::Down,
            Action::PageUp,
            Action::PageDown,
            Action::HalfPageUp,
            Action::HalfPageDown,
            Action::First,
            Action::Last,
        ] {
            assert!(!view.live().contains(&action), "{action:?}");
        }

        let paragraphs = (0..60)
            .map(|index| format!("Paragraph {index}.\n\n"))
            .collect::<String>();
        let mut view = View::new(document(&format!("# Index\n\n{paragraphs}")), &load);
        let area = Rect::new(0, 0, 60, 10);
        frame(&mut view, area);
        assert!(!view.live().contains(&Action::Up));
        assert!(!view.live().contains(&Action::First));
        assert!(view.live().contains(&Action::Down));
        assert!(view.live().contains(&Action::PageDown));
        assert!(view.live().contains(&Action::Last));
        view.answer(Action::Last);
        frame(&mut view, area);
        assert!(!view.live().contains(&Action::Down));
        assert!(!view.live().contains(&Action::PageDown));
        assert!(!view.live().contains(&Action::Last));
        assert!(view.live().contains(&Action::Up));
        assert!(view.live().contains(&Action::First));
    }
}
