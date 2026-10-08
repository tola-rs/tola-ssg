//! Cancellable terminal input with one owner for raw mode and cursor restoration.

use anyhow::{Result, bail};
use crossterm::cursor::{Hide, MoveToColumn, MoveUp, Show};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{self, Clear, ClearType};
use owo_colors::OwoColorize;
use std::io::{self, IsTerminal, Write};
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

use super::input::InputLine;
use super::session::{Claim, Holder};
use super::sink::OutputSink;

/// The user dismissed an input request or its caller cancelled it.
#[derive(Debug, thiserror::Error)]
#[error("input cancelled")]
pub(crate) struct InputCancelled;

pub(super) fn ensure_active(cancelled: &dyn Fn() -> bool) -> Result<()> {
    if cancelled() {
        return Err(InputCancelled.into());
    }
    Ok(())
}

/// Restore interactive modes without waiting for the shared output lock.
/// A panic can occur while its thread holds that lock.
pub(super) fn restore() {
    if terminal::is_raw_mode_enabled().unwrap_or(false) {
        let _ = terminal::disable_raw_mode();
        if std::env::var_os("TERM").is_none_or(|term| term != "dumb") {
            let _ = execute!(io::stderr(), Show, DisableBracketedPaste);
        }
        let _ = io::stderr().write_all(b"\r\n");
    }
}

/// Reader of one interactive answer from the controlling terminal.
#[derive(Clone)]
pub(crate) struct PromptReader {
    sink: OutputSink,
}

impl PromptReader {
    pub(crate) fn new(sink: OutputSink) -> Self {
        Self { sink }
    }

    pub(crate) fn is_interactive(&self) -> bool {
        io::stdin().is_terminal() && io::stderr().is_terminal()
    }

    pub(crate) fn select_many(
        &self,
        label: &str,
        choices: &[&str],
        use_color: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<usize>> {
        self.run_prompt(cancelled, |sink| {
            if choices.is_empty() {
                return Ok(Vec::new());
            }
            in_terminal(sink, cancelled, |prompt| {
                if prompt.cursor_controls {
                    let label =
                        format!("{label} (arrows to move, Space to toggle, Enter to confirm)");
                    prompt.select_many(&label, choices, use_color, cancelled)
                } else {
                    prompt.select_numbers(label, choices, cancelled)
                }
            })
        })
    }

    pub(crate) fn select_one(
        &self,
        label: &str,
        choices: &[&str],
        initial: usize,
        use_color: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<usize> {
        assert!(
            !choices.is_empty(),
            "a single-choice prompt needs at least one choice"
        );
        self.run_prompt(cancelled, |sink| {
            let initial = initial.min(choices.len() - 1);
            in_terminal(sink, cancelled, |prompt| {
                if prompt.cursor_controls {
                    let label = format!("{label} (arrows to move, Enter to confirm)");
                    prompt.select_one(&label, choices, initial, use_color, cancelled)
                } else {
                    prompt.select_number(label, choices, initial, cancelled)
                }
            })
        })
    }

    pub(crate) fn read_line(
        &self,
        label: &str,
        default: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<String> {
        let label = if default.is_empty() {
            format!("{label}: ")
        } else {
            format!("{label} [{default}]: ")
        };
        self.run_prompt(cancelled, |sink| {
            in_terminal(sink, cancelled, |prompt| {
                prompt.read_line(&label, cancelled)
            })
        })
        .map(|answer| line_or_default(answer, default))
    }

    pub(crate) fn confirm(
        &self,
        label: &str,
        default: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool> {
        self.run_prompt(cancelled, |sink| {
            in_terminal(sink, cancelled, |prompt| {
                prompt.confirm(label, default, cancelled)
            })
        })
    }

    fn require_terminal(&self) -> Result<()> {
        if !self.is_interactive() {
            bail!("interactive input requires a terminal");
        }
        Ok(())
    }

    fn run_prompt<T>(
        &self,
        cancelled: &dyn Fn() -> bool,
        request: impl FnOnce(&OutputSink) -> Result<T>,
    ) -> Result<T> {
        ensure_active(cancelled)?;
        let _claim = Claim::take(Holder::Prompt)?;
        self.sink.with_stderr_lock(|sink| {
            ensure_active(cancelled)?;
            self.require_terminal()?;
            request(sink)
        })
    }
}

/// Runs one prompt with a raw-mode terminal, restoring it before returning.
fn in_terminal<T>(
    sink: &OutputSink,
    cancelled: &dyn Fn() -> bool,
    prompt: impl FnOnce(&mut PromptTerminal<'_>) -> Result<T>,
) -> Result<T> {
    let mut terminal = PromptTerminal::enter(sink)?;
    let answer = prompt(&mut terminal);
    let restored = terminal.restore();
    let answer = answer?;
    restored?;
    ensure_active(cancelled)?;
    Ok(answer)
}

/// Whether Esc or Ctrl-C/Ctrl-D dismisses the open input request.
fn dismisses_prompt(key: &KeyEvent) -> bool {
    key.code == KeyCode::Esc
        || (key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'd')))
}

fn next_event(cancelled: &dyn Fn() -> bool) -> Result<Event> {
    loop {
        if cancelled() {
            return Err(InputCancelled.into());
        }
        let pending = match event::poll(Duration::from_millis(50)) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            pending => pending?,
        };
        if !pending {
            continue;
        }
        let event = match event::read() {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(InputCancelled.into());
            }
            event => event?,
        };
        if cancelled() {
            return Err(InputCancelled.into());
        }
        if let Event::Key(key) = &event {
            if key.kind == KeyEventKind::Release {
                continue;
            }
            if dismisses_prompt(key) {
                return Err(InputCancelled.into());
            }
        }
        return Ok(event);
    }
}

struct PromptOutput<'a>(&'a OutputSink);

