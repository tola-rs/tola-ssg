//! Development diagnostics and progress drawn as one complete terminal frame.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, NaiveDate};
use crossterm::execute;
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use tola_build::diagnostic::Diagnostic;

use super::ansi::{self, Segment};
use super::session::{self, Claim, ProcessTerminal, ViewModes};
use super::sink::{OutputSink, StreamWriter};
use super::style::Palette;
use super::ui::{Action, Screen, hints, keymap};

const HISTORY_ROUNDS: usize = 64;
const HOOK_LINES: usize = 200;
const FRAME_INTERVAL: Duration = Duration::from_millis(50);
/// How long a copy confirmation stays above the hints.
const NOTICE_LIFETIME: Duration = Duration::from_secs(2);

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

/// The stamp a round carries in the counter: a reader places today's rounds by
/// time alone, while an earlier day is named, because its bare time would read
/// as today's.
fn round_stamp(observed_at: DateTime<Local>, today: NaiveDate) -> String {
    let format = if observed_at.date_naive() == today {
        "%H:%M:%S"
    } else {
        "%Y-%m-%d %H:%M:%S"
    };
    observed_at.format(format).to_string()
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct HookRun {
    pub(crate) scope: u64,
    pub(crate) name: String,
}

struct Job {
    run: HookRun,
    stage: String,
    lines: VecDeque<(usize, Vec<Segment>)>,
    partial: [Vec<Segment>; 2],
    parsers: [ansi::Parser; 2],
    omitted_lines: usize,
}

impl Job {
    fn new(run: HookRun, stage: &str) -> Self {
        Self {
            run,
            stage: stage.to_owned(),
            lines: VecDeque::new(),
            partial: Default::default(),
            parsers: [ansi::Parser::new(), ansi::Parser::new()],
            omitted_lines: 0,
        }
    }

    fn output(&mut self, stream: &str, chunk: &str) {
        let stream = stream_index(stream);
        for segment in self.parsers[stream].push(chunk) {
            if segment.text == "\n" {
                self.lines
                    .push_back((stream, std::mem::take(&mut self.partial[stream])));
                if self.lines.len() > HOOK_LINES {
                    self.lines.pop_front();
                    self.omitted_lines += 1;
                }
            } else {
                push_segment(&mut self.partial[stream], segment);
            }
        }
    }

    fn body(&self, styled: bool) -> Vec<Line<'static>> {
        let mut header = format!("{}/{}", self.stage, self.run.name);
        if self.omitted_lines > 0 {
            let omitted = super::progress::plural_count(self.omitted_lines, "earlier line");
            header.push_str(&format!(" · {omitted} not shown"));
        }
        let mut lines = vec![Line::from(header)];
        let partial = self
            .partial
            .iter()
            .enumerate()
            .filter(|(_, segments)| !segments.is_empty());
        for (stream, segments) in self
            .lines
            .iter()
            .map(|(stream, segments)| (*stream, segments.as_slice()))
            .chain(partial.map(|(stream, segments)| (stream, segments.as_slice())))
        {
            let label = if stream == 0 {
                "[stdout] "
            } else {
                "[stderr] "
            };
            let mut spans = vec![Span::raw(label)];
            for segment in segments {
                let style = if styled {
                    segment.style
                } else {
                    Style::default()
                };
                spans.push(Span::styled(tabs_expanded(&segment.text), style));
            }
            lines.push(Line::from(spans));
        }
        lines
    }
}

/// The stream index a hook event names; both streams share one execution's parsers.
pub(crate) fn stream_index(stream: &str) -> usize {
    usize::from(stream == "stderr")
}

/// Adjacent segments under one style are one span.
fn push_segment(segments: &mut Vec<Segment>, segment: Segment) {
    match segments.last_mut() {
        Some(last) if last.style == segment.style => last.text.push_str(&segment.text),
        _ => segments.push(segment),
    }
}

fn tabs_expanded(text: &str) -> String {
    text.replace('\t', "    ")
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

    /// The places round navigation walks: every diagnostic round, plus the latest clean
    /// result, which is a place to stand though it never enters history.
    fn positions(&self) -> usize {
        self.entries.len()
            + usize::from(
                self.latest
                    .as_ref()
                    .is_some_and(|round| round.diagnostics.is_empty()),
            )
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
            Action::Copy => self
                .current()
                .is_some_and(|round| !round.transcript.trim().is_empty()),
            _ => true,
        }
    }
}

/// What the reader's last copy took, and when it was made.
struct Notice {
    text: String,
    shown_at: Instant,
}

