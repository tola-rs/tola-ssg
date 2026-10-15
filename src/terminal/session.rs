//! Terminal ownership and restoration for the development view.

use anyhow::Result;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, Event, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode,
    enable_raw_mode,
};
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// A terminal read the caller cancelled.
#[derive(Debug, thiserror::Error)]
#[error("input cancelled")]
pub(crate) struct InputCancelled;

/// Refuses to keep waiting for terminal input once the caller asked to stop.
fn ensure_active(cancelled: &dyn Fn() -> bool) -> anyhow::Result<()> {
    if cancelled() {
        return Err(InputCancelled.into());
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
#[error("interactive view is open")]
pub(crate) struct TerminalRefused;

/// Whether the process terminal's modes are currently claimed.
///
/// Raw mode is global and cannot be shared: a second surface that asks while the development
/// view holds it is refused instead of corrupting the screen.
static CLAIMED: AtomicBool = AtomicBool::new(false);

/// The claim one surface holds while it owns the terminal's modes.
pub(crate) struct Claim;

impl Claim {
    /// Claims the terminal; another surface's live claim refuses this one.
    pub(crate) fn take() -> Result<Self, TerminalRefused> {
        CLAIMED
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Self)
            .map_err(|_| TerminalRefused)
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let _ = CLAIMED.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst);
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
        self.restore_needed && CLAIMED.load(Ordering::SeqCst)
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

pub(crate) struct ProcessTerminal;

impl ProcessTerminal {
    pub(super) fn is_interactive(&self) -> bool {
        io::stdin().is_terminal() && io::stderr().is_terminal()
    }

    pub(super) fn can_draw(&self) -> bool {
        std::env::var_os("TERM").is_none_or(|term| term != "dumb")
    }

    pub(super) fn size(&self) -> io::Result<(u16, u16)> {
        crossterm::terminal::size()
    }

    pub(super) fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        event::poll(timeout)
    }

    pub(super) fn read(&mut self) -> io::Result<Event> {
        event::read()
    }
}

/// The next event, skipping key releases.
///
/// An interrupted wait is retried, a closed input cancels the view, and `cancelled` is observed
/// before and after every wait, so the caller stops the frame loop between polls. `None` means
/// the wait ended with no event.
pub(crate) fn next_event(
    terminal: &mut ProcessTerminal,
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
/// The claim records only that a view held the terminal, so the restore always leaves the
/// alternate screen; the sequence is harmless for a view that entered none.
pub(super) fn restore() {
    if !CLAIMED.load(Ordering::SeqCst) {
        return;
    }
    CLAIMED.store(false, Ordering::SeqCst);
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
/// go.
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
    use super::*;

    #[test]
    fn terminal_claim_is_released() {
        let first = Claim::take().unwrap();
        assert!(Claim::take().is_err());
        drop(first);
        assert!(Claim::take().is_ok());
    }

    #[test]
    fn restore_releases_screen_and_cursor() {
        let mut bytes = Vec::new();
        leave_commands(&mut bytes).unwrap();
        let written = String::from_utf8(bytes).unwrap();
        assert!(written.contains("\x1b[?1049l"));
        assert!(written.contains("\x1b[?25h"));
    }
}
