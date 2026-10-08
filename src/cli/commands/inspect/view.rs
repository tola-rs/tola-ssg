//! The interactive table over one inspection projection.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use serde_json::Value;

use crate::terminal::style::Palette;
use crate::terminal::text;
use crate::terminal::ui::filter::{Reply, TextLine};
use crate::terminal::ui::table::Table;
use crate::terminal::ui::{Action, Step, Surface, answer_key, hints, keymap};

/// The actions the table answers; `Export` writes the rows it currently shows.
const ACCEPTED: &[Action] = &[
    Action::Quit,
    Action::Dismiss,
    Action::Up,
    Action::Down,
    Action::PageUp,
    Action::PageDown,
    Action::First,
    Action::Last,
    Action::Search,
    Action::NextMatch,
    Action::PreviousMatch,
    Action::Open,
    Action::Export,
];

/// The interactive table over one projection's rows.
pub(crate) struct View<'a> {
    /// Every row the projection shows, in projection order, unmodified.
    rows: Vec<Value>,
    /// The columns the table shows, in the order the rows first name them.
    columns: Vec<Column>,
    /// The rows the filter keeps, as indices into `rows`.
    visible: Vec<usize>,
    /// The table component over the visible rows.
    table: Table,
    /// The row the reader is on, as an index into `rows`.
    row: Option<usize>,
    prompt: Prompt,
    /// The query the last accepted filter ran.
    query: Option<String>,
    /// What the export prompt starts with: the command's own output path, when it has one.
    target: Option<String>,
    /// What the last action reported; the next key clears it.
    notice: Option<String>,
    /// The line above the hints: what the reader is looking at.
    status: String,
    /// Whether the table still shows the current rows and filter.
    built: bool,
    /// Whether cells render the serialized JSON instead of content text.
    raw: bool,
    /// The rows to write to stdout once the view ends, when the reader chose that.
    stdout: Option<String>,
    /// The projection this table browses: what the window title names.
    projection: String,
    /// Encodes exported rows exactly as the non-interactive path does.
    encode: &'a dyn Fn(&[Value]) -> Result<String>,
    /// Writes one export of `rows` rows to `path`, as the command's own output path does.
    save: &'a dyn Fn(&Path, &str, usize) -> Result<()>,
}

enum Prompt {
    Closed,
    Filter(TextLine),
    Export(TextLine),
}

/// One column of the table: the dotted path that reaches it, and what it holds.
struct Column {
    /// The header, dotted for a nested object (`properties.title`).
    name: String,
    /// The keys that reach the value inside a row.
    path: Vec<String>,
}

impl<'a> View<'a> {
    /// The table over `rows`; `encode` and `save` write what the reader exports.
    pub(crate) fn new(
        rows: Vec<Value>,
        encode: &'a dyn Fn(&[Value]) -> Result<String>,
        save: &'a dyn Fn(&Path, &str, usize) -> Result<()>,
    ) -> Self {
        let columns = columns(&rows);
        let row = (!rows.is_empty()).then_some(0);
        Self {
            visible: (0..rows.len()).collect(),
            table: Table::new(
                columns.iter().map(|column| column.name.clone()).collect(),
                Vec::new(),
            ),
            rows,
            columns,
            row,
            prompt: Prompt::Closed,
            query: None,
            target: None,
            notice: None,
            status: String::new(),
            built: false,
            raw: false,
            stdout: None,
            projection: String::new(),
            encode,
            save,
        }
    }

    /// The projection this table browses, as the window title names it.
    pub(crate) fn set_projection(&mut self, projection: &str) {
        self.projection = projection.to_owned();
    }

    /// Whether cells render serialized JSON (`--raw`) instead of content text.
    pub(crate) fn set_raw(&mut self, raw: bool) {
        self.raw = raw;
        self.built = false;
    }

    /// The file the export prompt starts with, so `--output` stays the reader's target.
    pub(crate) fn set_target(&mut self, target: &Path) {
        self.target = Some(target.display().to_string());
    }

    /// The rows the reader asked to write to stdout, once the view has ended.
    pub(crate) fn stdout_export(&mut self) -> Option<String> {
        self.stdout.take()
    }

