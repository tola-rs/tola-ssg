//! Development diagnostics and progress drawn as one complete terminal frame.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use anstyle_parse::{DefaultCharAccumulator, Params, Parser, Perform};
use chrono::{DateTime, Local};
use crossterm::execute;
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use tola_build::diagnostic::Diagnostic;

use super::session::{self, Claim, ProcessTerminal, ViewModes};
use super::sink::{OutputSink, StreamWriter};
use super::style::Palette;
use super::ui::{Action, Screen, hints, keymap};

const HISTORY_ROUNDS: usize = 64;
const HOOK_LINES: usize = 200;
const FRAME_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy)]
pub(crate) enum Completion<'a> {
    Published(&'a str),
    Failed(&'a str),
}

pub(crate) struct Round {
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) transcript: String,
    pub(crate) observed_at: DateTime<Local>,
}

impl Round {
    fn same_diagnostics(&self, other: &Self) -> bool {
        self.diagnostics
            .iter()
            .all(|diagnostic| other.diagnostics.contains(diagnostic))
            && other
                .diagnostics
                .iter()
                .all(|diagnostic| self.diagnostics.contains(diagnostic))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct HookRun {
    pub(crate) scope: u64,
    pub(crate) name: String,
}

struct Job {
    run: HookRun,
    stage: String,
    lines: VecDeque<(usize, String)>,
    partial: [String; 2],
    carriage_return: [bool; 2],
}

impl Job {
    fn output(&mut self, stream: &str, chunk: &str) {
        let stream = usize::from(stream == "stderr");
        for fragment in super::text::hook_fragments(chunk, &mut self.carriage_return[stream]) {
            let complete = fragment.ends_with('\n');
            self.partial[stream].push_str(fragment.trim_end_matches('\n'));
            if complete {
                self.lines
                    .push_back((stream, std::mem::take(&mut self.partial[stream])));
                if self.lines.len() > HOOK_LINES {
                    self.lines.pop_front();
                }
            }
        }
    }

    fn body(&self) -> Vec<String> {
        let mut lines = vec![format!("{}/{}", self.stage, self.run.name)];
        for (stream, text) in self
            .lines
            .iter()
            .map(|(stream, line)| (*stream, line))
            .chain(
                self.partial
                    .iter()
                    .enumerate()
                    .filter(|(_, line)| !line.is_empty()),
            )
        {
            let label = if stream == 0 { "stdout" } else { "stderr" };
            lines.push(format!("[{label}] {text}"));
        }
        lines
    }
}

/// Drop stops the input worker before the development command releases its terminal.
pub(crate) struct DevView {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<io::Result<()>>>,
}

impl DevView {
    pub(crate) fn start(
        sink: &OutputSink,
        palette: Palette,
        log: Option<String>,
        cancellation: crate::cancellation::Cancellation,
    ) -> io::Result<Option<Self>> {
        if !sink.open_development(palette, log)? {
            return Ok(None);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let requested = Arc::clone(&stop);
        let worker = ViewWorker {
            sink: sink.clone(),
            cancellation,
        };
        let spawned = std::thread::Builder::new()
            .name("tola-dev-view".to_owned())
            .spawn(move || {
                let shown = run(&worker.sink, &requested, &worker.cancellation);
                let finished = shown.and(worker.sink.close_development());
                if finished.is_err() {
                    worker.cancellation.request();
                }
                finished
            });
        spawned.map(|thread| {
            Some(Self {
                stop,
                thread: Some(thread),
            })
        })
    }

    pub(crate) fn finish(mut self) -> io::Result<()> {
        self.join()
    }

    fn join(&mut self) -> io::Result<()> {
        self.stop.store(true, Ordering::Release);
        match self.thread.take() {
            Some(thread) => thread
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("development view stopped unexpectedly"))),
            None => Ok(()),
        }
    }
}

impl Drop for DevView {
    fn drop(&mut self) {
        let _ = self.join();
    }
}

struct ViewWorker {
    sink: OutputSink,
    cancellation: crate::cancellation::Cancellation,
}

impl Drop for ViewWorker {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.cancellation.request();
        }
        let _ = self.sink.close_development();
    }
}

