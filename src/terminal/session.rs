//! The one owner of raw mode, the alternate screen, and the stderr lock of an interactive view.
//!
//! Interactive views share raw mode, bracketed paste, and the alternate screen.

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use anyhow::Result;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    KeyEventKind,
};
use crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    SetTitle, disable_raw_mode, enable_raw_mode,
};
use crossterm::{event, execute};
use ratatui::backend::CrosstermBackend;

use super::prompt::{InputCancelled, ensure_active};
use super::sink::OutputSink;
use super::style::Palette;
use super::ui::{Screen, Surface};

/// Why an interactive surface could not take the terminal.
#[derive(Debug, thiserror::Error)]
pub(crate) enum TerminalRefused {
    /// The process has no terminal to read from or draw on, so nothing can be shown or asked.
    #[error("interactive views need a terminal; run the command without the interactive flag")]
    NotTerminal,
    /// Another interactive surface already owns the terminal.
    #[error("interactive view is open")]
    AlreadyOpen,
}

/// Which surface holds the process terminal's modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Holder {
    /// The prompt layer holds raw mode for one answer.
    Prompt = 1,
    /// The interaction layer holds raw mode for one view.
    View = 2,
}

/// No surface holds the terminal.
const FREE: u8 = 0;

/// Who holds the terminal's modes, if anyone.
///
/// Raw mode is global and cannot be shared, so a prompt and a view never overlap: the second one
/// to ask is refused instead of corrupting the screen.
static HOLDER: AtomicU8 = AtomicU8::new(FREE);

/// The claim one surface holds while it owns the terminal's modes.
pub(crate) struct Claim {
    holder: Holder,
}

impl Claim {
    /// Claims the terminal for `holder`; another surface's claim refuses this one.
    pub(crate) fn take(holder: Holder) -> Result<Self, TerminalRefused> {
        HOLDER
            .compare_exchange(FREE, holder as u8, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Self { holder })
            .map_err(|_| TerminalRefused::AlreadyOpen)
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let _ =
            HOLDER.compare_exchange(self.holder as u8, FREE, Ordering::SeqCst, Ordering::SeqCst);
    }
}

pub(crate) struct ViewModes<W: Write> {
    writer: W,
    // A failed mode write may already have changed the terminal.
    restore_needed: bool,
    _claim: Claim,
}

impl<W: Write> ViewModes<W> {
    pub(crate) fn is_active(&self) -> bool {
        self.restore_needed && HOLDER.load(Ordering::SeqCst) == self._claim.holder as u8
    }

    pub(crate) fn enter(writer: W, claim: Claim) -> io::Result<Self> {
        let mut modes = Self {
            writer,
            restore_needed: true,
            _claim: claim,
        };
        enable_raw_mode()?;
        enter_commands(&mut modes.writer)?;
        Ok(modes)
    }

    pub(crate) fn finish(&mut self) -> io::Result<()> {
        if !self.is_active() {
            self.restore_needed = false;
            return Ok(());
        }
        self.restore_needed = false;
        let screen = leave_commands(&mut self.writer);
        let mode = disable_raw_mode();
        screen.and(mode)
    }
}

impl<W: Write> Drop for ViewModes<W> {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn enter_commands(out: &mut impl Write) -> io::Result<()> {
    execute!(out, EnterAlternateScreen, EnableBracketedPaste, Hide)
}

/// The process terminal operations one interactive view drives.
///
/// The seam exists so a test scripts the modes, the size, and the events of a terminal that has
/// none of its own.
pub(crate) trait Terminal {
    /// Whether both streams are terminals.
    fn is_interactive(&self) -> bool;
    /// Whether the terminal draws frames: `TERM` is not `dumb`.
    fn can_draw(&self) -> bool;
    /// The window's size in columns and rows.
    fn size(&self) -> io::Result<(u16, u16)>;
    fn enter(&mut self) -> io::Result<()>;
    fn leave(&mut self) -> io::Result<()>;
    /// Sets the window title; the empty title clears it.
    fn set_title(&mut self, title: &str) -> io::Result<()>;
    /// Captures the pointer for the wheel, or releases it; a released pointer selects text again.
    fn set_mouse_capture(&mut self, capture: bool) -> io::Result<()>;
    /// Opens a synchronized update, so the frame that follows reaches the terminal whole.
    ///
    /// A terminal that does not know the sequence ignores it.
    fn begin_synchronized_update(&mut self) -> io::Result<()>;
    /// Closes the synchronized update opened by [`Terminal::begin_synchronized_update`].
    fn end_synchronized_update(&mut self) -> io::Result<()>;
    /// Whether an event waits, after at most `timeout`.
    fn poll(&mut self, timeout: Duration) -> io::Result<bool>;
    /// Reads one event; only called after [`Terminal::poll`] reported one.
    fn read(&mut self) -> io::Result<Event>;
}

/// The process's own terminal.
pub(crate) struct ProcessTerminal<'a> {
    sink: &'a OutputSink,
}