    fn build(&mut self) {
        let rows = self
            .visible
            .iter()
            .map(|index| {
                let row = &self.rows[*index];
                self.columns
                    .iter()
                    .map(|column| cell(value_at(row, &column.path), self.raw))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        self.table.set_rows(rows);
        self.table.select(
            self.visible
                .iter()
                .position(|index| Some(*index) == self.row)
                .unwrap_or(0),
        );
        self.built = true;
    }

    /// Fills the status line: how many rows show, and the query that holds them.
    fn refresh_status(&mut self) {
        self.status.clear();
        let _ = write!(
            self.status,
            "{} of {}",
            crate::terminal::plural_count(self.visible.len(), "row"),
            self.rows.len()
        );
        if let Some(query) = &self.query {
            let _ = write!(self.status, " matching “{query}”");
        }
        if self.table.detail_is_open() {
            self.status.push_str(" · Enter closes detail");
        }
    }

    /// Whether one row renders any cell containing the query, ignoring case.
    fn matches(&self, row: &Value) -> bool {
        let Some(query) = &self.query else {
            return true;
        };
        let query = query.to_lowercase();
        self.columns.iter().any(|column| {
            cell(value_at(row, &column.path), self.raw)
                .to_lowercase()
                .contains(&query)
        })
    }

    /// Applies the accepted query: the visible rows become the matching ones, in order.
    fn apply_query(&mut self) {
        self.visible = (0..self.rows.len())
            .filter(|index| self.matches(&self.rows[*index]))
            .collect();
        self.row = self
            .row
            .filter(|row| self.visible.contains(row))
            .or_else(|| self.visible.first().copied());
        self.table.close_detail();
        self.table.select(
            self.visible
                .iter()
                .position(|index| Some(*index) == self.row)
                .unwrap_or(0),
        );
        self.built = false;
    }

    /// Moves the reader to the next or previous matching row, wrapping around the table.
    fn step_match(&mut self, forward: bool) {
        if self.query.is_none() {
            return;
        }
        let Some(position) = self
            .visible
            .iter()
            .position(|index| Some(*index) == self.row)
        else {
            return;
        };
        let position = if forward {
            (position + 1) % self.visible.len()
        } else {
            (position + self.visible.len() - 1) % self.visible.len()
        };
        self.row = Some(self.visible[position]);
        self.table.select(position);
        if self.table.detail_is_open() {
            self.open_detail();
        }
    }

    /// Opens the selected row's detail: its JSON, unmodified and pretty-printed.
    fn open_detail(&mut self) {
        let Some(index) = self.row else {
            return;
        };
        let row = &self.rows[index];
        self.table
            .open_detail(format!("row {}", index + 1), detail_lines(row));
    }

    fn move_selection(&mut self, select: impl FnOnce(&mut Table)) {
        select(&mut self.table);
        self.row = self.visible.get(self.table.selected()).copied();
    }

    /// Accepts the export prompt's target: empty means stdout once the view ends.
    fn accept_export(&mut self, target: &str) -> Step {
        let rows = self
            .visible
            .iter()
            .map(|index| self.rows[*index].clone())
            .collect::<Vec<_>>();
        let encoded = match (self.encode)(&rows) {
            Ok(encoded) => encoded,
            Err(error) => {
                self.notice = Some(format!("{error}"));
                return Step::Continue;
            }
        };
        if target.trim().is_empty() {
            self.stdout = Some(encoded);
            return Step::Done;
        }
        let path = Path::new(target.trim());
        match (self.save)(path, &encoded, rows.len()) {
            Ok(()) => {
                self.notice = Some(format!(
                    "wrote {} to `{}`",
                    crate::terminal::plural_count(rows.len(), "row"),
                    path.display()
                ));
            }
            Err(error) => self.notice = Some(format!("{error}")),
        }
        Step::Continue
    }
}

impl Surface for View<'_> {
    fn draw(&mut self, frame: &mut Frame, palette: Palette) {
        let area = frame.area();
        if area.height == 0 || area.width == 0 {
            return;
        }
        if !self.built {
            self.build();
        }
        let (content, footer) = split_footer(area, !matches!(self.prompt, Prompt::Closed));
        self.table.draw(frame, content, palette);
        self.refresh_status();
        let line = self.notice.as_deref().unwrap_or(&self.status);
        let bindings = if matches!(self.prompt, Prompt::Closed) {
            self.bindings()
        } else {
            &keymap::TEXT_INPUT
        };
        hints::draw(frame, footer, line, &self.live(), bindings, palette);
        if let Prompt::Filter(prompt) | Prompt::Export(prompt) = &self.prompt {
            let line = Rect {
                y: footer.y,
                height: 1,
                ..footer
            };
            prompt.draw(frame, line, palette);
        }
    }

    fn answer(&mut self, action: Action) -> Step {
        self.notice = None;
        // An open detail owns the movement keys until the reader closes it.
        if self.table.detail_is_open() {
            match action {
                Action::Up => self.table.scroll_detail(-1),
                Action::Down => self.table.scroll_detail(1),
                Action::PageUp => self.table.page_detail_by(-1),
                Action::PageDown => self.table.page_detail_by(1),
                Action::Open | Action::Dismiss => {
                    self.table.close_detail();
                }
                _ => return self.answer_over_detail(action),
            }
            return Step::Continue;
        }
        match action {
            Action::Quit => return Step::Done,
            Action::Dismiss => return Step::Cancel,
            Action::Up => self.move_selection(|table| table.select_by(-1)),
            Action::Down => self.move_selection(|table| table.select_by(1)),
            Action::PageUp => self.move_selection(|table| table.page_by(-1)),
            Action::PageDown => self.move_selection(|table| table.page_by(1)),
            Action::First => self.move_selection(Table::select_first),
            Action::Last => self.move_selection(Table::select_last),
            Action::Search => self.search(),
            Action::NextMatch => self.step_match(true),
            Action::PreviousMatch => self.step_match(false),
            Action::Open => self.open_detail(),
            Action::Export => self.export(),
            _ => return Step::Continue,
        }
        Step::Continue
    }

    fn key(&mut self, key: &KeyEvent) -> Step {
        self.notice = None;
        if let Prompt::Filter(filter) = &mut self.prompt {
            return match filter.key(key) {
                Reply::Editing => Step::Continue,
                Reply::Accepted => {
                    self.query = (!filter.text().is_empty()).then(|| filter.text().to_owned());
                    self.prompt = Prompt::Closed;
                    self.apply_query();
                    Step::Continue
                }
                Reply::Dismissed => {
                    self.prompt = Prompt::Closed;
                    Step::Continue
                }
            };
        }
        if let Prompt::Export(export) = &mut self.prompt {
            return match export.key(key) {
                Reply::Editing => Step::Continue,
                Reply::Accepted => {
                    let target = export.text().to_owned();
                    self.prompt = Prompt::Closed;
                    self.accept_export(&target)
                }
                Reply::Dismissed => {
                    self.prompt = Prompt::Closed;
                    Step::Continue
                }
            };
        }
        answer_key(self, key)
    }

    fn paste(&mut self, text: &str) -> Step {
        if let Prompt::Filter(prompt) | Prompt::Export(prompt) = &mut self.prompt {
            prompt.paste(text);
        }
        Step::Continue
    }

    fn live(&self) -> Vec<Action> {
        if !matches!(self.prompt, Prompt::Closed) {
            return vec![Action::Open, Action::Dismiss];
        }
        let mut live = ACCEPTED.to_vec();
        if self.row.is_none() {
            live.retain(|action| !matches!(action, Action::Open));
        }
        live.retain(|action| self.movement_changes(*action));
        if self.query.is_none() || self.visible.len() < 2 {
            live.retain(|action| !matches!(action, Action::NextMatch | Action::PreviousMatch));
        }
        if self.table.detail_is_open() {
            live.retain(|action| !matches!(action, Action::Open));
        }
        live
    }

    fn title(&self) -> Option<String> {
        (!self.projection.is_empty())
            .then(|| format!("{} — {} rows", self.projection, self.visible.len()))
    }
}

impl View<'_> {
    fn search(&mut self) {
        let mut prompt = TextLine::new("filter");
        if let Some(query) = &self.query {
            prompt.paste(query);
        }
        self.prompt = Prompt::Filter(prompt);
    }