impl Write for PromptOutput<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write_stderr_locked(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct PromptTerminal<'a> {
    output: PromptOutput<'a>,
    cursor_controls: bool,
    restored: bool,
    line_open: bool,
    previous_columns: usize,
    selection_rows: usize,
}

fn dimensions() -> (u16, u16) {
    terminal::size()
        .ok()
        .filter(|&(width, height)| width > 0 && height > 0)
        .unwrap_or((80, 24))
}

/// What one key press does to the open choice selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyAction {
    Confirm,
    MoveTo(usize),
    Toggle,
    Ignore,
}

fn key_action(code: KeyCode, active: usize, count: usize, selecting_many: bool) -> KeyAction {
    match code {
        KeyCode::Enter => KeyAction::Confirm,
        KeyCode::Up | KeyCode::BackTab => {
            KeyAction::MoveTo(active.checked_sub(1).unwrap_or(count - 1))
        }
        KeyCode::Down | KeyCode::Tab => KeyAction::MoveTo((active + 1) % count),
        KeyCode::Home => KeyAction::MoveTo(0),
        KeyCode::End => KeyAction::MoveTo(count - 1),
        KeyCode::Char(' ') if selecting_many => KeyAction::Toggle,
        _ => KeyAction::Ignore,
    }
}

impl<'a> PromptTerminal<'a> {
    fn enter(sink: &'a OutputSink) -> Result<Self> {
        let mut prompt = Self {
            output: PromptOutput(sink),
            cursor_controls: std::env::var_os("TERM").is_none_or(|term| term != "dumb"),
            restored: false,
            line_open: false,
            previous_columns: 0,
            selection_rows: 0,
        };
        terminal::enable_raw_mode()?;
        if prompt.cursor_controls {
            execute!(prompt.output, EnableBracketedPaste, Show)?;
        }
        Ok(prompt)
    }

    fn read_line(&mut self, label: &str, cancelled: &dyn Fn() -> bool) -> Result<String> {
        let mut input = InputLine::default();
        self.previous_columns = 0;
        loop {
            self.render_input(label, &input)?;
            match next_event(cancelled)? {
                Event::Key(key) if key.code == KeyCode::Enter => {
                    self.newline()?;
                    return Ok(input.into_text());
                }
                Event::Key(key) => input.edit(key),
                Event::Paste(source) => {
                    input.paste(&source);
                }
                _ => {}
            }
        }
    }