impl<'a> ProcessTerminal<'a> {
    pub(crate) fn new(sink: &'a OutputSink) -> Self {
        Self { sink }
    }
}

impl Terminal for ProcessTerminal<'_> {
    fn is_interactive(&self) -> bool {
        io::stdin().is_terminal() && io::stderr().is_terminal()
    }

    fn can_draw(&self) -> bool {
        std::env::var_os("TERM").is_none_or(|term| term != "dumb")
    }

    fn size(&self) -> io::Result<(u16, u16)> {
        crossterm::terminal::size()
    }

    fn enter(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        let mut out = SinkWriter::new(self.sink.clone());
        let entered = enter_commands(&mut out);
        if let Err(error) = entered {
            let _ = self.leave();
            return Err(error);
        }
        Ok(())
    }

    fn leave(&mut self) -> io::Result<()> {
        let mut out = SinkWriter::new(self.sink.clone());
        let screen = execute!(out, Show, LeaveAlternateScreen, DisableBracketedPaste);
        let mode = disable_raw_mode();
        screen.and(mode)
    }

    fn set_title(&mut self, title: &str) -> io::Result<()> {
        let mut out = SinkWriter::new(self.sink.clone());
        execute!(out, SetTitle(title.to_owned()))
    }

    fn set_mouse_capture(&mut self, capture: bool) -> io::Result<()> {
        let mut out = SinkWriter::new(self.sink.clone());
        if capture {
            execute!(out, EnableMouseCapture)
        } else {
            execute!(out, DisableMouseCapture)
        }
    }

    fn begin_synchronized_update(&mut self) -> io::Result<()> {
        let mut out = SinkWriter::new(self.sink.clone());
        execute!(out, BeginSynchronizedUpdate)
    }

    fn end_synchronized_update(&mut self) -> io::Result<()> {
        let mut out = SinkWriter::new(self.sink.clone());
        execute!(out, EndSynchronizedUpdate)
    }

    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        event::poll(timeout)
    }

    fn read(&mut self) -> io::Result<Event> {
        event::read()
    }
}

/// The writer an open view draws through: bytes reach stderr while the session holds its lock.
///
/// It owns a clone of the destination (an `Arc` inside), so a view that owns its own ratatui
/// terminal can hold one without borrowing the session that created it.
pub(crate) struct SinkWriter {
    sink: OutputSink,
}

impl SinkWriter {
    pub(crate) fn new(sink: OutputSink) -> Self {
        Self { sink }
    }
}

impl Write for SinkWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.sink.write_stderr_locked(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// What the process terminal gave one interactive view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    /// The surface owned the terminal and ended the view.
    Interactive,
    /// The terminal cannot draw a frame; the caller writes its plain-text form instead.
    Plain,
}

pub(crate) fn show(
    sink: &OutputSink,
    palette: Palette,
    cancelled: &dyn Fn() -> bool,
    surface: &mut impl Surface,
) -> Result<Shown> {
    run(
        &mut ProcessTerminal::new(sink),
        sink,
        palette,
        cancelled,
        surface,
    )
}

/// Holds the terminal modes and stderr lock for the whole view.
///
/// The view owns the terminal exclusively: a prompt, or another view, asking for it meanwhile is
/// refused. A terminal that cannot draw — `dumb`, or a window with no size — writes nothing,
/// changes no mode, and answers [`Shown::Plain`].
pub(crate) fn run(
    terminal: &mut dyn Terminal,
    sink: &OutputSink,
    palette: Palette,
    cancelled: &dyn Fn() -> bool,
    surface: &mut impl Surface,
) -> Result<Shown> {
    ensure_active(cancelled)?;
    let claim = Claim::take(Holder::View)?;
    if !terminal.is_interactive() {
        return Err(TerminalRefused::NotTerminal.into());
    }
    let (columns, rows) = terminal.size()?;
    if !terminal.can_draw() || columns == 0 || rows == 0 {
        return Ok(Shown::Plain);
    }
    sink.with_stderr_lock(|sink| {
        let mut session = Session::new(terminal, sink, palette, cancelled, claim);
        session.enter()?;
        session.show(surface)
    })?;
    Ok(Shown::Interactive)
}