fn run(
    sink: &OutputSink,
    stop: &AtomicBool,
    cancellation: &crate::cancellation::Cancellation,
) -> io::Result<()> {
    let mut terminal = ProcessTerminal::new(sink);
    let requested = || stop.load(Ordering::Acquire) || cancellation.is_requested();
    while !requested() {
        match sink.draw_development(None, cancellation) {
            Ok(true) => break,
            Ok(false) => {}
            Err(_) if requested() => break,
            Err(error) => return Err(error),
        }
        // Polling owns no output lock: producers can write while the reader waits for a key.
        let event = match session::next_event(&mut terminal, FRAME_INTERVAL, &requested) {
            Ok(event) => event,
            Err(_) if requested() => break,
            Err(error) => return Err(io::Error::other(error)),
        };
        if let Some(event) = event
            && sink.draw_development(Some(&event), cancellation)?
        {
            break;
        }
    }
    Ok(())
}

#[derive(Default)]
struct Rounds {
    entries: Vec<Arc<Round>>,
    latest: Option<Arc<Round>>,
    selected: Option<Arc<Round>>,
    top: usize,
    body_rows: usize,
    body_lines: usize,
}

impl Rounds {
    /// A result takes over the view: whatever the reader was reading is a superseded attempt, so
    /// the position it held goes with it. The one exception is a result repeating the visible
    /// diagnostics, which refreshes that entry where it sits and keeps the reading offset.
    fn push(&mut self, round: Round) {
        let round = Arc::new(round);
        let refreshing = !round.diagnostics.is_empty()
            && self
                .current()
                .is_some_and(|current| current.same_diagnostics(&round));
        if !round.diagnostics.is_empty() {
            if let Some(index) = self
                .entries
                .iter()
                .position(|previous| previous.same_diagnostics(&round))
            {
                let previous = self.entries.remove(index);
                if self
                    .selected
                    .as_ref()
                    .is_some_and(|selected| Arc::ptr_eq(selected, &previous))
                {
                    self.selected = Some(Arc::clone(&round));
                }
            }
            self.entries.push(Arc::clone(&round));
            if self.entries.len() > HISTORY_ROUNDS {
                let evicted = usize::from(
                    self.selected
                        .as_ref()
                        .is_some_and(|selected| Arc::ptr_eq(selected, &self.entries[0])),
                );
                self.entries.remove(evicted);
            }
        }
        if !refreshing {
            self.selected = None;
            self.top = 0;
        }
        self.latest = Some(round);
    }

    fn current(&self) -> Option<&Arc<Round>> {
        self.selected.as_ref().or(self.latest.as_ref())
    }

    fn index(&self) -> Option<usize> {
        self.current().and_then(|current| {
            self.entries
                .iter()
                .position(|round| Arc::ptr_eq(current, round))
        })
    }

    fn previous(&mut self) {
        if !self.live(Action::PreviousRound) {
            return;
        }
        let index = self
            .index()
            .map_or(self.entries.len().saturating_sub(1), |index| {
                index.saturating_sub(1)
            });
        self.selected = self.entries.get(index).cloned();
        self.top = 0;
    }

    fn next(&mut self) {
        if !self.live(Action::NextRound) {
            return;
        }
        self.selected = self
            .index()
            .and_then(|index| self.entries.get(index + 1).cloned());
        self.top = 0;
    }

    fn scroll(&mut self, lines: isize) {
        let last = self.last_top();
        self.top = self.top.saturating_add_signed(lines).min(last);
    }

    /// The top of the last body screenful.
    fn last_top(&self) -> usize {
        self.body_lines.saturating_sub(self.body_rows.max(1))
    }

    /// Whether one action still changes the view: a body that fits, an end already reached, or a
    /// single round earns no hint.
    fn live(&self, action: Action) -> bool {
        match action {
            Action::Up | Action::PageUp => self.top > 0,
            Action::Down | Action::PageDown => self.top < self.last_top(),
            Action::First => self.top != 0,
            Action::Last => self.top != self.last_top(),
            Action::PreviousRound => self
                .index()
                .map_or(!self.entries.is_empty(), |index| index > 0),
            Action::NextRound => self
                .selected
                .as_ref()
                .zip(self.latest.as_ref())
                .is_some_and(|(selected, latest)| !Arc::ptr_eq(selected, latest)),
            _ => true,
        }
    }
}