    /// Asks a yes/no question of one key: `y`/`n` answer, Enter takes the default, and every
    /// other key is ignored while the prompt waits.
    ///
    /// A terminal with no cursor control keeps the whole-line reader below, where an answer is
    /// typed and read back.
    fn confirm(
        &mut self,
        label: &str,
        default: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool> {
        let hint = if default { "Y/n" } else { "y/N" };
        let label = format!("{label} [{hint}] ");
        if !self.cursor_controls {
            return self.confirm_line(&label, default, cancelled);
        }
        self.previous_columns = 0;
        self.render_input(&label, &InputLine::default())?;
        loop {
            let Event::Key(key) = next_event(cancelled)? else {
                continue;
            };
            let Some(confirmed) = confirmation_key(key.code, default) else {
                continue;
            };
            // An answer is painted where a typed one would stand, then the line is let go.
            let mut input = InputLine::default();
            if let KeyCode::Char(character) = key.code {
                input.insert(&character.to_string());
                self.render_input(&label, &input)?;
            }
            self.newline()?;
            return Ok(confirmed);
        }
    }

    /// The whole-line reader: an answer is typed, and an answer that says neither yes nor no is
    /// asked again.
    fn confirm_line(
        &mut self,
        label: &str,
        default: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool> {
        loop {
            let answer = self.read_line(label, cancelled)?;
            if let Some(confirmed) = confirmation(&answer, default) {
                return Ok(confirmed);
            }
            write!(self.output, "Answer y or n.\r\n")?;
        }
    }

    fn render_input(&mut self, label: &str, input: &InputLine) -> io::Result<()> {
        let columns = usize::from(dimensions().0).saturating_sub(1);
        let label = super::text::single_line(label);
        let label = super::text::window(&label, columns.saturating_sub(4), 0);
        let available = columns.saturating_sub(label.text.width());
        let displayed = super::text::single_line(input.text());
        let cursor = super::text::single_line(input.before_cursor()).width();
        let window = super::text::window(&displayed, available, cursor);
        let cursor = label.text.width() + cursor.saturating_sub(window.start_column).min(available);
        if self.cursor_controls {
            execute!(self.output, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
        } else {
            write!(self.output, "\r{}\r", " ".repeat(self.previous_columns))?;
        }
        write!(self.output, "{}{}", label.text, window.text)?;
        self.previous_columns = label.text.width() + window.text.width();
        if self.cursor_controls {
            execute!(
                self.output,
                MoveToColumn(cursor.min(u16::MAX as usize) as u16)
            )?;
        } else {
            self.output.write_all(
                "\x08"
                    .repeat(self.previous_columns.saturating_sub(cursor))
                    .as_bytes(),
            )?;
        }
        self.line_open = true;
        Ok(())
    }

    fn select_many(
        &mut self,
        label: &str,
        choices: &[&str],
        use_color: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<usize>> {
        let mut selected = vec![false; choices.len()];
        self.select_choice(label, choices, Some(&mut selected), 0, use_color, cancelled)?;
        Ok(selected
            .iter()
            .enumerate()
            .filter_map(|(index, selected)| selected.then_some(index))
            .collect())
    }

    fn select_one(
        &mut self,
        label: &str,
        choices: &[&str],
        initial: usize,
        use_color: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<usize> {
        self.select_choice(label, choices, None, initial, use_color, cancelled)
    }

    /// Navigates `choices` until Enter confirms the active one, and returns that index.
    /// `selected` has the toggle state of a multi-choice prompt.
    fn select_choice(
        &mut self,
        label: &str,
        choices: &[&str],
        mut selected: Option<&mut [bool]>,
        initial: usize,
        use_color: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<usize> {
        let mut active = initial;
        let selecting_many = selected.is_some();
        execute!(self.output, Hide)?;
        loop {
            self.render_choices(label, choices, selected.as_deref(), active, use_color)?;
            if let Event::Key(key) = next_event(cancelled)? {
                match key_action(key.code, active, choices.len(), selecting_many) {
                    KeyAction::Confirm => {
                        self.newline()?;
                        return Ok(active);
                    }
                    KeyAction::MoveTo(index) => active = index,
                    KeyAction::Toggle => {
                        if let Some(selected) = selected.as_deref_mut() {
                            selected[active] = !selected[active];
                        }
                    }
                    KeyAction::Ignore => {}
                }
            }
        }
    }

    fn render_choices(
        &mut self,
        label: &str,
        choices: &[&str],
        selected: Option<&[bool]>,
        active: usize,
        use_color: bool,
    ) -> io::Result<()> {
        let (width, height) = dimensions();
        let columns = usize::from(width).saturating_sub(1);
        let visible = usize::from(height)
            .saturating_sub(2)
            .max(1)
            .min(choices.len());
        let start = active.saturating_sub(visible - 1);
        if self.selection_rows > 0 {
            let rows = (self.selection_rows - 1).min(usize::from(height).saturating_sub(1));
            execute!(
                self.output,
                MoveUp(rows as u16),
                MoveToColumn(0),
                Clear(ClearType::FromCursorDown)
            )?;
        }
        let label = super::text::single_line(label);
        write!(
            self.output,
            "{}\r\n",
            super::text::window(&label, columns, 0).text
        )?;
        for index in start..start + visible {
            let mark = if index == active { '>' } else { ' ' };
            let checkbox =
                selected.map_or("", |selected| if selected[index] { " [x]" } else { " [ ]" });
            let row = format!(
                "{mark}{checkbox} {}",
                super::text::single_line(choices[index])
            );
            let row = super::text::window(&row, columns, 0);
            if use_color && index == active {
                write!(self.output, "{}", row.text.cyan().bold())?;
            } else {
                self.output.write_all(row.text.as_bytes())?;
            }
            if index + 1 < start + visible {
                self.output.write_all(b"\r\n")?;
            }
        }
        self.selection_rows = visible + 1;
        self.line_open = true;
        Ok(())
    }

    fn select_numbers(
        &mut self,
        label: &str,
        choices: &[&str],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<usize>> {
        let retry = format!("Choose numbers from 1 to {}.", choices.len());
        self.select_numbered_choices(
            label,
            choices,
            "Numbers separated by commas or spaces: ",
            &retry,
            cancelled,
            |answer| selected_numbers(answer, choices.len()),
        )
    }

    fn select_number(
        &mut self,
        label: &str,
        choices: &[&str],
        initial: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<usize> {
        let retry = format!("Choose one number from 1 to {}.", choices.len());
        self.select_numbered_choices(label, choices, "Number: ", &retry, cancelled, |answer| {
            selected_number(answer, choices.len(), initial)
        })
    }

    /// Renders `label` with numbered choices, then reads answers until `accept` takes one.
    fn select_numbered_choices<T>(
        &mut self,
        label: &str,
        choices: &[&str],
        prompt: &str,
        retry: &str,
        cancelled: &dyn Fn() -> bool,
        accept: impl Fn(&str) -> Option<T>,
    ) -> Result<T> {
        write!(self.output, "{}\r\n", super::text::single_line(label))?;
        for (index, choice) in choices.iter().enumerate() {
            write!(
                self.output,
                "{}. {}\r\n",
                index + 1,
                super::text::single_line(choice)
            )?;
        }
        loop {
            let answer = self.read_line(prompt, cancelled)?;
            if let Some(selected) = accept(&answer) {
                return Ok(selected);
            }
            write!(self.output, "{retry}\r\n")?;
        }
    }

    fn newline(&mut self) -> io::Result<()> {
        if self.line_open {
            self.output.write_all(b"\r\n")?;
            self.line_open = false;
        }
        Ok(())
    }

    fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        let cursor = if self.cursor_controls {
            execute!(self.output, Show, DisableBracketedPaste)
        } else {
            Ok(())
        };
        let newline = self.newline();
        let mode = terminal::disable_raw_mode();
        self.restored = mode.is_ok();
        cursor.and(newline).and(mode)
    }
}

impl Drop for PromptTerminal<'_> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn selected_numbers(source: &str, count: usize) -> Option<Vec<usize>> {
    let mut selected = Vec::new();
    for number in source
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|number| !number.is_empty())
    {
        let index = number.parse::<usize>().ok()?.checked_sub(1)?;
        if index >= count {
            return None;
        }
        selected.push(index);
    }
    selected.sort_unstable();
    selected.dedup();
    Some(selected)
}

fn selected_number(source: &str, count: usize, fallback: usize) -> Option<usize> {
    match selected_numbers(source, count)?.as_slice() {
        [] => Some(fallback),
        [only] => Some(*only),
        _ => None,
    }
}

/// The answer a text prompt keeps: the typed line, or `default` when the line is blank.
fn line_or_default(answer: String, default: &str) -> String {
    if answer.is_empty() {
        default.to_owned()
    } else {
        answer
    }
}

/// What one key press answers a yes/no question, when it answers one at all.
///
/// Enter takes the default the prompt offers, `y`/`n` in either case answer plainly, and any
/// other key is not an answer: the prompt keeps waiting.
fn confirmation_key(code: KeyCode, default: bool) -> Option<bool> {
    match code {
        KeyCode::Enter => Some(default),
        KeyCode::Char('y' | 'Y') => Some(true),
        KeyCode::Char('n' | 'N') => Some(false),
        _ => None,
    }
}

fn confirmation(source: &str, default: bool) -> Option<bool> {
    match source.trim().to_ascii_lowercase().as_str() {
        "" => Some(default),
        "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn control(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn out_of_range_selection_is_rejected() {
        assert_eq!(selected_numbers("3, 1 3", 3), Some(vec![0, 2]));
        assert_eq!(selected_numbers("", 3), Some(vec![]));
        assert_eq!(selected_numbers("0", 3), None);
        assert_eq!(selected_numbers("4", 3), None);
    }

    #[test]
    fn one_choice_accepts_one_number() {
        assert_eq!(selected_number("2", 3, 0), Some(1));
        assert_eq!(selected_number("1 2", 3, 0), None);
        assert_eq!(selected_number("4", 3, 0), None);
    }

    #[test]
    fn blank_number_choice_keeps_default() {
        assert_eq!(selected_number("", 3, 2), Some(2));
    }

    #[test]
    fn blank_line_keeps_default() {
        assert_eq!(line_or_default(String::new(), "."), ".");
        assert_eq!(line_or_default("my-site".to_owned(), "."), "my-site");
    }

    #[test]
    fn confirmation_key_answers_on_one_press() {
        for (code, default, expected) in [
            (KeyCode::Char('y'), false, Some(true)),
            (KeyCode::Char('Y'), false, Some(true)),
            (KeyCode::Char('n'), true, Some(false)),
            (KeyCode::Char('N'), true, Some(false)),
            (KeyCode::Enter, true, Some(true)),
            (KeyCode::Enter, false, Some(false)),
            (KeyCode::Esc, true, None),
            (KeyCode::Char('q'), true, None),
            (KeyCode::Backspace, true, None),
        ] {
            assert_eq!(
                confirmation_key(code, default),
                expected,
                "{code:?} with default {default}"
            );
        }
    }

    #[test]
    fn confirmation_parses_answers() {
        assert_eq!(confirmation("y", false), Some(true));
        assert_eq!(confirmation("NO", true), Some(false));
        assert_eq!(confirmation("", true), Some(true));
        assert_eq!(confirmation("maybe", false), None);
    }

    #[test]
    fn navigation_keys_move_active_choice() {
        let moves = [
            (KeyCode::Down, 0, 1),
            (KeyCode::Tab, 0, 1),
            (KeyCode::Up, 1, 0),
            (KeyCode::BackTab, 1, 0),
            (KeyCode::Down, 2, 0),
            (KeyCode::Tab, 2, 0),
            (KeyCode::Up, 0, 2),
            (KeyCode::BackTab, 0, 2),
            (KeyCode::Home, 1, 0),
            (KeyCode::End, 1, 2),
        ];
        for (code, active, expected) in moves {
            assert_eq!(
                key_action(code, active, 3, false),
                KeyAction::MoveTo(expected),
                "{code:?} from {active}"
            );
        }
    }

    #[test]
    fn enter_confirms_active_choice() {
        assert_eq!(key_action(KeyCode::Enter, 1, 3, false), KeyAction::Confirm);
        assert_eq!(key_action(KeyCode::Enter, 1, 3, true), KeyAction::Confirm);
    }

    #[test]
    fn space_toggles_when_selecting_many() {
        assert_eq!(
            key_action(KeyCode::Char(' '), 1, 3, true),
            KeyAction::Toggle
        );
    }

    #[test]
    fn space_ignored_when_selecting_one() {
        assert_eq!(
            key_action(KeyCode::Char(' '), 1, 3, false),
            KeyAction::Ignore
        );
    }

    #[test]
    fn dismissal_keys_dismiss_prompt() {
        for dismissing in [
            key(KeyCode::Esc),
            control(KeyCode::Char('c')),
            control(KeyCode::Char('d')),
        ] {
            assert!(dismisses_prompt(&dismissing), "{dismissing:?}");
        }
        for retained in [
            key(KeyCode::Char('c')),
            key(KeyCode::Char('d')),
            control(KeyCode::Char('x')),
        ] {
            assert!(!dismisses_prompt(&retained), "{retained:?}");
        }
    }

    #[test]
    fn cancelled_prompts_read_nothing() {
        let (sink, output) = OutputSink::buffered();
        let prompt = PromptReader::new(sink);
        let cancelled = &|| true;
        assert!(
            prompt
                .select_many("Choose: ", &["first"], false, cancelled)
                .unwrap_err()
                .is::<InputCancelled>()
        );
        assert!(
            prompt
                .select_one("Choose: ", &["first"], 0, false, cancelled)
                .unwrap_err()
                .is::<InputCancelled>()
        );
        assert!(
            prompt
                .read_line("Site directory", ".", cancelled)
                .unwrap_err()
                .is::<InputCancelled>()
        );
        assert!(
            prompt
                .confirm("Create this site?", true, cancelled)
                .unwrap_err()
                .is::<InputCancelled>()
        );
        assert!(output.is_empty());
    }
}