/// One open interactive view.
struct Session<'a> {
    terminal: &'a mut dyn Terminal,
    sink: &'a OutputSink,
    palette: Palette,
    cancelled: &'a dyn Fn() -> bool,
    entered: bool,
    /// Held for the session's lifetime; dropping it frees the terminal for the next surface.
    _claim: Claim,
    mouse: bool,
    synchronized: bool,
    titled: bool,
}

impl<'a> Session<'a> {
    fn new(
        terminal: &'a mut dyn Terminal,
        sink: &'a OutputSink,
        palette: Palette,
        cancelled: &'a dyn Fn() -> bool,
        claim: Claim,
    ) -> Self {
        Self {
            terminal,
            sink,
            palette,
            cancelled,
            entered: false,
            _claim: claim,
            mouse: false,
            synchronized: false,
            titled: false,
        }
    }

    fn enter(&mut self) -> Result<()> {
        Terminal::enter(self)?;
        Ok(())
    }

    /// Draws frames until the surface ends the view or the caller cancels.
    fn show(&mut self, surface: &mut impl Surface) -> Result<()> {
        let (columns, rows) = self.terminal.size()?;
        let writer = SinkWriter::new(self.sink.clone());
        let mut screen = Screen::new(CrosstermBackend::new(writer), columns, rows)?;
        let palette = self.palette;
        let cancelled = self.cancelled;
        screen.run(self, surface, palette, cancelled)
    }
}

impl Terminal for Session<'_> {
    fn is_interactive(&self) -> bool {
        self.terminal.is_interactive()
    }

    fn can_draw(&self) -> bool {
        self.terminal.can_draw()
    }

    fn size(&self) -> io::Result<(u16, u16)> {
        self.terminal.size()
    }

    fn enter(&mut self) -> io::Result<()> {
        self.entered = true;
        self.terminal.enter()
    }

    fn leave(&mut self) -> io::Result<()> {
        self.terminal.leave()?;
        self.entered = false;
        Ok(())
    }

    fn set_title(&mut self, title: &str) -> io::Result<()> {
        self.titled |= !title.is_empty();
        self.terminal.set_title(title)?;
        self.titled = !title.is_empty();
        Ok(())
    }

    fn set_mouse_capture(&mut self, capture: bool) -> io::Result<()> {
        self.mouse |= capture;
        self.terminal.set_mouse_capture(capture)?;
        self.mouse = capture;
        Ok(())
    }

    fn begin_synchronized_update(&mut self) -> io::Result<()> {
        self.synchronized = true;
        self.terminal.begin_synchronized_update()
    }

    fn end_synchronized_update(&mut self) -> io::Result<()> {
        self.terminal.end_synchronized_update()?;
        self.synchronized = false;
        Ok(())
    }

    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        self.terminal.poll(timeout)
    }

    fn read(&mut self) -> io::Result<Event> {
        self.terminal.read()
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        if self.entered {
            // A mode write can change the terminal before reporting failure.
            if self.synchronized {
                let _ = self.terminal.end_synchronized_update();
            }
            if self.mouse {
                let _ = self.terminal.set_mouse_capture(false);
            }
            if self.titled {
                let _ = self.terminal.set_title("");
            }
            let _ = self.terminal.leave();
        }
    }
}

/// The next event, skipping key releases.
///
/// An interrupted wait is retried, a closed input cancels the view, and `cancelled` is observed
/// before and after every wait, so the caller stops the frame loop between polls. `None` means
/// the wait ended with no event.
pub(crate) fn next_event(
    terminal: &mut dyn Terminal,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Event>> {
    loop {
        ensure_active(cancelled)?;
        let pending = match terminal.poll(timeout) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            pending => pending?,
        };
        if !pending {
            return Ok(None);
        }
        let event = match terminal.read() {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(InputCancelled.into());
            }
            event => event?,
        };
        ensure_active(cancelled)?;
        if let Event::Key(key) = &event
            && key.kind == KeyEventKind::Release
        {
            continue;
        }
        return Ok(Some(event));
    }
}

