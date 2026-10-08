//! The frame loop and the drawing vocabulary every interactive view shares.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use ratatui::backend::Backend;
use ratatui::layout::{Rect, Size};
use ratatui::{Frame, TerminalOptions, Viewport};

use super::prompt::InputCancelled;
use super::session::{self, Terminal};
use super::style::Palette;

pub(crate) mod filter;
pub(crate) mod hints;
pub(crate) mod keymap;
pub(crate) mod pager;
pub(crate) mod table;

/// Cancellation is checked between event polls; unchanged surfaces need no redraw.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// What the frame loop does after one action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Draw the next frame.
    Continue,
    /// End the view normally.
    Done,
    /// End the view as cancelled, which the command reports as an interrupted run.
    Cancel,
}

/// One interactive screen: what a frame shows and how it answers an action.
pub(crate) trait Surface {
    /// Draws the whole frame.
    fn draw(&mut self, frame: &mut Frame, palette: Palette);
    /// Answers one action this screen accepts; an action it does not know leaves it unchanged.
    fn answer(&mut self, action: Action) -> Step;
    /// The keys this screen binds, and the hints they spell.
    fn bindings(&self) -> &'static keymap::Table {
        &keymap::DEFAULT
    }
    /// Answers one key press: a screen that reads text consumes the key as text.
    fn key(&mut self, key: &KeyEvent) -> Step {
        answer_key(self, key)
    }
    /// Answers pasted text; only a screen that reads text has anything to paste.
    fn paste(&mut self, _text: &str) -> Step {
        Step::Continue
    }
    /// The actions this screen always accepts; the table it binds decides which keys reach them.
    fn actions(&self) -> &'static [Action] {
        &[]
    }
    /// The actions this screen accepts now: a screen whose set changes spells only these.
    ///
    /// The hint row spells only what is live, so a key that would do nothing is never shown.
    fn live(&self) -> Vec<Action> {
        self.actions().to_vec()
    }
    /// What the screen wants the reader told right now, above the hints.
    fn caption(&self) -> Option<String> {
        None
    }
    /// Whether the session captures the pointer for this screen: the wheel scrolls it.
    fn wants_mouse(&self) -> bool {
        false
    }
    /// Answers one pointer event, when the screen captures the pointer; the default ignores it.
    fn pointer(&mut self, _event: MouseEvent) -> Step {
        Step::Continue
    }
    /// The window title this screen wants while it is open, when it wants one.
    fn title(&self) -> Option<String> {
        None
    }
}

/// Answers one key press through the screen's own table.
pub(crate) fn answer_key(surface: &mut (impl Surface + ?Sized), key: &KeyEvent) -> Step {
    surface
        .bindings()
        .action(key)
        .map_or(Step::Continue, |action| surface.answer(action))
}

/// What one key press asks a screen to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Close the view.
    Quit,
    /// Leave the current place: close an overlay, or end the view when nothing is open.
    Dismiss,
    /// Show the next tab.
    NextTab,
    /// Show the previous tab.
    PreviousTab,
    /// Move up one row or line.
    Up,
    /// Move down one row or line.
    Down,
    /// Move up by one page.
    PageUp,
    /// Move down by one page.
    PageDown,
    /// Move up by half a page.
    HalfPageUp,
    NextSection,
    PreviousSection,
    /// Move down by half a page.
    HalfPageDown,
    /// Move to the first row or line.
    First,
    /// Move to the last row or line.
    Last,
    /// Open the filter or search line.
    Search,
    /// Move to the next search hit.
    NextMatch,
    /// Move to the previous search hit.
    PreviousMatch,
    /// Open the selected row, or follow the cross-reference on the current line.
    Open,
    /// Write the table's current rows to a file or standard output.
    Export,
    /// Apply the preset at this position of the preset list.
    ApplyPreset(u8),
    /// Select or deselect the row under the cursor.
    Toggle,
    /// Go back one place: the previous place the reader was at.
    Back,
    /// Go forward one place: back into the place the reader left with `Back`.
    Forward,
    /// Label the visible jump targets: the next key jumps to the target it labels.
    Label,
    /// Show the round before the one on screen.
    PreviousRound,
    /// Show the round after the one on screen.
    NextRound,
}