    /// Opens the export prompt, starting from the command's own output path when it has one.
    fn export(&mut self) {
        let mut prompt = TextLine::new("write to (empty for stdout)");
        if let Some(target) = &self.target {
            prompt.paste(target);
        }
        self.prompt = Prompt::Export(prompt);
    }

    /// Answers the actions that still apply while a row detail is open.
    fn answer_over_detail(&mut self, action: Action) -> Step {
        match action {
            Action::Quit => Step::Done,
            Action::Search => {
                self.search();
                Step::Continue
            }
            Action::NextMatch => {
                self.step_match(true);
                Step::Continue
            }
            Action::PreviousMatch => {
                self.step_match(false);
                Step::Continue
            }
            Action::Export => {
                self.export();
                Step::Continue
            }
            _ => Step::Continue,
        }
    }

    /// Whether one action still changes what the reader sees: the open detail scrolls, or the
    /// table's selection moves. A movement that would leave everything as it is earns no hint.
    fn movement_changes(&self, action: Action) -> bool {
        if self.table.detail_is_open() {
            match action {
                Action::Up => self.table.can_scroll_detail(-1),
                Action::Down => self.table.can_scroll_detail(1),
                Action::PageUp => self.table.can_page_detail_by(-1),
                Action::PageDown => self.table.can_page_detail_by(1),
                Action::First | Action::Last => false,
                _ => true,
            }
        } else {
            match action {
                Action::Up => self.table.can_select_by(-1),
                Action::Down => self.table.can_select_by(1),
                Action::PageUp => self.table.can_page_by(-1),
                Action::PageDown => self.table.can_page_by(1),
                Action::First => self.table.can_select_first(),
                Action::Last => self.table.can_select_last(),
                _ => true,
            }
        }
    }
}