impl Notice {
    /// Whether the confirmation has had its time on screen.
    fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.shown_at) >= NOTICE_LIFETIME
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
    notice: Option<Notice>,
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
            notice: None,
            dirty: true,
        })
    }

    pub(crate) fn round(&mut self, round: Round) {
        self.rounds.push(round);
        self.message = None;
        self.notice = None;
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
        self.jobs.push(Job::new(run, stage));
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
                    self.answer(action, cancellation);
                }
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => self.answer(Action::Up, cancellation),
                MouseEventKind::ScrollDown => self.answer(Action::Down, cancellation),
                _ => {}
            },
            Event::Resize(_, _) => self.dirty = true,
            _ => {}
        }
        false
    }

    fn answer(&mut self, action: Action, cancellation: &crate::cancellation::Cancellation) {
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
            Action::Copy => self.copy(cancellation),
            _ => return,
        }
        self.dirty = true;
    }

    /// Put the visible round's text on the reader's clipboard.
    ///
    /// The terminal's own escape is best effort — a terminal only reads it when it allows
    /// programs to write the clipboard — so the system's pasteboard decides what the reader
    /// can trust.
    fn copy(&mut self, cancellation: &crate::cancellation::Cancellation) {
        if !self.rounds.live(Action::Copy) {
            return;
        }
        let Some(round) = self.rounds.current().cloned() else {
            return;
        };
        let text = plain_text(&round.transcript);
        let text = text.trim_end();
        if text.is_empty() {
            return;
        }
        let _ = self
            .writer
            .write_all(super::clipboard::escape(text).as_bytes());
        let copied = match crate::sys::clipboard_command() {
            Some(program) => {
                let mut command = std::process::Command::new(program);
                crate::sys::run_command_with_input(
                    &mut command,
                    text.as_bytes(),
                    &cancellation.token(),
                )
                .is_ok_and(|status| status.success())
            }
            None => true,
        };
        let what = match round.diagnostics.len() {
            0 => "build output".to_owned(),
            count => super::progress::plural_count(count, "diagnostic"),
        };
        self.notice = Some(Notice {
            text: if copied {
                format!("copied {what}")
            } else {
                format!("could not copy {what}")
            },
            shown_at: Instant::now(),
        });
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
        if self
            .notice
            .as_ref()
            .is_some_and(|notice| notice.expired(Instant::now()))
        {
            self.notice = None;
            self.dirty = true;
        }
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
        let styled = self.palette.uses_color();
        let job_lines = self
            .jobs
            .iter()
            .map(|job| {
                let mut lines = job.body(styled);
                job_paragraph(lines.split_off(1)).line_count(self.size.width)
            })
            .sum::<usize>();
        let jobs = &self.jobs;
        let [header_area, counter_area, body_area, jobs_area, footer_area] = frame_areas(
            Rect::new(0, 0, self.size.width, self.size.height),
            header_lines,
            job_lines,
            self.notice.is_some(),
        );
        let counter = match self.rounds.current() {
            Some(round) => {
                let time = round_stamp(round.observed_at, Local::now().date_naive());
                match self.rounds.index() {
                    Some(index) => {
                        format!("round {}/{} {time}", index + 1, self.rounds.positions())
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
            draw_jobs(frame, jobs_area, jobs, styled);
            draw_footer(
                frame,
                footer_area,
                self.notice.as_ref().map(|notice| notice.text.as_str()),
                &self.rounds,
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

fn frame_areas(area: Rect, header_lines: usize, job_lines: usize, notice: bool) -> [Rect; 5] {
    let hint_rows = u16::from(area.height > 1);
    // The notice takes a row above the hints only when the body keeps one as well.
    let footer = hint_rows + u16::from(notice && area.height > 3);
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

fn draw_footer(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    notice: Option<&str>,
    rounds: &Rounds,
    palette: Palette,
) {
    hints::draw(
        frame,
        area,
        notice.unwrap_or(""),
        &live_actions(rounds),
        &keymap::DEV,
        palette,
    );
}

fn draw_jobs(frame: &mut ratatui::Frame<'_>, area: Rect, jobs: &[Job], styled: bool) {
    let mut top = area.y;
    let visible = jobs.len().min(usize::from(area.height));
    for (index, job) in jobs.iter().take(visible).enumerate() {
        let rows = usize::from(area.bottom() - top) / (visible - index);
        let rows = rows as u16;
        let mut lines = job.body(styled);
        frame.render_widget(
            Paragraph::new(lines.remove(0)),
            Rect::new(area.x, top, area.width, 1),
        );
        let tail_area = Rect::new(area.x, top + 1, area.width, rows - 1);
        let tail = job_paragraph(lines);
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

const ACCEPTED: [Action; 10] = [
    Action::PreviousRound,
    Action::NextRound,
    Action::Up,
    Action::Down,
    Action::PageUp,
    Action::PageDown,
    Action::First,
    Action::Last,
    Action::Copy,
    Action::Quit,
];

fn styled_paragraph(text: &str) -> Paragraph<'static> {
    Paragraph::new(Text::from(styled_lines(text))).wrap(Wrap { trim: false })
}

fn job_paragraph(lines: Vec<Line<'static>>) -> Paragraph<'static> {
    Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false })
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
    let mut lines = vec![Line::default()];
    for segment in ansi::segments(text) {
        if segment.text == "\n" {
            lines.push(Line::default());
            continue;
        }
        push_span(
            lines.last_mut().expect("one line"),
            &tabs_expanded(&segment.text),
            segment.style,
        );
    }
    lines
}

/// The plain text a styled transcript prints: the clipboard takes text, not a terminal, so
/// styling is consumed and a hyperlink's payload never becomes characters.
fn plain_text(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    for segment in ansi::segments(text) {
        if segment.text == "\n" {
            plain.push('\n');
        } else {
            plain.push_str(&tabs_expanded(&segment.text));
        }
    }
    plain
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
    use ratatui::style::{Color, Modifier};
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
    fn latest_clean_result_is_one_position() {
        let mut rounds = Rounds::default();
        rounds.push(round(vec![diagnostic("content/first.typ")], 1, "failed"));
        assert_eq!(rounds.positions(), 1, "one diagnostic round");
        rounds.push(round(Vec::new(), 2, "Built site"));
        assert_eq!(
            rounds.positions(),
            2,
            "the latest clean result is a place to walk to"
        );
        rounds.push(round(vec![diagnostic("content/second.typ")], 3, "failed"));
        assert_eq!(
            rounds.positions(),
            2,
            "a later failure leaves that clean result no place"
        );
        rounds.push(round(Vec::new(), 4, "Built site"));
        assert_eq!(rounds.positions(), 3, "the newest clean result walks again");
    }

    /// History merges by the diagnostics a round reports, never by the time it arrived: the same
    /// set refreshes its entry in place and keeps the position, a new set is its own entry.
    #[test]
    fn rounds_merge_by_report_not_by_time() {
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
        assert_eq!(rounds.entries.len(), 2, "a new report is its own entry");
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
    fn stamps_show_the_date_only_off_today() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
        let yesterday = NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();
        let at = |day, hour, minute, second| {
            Local
                .with_ymd_and_hms(2026, 10, day, hour, minute, second)
                .single()
                .unwrap()
        };
        assert_eq!(round_stamp(at(15, 0, 6, 27), today), "00:06:27");
        assert_eq!(round_stamp(at(15, 0, 32, 14), today), "00:32:14");
        assert_eq!(
            round_stamp(at(14, 23, 33, 37), today),
            "2026-10-14 23:33:37"
        );
        assert_eq!(round_stamp(at(14, 23, 33, 37), yesterday), "23:33:37");
    }

    #[test]
    fn copy_follows_the_visible_round() {
        let mut rounds = Rounds::default();
        assert!(!rounds.live(Action::Copy), "nothing to copy yet");
        rounds.push(round(vec![diagnostic("content/first.typ")], 1, "failed"));
        assert!(rounds.live(Action::Copy));
        rounds.push(round(Vec::new(), 2, "Built site"));
        assert!(rounds.live(Action::Copy), "a clean round copies its output");
        rounds.push(round(Vec::new(), 3, ""));
        assert!(!rounds.live(Action::Copy), "an empty round copies nothing");
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
                for notice in [false, true] {
                    let area = Rect::new(0, 0, width, height);
                    let areas = frame_areas(area, 200, 200, notice);
                    assert!(areas[2].height > 0);
                    let footer = u16::from(height > 1) + u16::from(notice && height > 3);
                    assert_eq!(areas[4].height, footer);
                    assert_eq!(areas.iter().map(|part| part.height).sum::<u16>(), height);
                    assert!(
                        areas
                            .iter()
                            .all(|part| area.contains(part.as_position()) || part.is_empty())
                    );
                }
            }
        }
    }

    #[test]
    fn copy_notice_sits_above_the_hints() {
        let mut rounds = Rounds::default();
        rounds.push(round(vec![diagnostic("content/first.typ")], 1, "failed"));
        let area = Rect::new(0, 0, 80, 2);
        let palette = Palette::new(true);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                .unwrap();
        terminal
            .draw(|frame| draw_footer(frame, area, Some("copied 1 diagnostic"), &rounds, palette))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let row = |y: u16| {
            (area.x..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        };
        assert_eq!(row(0), "copied 1 diagnostic");
        assert!(row(1).contains("c copy"), "{}", row(1));
        assert_eq!(buffer[(0, 0)].fg, palette.notice_style().fg.unwrap());
    }

    #[test]
    fn copy_notice_leaves_after_its_time() {
        let shown_at = Instant::now();
        let notice = Notice {
            text: "copied 1 diagnostic".to_owned(),
            shown_at,
        };
        let almost = shown_at + NOTICE_LIFETIME - std::time::Duration::from_millis(1);
        assert!(!notice.expired(almost));
        assert!(notice.expired(shown_at + NOTICE_LIFETIME));
    }

    #[test]
    fn short_hook_area_shows_jobs() {
        let jobs = ["styles", "search", "assets"].map(|name| {
            Job::new(
                HookRun {
                    scope: 0,
                    name: name.to_owned(),
                },
                "generate",
            )
        });
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(24, 2)).unwrap();
        terminal
            .draw(|frame| draw_jobs(frame, frame.area(), &jobs, false))
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
    fn plain_text_drops_styling_and_payloads() {
        let text = "a\u{1b}]8;;http://127.0.0.1:1/\u{1b}\\link\u{1b}]8;;\u{1b}\\\u{1b}[31mred\u{1b}[0m\nsecond\tline";
        assert_eq!(plain_text(text), "alinkred\nsecond    line");
    }

    #[test]
    fn scroll_hints_follow_the_body() {
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

        rounds.top = 0;
        rounds.body_rows = 30;
        assert!(
            !rounds.live(Action::Down),
            "a body that fits does not scroll"
        );
        assert!(!rounds.live(Action::Last));
    }

    #[test]
    fn round_hints_follow_history() {
        let mut rounds = Rounds::default();
        rounds.push(round(vec![diagnostic("content/first.typ")], 1, "failed"));
        assert!(!rounds.live(Action::PreviousRound));
        assert!(!rounds.live(Action::NextRound));
        rounds.push(round(vec![diagnostic("content/second.typ")], 2, "again"));
        assert!(rounds.live(Action::PreviousRound), "two rounds to walk");
        assert!(!rounds.live(Action::NextRound));
        rounds.previous();
        assert!(!rounds.live(Action::PreviousRound));
        assert!(rounds.live(Action::NextRound));
    }

    fn body_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn hook_chunks_keep_stream_lines() {
        let mut job = Job::new(
            HookRun {
                scope: 0,
                name: "assets".to_owned(),
            },
            "",
        );
        job.output("stdout", "first");
        job.output("stdout", " line\r");
        job.output("stdout", "\n");
        job.output("stderr", "failure\n");
        let drawn = job
            .body(false)
            .iter()
            .skip(1)
            .map(body_text)
            .collect::<Vec<_>>();
        assert_eq!(drawn, ["[stdout] first line", "[stderr] failure"]);
    }

    #[test]
    fn hook_lines_keep_style_when_cropped() {
        let mut job = Job::new(
            HookRun {
                scope: 0,
                name: "assets".to_owned(),
            },
            "",
        );
        for index in 1..=HOOK_LINES + 1 {
            let color = if index == 1 { "\x1b[31m" } else { "\x1b[32m" };
            job.output("stdout", &format!("{color}line {index}\x1b[0m\n"));
        }
        let lines = job.body(true);
        assert_eq!(lines.len(), HOOK_LINES + 1);
        assert_eq!(body_text(&lines[1]), "[stdout] line 2");
        assert_eq!(lines[1].spans[1].style.fg, Some(Color::Green));
    }

    #[test]
    fn hook_header_names_omitted_lines() {
        let mut job = Job::new(
            HookRun {
                scope: 0,
                name: "assets".to_owned(),
            },
            "",
        );
        for index in 1..=HOOK_LINES {
            job.output("stdout", &format!("line {index}\n"));
        }
        assert_eq!(body_text(&job.body(false)[0]), "/assets");
        job.output("stdout", "one more\n");
        assert_eq!(
            body_text(&job.body(false)[0]),
            "/assets · 1 earlier line not shown"
        );
    }

    #[test]
    fn hook_body_follows_the_color_decision() {
        let mut job = Job::new(
            HookRun {
                scope: 0,
                name: "assets".to_owned(),
            },
            "",
        );
        job.output("stdout", "\x1b[31mred\n");
        assert_eq!(job.body(true)[1].spans[1].style.fg, Some(Color::Red));
        assert_eq!(job.body(false)[1].spans[1].style.fg, None);
    }
}