/// The frame loop of one interactive view.
pub(crate) struct Screen<B: Backend> {
    terminal: ratatui::Terminal<B>,
    /// The screen's area as the terminal reported it; a fullscreen view is resized to it.
    area: Rect,
    /// The title last written to the terminal, so it is only written when it changes.
    title: Option<String>,
}

impl<B: Backend> Screen<B> {
    pub(crate) fn new(backend: B, columns: u16, rows: u16) -> Result<Self, B::Error> {
        let area = Rect::new(0, 0, columns, rows);
        let terminal = ratatui::Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Fixed(area),
            },
        )?;
        Ok(Self {
            terminal,
            area,
            title: None,
        })
    }

    /// Cancellation is observed before each frame and between bounded event polls.
    pub(crate) fn run(
        &mut self,
        terminal: &mut dyn Terminal,
        surface: &mut impl Surface,
        palette: Palette,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()>
    where
        B::Error: std::error::Error + Send + Sync + 'static,
    {
        loop {
            if cancelled() {
                return Err(InputCancelled.into());
            }
            let title = surface.title();
            if title != self.title {
                terminal.set_title(title.as_deref().unwrap_or_default())?;
                self.title = title;
            }
            // The frame reaches the terminal whole, or not at all.
            terminal.begin_synchronized_update()?;
            let drawn = self.draw(terminal, surface, palette);
            terminal.end_synchronized_update()?;
            drawn?;
            let event = loop {
                if let Some(event) = session::next_event(terminal, POLL_INTERVAL, cancelled)? {
                    break event;
                }
            };
            let run = match event {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if cancels(&key) {
                        return Err(InputCancelled.into());
                    }
                    surface.key(&key)
                }
                Event::Paste(text) => surface.paste(&text),
                // The wheel scrolls the same line the arrow keys do; the rest reach the screen,
                // which acts on a click and ignores what it does not use.
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => surface.answer(Action::Up),
                    MouseEventKind::ScrollDown => surface.answer(Action::Down),
                    _ => surface.pointer(mouse),
                },
                // The next frame picks a resize up from the terminal's size.
                _ => Step::Continue,
            };
            match run {
                Step::Continue => {}
                Step::Done => return Ok(()),
                Step::Cancel => return Err(InputCancelled.into()),
            }
        }
    }

    /// Draws one frame; a window with no size draws nothing and keeps the screen as it is.
    fn draw(
        &mut self,
        terminal: &mut dyn Terminal,
        surface: &mut impl Surface,
        palette: Palette,
    ) -> Result<()>
    where
        B::Error: std::error::Error + Send + Sync + 'static,
    {
        let (columns, rows) = terminal.size()?;
        self.draw_frame(Size::new(columns, rows), |frame| {
            surface.draw(frame, palette)
        })?;
        Ok(())
    }

    pub(crate) fn draw_frame(
        &mut self,
        size: Size,
        draw: impl FnOnce(&mut Frame),
    ) -> Result<(), B::Error> {
        let area = Rect::new(0, 0, size.width, size.height);
        if area.is_empty() {
            self.area = area;
            return Ok(());
        }
        // Ratatui does not resize fixed viewports automatically.
        if area != self.area {
            self.terminal.resize(area)?;
            self.area = area;
        }
        self.terminal.draw(draw).map(|_| ())
    }
}

/// Whether a key press cancels the view.
///
/// Raw mode delivers this as an ordinary key, so the frame loop cancels the view itself; the
/// signal handler does not run while a view owns the terminal. Every other Ctrl key, `Ctrl-D`
/// included, belongs to the surface's own table.
pub(crate) fn cancels(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
}