pub(crate) struct DevScreen {
    screen: Screen<CrosstermBackend<StreamWriter>>,
    writer: StreamWriter,
    modes: ViewModes<StreamWriter>,
    size: Size,
    palette: Palette,
    log: Option<String>,
    hooks: Option<String>,
    serving: Option<String>,
    rounds: Rounds,
    jobs: Vec<Job>,
    message: Option<String>,
    dirty: bool,
}

impl DevScreen {
    pub(crate) fn open(
        writer: StreamWriter,
        palette: Palette,
        log: Option<String>,
        claim: Claim,
        size: Size,
    ) -> io::Result<Self> {
        let modes = ViewModes::enter(writer.clone(), claim)?;
        let screen = Screen::new(
            CrosstermBackend::new(writer.clone()),
            size.width,
            size.height,
        )?;
        Ok(Self {
            screen,
            writer,
            modes,
            size,
            palette,
            log,
            hooks: None,
            serving: None,
            rounds: Rounds::default(),
            jobs: Vec::new(),
            message: None,
            dirty: true,
        })
    }

    pub(crate) fn round(&mut self, round: Round) {
        self.rounds.push(round);
        self.message = None;
        self.dirty = true;
    }

    pub(crate) fn activity(&mut self, text: &str) {
        self.message = Some(text.trim_end_matches('\n').to_owned());
        self.dirty = true;
    }

    pub(crate) fn hooks(&mut self, text: &str) {
        self.hooks = Some(text.trim_end_matches('\n').to_owned());
        self.dirty = true;
    }

    pub(crate) fn serving(&mut self, text: &str) {
        self.serving = Some(text.trim_end_matches('\n').to_owned());
        self.dirty = true;
    }

    pub(crate) fn job_started(&mut self, run: HookRun, stage: &str) {
        self.jobs.push(Job {
            run,
            stage: stage.to_owned(),
            lines: VecDeque::new(),
            partial: Default::default(),
            carriage_return: [false; 2],
        });
        self.dirty = true;
    }

    pub(crate) fn job_output(&mut self, run: &HookRun, stream: &str, chunk: &str) {
        if let Some(job) = self.jobs.iter_mut().find(|job| job.run == *run) {
            job.output(stream, chunk);
            self.dirty = true;
        }
    }

    pub(crate) fn job_finished(&mut self, run: &HookRun) {
        self.jobs.retain(|job| job.run != *run);
        self.dirty = true;
    }