/// Restores the terminal without waiting for the stderr lock, which a panic can hold.
///
/// The view that panicked cannot be asked which screen it took, so the restore leaves the
/// alternate screen; a view that never entered it has nothing to leave, and the sequence is
/// harmless.
pub(super) fn restore() {
    if HOLDER.load(Ordering::SeqCst) != Holder::View as u8 {
        return;
    }
    HOLDER.store(FREE, Ordering::SeqCst);
    let _ = disable_raw_mode();
    let mut stderr = io::stderr();
    let _ = leave_commands(&mut stderr);
    let _ = stderr.write_all(b"\r\n");
}

/// The commands that put the terminal back after a view stops for any reason, in order.
///
/// Each of them is harmless when the mode it ends was never entered, so a panic mid-frame gets
/// the same treatment as an orderly exit: the frame in progress ends, the pointer is released,
/// the title goes back to the terminal's own, the cursor is shown, and only then does the screen
/// goes.
pub(super) fn leave_commands(out: &mut impl Write) -> io::Result<()> {
    execute!(
        out,
        EndSynchronizedUpdate,
        DisableMouseCapture,
        SetTitle(String::new()),
        Show,
        LeaveAlternateScreen,
        DisableBracketedPaste,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::sync::{Mutex, MutexGuard};
    use std::thread;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Frame;

    use super::*;
    use crate::terminal::ui::{Action, Step};

    /// The claim a view takes is process-global, so the tests below open one view at a time.
    static TERMINAL: Mutex<()> = Mutex::new(());

    /// Holds the process terminal for one test.
    fn terminal() -> MutexGuard<'static, ()> {
        TERMINAL.lock().unwrap_or_else(|error| error.into_inner())
    }
    /// What one scripted terminal records and answers.
    struct FakeTerminal {
        interactive: bool,
        drawable: bool,
        size: (u16, u16),
        polls: VecDeque<io::Result<bool>>,
        events: VecDeque<io::Result<Event>>,
        calls: Rc<RefCell<Vec<&'static str>>>,
        titles: Rc<RefCell<Vec<String>>>,
        updates: Rc<RefCell<Vec<&'static str>>>,
        captures: Rc<RefCell<Vec<bool>>>,
        failure: Option<ModeFailure>,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum ModeFailure {
        Mouse,
        Title,
        BeginUpdate,
        EndUpdate,
    }

    impl FakeTerminal {
        fn new() -> Self {
            Self {
                interactive: true,
                drawable: true,
                size: (80, 24),
                polls: VecDeque::new(),
                events: VecDeque::new(),
                calls: Rc::new(RefCell::new(Vec::new())),
                titles: Rc::new(RefCell::new(Vec::new())),
                updates: Rc::new(RefCell::new(Vec::new())),
                captures: Rc::new(RefCell::new(Vec::new())),
                failure: None,
            }
        }

        fn fail(&mut self, mode: ModeFailure) -> io::Result<()> {
            if self.failure == Some(mode) {
                self.failure = None;
                Err(io::Error::other("terminal mode failed"))
            } else {
                Ok(())
            }
        }

        /// The calls this terminal recorded, in order.
        fn calls(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }

        /// The titles this terminal was given, in order.
        fn titles(&self) -> Vec<String> {
            self.titles.borrow().clone()
        }

        /// The synchronized-update boundaries this terminal recorded, in order.
        fn updates(&self) -> Vec<&'static str> {
            self.updates.borrow().clone()
        }

        /// The pointer capture changes this terminal recorded, in order.
        fn captures(&self) -> Vec<bool> {
            self.captures.borrow().clone()
        }

        fn press(mut self, code: KeyCode) -> Self {
            self.events
                .push_back(Ok(Event::Key(KeyEvent::new(code, KeyModifiers::NONE))));
            self
        }

        /// The next poll finds no event, so the frame loop draws before it waits again.
        fn poll_empty(mut self) -> Self {
            self.polls.push_back(Ok(false));
            self
        }

        /// The next poll finds an event waiting.
        fn poll_waiting(mut self) -> Self {
            self.polls.push_back(Ok(true));
            self
        }
    }

    impl Terminal for FakeTerminal {
        fn is_interactive(&self) -> bool {
            self.interactive
        }

        fn can_draw(&self) -> bool {
            self.drawable
        }

        fn size(&self) -> io::Result<(u16, u16)> {
            Ok(self.size)
        }

        fn enter(&mut self) -> io::Result<()> {
            self.calls.borrow_mut().push("enter fullscreen");
            Ok(())
        }

        fn leave(&mut self) -> io::Result<()> {
            self.calls.borrow_mut().push("leave fullscreen");
            Ok(())
        }

        fn begin_synchronized_update(&mut self) -> io::Result<()> {
            self.updates.borrow_mut().push("begin");
            self.fail(ModeFailure::BeginUpdate)
        }

        fn end_synchronized_update(&mut self) -> io::Result<()> {
            self.updates.borrow_mut().push("end");
            self.fail(ModeFailure::EndUpdate)
        }

        fn set_title(&mut self, title: &str) -> io::Result<()> {
            self.titles.borrow_mut().push(title.to_owned());
            self.fail(ModeFailure::Title)
        }

        fn set_mouse_capture(&mut self, capture: bool) -> io::Result<()> {
            self.captures.borrow_mut().push(capture);
            self.fail(ModeFailure::Mouse)
        }

        fn poll(&mut self, _timeout: Duration) -> io::Result<bool> {
            match self.polls.pop_front() {
                Some(result) => result,
                None => Ok(!self.events.is_empty()),
            }
        }

        fn read(&mut self) -> io::Result<Event> {
            self.events
                .pop_front()
                .unwrap_or(Err(io::Error::other("the script holds no event")))
        }
    }

    /// A surface that runs `body` when it answers the one key its script holds, then ends the view.
    struct OnKey<F: FnMut()>(F);

    impl<F: FnMut()> Surface for OnKey<F> {
        fn draw(&mut self, _frame: &mut Frame, _palette: Palette) {}

        fn answer(&mut self, _action: Action) -> Step {
            (self.0)();
            Step::Done
        }

        fn live(&self) -> Vec<Action> {
            vec![Action::Quit]
        }
    }

    fn quit_key() -> KeyCode {
        KeyCode::Char('q')
    }

    /// A surface whose title changes when it answers its key.
    struct Titled {
        title: Option<String>,
        next: Option<String>,
    }

    impl Surface for Titled {
        fn draw(&mut self, _frame: &mut Frame, _palette: Palette) {}

        fn answer(&mut self, _action: Action) -> Step {
            match self.next.take() {
                // The surface keeps running after it changed its own title.
                Some(title) => {
                    self.title = Some(title);
                    Step::Continue
                }
                None => Step::Done,
            }
        }

        fn live(&self) -> Vec<Action> {
            vec![Action::Quit]
        }

        fn title(&self) -> Option<String> {
            self.title.clone()
        }
    }

    #[test]
    fn failed_modes_are_released() {
        let _terminal = terminal();
        for failure in [
            ModeFailure::Mouse,
            ModeFailure::Title,
            ModeFailure::BeginUpdate,
            ModeFailure::EndUpdate,
        ] {
            let (sink, _output) = OutputSink::buffered();
            let mut terminal = FakeTerminal::new().press(quit_key());
            terminal.failure = Some(failure);
            let mut surface = Titled {
                title: Some("tola help".to_owned()),
                next: None,
            };
            let result = if failure == ModeFailure::Mouse {
                run(
                    &mut terminal,
                    &sink,
                    Palette::new(false),
                    &|| false,
                    &mut Mousey(Step::Done),
                )
            } else {
                run(
                    &mut terminal,
                    &sink,
                    Palette::new(false),
                    &|| false,
                    &mut surface,
                )
            };
            assert!(result.is_err());
            match failure {
                ModeFailure::Mouse => assert_eq!(terminal.captures(), [true, false]),
                ModeFailure::Title => assert_eq!(terminal.titles(), ["tola help", ""]),
                ModeFailure::BeginUpdate => assert_eq!(terminal.updates(), ["begin", "end"]),
                ModeFailure::EndUpdate => {
                    assert_eq!(terminal.updates(), ["begin", "end", "end"]);
                }
            }
            assert_eq!(terminal.calls(), ["enter fullscreen", "leave fullscreen"]);
        }
    }

    #[test]
    fn window_title_follows_the_surface() {
        let _terminal = terminal();
        let (sink, _output) = OutputSink::buffered();
        let cancelled = || false;
        let mut surface = Titled {
            title: Some("tola help - [site]".to_owned()),
            next: Some("tola help - [typst]".to_owned()),
        };
        // Each key lands in its own input batch: the frame between them reads the new title.
        let mut terminal = FakeTerminal::new()
            .press(quit_key())
            .poll_waiting()
            .poll_empty()
            .press(quit_key());
        let result = run(
            &mut terminal,
            &sink,
            Palette::new(false),
            &cancelled,
            &mut surface,
        );

        assert_eq!(result.unwrap(), Shown::Interactive);
        // Set on the first frame, updated when the surface changed it, cleared on exit.
        assert_eq!(
            terminal.titles(),
            ["tola help - [site]", "tola help - [typst]", ""]
        );
        assert_eq!(terminal.updates(), ["begin", "end", "begin", "end"]);
    }

    #[test]
    fn dumb_terminal_gets_no_title_and_no_frames() {
        let _terminal = terminal();
        let (sink, _output) = OutputSink::buffered();
        let cancelled = || false;
        let mut surface = Titled {
            title: Some("tola help - [site]".to_owned()),
            next: None,
        };
        let mut terminal = FakeTerminal::new();
        terminal.drawable = false;
        let result = run(
            &mut terminal,
            &sink,
            Palette::new(false),
            &cancelled,
            &mut surface,
        );

        assert_eq!(result.unwrap(), Shown::Plain);
        assert!(
            terminal.titles().is_empty(),
            "nothing is drawn on a dumb terminal"
        );
        assert!(terminal.updates().is_empty());
    }

    /// A surface that takes the pointer, and ends the view the way it was built to.
    struct Mousey(Step);

    impl Surface for Mousey {
        fn draw(&mut self, _frame: &mut Frame, _palette: Palette) {}

        fn answer(&mut self, _action: Action) -> Step {
            self.0
        }

        fn live(&self) -> Vec<Action> {
            vec![Action::Quit]
        }

        fn wants_mouse(&self) -> bool {
            true
        }
    }

    #[test]
    fn abort_path_leaves_every_mode_it_could_have_entered() {
        let mut bytes = Vec::new();
        leave_commands(&mut bytes).unwrap();
        let text = String::from_utf8_lossy(&bytes);

        assert!(
            text.contains("\x1b[?2026l"),
            "the synchronized update ends: {text:?}"
        );
        assert!(
            text.contains("\x1b[?1000l"),
            "the pointer is released: {text:?}"
        );
        assert!(text.contains("]0;"), "the title is cleared: {text:?}");
        assert!(
            text.contains("\x1b[?1049l"),
            "the alternate screen goes: {text:?}"
        );
    }

    #[test]
    fn mouse_capture_wraps_runs_that_want_the_wheel() {
        let _terminal = terminal();
        let (sink, _output) = OutputSink::buffered();
        let cancelled = || false;
        let mut surface = Mousey(Step::Done);
        let mut terminal = FakeTerminal::new().press(quit_key());
        let result = run(
            &mut terminal,
            &sink,
            Palette::new(false),
            &cancelled,
            &mut surface,
        );

        assert_eq!(result.unwrap(), Shown::Interactive);
        assert_eq!(
            terminal.captures(),
            [true, false],
            "captured, then released"
        );
    }

    #[test]
    fn mouse_capture_releases_on_cancel() {
        let _terminal = terminal();
        let (sink, _output) = OutputSink::buffered();
        let cancelled = || false;
        let mut surface = Mousey(Step::Cancel);
        let mut terminal = FakeTerminal::new().press(quit_key());
        let result = run(
            &mut terminal,
            &sink,
            Palette::new(false),
            &cancelled,
            &mut surface,
        );

        assert!(result.is_err(), "a cancelled view reports cancellation");
        assert_eq!(terminal.captures(), [true, false]);
    }

    #[test]
    fn mouse_capture_stays_off_when_no_surface_asks() {
        let _terminal = terminal();
        let (sink, _output) = OutputSink::buffered();
        let cancelled = || false;
        let mut surface = Answer(Step::Done);
        let mut terminal = FakeTerminal::new().press(quit_key());
        let result = run(
            &mut terminal,
            &sink,
            Palette::new(false),
            &cancelled,
            &mut surface,
        );

        assert_eq!(result.unwrap(), Shown::Interactive);
        assert!(terminal.captures().is_empty(), "the pointer is left alone");
    }

    #[test]
    fn every_ending_restores_the_modes() {
        let _terminal = terminal();
        for answer in [Step::Done, Step::Cancel] {
            let (sink, _output) = OutputSink::buffered();
            let cancelled = || false;
            let mut surface = Answer(answer);
            let mut terminal = FakeTerminal::new().press(quit_key());
            let result = run(
                &mut terminal,
                &sink,
                Palette::new(false),
                &cancelled,
                &mut surface,
            );
            match answer {
                Step::Done => assert_eq!(result.unwrap(), Shown::Interactive),
                _ => assert!(result.is_err(), "a cancelled view reports cancellation"),
            }
            assert_eq!(
                terminal.calls(),
                ["enter fullscreen", "leave fullscreen"],
                "{answer:?}"
            );
            // A view that never wrote a title never clears one either.
            assert!(terminal.titles().is_empty(), "{answer:?}");
        }
    }

    /// A surface that answers every action with the same ending.
    struct Answer(Step);

    impl Surface for Answer {
        fn draw(&mut self, _frame: &mut Frame, _palette: Palette) {}

        fn answer(&mut self, _action: Action) -> Step {
            self.0
        }

        fn live(&self) -> Vec<Action> {
            vec![Action::Quit]
        }
    }

    #[test]
    fn views_never_overlap() {
        let _terminal = terminal();
        let (sink, _output) = OutputSink::buffered();
        let cancelled = || false;
        let mut refused = None;
        let mut nested = FakeTerminal::new().press(quit_key());
        {
            let mut surface = OnKey(|| {
                let mut idle = Answer(Step::Done);
                refused = Some(run(
                    &mut nested,
                    &sink,
                    Palette::new(false),
                    &cancelled,
                    &mut idle,
                ));
            });
            let mut terminal = FakeTerminal::new().press(quit_key());
            run(
                &mut terminal,
                &sink,
                Palette::new(false),
                &cancelled,
                &mut surface,
            )
            .unwrap();
            assert_eq!(terminal.calls(), ["enter fullscreen", "leave fullscreen"]);
        }
        assert!(matches!(
            refused
                .expect("the nested view ran")
                .unwrap_err()
                .downcast_ref(),
            Some(TerminalRefused::AlreadyOpen)
        ));
        assert!(nested.calls().is_empty());
    }

    #[test]
    fn prompt_is_refused_while_the_view_runs() {
        let _terminal = terminal();
        let (sink, _output) = OutputSink::buffered();
        let canceller = tola_build::cancellation::BuildCanceller::new();
        sink.set_cancellation(canceller.token());
        let prompt = crate::terminal::prompt::PromptReader::new(sink.clone());
        let cancelled = || false;
        let mut asked = None;
        {
            let mut surface = OnKey(|| {
                canceller.cancel();
                asked = Some(prompt.read_line("Site directory", ".", &cancelled));
            });
            let mut terminal = FakeTerminal::new().press(quit_key());
            run(
                &mut terminal,
                &sink,
                Palette::new(false),
                &cancelled,
                &mut surface,
            )
            .unwrap();
        }
        assert!(matches!(
            asked
                .expect("the prompt asked for the terminal")
                .unwrap_err()
                .downcast_ref(),
            Some(TerminalRefused::AlreadyOpen)
        ));
    }

    #[test]
    fn no_terminal_refuses_the_view() {
        let _terminal = terminal();
        let (sink, _output) = OutputSink::buffered();
        let cancelled = || false;
        let mut terminal = FakeTerminal::new();
        terminal.interactive = false;
        let error = run(
            &mut terminal,
            &sink,
            Palette::new(false),
            &cancelled,
            &mut Answer(Step::Done),
        )
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref(),
            Some(TerminalRefused::NotTerminal)
        ));
        assert!(terminal.calls().is_empty());
    }

    #[test]
    fn dumb_terminals_show_plain_text() {
        let _terminal = terminal();
        let (sink, _output) = OutputSink::buffered();
        let cancelled = || false;
        let mut terminal = FakeTerminal::new();
        terminal.drawable = false;
        let shown = run(
            &mut terminal,
            &sink,
            Palette::new(false),
            &cancelled,
            &mut Answer(Step::Done),
        )
        .unwrap();
        assert_eq!(shown, Shown::Plain);
        assert!(terminal.calls().is_empty());
    }

    #[test]
    fn zero_sized_windows_show_plain_text() {
        let _terminal = terminal();
        let (sink, _output) = OutputSink::buffered();
        let cancelled = || false;
        let mut terminal = FakeTerminal::new();
        terminal.size = (0, 0);
        let shown = run(
            &mut terminal,
            &sink,
            Palette::new(false),
            &cancelled,
            &mut Answer(Step::Done),
        )
        .unwrap();
        assert_eq!(shown, Shown::Plain);
        assert!(terminal.calls().is_empty());
    }

    #[test]
    fn terminal_restore_frees_the_view() {
        let _terminal = terminal();
        let (sink, _output) = OutputSink::buffered();
        let cancelled = || false;
        let mut freed = None;
        let mut nested = FakeTerminal::new().press(quit_key());
        {
            // The restored view runs while the first one still holds its own sink's lock.
            let (nested_sink, _nested_output) = OutputSink::buffered();
            let mut surface = OnKey(|| {
                crate::terminal::restore();
                let mut idle = Answer(Step::Done);
                freed = Some(run(
                    &mut nested,
                    &nested_sink,
                    Palette::new(false),
                    &cancelled,
                    &mut idle,
                ));
            });
            let mut terminal = FakeTerminal::new().press(quit_key());
            run(
                &mut terminal,
                &sink,
                Palette::new(false),
                &cancelled,
                &mut surface,
            )
            .unwrap();
        }
        assert!(matches!(
            freed.expect("the restored view ran"),
            Ok(Shown::Interactive)
        ));
    }

    #[test]
    fn other_writers_park_while_the_view_runs() {
        let _terminal = terminal();
        let (sink, output) = OutputSink::buffered();
        let canceller = tola_build::cancellation::BuildCanceller::new();
        sink.set_cancellation(canceller.token());
        let cancelled = || false;
        let mut written = None;
        {
            let writer = sink.clone();
            let mut surface = OnKey(|| {
                let writer = writer.clone();
                let handle = thread::spawn(move || writer.write_stderr(b"parked"));
                canceller.cancel();
                written = Some(handle.join().expect("the writer thread ends"));
            });
            let mut terminal = FakeTerminal::new().press(quit_key());
            run(
                &mut terminal,
                &sink,
                Palette::new(false),
                &cancelled,
                &mut surface,
            )
            .unwrap();
        }
        assert!(written.expect("the writer ran").is_err());
        assert!(!String::from_utf8_lossy(&output.bytes()).contains("parked"));
    }

    #[test]
    fn parked_writes_land_after_the_view_ends() {
        let _terminal = terminal();
        let (sink, output) = OutputSink::buffered();
        let cancelled = || false;
        let mut parked = None;
        {
            let writer = sink.clone();
            let mut surface = OnKey(|| {
                let writer = writer.clone();
                parked = Some(thread::spawn(move || writer.write_stderr(b"late")));
            });
            let mut terminal = FakeTerminal::new().press(quit_key());
            run(
                &mut terminal,
                &sink,
                Palette::new(false),
                &cancelled,
                &mut surface,
            )
            .unwrap();
        }
        parked
            .expect("the writer started")
            .join()
            .expect("the parked writer ends")
            .expect("the parked write lands");
        assert!(String::from_utf8_lossy(&output.bytes()).contains("late"));
    }

    #[test]
    fn key_releases_are_skipped() {
        let mut release = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        let mut terminal = FakeTerminal::new();
        terminal.events.push_back(Ok(Event::Key(release)));
        terminal.events.push_back(Ok(Event::Key(KeyEvent::new(
            KeyCode::Char('b'),
            KeyModifiers::NONE,
        ))));
        let cancelled = || false;
        let event = next_event(&mut terminal, Duration::ZERO, &cancelled).unwrap();
        assert!(matches!(
            event,
            Some(Event::Key(KeyEvent {
                code: KeyCode::Char('b'),
                ..
            }))
        ));
    }

    #[test]
    fn interrupted_waits_are_retried() {
        let mut terminal = FakeTerminal::new().press(KeyCode::Char('x'));
        terminal
            .polls
            .push_back(Err(io::Error::from(io::ErrorKind::Interrupted)));
        let cancelled = || false;
        let event = next_event(&mut terminal, Duration::ZERO, &cancelled).unwrap();
        assert!(matches!(
            event,
            Some(Event::Key(KeyEvent {
                code: KeyCode::Char('x'),
                ..
            }))
        ));
    }

    #[test]
    fn closed_input_cancels_the_view() {
        let mut terminal = FakeTerminal::new();
        terminal
            .events
            .push_back(Err(io::Error::from(io::ErrorKind::UnexpectedEof)));
        let cancelled = || false;
        let error = next_event(&mut terminal, Duration::ZERO, &cancelled).unwrap_err();
        assert!(error.downcast_ref::<InputCancelled>().is_some());
    }

    #[test]
    fn empty_waits_return_nothing() {
        let mut terminal = FakeTerminal::new();
        let cancelled = || false;
        assert!(
            next_event(&mut terminal, Duration::ZERO, &cancelled)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn caller_cancellation_stops_the_wait() {
        let mut terminal = FakeTerminal::new().press(KeyCode::Char('x'));
        let cancelled = || true;
        let error = next_event(&mut terminal, Duration::ZERO, &cancelled).unwrap_err();
        assert!(error.downcast_ref::<InputCancelled>().is_some());
    }
}
