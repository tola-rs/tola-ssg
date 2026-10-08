//! The help reader coordinates navigation and input over one laid-out document.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::jump::Jump;
use super::layout::{DocumentPosition, LinkSpan, PageLayout, slice_columns};
use super::model::{Anchor, HelpDocument, LinkTarget, PageId};
use super::navigation::{History, Visit};
use crate::i18n::HelpLanguage;
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

/// Where the export prompt reads the filesystem: the directories under one path, the
/// absolute path a typed destination names, and the directory a relative one starts from.
pub(crate) trait DestinationSource {
    /// The directories directly under `directory` whose name starts with `prefix`.
    fn directories(&self, directory: &Path, prefix: &str) -> Vec<String>;
    /// `typed`, as an absolute path.
    fn resolve(&self, typed: &Path) -> PathBuf;
    /// The directory a relative destination starts from.
    fn base(&self) -> PathBuf;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreviewState {
    pub demo: String,
    pub title: String,
    pub phase: PreviewPhase,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PreviewPhase {
    Preparing,
    Ready { url: String },
    Failed { message: String },
    Stopped,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReaderAction {
    Preview {
        demo: String,
    },
    StopPreview,
    Export {
        demo: String,
        destination: PathBuf,
        edit: bool,
    },
    OpenBrowser {
        url: String,
    },
}

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
    hovered_button: Option<Action>,
    buttons: Vec<(Rect, Action)>,
    palette: Palette,
    load: &'a dyn Fn(&PageId) -> Result<HelpDocument>,
    language: HelpLanguage,
    preview: Option<PreviewState>,
    poll: Option<&'a dyn Fn() -> Option<PreviewState>>,
    pending: Option<ReaderAction>,
    destinations: Option<&'a dyn DestinationSource>,
    clipboard: Option<&'a dyn Clipboard>,
    selection: Option<Selection>,
    /// The press a release still decides: a click opens what it landed on, a drag copies.
    pressed: Option<(u16, u16)>,
    dragging: bool,
}

enum Mode {
    Reading,
    Search(Search),
    Jump(Jump),
    Destination {
        demo: String,
        line: TextLine,
        edit: bool,
    },
}

struct Search {
    line: TextLine,
    origin: DocumentPosition,
    focus: Option<DocumentPosition>,
}

/// Where a copied selection goes.
pub(crate) trait Clipboard {
    /// Copies `text`; answers whether a clipboard the reader can trust took it. A terminal's
    /// own escape is written either way.
    fn copy(&self, text: &str) -> bool;
}

/// The content cells one mouse drag selected, in page lines and display columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selection {
    from_line: usize,
    from_column: usize,
    to_line: usize,
    to_column: usize,
}

impl Selection {
    fn at(cell: (usize, usize)) -> Self {
        Self {
            from_line: cell.0,
            from_column: cell.1,
            to_line: cell.0,
            to_column: cell.1,
        }
    }

    fn extend(&mut self, cell: (usize, usize)) {
        self.to_line = cell.0;
        self.to_column = cell.1;
    }

    /// The selection's ends, from the earliest cell to the latest.
    fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if (self.from_line, self.from_column) <= (self.to_line, self.to_column) {
            (
                (self.from_line, self.from_column),
                (self.to_line, self.to_column),
            )
        } else {
            (
                (self.to_line, self.to_column),
                (self.from_line, self.from_column),
            )
        }
    }
}