    pub(crate) fn answer_event(
        &mut self,
        event: &crossterm::event::Event,
        cancellation: &crate::cancellation::Cancellation,
    ) -> bool {
        use crossterm::event::{Event, KeyEventKind, MouseEventKind};
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if super::ui::cancels(key) {
                    cancellation.interrupt();
                    return true;
                }
                if let Some(action) = keymap::DEV.action(key) {
                    self.answer(action);
                }
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => self.answer(Action::Up),
                MouseEventKind::ScrollDown => self.answer(Action::Down),
                _ => {}
            },
            Event::Resize(_, _) => self.dirty = true,
            _ => {}
        }
        false
    }

    fn answer(&mut self, action: Action) {
        let page = self.rounds.body_rows.max(1) as isize;
        match action {
            Action::PreviousRound => self.rounds.previous(),
            Action::NextRound => self.rounds.next(),
            Action::Up => self.rounds.scroll(-1),
            Action::Down => self.rounds.scroll(1),
            Action::PageUp => self.rounds.scroll(-page),
            Action::PageDown => self.rounds.scroll(page),
            Action::First => self.rounds.top = 0,
            Action::Last => self.rounds.top = usize::MAX,
            _ => return,
        }
        self.dirty = true;
    }

    pub(crate) fn prepare(&mut self) -> io::Result<()> {
        if !self.modes.is_active() {
            return Err(io::Error::other("development view stopped"));
        }
        let (width, height) = crossterm::terminal::size()?;
        let size = Size::new(width, height);
        if size != self.size {
            self.size = size;
            self.dirty = true;
        }
        Ok(())
    }

    pub(crate) fn draw(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if self.size.width == 0 || self.size.height == 0 {
            self.screen.draw_frame(self.size, |_| {})?;
            return Ok(());
        }
        let header = [
            self.serving.as_deref(),
            self.log.as_deref(),
            self.hooks.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
        let header = styled_paragraph(&header);
        let header_lines = if self.log.is_none() && self.hooks.is_none() && self.serving.is_none() {
            0
        } else {
            header.line_count(self.size.width)
        };
        let job_lines = self
            .jobs
            .iter()
            .map(|job| styled_paragraph(&job.body().join("\n")).line_count(self.size.width))
            .sum::<usize>();
        let jobs = &self.jobs;
        let [header_area, counter_area, body_area, jobs_area, footer_area] = frame_areas(
            Rect::new(0, 0, self.size.width, self.size.height),
            header_lines,
            job_lines,
        );
        let counter = match self.rounds.current() {
            Some(round) => {
                let time = round.observed_at.format("%H:%M:%S");
                match self.rounds.index() {
                    Some(index) => {
                        format!("round {}/{} {time}", index + 1, self.rounds.entries.len())
                    }
                    None => format!("Ready {time}"),
                }
            }
            None => "Building site…".to_owned(),
        };
        let mut body = self
            .message
            .as_ref()
            .map_or_else(String::new, |message| format!("{message}\n"));
        if let Some(round) = self.rounds.current() {
            body.push_str(round.transcript.trim_end_matches('\n'));
        }
        if body.trim().is_empty() && counter_area.height == 0 {
            body.push_str(&counter);
        }
        let body = styled_paragraph(&body);
        self.rounds.body_rows = usize::from(body_area.height);
        self.rounds.body_lines = body.line_count(self.size.width);
        self.rounds.top = self.rounds.top.min(self.rounds.last_top());
        let body = body.scroll((u16::try_from(self.rounds.top).unwrap_or(u16::MAX), 0));
        let palette = self.palette;
        let counter = Line::from(Span::styled(counter, palette.dim_style()));
        execute!(self.writer, BeginSynchronizedUpdate)?;
        let drawn = self.screen.draw_frame(self.size, |frame| {
            frame.render_widget(header, header_area);
            frame.render_widget(Paragraph::new(counter), counter_area);
            frame.render_widget(body, body_area);
            draw_jobs(frame, jobs_area, jobs);
            hints::draw(
                frame,
                footer_area,
                "",
                &live_actions(&self.rounds),
                &keymap::DEV,
                palette,
            );
        });
        let ended = execute!(self.writer, EndSynchronizedUpdate);
        drawn.and(ended)?;
        self.dirty = false;
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> io::Result<()> {
        let restored = self.modes.finish();
        let printed = match self.rounds.current() {
            Some(round) => self.writer.write_all(round.transcript.as_bytes()),
            None => Ok(()),
        };
        restored.and(printed)
    }
}

fn frame_areas(area: Rect, header_lines: usize, job_lines: usize) -> [Rect; 5] {
    let footer = u16::from(area.height > 1);
    let counter = u16::from(area.height > 2);
    let available = area.height.saturating_sub(footer + counter);
    // Header and running hooks each leave room to read the build result.
    let header = header_lines.min(usize::from(available.saturating_sub(1) / 2)) as u16;
    let free = available - header;
    let jobs = job_lines.min(usize::from(free.saturating_sub(1) / 2)) as u16;
    Layout::vertical([
        Constraint::Length(header),
        Constraint::Length(counter),
        Constraint::Length(free - jobs),
        Constraint::Length(jobs),
        Constraint::Length(footer),
    ])
    .areas(area)
}

fn draw_jobs(frame: &mut ratatui::Frame<'_>, area: Rect, jobs: &[Job]) {
    let mut top = area.y;
    let visible = jobs.len().min(usize::from(area.height));
    for (index, job) in jobs.iter().take(visible).enumerate() {
        let rows = usize::from(area.bottom() - top) / (visible - index);
        let rows = rows as u16;
        let lines = job.body();
        frame.render_widget(
            Paragraph::new(lines[0].clone()),
            Rect::new(area.x, top, area.width, 1),
        );
        let tail_area = Rect::new(area.x, top + 1, area.width, rows - 1);
        let tail = styled_paragraph(&lines[1..].join("\n"));
        let offset = tail
            .line_count(area.width)
            .saturating_sub(usize::from(tail_area.height));
        frame.render_widget(
            tail.scroll((u16::try_from(offset).unwrap_or(u16::MAX), 0)),
            tail_area,
        );
        top += rows;
    }
}

const ACCEPTED: [Action; 9] = [
    Action::PreviousRound,
    Action::NextRound,
    Action::Up,
    Action::Down,
    Action::PageUp,
    Action::PageDown,
    Action::First,
    Action::Last,
    Action::Quit,
];

fn styled_paragraph(text: &str) -> Paragraph<'static> {
    Paragraph::new(Text::from(styled_lines(text))).wrap(Wrap { trim: false })
}