/// The index `steps` away from `index`, inside `len`.
pub(crate) fn stepped_index(index: usize, steps: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    index.saturating_add_signed(steps).min(len - 1)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use crossterm::event::KeyEvent;
    use ratatui::backend::{CrosstermBackend, TestBackend};
    use ratatui::widgets::Paragraph;

    use super::*;
    use crate::terminal::session::SinkWriter;
    use crate::terminal::sink::OutputSink;

    /// A terminal whose size and events a test scripts.
    struct ScriptedTerminal {
        sizes: RefCell<VecDeque<(u16, u16)>>,
        events: VecDeque<Event>,
        idle_polls: usize,
    }

    impl ScriptedTerminal {
        fn new(size: (u16, u16)) -> Self {
            Self {
                sizes: RefCell::new(VecDeque::from([size])),
                events: VecDeque::new(),
                idle_polls: 0,
            }
        }

        fn resized(self, size: (u16, u16)) -> Self {
            self.sizes.borrow_mut().push_back(size);
            self
        }

        fn event(mut self, event: Event) -> Self {
            self.events.push_back(event);
            self
        }
    }

    impl Terminal for ScriptedTerminal {
        fn is_interactive(&self) -> bool {
            true
        }

        fn can_draw(&self) -> bool {
            true
        }

        fn size(&self) -> std::io::Result<(u16, u16)> {
            let mut sizes = self.sizes.borrow_mut();
            if sizes.len() > 1 {
                return Ok(sizes.pop_front().expect("checked above"));
            }
            Ok(*sizes.front().expect("a scripted size"))
        }

        fn enter(&mut self) -> std::io::Result<()> {
            Ok(())
        }

        fn leave(&mut self) -> std::io::Result<()> {
            Ok(())
        }

        fn set_title(&mut self, _title: &str) -> std::io::Result<()> {
            Ok(())
        }

        fn set_mouse_capture(&mut self, _capture: bool) -> std::io::Result<()> {
            Ok(())
        }

        fn begin_synchronized_update(&mut self) -> std::io::Result<()> {
            Ok(())
        }

        fn end_synchronized_update(&mut self) -> std::io::Result<()> {
            Ok(())
        }

        fn poll(&mut self, _timeout: Duration) -> std::io::Result<bool> {
            if self.idle_polls > 0 {
                self.idle_polls -= 1;
                return Ok(false);
            }
            Ok(!self.events.is_empty())
        }

        fn read(&mut self) -> std::io::Result<Event> {
            Ok(self.events.pop_front().expect("polled"))
        }
    }

    /// A surface that records every frame's area, drawing one line of text.
    struct Recorder {
        areas: Rc<RefCell<Vec<Rect>>>,
        content: &'static str,
    }

    impl Recorder {
        fn new(content: &'static str) -> Self {
            Self {
                areas: Rc::new(RefCell::new(Vec::new())),
                content,
            }
        }
    }

    impl Surface for Recorder {
        fn draw(&mut self, frame: &mut Frame, _palette: Palette) {
            self.areas.borrow_mut().push(frame.area());
            frame.render_widget(Paragraph::new(self.content), frame.area());
        }

        fn answer(&mut self, _action: Action) -> Step {
            Step::Done
        }

        fn live(&self) -> Vec<Action> {
            vec![Action::Quit]
        }
    }

    fn quit() -> Event {
        Event::Key(KeyEvent::from(KeyCode::Char('q')))
    }

    /// A surface that records the actions it answers, then quits on the first key.
    struct Watcher {
        actions: Rc<RefCell<Vec<Action>>>,
    }

    impl Watcher {
        fn new() -> Self {
            Self {
                actions: Rc::new(RefCell::new(Vec::new())),
            }
        }

        fn actions(&self) -> Vec<Action> {
            self.actions.borrow().clone()
        }
    }

    impl Surface for Watcher {
        fn draw(&mut self, _frame: &mut Frame, _palette: Palette) {}

        fn answer(&mut self, action: Action) -> Step {
            self.actions.borrow_mut().push(action);
            match action {
                Action::Quit => Step::Done,
                _ => Step::Continue,
            }
        }

        fn live(&self) -> Vec<Action> {
            vec![Action::Quit]
        }
    }

    fn scroll(kind: crossterm::event::MouseEventKind) -> Event {
        Event::Mouse(crossterm::event::MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        })
    }

    /// A surface that binds the shared help table, so its keys are the layer's own.
    struct HelpKeys {
        actions: Rc<RefCell<Vec<Action>>>,
    }

    impl Surface for HelpKeys {
        fn draw(&mut self, _frame: &mut Frame, _palette: Palette) {}

        fn answer(&mut self, action: Action) -> Step {
            self.actions.borrow_mut().push(action);
            match action {
                Action::Quit => Step::Done,
                _ => Step::Continue,
            }
        }

        fn live(&self) -> Vec<Action> {
            vec![Action::HalfPageDown, Action::Quit]
        }

        fn bindings(&self) -> &'static keymap::Table {
            &keymap::HELP
        }
    }

    #[test]
    fn control_d_reaches_the_surface_instead_of_cancelling() {
        let (sink, _output) = OutputSink::buffered();
        let mut surface = HelpKeys {
            actions: Rc::new(RefCell::new(Vec::new())),
        };
        let control_d = Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        let mut terminal = ScriptedTerminal::new((80, 24))
            .event(control_d)
            .event(quit());

        show(&mut terminal, &sink, &mut surface).expect("the view ends through its own table");

        assert_eq!(
            surface.actions.borrow().clone(),
            [Action::HalfPageDown, Action::Quit],
            "Ctrl-D half-pages, as the help table binds it"
        );
    }

    #[test]
    fn control_c_cancels_before_the_table() {
        let (sink, _output) = OutputSink::buffered();
        let mut surface = HelpKeys {
            actions: Rc::new(RefCell::new(Vec::new())),
        };
        let control_c = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        let mut terminal = ScriptedTerminal::new((80, 24)).event(control_c);

        let error = show(&mut terminal, &sink, &mut surface).unwrap_err();

        assert!(error.downcast_ref::<InputCancelled>().is_some());
        assert!(surface.actions.borrow().is_empty());
    }

    #[test]
    fn wheel_scrolls_lines() {
        let (sink, _output) = OutputSink::buffered();
        let mut surface = Watcher::new();
        let mut terminal = ScriptedTerminal::new((80, 24))
            .event(scroll(crossterm::event::MouseEventKind::ScrollDown))
            .event(scroll(crossterm::event::MouseEventKind::ScrollUp))
            .event(scroll(crossterm::event::MouseEventKind::Moved))
            .event(quit());
        show(&mut terminal, &sink, &mut surface).unwrap();

        assert_eq!(
            surface.actions(),
            [Action::Down, Action::Up, Action::Quit],
            "the wheel scrolls one line; a move is ignored and the key quits"
        );
    }

    /// Runs one frame loop over a buffered sink, as production runs it.
    fn show(
        terminal: &mut ScriptedTerminal,
        sink: &OutputSink,
        surface: &mut impl Surface,
    ) -> anyhow::Result<()> {
        let writer = SinkWriter::new(sink.clone());
        // The view opens at the size the caller measured; the loop follows the terminal after.
        let mut screen = Screen::new(CrosstermBackend::new(writer), 80, 24)?;
        screen.run(terminal, surface, Palette::new(false), &|| false)
    }

    #[test]
    fn caller_cancellation_stops_the_frame_loop() {
        let (sink, _output) = OutputSink::buffered();
        let mut terminal = ScriptedTerminal::new((80, 24));
        let mut surface = Recorder::new("frame");
        let drawn = Rc::clone(&surface.areas);
        let cancelled = move || !drawn.borrow().is_empty();
        let writer = SinkWriter::new(sink.clone());
        let mut screen = Screen::new(CrosstermBackend::new(writer), 80, 24).unwrap();
        let error = screen
            .run(&mut terminal, &mut surface, Palette::new(false), &cancelled)
            .unwrap_err();
        assert!(error.downcast_ref::<InputCancelled>().is_some());
        assert_eq!(surface.areas.borrow().len(), 1);
    }

    #[test]
    fn idle_view_keeps_its_frame() {
        let (sink, _output) = OutputSink::buffered();
        let mut terminal = ScriptedTerminal::new((80, 24)).event(quit());
        terminal.idle_polls = 4;
        let mut surface = Recorder::new("frame");
        show(&mut terminal, &sink, &mut surface).unwrap();
        assert_eq!(surface.areas.borrow().len(), 1);
    }

    #[test]
    fn zero_sized_frames_draw_nothing() {
        let (sink, _output) = OutputSink::buffered();
        let mut terminal = ScriptedTerminal::new((0, 0)).event(quit());
        let mut surface = Recorder::new("frame");
        show(&mut terminal, &sink, &mut surface).unwrap();
        assert!(surface.areas.borrow().is_empty());
    }

    #[test]
    fn restored_window_repaints_content() {
        let mut screen = Screen::new(TestBackend::new(12, 4), 12, 4).unwrap();
        let draw = |frame: &mut Frame| {
            frame.render_widget(Paragraph::new("visible"), frame.area());
        };
        screen.draw_frame(Size::new(12, 4), draw).unwrap();
        screen
            .draw_frame(Size::new(0, 0), |_| panic!("an empty window drew content"))
            .unwrap();
        screen.terminal.backend_mut().clear().unwrap();
        screen.draw_frame(Size::new(12, 4), draw).unwrap();
        let visible = screen
            .terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(visible.contains("visible"));
    }

    #[test]
    fn resized_window_redraws_at_the_new_size() {
        let mut terminal = ScriptedTerminal::new((80, 24))
            .resized((100, 30))
            .event(Event::Resize(100, 30))
            .event(quit());
        let mut surface = Recorder::new("frame");
        // A resize sends the frame loop through the backend's clear path, which asks the backend
        // for the terminal's own size: the test backend answers from its buffer, so it spans the
        // size the window grows to. A crossterm backend would ask the machine the test runs on,
        // which a runner without a terminal cannot answer.
        let mut screen = Screen::new(TestBackend::new(100, 30), 80, 24).unwrap();
        screen
            .run(&mut terminal, &mut surface, Palette::new(false), &|| false)
            .unwrap();
        assert_eq!(
            surface.areas.borrow().as_slice(),
            [Rect::new(0, 0, 80, 24), Rect::new(0, 0, 100, 30)]
        );
    }

    #[test]
    fn interactive_frames_are_never_logged() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("session.jsonl");
        let log = crate::cli::log::LogFile::prepare_session(&path, |path, error| {
            panic!("the log failed to write {path:?}: {error}")
        })
        .expect("the log prepares");
        assert!(log.start().expect("the log starts"), "the log records");
        let (sink, output) = OutputSink::buffered();
        let mut terminal = ScriptedTerminal::new((80, 24)).event(quit());
        let mut surface = Recorder::new("frame");
        show(&mut terminal, &sink, &mut surface).unwrap();
        // A record the session writes lands in the log; a frame it draws never does.
        log.record("info", "test", serde_json::json!({ "event": "ran" }));
        let logged = std::fs::read(&path).expect("the log holds the record");
        assert!(!logged.is_empty(), "the log recorded nothing");
        assert!(
            !logged.contains(&0x1b),
            "a frame reached the log: {}",
            String::from_utf8_lossy(&logged)
        );
        assert!(
            output.bytes().contains(&0x1b),
            "the frame reached the terminal"
        );
    }
}