impl<'a> View<'a> {
    pub(crate) fn new(
        document: HelpDocument,
        load: &'a dyn Fn(&PageId) -> Result<HelpDocument>,
        columns: usize,
        palette: Palette,
    ) -> Self {
        let page = PageLayout::new(&document, columns, palette);
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
            hovered_button: None,
            buttons: Vec::new(),
            palette,
            load,
            language: HelpLanguage::English,
            preview: None,
            poll: None,
            pending: None,
            destinations: None,
            clipboard: None,
            selection: None,
            pressed: None,
            dragging: false,
        }
    }

    /// Whether this reader holds the pointer: the wheel scrolls it, a click opens what it is
    /// on, and a drag selects the text under it. A released pointer leaves all of that to the
    /// terminal, and `--no-mouse` starts it released.
    pub(crate) fn set_mouse(&mut self, mouse: bool) {
        self.mouse = mouse;
        self.hovered = None;
    }

    pub(crate) fn set_clipboard(&mut self, clipboard: &'a dyn Clipboard) {
        self.clipboard = Some(clipboard);
    }

    pub(crate) fn set_language(&mut self, language: HelpLanguage) {
        self.language = language;
    }

    pub(crate) fn set_preview(&mut self, poll: &'a dyn Fn() -> Option<PreviewState>) {
        self.poll = Some(poll);
    }

    pub(crate) fn set_destinations(&mut self, destinations: &'a dyn DestinationSource) {
        self.destinations = Some(destinations);
    }

    pub(crate) fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
    }

    pub(crate) fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub(crate) fn take_action(&mut self) -> Option<ReaderAction> {
        self.pending.take()
    }

    pub(crate) fn navigate(&mut self, target: &LinkTarget) {
        self.follow(target);
    }

    fn poll_preview(&mut self) -> bool {
        let Some(poll) = self.poll else { return false };
        let next = poll();
        if next == self.preview {
            return false;
        }
        self.preview = next;
        true
    }

    fn request(&mut self, action: ReaderAction) -> Step {
        self.pending = Some(action);
        Step::Done
    }

    fn begin_export(&mut self, edit: bool) {
        if let Some(demo) = self.document.id.demo() {
            self.mode = Mode::Destination {
                demo: demo.into(),
                line: TextLine::new("export directory"),
                edit,
            };
        }
    }

    fn finish_export(&mut self) -> Step {
        let Mode::Destination { demo, line, edit } = &self.mode else {
            return Step::Continue;
        };
        let destination = line.text().trim();
        if destination.is_empty() {
            self.notice = Some("choose a new export directory".into());
            return Step::Continue;
        }
        let action = ReaderAction::Export {
            demo: demo.clone(),
            destination: PathBuf::from(destination),
            edit: *edit,
        };
        self.mode = Mode::Reading;
        self.request(action)
    }

    /// Completes the destination being typed with the directories under it, and names the
    /// candidates when the matches do not decide the rest of the name.
    fn complete_destination(&mut self) {
        let Some(source) = self.destinations else {
            return;
        };
        let Mode::Destination { line, .. } = &self.mode else {
            return;
        };
        let typed = line.text().to_owned();
        let (directory, prefix) = completion_parts(&typed);
        let matches = source.directories(&directory, &prefix);
        let head = &typed[..typed.len() - prefix.len()];
        let completion = match matches.as_slice() {
            [] => None,
            [only] => Some(format!("{head}{only}/")),
            matches => Some(format!("{head}{}", shared_prefix(matches))),
        };
        if let Some(completion) = completion.filter(|completion| completion.len() > typed.len()) {
            if let Mode::Destination { line, .. } = &mut self.mode {
                line.set_text(completion);
            }
            return;
        }
        self.notice = Some(match matches.as_slice() {
            [] => "no matching directory".to_owned(),
            matches => format!("matches {}", listed(matches)),
        });
    }

    /// What the export prompt tells the reader about the destination it names: where a
    /// relative path starts, or the absolute path it writes to.
    fn destination_caption(&self) -> String {
        let Mode::Destination { line, .. } = &self.mode else {
            return String::new();
        };
        let Some(source) = self.destinations else {
            return String::new();
        };
        let typed = line.text().trim();
        if typed.is_empty() {
            format!("relative to {}", source.base().display())
        } else {
            format!("writes to {}", source.resolve(Path::new(typed)).display())
        }
    }

    fn demo_actions(&self) -> Vec<(Action, &'static str)> {
        let live = self.live();
        [
            (Action::Preview, "Preview"),
            (Action::StopPreview, "Stop"),
            (Action::Export, "Export"),
            (Action::ExportAndEdit, "Export & edit"),
            (Action::OpenBrowser, "Open browser"),
        ]
        .into_iter()
        .filter(|(action, _)| live.contains(action))
        .collect()
    }

    fn draw_buttons(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        actions: &[(Action, &'static str)],
        palette: Palette,
    ) -> u16 {
        self.buttons.clear();
        let mut column = area.x;
        let mut row = area.y;
        for (action, label) in actions {
            let text = format!("[{label}]");
            let width = text.len() as u16;
            if width > area.right().saturating_sub(column) {
                if row + 1 == area.bottom() {
                    break;
                }
                row += 1;
                column = area.x;
            }
            if width > area.width {
                continue;
            }
            let rectangle = Rect::new(column, row, width, 1);
            let style = if self.hovered_button == Some(*action) {
                palette.selected_style()
            } else {
                palette.accent_style()
            };
            frame.render_widget(Paragraph::new(text).style(style), rectangle);
            self.buttons.push((rectangle, *action));
            column = column.saturating_add(width + 1);
        }
        row - area.y + 1
    }

    /// The preview's status: the row it shows, and the address that row ends in once ready.
    fn preview_status(&self) -> Option<(String, Option<String>)> {
        let preview = self.preview.as_ref()?;
        Some(match &preview.phase {
            PreviewPhase::Preparing => (format!("{} · Building", preview.title), None),
            PreviewPhase::Ready { url } => {
                (format!("{} · Ready · ", preview.title), Some(url.clone()))
            }
            PreviewPhase::Failed { message } => {
                (format!("{} · Failed · {message}", preview.title), None)
            }
            PreviewPhase::Stopped => (format!("{} · Stopped", preview.title), None),
        })
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
        let visible_hit = self
            .page
            .pager
            .current_hit()
            .filter(|hit| self.page.pager.visible().contains(hit));
        if reflow {
            self.page = PageLayout::new(&self.document, usize::from(content.width), palette);
        }
        self.content = content;
        self.palette = palette;
        self.page.pager.resize(usize::from(content.height));
        if reflow {
            self.restore_position(position, focus);
        } else if changed && let Some(hit) = visible_hit {
            self.page.pager.move_to(hit);
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

    fn follow(&mut self, target: &LinkTarget) -> Step {
        self.hovered = None;
        self.mode = Mode::Reading;
        match target {
            LinkTarget::External(url) => self.notice = Some(url.clone()),
            LinkTarget::Page(id) => self.open(id, None),
            LinkTarget::PageAnchor(id, anchor) => self.open(id, Some(anchor)),
        }
        Step::Continue
    }

    fn open(&mut self, id: &PageId, anchor: Option<&Anchor>) {
        let document = if *id == self.document.id {
            Rc::clone(&self.document)
        } else {
            match (self.load)(id) {
                Ok(document) => Rc::new(document),
                Err(error) => {
                    tracing::debug!(?error, "could not open help page");
                    self.notice = Some(format!(
                        "could not open `{}`",
                        super::pages::label(id, self.language)
                    ));
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
        if document.id == self.document.id {
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
            super::pages::label(id, self.language)
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

    /// The content cell one screen position names, kept inside the content and the page.
    fn cell_at(&self, column: u16, row: u16) -> (usize, usize) {
        let line = self.page.pager.top() + usize::from(row.saturating_sub(self.content.y));
        let line = line.min(self.page.pager.line_count().saturating_sub(1));
        let column = usize::from(column.saturating_sub(self.content.x));
        (
            line,
            column.min(usize::from(self.content.width.saturating_sub(1))),
        )
    }

    /// The content cell one screen position names, kept inside the content's rows.
    fn clamped_cell(&self, column: u16, row: u16) -> (usize, usize) {
        let row = row.clamp(self.content.y, self.content.bottom().saturating_sub(1));
        self.cell_at(column, row)
    }

    /// Copies the selected cells and tells the reader what was taken.
    fn copy_selection(&mut self) {
        let Some(selection) = self.selection else {
            return;
        };
        let text = self.selected_text(selection);
        if text.is_empty() {
            self.notice = Some("nothing selected".to_owned());
            return;
        }
        let lines = text.lines().count();
        let taken = self
            .clipboard
            .is_some_and(|clipboard| clipboard.copy(&text));
        self.notice = Some(match (taken, lines) {
            (false, _) => format!("could not copy {lines} lines"),
            (true, 1) => "copied 1 line".to_owned(),
            (true, lines) => format!("copied {lines} lines"),
        });
    }

    /// The text the selected cells show, one source line at a time.
    fn selected_text(&self, selection: Selection) -> String {
        let ((from_line, from_column), (to_line, to_column)) = selection.ordered();
        let mut text = String::new();
        for line in from_line..=to_line {
            let Some(source) = self.page.pager.text(line) else {
                continue;
            };
            let from = if line == from_line { from_column } else { 0 };
            let to = if line == to_line {
                to_column
            } else {
                usize::MAX
            };
            text.push_str(&slice_columns(&source, from, to));
            if line != to_line {
                text.push('\n');
            }
        }
        text
    }

    /// Marks the selected cells, so the reader sees what a release copies.
    fn draw_selection(&self, frame: &mut Frame) {
        let Some(selection) = self.selection else {
            return;
        };
        let ((from_line, from_column), (to_line, to_column)) = selection.ordered();
        let top = self.page.pager.top();
        for line in self.page.pager.visible() {
            if !(from_line..=to_line).contains(&line) {
                continue;
            }
            let y = self.content.y + (line - top) as u16;
            let from = if line == from_line { from_column } else { 0 };
            let to = if line == to_line {
                to_column
            } else {
                usize::from(self.content.width)
            };
            for column in from..to {
                let x = self.content.x + u16::try_from(column).unwrap_or(u16::MAX);
                if x < self.content.right() {
                    frame.buffer_mut()[(x, y)].set_style(self.palette.selection_style());
                }
            }
        }
    }

    fn status(&self) -> String {
        let mut status = format!(
            "{} - {}/{} lines",
            super::pages::label(&self.document.id, self.language),
            (self.page.pager.top() + 1).min(self.page.pager.line_count()),
            self.page.pager.line_count()
        );
        let query = self.active_query();
        if !query.is_empty() {
            status = format!("{} · {status}", self.search_status());
            let _ = write!(status, " · /{query}");
        }
        status
    }

    fn search_status(&self) -> String {
        match self.page.pager.search_position() {
            Some((_, 0)) => "no matches".to_owned(),
            Some((current, total)) => format!("{current}/{total} matches"),
            None => String::new(),
        }
    }

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
        self.poll_preview();
        let mut area = frame.area();
        let actions = self.demo_actions();
        self.buttons.clear();
        if !actions.is_empty() && area.height > 3 {
            let rows = self.draw_buttons(
                frame,
                Rect {
                    height: 2.min(area.height - 3),
                    ..area
                },
                &actions,
                palette,
            );
            area.y += rows;
            area.height -= rows;
        }
        let (content, mut footer) = split_footer(area, &self.mode);
        if let Some((text, address)) = self.preview_status()
            && content.height > 1
        {
            let status = Rect::new(content.x, content.bottom() - 1, content.width, 1);
            let content = Rect {
                height: content.height - 1,
                ..content
            };
            self.resize(content, palette);
            let mut spans = vec![Span::styled(text, palette.notice_style())];
            if let Some(address) = address {
                spans.push(Span::styled(address, palette.link_style()));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), status);
        } else {
            self.resize(content, palette);
        }
        footer.y = self.content.bottom() + u16::from(self.preview.is_some() && content.height > 1);
        self.page.pager.draw(frame, self.content, palette);
        self.draw_selection(frame);
        self.draw_hover(frame);
        if let Mode::Jump(jump) = &mut self.mode
            && !jump.draw(frame, self.content, self.page.pager.top(), palette)
        {
            self.mode = Mode::Reading;
            self.notice = Some("no links on screen".to_owned());
        }
        if let Mode::Destination { line, .. } = &self.mode {
            let caption = self.destination_caption();
            line.draw(
                frame,
                Rect {
                    height: footer.height.min(1),
                    ..footer
                },
                palette,
            );
            hints::draw(
                frame,
                Rect {
                    y: footer.y + footer.height.min(1),
                    height: footer.height.saturating_sub(1),
                    ..footer
                },
                self.notice.as_deref().unwrap_or(&caption),
                &self.live(),
                self.bindings(),
                palette,
            );
        } else if let Mode::Search(search) = &self.mode {
            let status = self.search_status();
            let status_width = status.len() as u16;
            let feedback =
                footer.height > 1 && !status.is_empty() && footer.width >= status_width + 12;
            search.line.draw(
                frame,
                Rect {
                    height: footer.height.min(1),
                    width: if feedback {
                        footer.width - status_width - 2
                    } else {
                        footer.width
                    },
                    ..footer
                },
                palette,
            );
            if feedback {
                frame.render_widget(
                    Paragraph::new(status).style(palette.notice_style()),
                    Rect::new(
                        footer.right() - status_width,
                        footer.y,
                        status_width,
                        footer.height.min(1),
                    ),
                );
            }
            hints::draw(
                frame,
                Rect {
                    y: footer.y + footer.height.min(1),
                    height: footer.height.saturating_sub(1),
                    ..footer
                },
                "",
                &self.live(),
                self.bindings(),
                palette,
            );
        } else {
            let caption = self.caption();
            let status = self.status();
            let line = self
                .notice
                .as_deref()
                .or(caption.as_deref())
                .unwrap_or(&status);
            hints::draw(frame, footer, line, &self.live(), self.bindings(), palette);
        }
    }

    fn answer(&mut self, action: Action) -> Step {
        if matches!(self.mode, Mode::Search(_) | Mode::Destination { .. })
            && !matches!(action, Action::Open | Action::Dismiss | Action::Quit)
        {
            return Step::Continue;
        }
        self.notice = None;
        match action {
            Action::Quit => return Step::Done,
            Action::OpenBrowser => {
                if let Some(preview) = &self.preview
                    && let PreviewPhase::Ready { url } = &preview.phase
                {
                    return self.request(ReaderAction::OpenBrowser { url: url.clone() });
                }
            }
            Action::Open if matches!(self.mode, Mode::Search(_)) => self.finish_search(true),
            Action::Open if matches!(self.mode, Mode::Destination { .. }) => {
                return self.finish_export();
            }
            Action::Preview => {
                if let Some(demo) = self.document.id.demo() {
                    return self.request(ReaderAction::Preview { demo: demo.into() });
                }
            }
            Action::StopPreview if self.preview.is_some() => {
                return self.request(ReaderAction::StopPreview);
            }
            Action::Export => self.begin_export(false),
            Action::ExportAndEdit => self.begin_export(true),
            Action::Dismiss => match self.mode {
                Mode::Search(_) => self.finish_search(false),
                Mode::Jump(_) | Mode::Destination { .. } => self.mode = Mode::Reading,
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
                Mode::Search(_) | Mode::Destination { .. } => {}
            },
            _ => {}
        }
        Step::Continue
    }

    fn key(&mut self, key: &KeyEvent) -> Step {
        self.notice = None;
        // The destination line completes from the names under it; every other key edits it.
        if matches!(self.mode, Mode::Destination { .. })
            && self.bindings().action(key) == Some(Action::Complete)
        {
            self.complete_destination();
            return Step::Continue;
        }
        match &mut self.mode {
            Mode::Jump(jump) => {
                if keymap::HELP_JUMP.action(key) == Some(Action::Dismiss) {
                    self.mode = Mode::Reading;
                } else if let KeyCode::Char(character) = key.code
                    && key.modifiers.difference(KeyModifiers::SHIFT).is_empty()
                    && let Some(target) = jump.key(character)
                {
                    return self.follow(&target);
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
            Mode::Destination { line, .. } => match line.key(key) {
                Reply::Editing => Step::Continue,
                Reply::Dismissed => {
                    self.mode = Mode::Reading;
                    Step::Continue
                }
                Reply::Accepted => self.finish_export(),
            },
            Mode::Reading => answer_key(self, key),
        }
    }

    fn paste(&mut self, text: &str) -> Step {
        match &mut self.mode {
            Mode::Search(search) => {
                search.line.paste(text);
                self.edit_search();
            }
            Mode::Destination { line, .. } => line.paste(text),
            _ => {}
        }
        Step::Continue
    }

    /// Answers one pointer event: the jump overlay opens what the pointer rests on, and the
    /// text prompts leave the pointer to the terminal.
    /// Answers one pointer event: a drag selects the text under it and copies it when the
    /// pointer is released, a press that does not move opens what it landed on, and a text
    /// prompt leaves the pointer to the terminal.
    fn pointer(&mut self, event: MouseEvent) -> Step {
        if !self.mouse || matches!(self.mode, Mode::Search(_) | Mode::Destination { .. }) {
            return Step::Continue;
        }
        match event.kind {
            MouseEventKind::Moved => {
                self.hovered_button = self
                    .buttons
                    .iter()
                    .find(|(area, _)| area.contains((event.column, event.row).into()))
                    .map(|(_, action)| *action);
                self.hovered = self.link_at(event.column, event.row).map(|link| link.id);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(action) = self
                    .buttons
                    .iter()
                    .find(|(area, _)| area.contains((event.column, event.row).into()))
                    .map(|(_, action)| *action)
                {
                    return self.answer(action);
                }
                if self.content.contains((event.column, event.row).into()) {
                    self.selection = Some(Selection::at(self.cell_at(event.column, event.row)));
                    self.pressed = Some((event.column, event.row));
                    self.dragging = false;
                    self.hovered = None;
                    self.hovered_button = None;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.selection.is_some() {
                    let cell = self.clamped_cell(event.column, event.row);
                    self.dragging = true;
                    if let Some(selection) = &mut self.selection {
                        selection.extend(cell);
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let pressed = self.pressed.take();
                if std::mem::take(&mut self.dragging) {
                    self.copy_selection();
                } else if let Some((column, row)) = pressed
                    && let Some(target) = self.link_at(column, row).map(|link| link.target.clone())
                {
                    self.notice = None;
                    return self.follow(&target);
                }
            }
            _ => {}
        }
        Step::Continue
    }

    fn bindings(&self) -> &'static keymap::Table {
        match self.mode {
            Mode::Search(_) => &keymap::TEXT_INPUT,
            Mode::Jump(_) => &keymap::HELP_JUMP,
            Mode::Destination { .. } => &keymap::HELP_EXPORT,
            Mode::Reading => &keymap::HELP,
        }
    }

    fn title(&self) -> Option<String> {
        Some(format!(
            "tola help - {}",
            super::pages::label(&self.document.id, self.language)
        ))
    }

    fn wants_mouse(&self) -> bool {
        self.mouse
    }

    fn live(&self) -> Vec<Action> {
        match &self.mode {
            Mode::Search(_) => return vec![Action::Open, Action::Dismiss],
            Mode::Destination { .. } => {
                return vec![Action::Open, Action::Complete, Action::Dismiss];
            }
            Mode::Jump(_) => return vec![Action::Dismiss],
            Mode::Reading => {}
        }
        let mut live = ACCEPTED.to_vec();
        live.retain(|action| self.scroll_changes(*action));
        if !self.page.links.iter().any(|link| {
            self.page.pager.visible().contains(&link.line)
                && link.column < usize::from(self.content.width)
                && link.width > 0
        }) {
            live.retain(|action| *action != Action::Label);
        }
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
        if self.document.id.demo().is_some() {
            live.extend([Action::Preview, Action::Export, Action::ExportAndEdit]);
        }
        if let Some(preview) = &self.preview {
            if !matches!(preview.phase, PreviewPhase::Stopped) {
                live.push(Action::StopPreview);
            }
            if matches!(preview.phase, PreviewPhase::Ready { .. }) {
                live.push(Action::OpenBrowser);
            }
        }
        live
    }

    fn idle(&mut self) -> bool {
        self.poll_preview()
    }

    fn caption(&self) -> Option<String> {
        match &self.mode {
            Mode::Jump(jump) => Some(jump.caption()),
            _ => None,
        }
    }
}

/// The directory to read and the name prefix to match, for the destination being typed.
fn completion_parts(typed: &str) -> (PathBuf, String) {
    if typed.is_empty() || typed.ends_with('/') || typed.ends_with(std::path::MAIN_SEPARATOR) {
        let directory = if typed.is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(typed)
        };
        return (directory, String::new());
    }
    let path = Path::new(typed);
    let prefix = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    (directory, prefix)
}

/// The longest name every match starts with.
fn shared_prefix(names: &[String]) -> String {
    let Some(first) = names.first() else {
        return String::new();
    };
    let mut end = first.len();
    for name in &names[1..] {
        end = first[..end]
            .char_indices()
            .zip(name.chars())
            .take_while(|((_, left), right)| left == right)
            .map(|((offset, left), _)| offset + left.len_utf8())
            .last()
            .unwrap_or(0);
        if end == 0 {
            break;
        }
    }
    first[..end].to_owned()
}

/// The matches one notice names: at most four of them, then how many follow.
fn listed(matches: &[String]) -> String {
    let (shown, rest) = matches.split_at(matches.len().min(4));
    if rest.is_empty() {
        shown.join(" ")
    } else {
        format!("{} +{}", shown.join(" "), rest.len())
    }
}

fn split_footer(area: Rect, mode: &Mode) -> (Rect, Rect) {
    let height = match mode {
        // A destination line, the path it names, and the keys it answers.
        Mode::Destination { .. } => area.height.saturating_sub(1).min(3),
        Mode::Search(_) => area.height.min(2),
        _ => area.height.saturating_sub(1).min(2),
    };
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
    use std::cell::RefCell;

    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;
    use ratatui::{Terminal, TerminalOptions, Viewport};

    use super::*;
    use crate::help::model::{HelpPage, anchor};

    fn reader<'a>(
        document: HelpDocument,
        load: &'a dyn Fn(&PageId) -> Result<HelpDocument>,
    ) -> View<'a> {
        View::new(document, load, 80, Palette::new(false))
    }

    fn document(markdown: &str) -> HelpDocument {
        HelpDocument::parse(HelpPage::new(PageId::Overview, markdown.to_owned()))
    }

    fn linked() -> HelpDocument {
        document(
            "# Index\n\nRead [the whole manual](tola-help://packages/schema).\n\n## Details\n\nMore text.\n\n## Tail\n\nEnd.\n",
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
            name: "schema".to_owned(),
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
        let mut view = reader(linked(), &load);
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
            let mut view = reader(linked(), &load);
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
        let mut view = reader(
            document(
                "# A title that wraps across several lines\n\nfirst [go](tola-help://packages/schema)\n\nmore content\n",
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
        let mut view = reader(linked(), &load);
        frame(&mut view, Rect::new(0, 0, 60, 10));
        press(&mut view, KeyCode::Tab);
        frame(&mut view, Rect::new(0, 0, 60, 3));
        assert!(matches!(view.mode, Mode::Reading));
        press(&mut view, KeyCode::Char('a'));
        assert_eq!(view.document.id, PageId::Overview);
        frame(&mut view, Rect::new(0, 0, 60, 2));
        assert!(!view.page.pager.visible().is_empty());
        assert!(!view.live().contains(&Action::Label));
        press(&mut view, KeyCode::Tab);
        assert!(matches!(view.mode, Mode::Reading));
    }

    #[test]
    fn scrolling_dismisses_jump_labels() {
        let mut view = reader(linked(), &load);
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
        let mut view = reader(linked(), &load);
        let area = Rect::new(0, 0, 60, 3);
        frame(&mut view, area);
        view.follow(&LinkTarget::PageAnchor(PageId::Overview, anchor("Details")));
        assert_eq!(row(&frame(&mut view, area), area, 0), "Details");
        assert!(!view.history.has_back());
        assert_eq!(view.answer(Action::Dismiss), Step::Cancel);
    }

    #[test]
    fn back_returns_to_the_page_that_was_left() {
        let mut view = reader(linked(), &load);
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
        let mut view = reader(HelpDocument::parse(page), &load);
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
        let mut view = reader(linked(), &load);
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
        let mut view = reader(document(&format!("# Index\n\n{words}\n\nTail.\n")), &load);
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
        let mut view = reader(linked(), &load);
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
        let mut view = reader(linked(), &load);
        frame(&mut view, Rect::new(0, 0, 60, 6));
        view.follow(&destination());
        view.answer(Action::Back);
        let origin = view.position();
        for id in [
            PageId::Overview,
            PageId::Package {
                name: "schema".to_owned(),
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
        let mut view = reader(linked(), &failed);
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
        let mut view = reader(search_page(), &load);
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
    fn search_reports_match_position() {
        let mut view = reader(search_page(), &load);
        let area = Rect::new(0, 0, 50, 6);
        frame(&mut view, area);
        press(&mut view, KeyCode::Char('/'));
        view.paste("absent");
        let shown = frame(&mut view, area);
        assert!(row(&shown, area, view.content.bottom()).contains("no matches"));
        assert_eq!(view.live(), [Action::Open, Action::Dismiss]);
        assert_eq!(
            view.bindings().action(&KeyEvent::from(KeyCode::Enter)),
            Some(Action::Open)
        );
        press(&mut view, KeyCode::Enter);
        assert!(view.status().contains("no matches"));
        search(&mut view, "needle");
        assert_eq!(view.page.pager.search_position(), Some((1, 2)));
        press(&mut view, KeyCode::Char('n'));
        assert_eq!(view.page.pager.search_position(), Some((2, 2)));
        search(&mut view, "");
        assert_eq!(view.page.pager.search_position(), None);
        assert!(!view.status().contains("matches"));
    }

    #[test]
    fn tiny_viewport_keeps_document_content() {
        for height in [1, 2] {
            let mut view = reader(document("visible documentation"), &load);
            let area = Rect::new(3, 2, 40, height);
            let shown = frame(&mut view, area);
            assert!(!view.page.pager.visible().is_empty());
            assert!(row(&shown, area, view.content.y).contains("visible documentation"));
            assert_eq!(press(&mut view, KeyCode::Char('q')), Step::Done);
        }
    }

    #[test]
    fn tiny_search_keeps_text_editable() {
        for height in [1, 2] {
            let mut view = reader(search_page(), &load);
            let area = Rect::new(3, 2, 40, height);
            frame(&mut view, area);
            press(&mut view, KeyCode::Char('/'));
            view.paste("needle");
            let shown = frame(&mut view, area);
            assert!(row(&shown, area, area.y).contains("needle"));
            press(&mut view, KeyCode::Enter);
            assert_eq!(view.query, "needle");
            assert!(matches!(view.mode, Mode::Reading));
            press(&mut view, KeyCode::Char('/'));
            view.paste("absent");
            frame(&mut view, area);
            press(&mut view, KeyCode::Esc);
            assert_eq!(view.query, "needle");
            assert!(matches!(view.mode, Mode::Reading));
        }
    }

    #[test]
    fn search_ignores_scroll_actions() {
        let mut view = reader(search_page(), &load);
        frame(&mut view, Rect::new(0, 0, 30, 5));
        press(&mut view, KeyCode::Char('/'));
        view.paste("second needle");
        let position = view.position();
        let focus = view.focus();
        for action in [Action::Up, Action::Down, Action::Back, Action::NextSection] {
            view.answer(action);
            assert!(matches!(view.mode, Mode::Search(_)));
            assert_eq!(view.position(), position);
            assert_eq!(view.focus(), focus);
        }
        assert_eq!(view.active_query(), "second needle");
    }

    #[test]
    fn draft_search_survives_reflow() {
        let mut view = reader(search_page(), &load);
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
        let mut view = reader(search_page(), &load);
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
        let mut view = reader(search_page(), &load);
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
        let mut view = reader(search_page(), &load);
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
        let mut view = reader(linked(), &load);
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
        let mut view = reader(
            document(
                "[one two three four five six](tola-help://packages/schema) [other](https://example.com)\n",
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
        view.pointer(mouse(MouseEventKind::Up(MouseButton::Left), column, y));
        assert_eq!(LinkTarget::Page(view.document.id.clone()), destination());
    }

    #[test]
    fn search_movement_clears_hover() {
        let mut view = reader(linked(), &load);
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
        let mut view = reader(linked(), &load);
        frame(&mut view, Rect::new(0, 0, 60, 10));
        let (column, line) = {
            let link = &view.page.links[0];
            (link.column as u16, link.line as u16)
        };
        let click = |kind| mouse(kind, column, line);
        view.pointer(click(MouseEventKind::Down(MouseButton::Left)));
        view.pointer(click(MouseEventKind::Up(MouseButton::Left)));
        assert_eq!(view.document.id, PageId::Overview);
        view.set_mouse(true);
        press(&mut view, KeyCode::Char('/'));
        view.pointer(click(MouseEventKind::Down(MouseButton::Left)));
        view.pointer(click(MouseEventKind::Up(MouseButton::Left)));
        assert!(matches!(view.mode, Mode::Search(_)));
        assert_eq!(view.document.id, PageId::Overview);
        press(&mut view, KeyCode::Esc);
        view.pointer(click(MouseEventKind::Down(MouseButton::Left)));
        view.pointer(click(MouseEventKind::Up(MouseButton::Left)));
        assert_eq!(LinkTarget::Page(view.document.id.clone()), destination());
    }

    /// The clipboard a test reader copies into.
    #[derive(Default)]
    struct FakeClipboard {
        copied: RefCell<Vec<String>>,
    }

    impl Clipboard for FakeClipboard {
        fn copy(&self, text: &str) -> bool {
            self.copied.borrow_mut().push(text.to_owned());
            true
        }
    }

    #[test]
    fn dragging_copies_the_selected_text() {
        let mut view = reader(linked(), &load);
        view.set_mouse(true);
        let clipboard = FakeClipboard::default();
        view.set_clipboard(&clipboard);
        frame(&mut view, Rect::new(0, 0, 60, 12));

        view.pointer(mouse(MouseEventKind::Down(MouseButton::Left), 0, 2));
        view.pointer(mouse(MouseEventKind::Drag(MouseButton::Left), 7, 4));
        view.pointer(mouse(MouseEventKind::Up(MouseButton::Left), 7, 4));

        assert_eq!(
            clipboard.copied.borrow().as_slice(),
            ["Read the whole manual.\n\nDetails"],
            "the drag copies the text the selected cells show"
        );
        assert_eq!(view.notice(), Some("copied 3 lines"));
    }

    #[test]
    fn press_without_drag_copies_nothing() {
        let mut view = reader(linked(), &load);
        view.set_mouse(true);
        let clipboard = FakeClipboard::default();
        view.set_clipboard(&clipboard);
        frame(&mut view, Rect::new(0, 0, 60, 12));

        let link = view.page.links[0].clone();
        let (column, line) = (link.column as u16, link.line as u16);
        view.pointer(mouse(MouseEventKind::Down(MouseButton::Left), column, line));
        view.pointer(mouse(MouseEventKind::Up(MouseButton::Left), column, line));

        assert!(
            clipboard.copied.borrow().is_empty(),
            "a click copies nothing"
        );
        assert_eq!(
            view.document.id,
            PageId::Package {
                name: "schema".to_owned()
            },
            "it opened the link instead"
        );
    }

    #[test]
    fn open_browser_requests_address() {
        let mut view = reader(demo_document(), &load);
        assert!(!view.live().contains(&Action::OpenBrowser));
        view.preview = ready_preview();
        assert!(view.live().contains(&Action::OpenBrowser));
        assert_eq!(view.answer(Action::OpenBrowser), Step::Done);
        assert_eq!(
            view.take_action(),
            Some(ReaderAction::OpenBrowser {
                url: "http://127.0.0.1:1234/".into()
            })
        );
    }

    #[test]
    fn dragged_cells_are_marked() {
        let mut view = reader(linked(), &load);
        view.set_mouse(true);
        frame(&mut view, Rect::new(0, 0, 60, 12));

        view.pointer(mouse(MouseEventKind::Down(MouseButton::Left), 0, 2));
        view.pointer(mouse(MouseEventKind::Drag(MouseButton::Left), 7, 4));
        let shown = frame(&mut view, Rect::new(0, 0, 60, 12));

        // The mark is added to what the cell already shows, as a terminal's own selection is.
        let reversed = Modifier::REVERSED;
        assert!(
            shown[(3, 2)].modifier.contains(reversed),
            "the first line is marked"
        );
        assert!(
            shown[(3, 4)].modifier.contains(reversed),
            "the line reached is marked"
        );
        assert!(
            !shown[(20, 4)].modifier.contains(reversed),
            "the last line stops at the pointer"
        );
    }

    #[test]
    fn external_link_shows_address() {
        let mut view = reader(
            document("[read manual](https://example.com/manual)\n"),
            &load,
        );
        let area = Rect::new(0, 0, 40, 6);
        frame(&mut view, area);
        press(&mut view, KeyCode::Tab);
        frame(&mut view, area);
        press(&mut view, KeyCode::Char('a'));
        assert_eq!(view.notice(), Some("https://example.com/manual"));
        assert!(view.take_action().is_none());
        assert!(!view.history.has_back());
    }

    #[test]
    fn scrolling_hints_follow_the_page() {
        let short = document("# Index\n\nOne line.\n");
        let mut view = reader(short, &load);
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
        let mut view = reader(document(&format!("# Index\n\n{paragraphs}")), &load);
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

    #[test]
    fn jump_hints_require_visible_links() {
        for markdown in ["", "# Index\n\nOne line.\n"] {
            let mut view = reader(document(markdown), &load);
            frame(&mut view, Rect::new(0, 0, 40, 6));
            assert!(!view.live().contains(&Action::Label));
        }
        let mut view = reader(linked(), &load);
        frame(&mut view, Rect::new(0, 0, 40, 10));
        assert!(view.live().contains(&Action::Label));
        press(&mut view, KeyCode::Tab);
        assert_eq!(
            view.bindings().action(&KeyEvent::from(KeyCode::Tab)),
            Some(Action::Dismiss)
        );
        frame(&mut view, Rect::new(0, 0, 40, 2));
        assert!(!view.live().contains(&Action::Label));
    }

    fn demo_document() -> HelpDocument {
        HelpDocument::parse(HelpPage::new(
            PageId::Demo {
                id: "backlinks".into(),
            },
            format!("# Backlinks\n\n{}", "A useful paragraph.\n\n".repeat(12)),
        ))
    }

    fn ready_preview() -> Option<PreviewState> {
        Some(PreviewState {
            demo: "backlinks".into(),
            title: "Backlinks".into(),
            phase: PreviewPhase::Ready {
                url: "http://127.0.0.1:1234/".into(),
            },
        })
    }

    #[test]
    fn source_navigation_starts_no_operations() {
        let mut view = reader(demo_document(), &load);
        frame(&mut view, Rect::new(0, 0, 80, 10));
        let file = PageId::DemoFile {
            id: "backlinks".into(),
            path: "site/page.typ".into(),
        };
        assert_eq!(view.follow(&LinkTarget::Page(file.clone())), Step::Continue);
        assert_eq!(view.document.id, file);
        assert!(view.take_action().is_none());
        view.answer(Action::Back);
        assert_eq!(view.document.id.demo(), Some("backlinks"));
        assert!(view.take_action().is_none());
    }

    #[test]
    fn export_requires_entered_directory() {
        for edit in [false, true] {
            let mut view = reader(demo_document(), &load);
            frame(&mut view, Rect::new(0, 0, 80, 10));
            view.answer(if edit {
                Action::ExportAndEdit
            } else {
                Action::Export
            });
            assert_eq!(press(&mut view, KeyCode::Enter), Step::Continue);
            assert!(view.take_action().is_none());
            view.paste("my demo");
            assert!(view.take_action().is_none());
            assert_eq!(press(&mut view, KeyCode::Enter), Step::Done);
            assert_eq!(
                view.take_action(),
                Some(ReaderAction::Export {
                    demo: "backlinks".into(),
                    destination: PathBuf::from("my demo"),
                    edit
                })
            );
        }
    }

    #[test]
    fn export_cancel_keeps_the_document() {
        let mut view = reader(demo_document(), &load);
        frame(&mut view, Rect::new(0, 0, 80, 10));
        view.page.pager.set_top(4);
        let position = view.position();
        view.answer(Action::Export);
        view.paste("discarded directory");
        press(&mut view, KeyCode::Esc);
        assert_eq!(view.position(), position);
        assert!(view.take_action().is_none());
        assert!(matches!(view.mode, Mode::Reading));
    }

    #[test]
    fn buttons_match_keyboard_operations() {
        for (action, key) in [(Action::Preview, 'p'), (Action::StopPreview, 'x')] {
            let mut keyboard = reader(demo_document(), &load);
            keyboard.set_preview(&ready_preview);
            frame(&mut keyboard, Rect::new(0, 0, 100, 12));
            let key_step = press(&mut keyboard, KeyCode::Char(key));

            let mut pointer = reader(demo_document(), &load);
            pointer.set_mouse(true);
            pointer.set_preview(&ready_preview);
            frame(&mut pointer, Rect::new(0, 0, 100, 12));
            let button = pointer
                .buttons
                .iter()
                .find(|(_, candidate)| *candidate == action)
                .unwrap()
                .0;
            let click_step = pointer.pointer(mouse(
                MouseEventKind::Down(MouseButton::Left),
                button.x,
                button.y,
            ));
            assert_eq!(click_step, key_step);
            assert_eq!(pointer.take_action(), keyboard.take_action());
            assert_eq!(pointer.document.id, keyboard.document.id);
        }
    }

    #[test]
    fn stop_requires_running_preview() {
        let mut view = reader(demo_document(), &load);
        view.preview = ready_preview();
        assert!(view.live().contains(&Action::StopPreview));
        view.preview.as_mut().unwrap().phase = PreviewPhase::Stopped;
        assert!(!view.live().contains(&Action::StopPreview));
    }

    #[test]
    fn ready_address_is_underlined() {
        let area = Rect::new(0, 0, 80, 10);
        let url = "http://127.0.0.1:1234/";
        let mut view = reader(demo_document(), &load);
        view.preview = ready_preview();
        let buffer = frame(&mut view, area);
        let status = (0..area.height)
            .find(|line| row(&buffer, area, *line).contains(url))
            .expect("the status row names the preview address");
        assert!(
            (0..area.width).all(|column| !buffer[(column, status)].symbol().contains("]8;;")),
            "the address is plain text, so selecting it copies what it shows"
        );
        let underlined = (0..area.width)
            .filter(|column| {
                buffer[(*column, status)]
                    .modifier
                    .contains(Modifier::UNDERLINED)
            })
            .count();
        assert_eq!(
            underlined,
            url.chars().count(),
            "every address character is underlined"
        );
    }

    #[test]
    fn preview_changes_keep_the_reading_position() {
        let preview = std::cell::RefCell::new(Some(PreviewState {
            demo: "backlinks".into(),
            title: "Backlinks".into(),
            phase: PreviewPhase::Preparing,
        }));
        let poll = || preview.borrow().clone();
        let mut view = reader(demo_document(), &load);
        view.set_preview(&poll);
        frame(&mut view, Rect::new(0, 0, 80, 10));
        view.page.pager.set_top(4);
        let position = view.position();
        assert!(!view.idle());
        *preview.borrow_mut() = ready_preview();
        assert!(view.idle());
        assert_eq!(view.position(), position);
        assert!(!view.idle());
        view.follow(&destination());
        assert_eq!(view.preview, ready_preview());
        view.answer(Action::Back);
        assert_eq!(view.preview, ready_preview());
        assert_eq!(view.position(), position);
        assert!(view.take_action().is_none());
    }

    #[test]
    fn accepted_search_keeps_its_hit_visible() {
        let markdown = format!(
            "# Backlinks\n\n{}\nsite/backlinks.typ\n\n{}",
            "Before the file.\n\n".repeat(12),
            "After the file.\n\n".repeat(12)
        );
        let document = HelpDocument::parse(HelpPage::new(
            PageId::Demo {
                id: "backlinks".into(),
            },
            markdown,
        ));
        let mut view = reader(document, &load);
        let area = Rect::new(0, 0, 80, 10);
        frame(&mut view, area);
        press(&mut view, KeyCode::Char('/'));
        frame(&mut view, area);
        view.paste("site/backlinks.typ");
        frame(&mut view, area);
        let hit = view.page.pager.current_hit().unwrap();
        assert!(view.page.pager.visible().contains(&hit));
        press(&mut view, KeyCode::Enter);
        frame(&mut view, area);
        assert_eq!(view.page.pager.current_hit(), Some(hit));
        assert!(view.page.pager.visible().contains(&hit));
        assert_eq!(view.query, "site/backlinks.typ");
    }

    /// The export prompt's filesystem: the names a test offers, resolved under `/site`.
    struct FakeDestinations {
        names: &'static [&'static str],
    }

    impl DestinationSource for FakeDestinations {
        fn directories(&self, directory: &Path, prefix: &str) -> Vec<String> {
            assert_eq!(directory, Path::new("."), "the test types a bare name");
            self.names
                .iter()
                .filter(|name| name.starts_with(prefix))
                .map(|name| (*name).to_owned())
                .collect()
        }

        fn resolve(&self, typed: &Path) -> PathBuf {
            Path::new("/site").join(typed)
        }

        fn base(&self) -> PathBuf {
            PathBuf::from("/site")
        }
    }

    fn demo_reader() -> HelpDocument {
        HelpDocument::parse(HelpPage::new(
            PageId::Demo {
                id: "backlinks".into(),
            },
            "# Demo\n\nText.\n".to_owned(),
        ))
    }

    #[test]
    fn tab_completes_the_export_destination() {
        let mut view = reader(demo_reader(), &load);
        view.set_destinations(&FakeDestinations {
            names: &["site", "static", "stash"],
        });

        assert_eq!(view.answer(Action::Export), Step::Continue);
        assert_eq!(
            view.destination_caption(),
            "relative to /site",
            "an empty line says where a relative destination starts"
        );

        press(&mut view, KeyCode::Char('s'));
        press(&mut view, KeyCode::Tab);
        assert_eq!(
            view.notice(),
            Some("matches site static stash"),
            "matches that share nothing more than the typed name are named"
        );
        press(&mut view, KeyCode::Char('t'));
        press(&mut view, KeyCode::Tab);
        assert_eq!(
            view.destination_caption(),
            "writes to /site/sta",
            "the shared prefix extends the destination"
        );
        press(&mut view, KeyCode::Char('t'));
        press(&mut view, KeyCode::Tab);
        assert_eq!(view.destination_caption(), "writes to /site/static/");

        press(&mut view, KeyCode::Enter);
        assert_eq!(
            view.take_action(),
            Some(ReaderAction::Export {
                demo: "backlinks".to_owned(),
                destination: PathBuf::from("static/"),
                edit: false,
            })
        );
    }

    #[test]
    fn jump_overlay_keeps_the_pointer_working() {
        let mut view = reader(linked(), &load);
        view.set_mouse(true);
        frame(&mut view, Rect::new(0, 0, 60, 12));
        press(&mut view, KeyCode::Tab);
        assert!(matches!(view.mode, Mode::Jump(_)), "Tab opens the overlay");

        let link = view.page.links[0].clone();
        let (column, line) = (link.column as u16, link.line as u16);
        view.pointer(mouse(MouseEventKind::Moved, column, line));
        assert_eq!(
            view.hovered,
            Some(link.id),
            "hover marks the link under the pointer"
        );
        view.pointer(mouse(MouseEventKind::Down(MouseButton::Left), column, line));
        view.pointer(mouse(MouseEventKind::Up(MouseButton::Left), column, line));
        assert!(
            matches!(view.mode, Mode::Reading),
            "a click leaves the overlay"
        );
        assert_eq!(
            view.document.id,
            PageId::Package {
                name: "schema".to_owned()
            },
            "the click followed the link"
        );
    }
}