/// The dev keys whose action still changes the view.
fn live_actions(rounds: &Rounds) -> Vec<Action> {
    ACCEPTED
        .into_iter()
        .filter(|action| rounds.live(*action))
        .collect()
}

/// The dev view's text as styled lines, every control sequence applied or consumed whole.
///
/// An SGR sequence chooses the style of what follows; any other sequence, a hyperlink's OSC
/// included, is control text that never reaches the frame as characters.
fn styled_lines(text: &str) -> Vec<Line<'static>> {
    struct Stylist {
        lines: Vec<Line<'static>>,
        run: String,
        style: Style,
    }

    impl Stylist {
        fn flush(&mut self) {
            if !self.run.is_empty() {
                push_span(
                    self.lines.last_mut().expect("one line"),
                    &self.run,
                    self.style,
                );
                self.run.clear();
            }
        }
    }

    impl Perform for Stylist {
        fn print(&mut self, character: char) {
            self.run.push(character);
        }

        fn execute(&mut self, byte: u8) {
            match byte {
                b'\n' => {
                    self.flush();
                    self.lines.push(Line::default());
                }
                b'\t' => self.run.push_str("    "),
                _ => {}
            }
        }

        fn csi_dispatch(
            &mut self,
            params: &Params,
            _intermediates: &[u8],
            _ignore: bool,
            action: u8,
        ) {
            if action == b'm' {
                self.flush();
                apply_sgr(&mut self.style, params);
            }
        }
    }

    let mut stylist = Stylist {
        lines: vec![Line::default()],
        run: String::new(),
        style: Style::default(),
    };
    let mut parser = Parser::<DefaultCharAccumulator>::new();
    for byte in text.bytes() {
        parser.advance(&mut stylist, byte);
    }
    stylist.flush();
    stylist.lines
}

fn apply_sgr(style: &mut Style, params: &Params) {
    let mut values = params
        .iter()
        .map(|values| values.first().copied().unwrap_or(0));
    while let Some(value) = values.next() {
        match value {
            0 => *style = Style::default(),
            1 => *style = style.add_modifier(Modifier::BOLD),
            2 => *style = style.add_modifier(Modifier::DIM),
            4 => *style = style.add_modifier(Modifier::UNDERLINED),
            22 => *style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            24 => *style = style.remove_modifier(Modifier::UNDERLINED),
            30..=37 | 90..=97 => *style = style.fg(ansi_color(value)),
            38 => {
                if values.next() == Some(5)
                    && let Some(color) = values.next()
                {
                    *style = style.fg(Color::Indexed(color as u8));
                }
            }
            39 => *style = style.fg(Color::Reset),
            _ => {}
        }
    }
}