/// The columns the table shows: every key the rows name, in first-seen order, with nested
/// objects flattened to dotted columns.
fn columns(rows: &[Value]) -> Vec<Column> {
    let mut columns = Vec::new();
    for row in rows {
        collect_columns(row, &mut Vec::new(), &mut columns);
    }
    columns
}

fn collect_columns(value: &Value, path: &mut Vec<String>, columns: &mut Vec<Column>) {
    let Value::Object(object) = value else {
        return;
    };
    for (key, value) in object {
        path.push(key.clone());
        if value.is_object() {
            collect_columns(value, path, columns);
        } else if !columns.iter().any(|column| column.path == *path) {
            columns.push(Column {
                name: path.join("."),
                path: path.clone(),
            });
        }
        path.pop();
    }
}

/// The value `path` reaches inside one row.
fn value_at<'a>(row: &'a Value, path: &[String]) -> &'a Value {
    let mut value = row;
    for key in path {
        match value.get(key) {
            Some(inner) => value = inner,
            None => return &Value::Null,
        }
    }
    value
}

fn cell(value: &Value, raw: bool) -> String {
    let text = match value {
        Value::Null => "·".to_owned(),
        Value::String(value) if !raw => value.clone(),
        Value::String(value) => serde_json::to_string(value).unwrap_or_else(|_| value.clone()),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Array(values) => {
            format!("[{}]", crate::terminal::plural_count(values.len(), "item"))
        }
        Value::Object(object) => format!(
            "{{{}}}",
            crate::terminal::plural_count(object.len(), "field")
        ),
    };
    text::single_line(&text)
}

/// One row's detail lines: the row's JSON, unmodified and pretty-printed.
fn detail_lines(row: &Value) -> Vec<String> {
    let pretty = serde_json::to_string_pretty(row).unwrap_or_else(|_| row.to_string());
    pretty.lines().map(str::to_owned).collect()
}

/// The content above the status line and the key hints.
fn split_footer(area: Rect, prompt_is_open: bool) -> (Rect, Rect) {
    let footer = if prompt_is_open {
        area.height.min(2)
    } else {
        area.height.saturating_sub(1).min(2)
    };
    (
        Rect {
            height: area.height - footer,
            ..area
        },
        Rect {
            y: area.y + area.height - footer,
            height: footer,
            ..area
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use serde_json::json;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn encode(rows: &[Value]) -> Result<String> {
        Ok(serde_json::to_string(rows)?)
    }

    fn save(_path: &Path, _encoded: &str, _rows: usize) -> Result<()> {
        Ok(())
    }

    fn view(rows: Value) -> View<'static> {
        let Value::Array(rows) = rows else {
            panic!("a projection shows an array of rows");
        };
        View::new(rows, &encode, &save)
    }

    /// Types `text` into the open prompt and accepts it.
    fn type_and_accept(view: &mut View<'_>, text: &str) -> Step {
        for character in text.chars() {
            view.key(&key(KeyCode::Char(character)));
        }
        view.key(&key(KeyCode::Enter))
    }

    fn frame(view: &mut View<'_>, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| view.draw(frame, Palette::new(false)))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn frame_text(buffer: &Buffer) -> String {
        buffer
            .content()
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn long_row() -> Value {
        json!([{ "values": (0..24).collect::<Vec<_>>() }])
    }

    #[test]
    fn unmatched_filter_opens_no_detail() {
        let mut view = view(json!([{"name": "Alpha"}]));
        frame(&mut view, 40, 10);
        view.key(&key(KeyCode::Char('/')));
        type_and_accept(&mut view, "absent");
        frame(&mut view, 40, 10);
        view.key(&key(KeyCode::Enter));
        assert!(!view.table.detail_is_open());
    }

    #[test]
    fn resizing_preserves_detail_scroll() {
        let mut view = view(long_row());
        frame(&mut view, 40, 10);
        view.key(&key(KeyCode::Enter));
        frame(&mut view, 40, 10);
        for _ in 0..5 {
            view.key(&key(KeyCode::Down));
        }
        assert!(frame_text(&frame(&mut view, 40, 10)).contains("    3,"));
        let resized = frame(&mut view, 60, 10);
        assert!(view.table.detail_is_open());
        assert!(frame_text(&resized).contains("    3,"));
    }

    #[test]
    fn matching_row_updates_open_detail() {
        let mut view = view(json!([
            {"name": "Alpha", "group": "match"},
            {"name": "Beta", "group": "match"}
        ]));
        frame(&mut view, 40, 10);
        view.key(&key(KeyCode::Char('/')));
        type_and_accept(&mut view, "match");
        frame(&mut view, 40, 10);
        view.key(&key(KeyCode::Enter));
        frame(&mut view, 40, 10);
        for (code, name) in [('n', "Beta"), ('N', "Alpha")] {
            view.key(&key(KeyCode::Char(code)));
            assert!(frame_text(&frame(&mut view, 40, 10)).contains(&format!("\"{name}\"")));
        }
    }

    #[test]
    fn detail_pages_without_skipping_lines() {
        let mut view = view(long_row());
        frame(&mut view, 40, 10);
        view.key(&key(KeyCode::Enter));
        frame(&mut view, 40, 10);
        view.key(&key(KeyCode::PageDown));
        assert!(frame_text(&frame(&mut view, 40, 10)).contains("    2,"));
        view.key(&key(KeyCode::PageUp));
        assert!(frame_text(&frame(&mut view, 40, 10)).contains("\"values\""));
    }

    #[test]
    fn open_detail_end_hides_downward_actions() {
        let mut view = view(long_row());
        frame(&mut view, 40, 10);
        view.key(&key(KeyCode::Enter));
        frame(&mut view, 40, 10);
        assert!(view.live().contains(&Action::Down));
        for _ in 0..6 {
            view.key(&key(KeyCode::PageDown));
        }
        assert!(!view.live().contains(&Action::Down));
        assert!(!view.live().contains(&Action::PageDown));
        assert!(view.live().contains(&Action::Up));
    }

    #[test]
    fn footer_actions_follow_reader_mode() {
        let mut view = view(json!([{"name": "Alpha"}, {"name": "Beta"}]));
        frame(&mut view, 40, 10);
        assert!(view.live().contains(&Action::Open));
        assert!(!view.live().contains(&Action::NextMatch));
        view.key(&key(KeyCode::Enter));
        assert!(!view.live().contains(&Action::Open));
        // The detail is one line in a taller overlay: nothing to scroll.
        assert!(!view.live().contains(&Action::Down));
        view.key(&key(KeyCode::Char('/')));
        assert_eq!(view.live(), [Action::Open, Action::Dismiss]);
        type_and_accept(&mut view, "absent");
        frame(&mut view, 40, 10);
        assert!(!view.live().contains(&Action::Open));
        assert!(!view.live().contains(&Action::Down));
        assert!(view.live().contains(&Action::Export));
        view.key(&key(KeyCode::Char('e')));
        assert_eq!(view.live(), [Action::Open, Action::Dismiss]);
    }

    #[test]
    fn fitting_table_hides_paging_and_end_moves() {
        let mut view = view(json!([{"name": "Alpha"}, {"name": "Beta"}]));
        frame(&mut view, 40, 20);
        assert!(!view.live().contains(&Action::PageDown));
        assert!(!view.live().contains(&Action::PageUp));
        assert!(view.live().contains(&Action::Down));
        view.key(&key(KeyCode::Down));
        assert!(!view.live().contains(&Action::Last));
        assert!(!view.live().contains(&Action::Down));
        assert!(view.live().contains(&Action::Up));
        view.key(&key(KeyCode::Up));
        assert!(!view.live().contains(&Action::First));
    }

    #[test]
    fn columns_flatten_nested_objects() {
        let view = view(json!([
            {"path": "content/a.typ", "properties": {"title": "Alpha", "draft": true}},
            {"path": "content/b.typ", "properties": {"title": "Beta", "extra": null}},
        ]));
        let names = view
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "path",
                "properties.title",
                "properties.draft",
                "properties.extra"
            ]
        );
    }

    #[test]
    fn cells_fit_one_line() {
        assert_eq!(cell(&json!("a\nb"), false), "a\\nb");
        assert_eq!(cell(&json!("a"), true), "\"a\"");
        assert_eq!(cell(&json!(null), false), "·");
        assert_eq!(cell(&json!(["a", "b"]), false), "[2 items]");
        assert_eq!(cell(&json!({"a": 1}), false), "{1 field}");
    }

    #[test]
    fn filter_keeps_matching_rows_in_order() {
        let mut view = view(json!([
            {"path": "content/a.typ", "title": "Alpha"},
            {"path": "content/b.typ", "title": "Beta"},
            {"path": "content/c.typ", "title": "alpha two"},
        ]));
        view.answer(Action::Search);
        assert_eq!(type_and_accept(&mut view, "ALPHA"), Step::Continue);

        assert_eq!(view.visible, vec![0, 2]);
        assert_eq!(view.row, Some(0));
    }

    #[test]
    fn cancelled_filter_preserves_detail() {
        let mut view = view(json!([
            {"name": "Alpha"},
            {"name": "Alpha two", "values": (0..24).collect::<Vec<_>>()},
            {"name": "Beta"},
        ]));
        frame(&mut view, 40, 10);
        view.key(&key(KeyCode::Down));
        view.key(&key(KeyCode::Char('/')));
        type_and_accept(&mut view, "Alpha");
        frame(&mut view, 40, 10);
        view.key(&key(KeyCode::Enter));
        frame(&mut view, 40, 10);
        view.key(&key(KeyCode::PageDown));
        let before = frame(&mut view, 40, 10);
        view.key(&key(KeyCode::Char('/')));
        let Prompt::Filter(prompt) = &view.prompt else {
            panic!("filter is open");
        };
        assert_eq!(prompt.text(), "Alpha");
        view.paste("absent");
        view.key(&key(KeyCode::Esc));
        assert_eq!(view.visible, [0, 1]);
        assert_eq!(view.row, Some(1));
        assert_eq!(frame(&mut view, 40, 10), before);
    }

    #[test]
    fn empty_filter_restores_rows() {
        let mut view = view(json!([{"name": "Alpha"}, {"name": "Beta"}]));
        frame(&mut view, 40, 10);
        view.key(&key(KeyCode::Char('/')));
        type_and_accept(&mut view, "Beta");
        frame(&mut view, 40, 10);
        view.key(&key(KeyCode::Char('/')));
        for _ in 0..4 {
            view.key(&key(KeyCode::Backspace));
        }
        view.key(&key(KeyCode::Enter));
        frame(&mut view, 40, 10);
        assert_eq!(view.visible, [0, 1]);
        assert_eq!(view.row, Some(1));
        assert!(view.query.is_none());
        assert!(!view.live().contains(&Action::NextMatch));
    }

    #[test]
    fn detail_retains_available_actions() {
        let mut view = view(json!([
            {"name": "Alpha", "group": "match"},
            {"name": "Beta", "group": "match"},
        ]));
        frame(&mut view, 40, 10);
        view.key(&key(KeyCode::Char('/')));
        type_and_accept(&mut view, "match");
        frame(&mut view, 40, 10);
        view.key(&key(KeyCode::Enter));
        frame(&mut view, 40, 10);
        for action in [
            Action::Quit,
            Action::Dismiss,
            Action::Search,
            Action::Export,
            Action::NextMatch,
            Action::PreviousMatch,
        ] {
            assert!(view.live().contains(&action), "{action:?}");
        }
        assert!(!view.live().contains(&Action::First));
        assert!(!view.live().contains(&Action::Last));
        view.key(&key(KeyCode::Esc));
        assert!(!view.table.detail_is_open());
        assert_eq!(view.key(&key(KeyCode::Char('q'))), Step::Done);
    }

    #[test]
    fn export_writes_the_rows_the_filter_keeps() {
        let written = std::cell::RefCell::new(None);
        let saved = |_: &Path, encoded: &str, rows: usize| {
            *written.borrow_mut() = Some((encoded.to_owned(), rows));
            Ok(())
        };
        let mut view = View::new(
            vec![
                json!({"path": "content/a.typ", "title": "Alpha"}),
                json!({"path": "content/b.typ", "title": "Beta"}),
            ],
            &encode,
            &saved,
        );
        view.answer(Action::Search);
        type_and_accept(&mut view, "Beta");
        view.answer(Action::Export);

        assert_eq!(type_and_accept(&mut view, "rows.json"), Step::Continue);
        drop(view);
        assert_eq!(
            written.into_inner(),
            Some((
                "[{\"path\":\"content/b.typ\",\"title\":\"Beta\"}]".to_owned(),
                1
            ))
        );
    }

    #[test]
    fn export_to_stdout_ends_the_view() {
        let mut view = view(json!([{"path": "content/a.typ"}]));
        view.answer(Action::Export);

        assert_eq!(type_and_accept(&mut view, ""), Step::Done);
        assert_eq!(
            view.stdout_export(),
            Some("[{\"path\":\"content/a.typ\"}]".to_owned())
        );
    }

    #[test]
    fn export_prompt_starts_at_the_command_output_path() {
        let mut view = view(json!([{"path": "content/a.typ"}]));
        view.set_target(Path::new("metadata.json"));
        view.answer(Action::Export);

        assert_eq!(
            match &view.prompt {
                Prompt::Export(prompt) => Some(prompt.text()),
                _ => None,
            },
            Some("metadata.json")
        );
    }

    #[test]
    fn detail_lines_hold_the_unmodified_row() {
        let row = json!({"path": "content/a.typ", "title": "Alpha"});
        let lines = detail_lines(&row);

        assert!(lines.len() > 1);
        assert_eq!(
            serde_json::from_str::<Value>(&lines.join("\n")).unwrap(),
            row
        );
    }

    #[test]
    fn empty_table_opens_no_detail() {
        let mut view = view(json!([]));

        assert_eq!(view.answer(Action::Open), Step::Continue);
        assert!(!view.table.detail_is_open());
    }

    #[test]
    fn short_view_keeps_row_reachable() {
        let mut view = view(json!([{"name": "Alpha"}]));
        for height in [1, 2] {
            assert!(frame_text(&frame(&mut view, 20, height)).contains("Alpha"));
            view.key(&key(KeyCode::Enter));
            assert!(frame_text(&frame(&mut view, 20, height)).contains('{'));
            view.key(&key(KeyCode::Esc));
        }
        view.key(&key(KeyCode::Char('/')));
        view.paste("Alpha");
        assert!(frame_text(&frame(&mut view, 20, 1)).contains("Alpha"));
        view.key(&key(KeyCode::Enter));
        assert_eq!(view.visible, [0]);
    }
}