fn ansi_color(value: u16) -> Color {
    const COLORS: [Color; 16] = [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    COLORS[usize::from(if value >= 90 {
        value - 90 + 8
    } else {
        value - 30
    })]
}

fn push_span(line: &mut Line<'static>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = line.spans.last_mut().filter(|last| last.style == style) {
        last.content.to_mut().push_str(text);
    } else {
        line.spans.push(Span::styled(text.to_owned(), style));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use tola_build::diagnostic::Severity;

    fn diagnostic(path: &str) -> Diagnostic {
        Diagnostic::at_path(
            tola_build::codes::typst::COMPILE,
            Severity::Error,
            path,
            "expected expression",
        )
    }

    fn round(diagnostics: Vec<Diagnostic>, second: i64, transcript: &str) -> Round {
        Round {
            diagnostics,
            transcript: transcript.to_owned(),
            observed_at: Local.timestamp_opt(second, 0).single().unwrap(),
        }
    }

    #[test]
    fn clean_reports_stay_outside_history() {
        let mut rounds = Rounds::default();
        rounds.push(round(Vec::new(), 1, "Built site"));
        assert!(rounds.entries.is_empty());
        assert_eq!(rounds.current().unwrap().transcript, "Built site");
    }

    #[test]
    fn repeated_diagnostics_refresh_history() {
        let mut rounds = Rounds::default();
        let first = diagnostic("content/first.typ");
        let second = diagnostic("content/second.typ");
        rounds.push(round(vec![first.clone(), second.clone()], 1, "old report"));
        rounds.selected = rounds.entries.first().cloned();
        rounds.top = 9;
        rounds.push(round(
            vec![diagnostic("content/other.typ")],
            2,
            "other report",
        ));
        rounds.selected = rounds.entries.first().cloned();
        rounds.top = 9;
        rounds.push(round(vec![second, first], 3, "refreshed report"));
        assert_eq!(rounds.entries.len(), 2);
        let refreshed = rounds.entries.last().unwrap();
        assert_eq!(refreshed.transcript, "refreshed report");
        assert_eq!(
            refreshed.observed_at,
            Local.timestamp_opt(1, 0).single().unwrap() + Duration::seconds(2)
        );
        assert!(Arc::ptr_eq(rounds.selected.as_ref().unwrap(), refreshed));
        assert_eq!(rounds.top, 9);
    }

    #[test]
    fn source_locations_keep_distinct_reports() {
        let mut rounds = Rounds::default();
        rounds.push(round(vec![diagnostic("content/first.typ")], 1, "first"));
        rounds.push(round(vec![diagnostic("content/second.typ")], 2, "second"));
        assert_eq!(rounds.entries.len(), 2);
    }

    #[test]
    fn new_round_takes_over_the_view() {
        let mut rounds = Rounds::default();
        rounds.push(round(vec![diagnostic("content/first.typ")], 1, "failed"));
        rounds.previous();
        rounds.top = 9;

        rounds.push(round(Vec::new(), 2, "Built site"));
        assert!(
            rounds.selected.is_none(),
            "a clean build shows its own result"
        );
        assert_eq!(rounds.current().unwrap().transcript, "Built site");
        assert_eq!(rounds.top, 0);

        rounds.previous();
        rounds.push(round(
            vec![diagnostic("content/second.typ")],
            3,
            "failed again",
        ));
        assert!(
            rounds.selected.is_none(),
            "another failure shows its own result"
        );
        assert_eq!(rounds.current().unwrap().transcript, "failed again");
    }

    #[test]
    fn history_returns_to_clean_result() {
        let mut rounds = Rounds::default();
        rounds.push(round(vec![diagnostic("content/first.typ")], 1, "failed"));
        rounds.push(round(Vec::new(), 2, "Built site"));
        assert!(rounds.live(Action::PreviousRound));
        assert!(!rounds.live(Action::NextRound));
        rounds.previous();
        assert_eq!(rounds.current().unwrap().transcript, "failed");
        assert!(!rounds.live(Action::PreviousRound));
        assert!(rounds.live(Action::NextRound));
        rounds.next();
        assert_eq!(rounds.current().unwrap().transcript, "Built site");
        assert!(!rounds.live(Action::NextRound));
    }

    #[test]
    fn unchanged_diagnostics_keep_scroll() {
        let mut rounds = Rounds::default();
        rounds.push(round(vec![diagnostic("content/first.typ")], 1, "failed"));
        rounds.top = 9;
        rounds.push(round(
            vec![diagnostic("content/first.typ")],
            2,
            "failed again",
        ));
        assert_eq!(rounds.top, 9);
        assert_eq!(rounds.current().unwrap().transcript, "failed again");
        rounds.previous();
        rounds.next();
        assert_eq!(
            rounds.top, 9,
            "unavailable history movements leave the view alone"
        );
    }

    #[test]
    fn short_frames_preserve_body() {
        for width in [1, 20, 40, 80, 160] {
            for height in [1, 2, 3, 8, 24, 50] {
                let area = Rect::new(0, 0, width, height);
                let areas = frame_areas(area, 200, 200);
                assert!(areas[2].height > 0);
                assert_eq!(areas[4].height, u16::from(height > 1));
                assert_eq!(areas.iter().map(|part| part.height).sum::<u16>(), height);
                assert!(
                    areas
                        .iter()
                        .all(|part| area.contains(part.as_position()) || part.is_empty())
                );
            }
        }
    }

    #[test]
    fn short_hook_area_shows_jobs() {
        let jobs = ["styles", "search", "assets"].map(|name| Job {
            run: HookRun {
                scope: 0,
                name: name.to_owned(),
            },
            stage: "generate".to_owned(),
            lines: VecDeque::new(),
            partial: Default::default(),
            carriage_return: [false; 2],
        });
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(24, 2)).unwrap();
        terminal
            .draw(|frame| draw_jobs(frame, frame.area(), &jobs))
            .unwrap();
        let drawn = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(drawn.contains("styles"));
        assert!(drawn.contains("search"));
    }

    #[test]
    fn diagnostic_colors_survive_framing() {
        for severity in [Severity::Error, Severity::Warning] {
            let diagnostic = Diagnostic::new(
                tola_build::codes::typst::COMPILE,
                severity,
                "expected expression",
            );
            let rendered = crate::terminal::render_diagnostic(&diagnostic, None, true);
            assert!(
                styled_lines(&rendered)
                    .iter()
                    .flat_map(|line| &line.spans)
                    .any(|span| span.style.fg.is_some_and(|color| color != Color::Reset))
            );
        }
    }

    #[test]
    fn osc_payloads_never_become_text() {
        let lines =
            styled_lines("a\u{1b}]8;;http://127.0.0.1:1/\u{1b}\\link\u{1b}]8;;\u{1b}\\\u{1b}[4mb");
        let drawn = lines
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(drawn, "alinkb", "a hyperlink's address never becomes text");
        assert!(
            lines
                .iter()
                .flat_map(|line| &line.spans)
                .last()
                .is_some_and(|span| span.style.add_modifier.contains(Modifier::UNDERLINED)),
            "the SGR that follows still styles"
        );
    }

    #[test]
    fn hints_follow_body_and_round_ends() {
        let mut rounds = Rounds::default();
        rounds.push(round(vec![diagnostic("content/first.typ")], 1, "failed"));
        rounds.body_rows = 10;
        rounds.body_lines = 30;
        assert!(!rounds.live(Action::Up));
        assert!(!rounds.live(Action::First));
        assert!(rounds.live(Action::Down));
        assert!(rounds.live(Action::Last));
        rounds.top = rounds.last_top();
        assert!(!rounds.live(Action::Down));
        assert!(!rounds.live(Action::Last));
        assert!(rounds.live(Action::Up));

        assert!(!rounds.live(Action::PreviousRound));
        assert!(!rounds.live(Action::NextRound));
        rounds.push(round(vec![diagnostic("content/second.typ")], 2, "again"));
        assert!(rounds.live(Action::PreviousRound), "two rounds to walk");
        assert!(!rounds.live(Action::NextRound));
        rounds.previous();
        assert!(!rounds.live(Action::PreviousRound));
        assert!(rounds.live(Action::NextRound));

        rounds.body_rows = 30;
        assert!(
            !rounds.live(Action::Down),
            "a body that fits does not scroll"
        );
        assert!(!rounds.live(Action::Last));
    }

    #[test]
    fn hook_chunks_keep_stream_lines() {
        let mut job = Job {
            run: HookRun {
                scope: 0,
                name: "assets".to_owned(),
            },
            stage: String::new(),
            lines: VecDeque::new(),
            partial: Default::default(),
            carriage_return: [false; 2],
        };
        job.output("stdout", "first");
        job.output("stdout", " line\r");
        job.output("stdout", "\n");
        job.output("stderr", "failure\n");
        assert_eq!(
            job.lines
                .iter()
                .map(|(stream, line)| (*stream, line.as_str()))
                .collect::<Vec<_>>(),
            [(0, "first line"), (1, "failure")]
        );
    }
}
