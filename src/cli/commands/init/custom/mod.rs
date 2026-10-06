//! Interactive scaffold selection.

mod rows;

use std::path::Path;

use crossterm::event::{KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListState, Paragraph};

use crate::terminal::display_path_toward_home;
use crate::terminal::style::Palette;
use crate::terminal::ui::filter::{Reply, TextLine};
use crate::terminal::ui::{Action, Step, Surface, answer_key, hints, keymap, stepped_index};

use super::features::{self, Feature, FeatureSet, canon};
use super::selection;

use rows::{Row, role_style};

/// The actions every screen offers, in the hint row's order.
const SHARED: &[Action] = &[Action::Quit, Action::Dismiss];

/// The preset keys, one per offered preset.
fn presets() -> Vec<Action> {
    (0..features::preset_choices().count())
        .map(|position| Action::ApplyPreset(position as u8))
        .collect()
}

/// The list view over the scaffold a reader selects.
pub(super) struct View {
    /// Completed at the prompt boundary; every transition preserves its requirements.
    selected: FeatureSet,
    /// The site the scaffold is written to, as the title shows it.
    site: String,
    /// The position in the rows of the row under the cursor.
    cursor: usize,
    /// The list's cursor and scroll position, as `List` keeps them.
    list: ListState,
    /// What the last action changed; the next key clears it.
    footnote: Option<String>,
    /// The open filter line, when the reader is typing one.
    filter: Option<TextLine>,
    /// Updated while typing; only names containing the text stay visible.
    query: String,
}

impl View {
    /// The view over `selected`, titled for the site at `root`.
    pub(super) fn new(selected: FeatureSet, root: &Path) -> Self {
        let site = display_path_toward_home(root);
        Self {
            selected,
            site,
            cursor: 0,
            list: ListState::default(),
            footnote: None,
            filter: None,
            query: String::new(),
        }
    }

    /// The selection the reader accepted, canonical so that what is written is what it means.
    pub(super) fn selection(&self) -> FeatureSet {
        self.selected.clone()
    }

    /// The rows the list shows.
    fn rows(&self) -> &'static [Row] {
        rows::rows()
    }

    /// The rows the frame draws: the visible items, and every title that has one.
    fn visible_rows(&self, rows: &[Row]) -> Vec<usize> {
        let mut kept = Vec::new();
        let mut title = None;
        let query = self.query.to_ascii_lowercase();
        for (index, row) in rows.iter().enumerate() {
            if !rows::is_item(*row) {
                title = Some(index);
                continue;
            }
            if rows::name(*row).is_some_and(|name| name.contains(&query)) {
                if let Some(title) = title.take() {
                    kept.push(title);
                }
                kept.push(index);
            }
        }
        kept
    }

    /// The positions in `visible_rows` that hold items, so the cursor counts items only.
    fn visible_items(rows: &[Row], visible: &[usize]) -> Vec<usize> {
        visible
            .iter()
            .enumerate()
            .filter(|(_, index)| rows::is_item(rows[**index]))
            .map(|(position, _)| position)
            .collect()
    }

    /// The row the cursor is on, among the items the filter leaves visible.
    fn current(&self, rows: &[Row]) -> Option<Row> {
        let visible = self.visible_rows(rows);
        Self::visible_items(rows, &visible)
            .get(self.cursor)
            .map(|position| rows[visible[*position]])
    }

    /// Keeps the cursor on a visible item that exists.
    fn settle(&mut self, rows: &[Row]) {
        let visible = self.visible_rows(rows);
        self.cursor = self
            .cursor
            .min(Self::visible_items(rows, &visible).len().saturating_sub(1));
    }

    /// Answers one action with the list in front.
    fn answer_list(&mut self, action: Action) -> Step {
        let rows = self.rows();
        let visible = self.visible_rows(rows);
        let items = Self::visible_items(rows, &visible).len();
        match action {
            Action::Down => self.cursor = stepped_index(self.cursor, 1, items),
            Action::Up => self.cursor = stepped_index(self.cursor, -1, items),
            Action::First => self.cursor = 0,
            Action::Last => self.cursor = items.saturating_sub(1),
            Action::Toggle => self.toggle(rows),
            Action::Search => {
                self.filter = Some(TextLine::new("filter"));
                self.query.clear();
                self.cursor = 0;
                self.settle(rows);
            }
            Action::ApplyPreset(position) => {
                apply_preset(&mut self.selected, &mut self.footnote, position);
                let rows = self.rows();
                self.settle(rows);
            }
            _ => return Step::Continue,
        }
        Step::Continue
    }

    /// Files follow their feature writers; an ambiguous writer remains a feature choice.
    fn toggle(&mut self, rows: &[Row]) {
        match self.current(rows) {
            Some(Row::Atom(feature)) => {
                toggle_feature(&mut self.selected, feature, &mut self.footnote)
            }
            Some(Row::File(file)) => {
                let writers = features::writers(file, &self.selected);
                if !writers.is_empty() {
                    self.footnote = Some(format!("written by {}", writers.join(", ")));
                } else {
                    match features::file_writers(file).as_slice() {
                        [] => {}
                        [only] => toggle_feature(&mut self.selected, *only, &mut self.footnote),
                        many => {
                            self.footnote = Some(format!(
                                "check {} to write it",
                                many.iter()
                                    .map(|feature| features::feature_name(*feature))
                                    .collect::<Vec<_>>()
                                    .join(" or ")
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Cancels the session.
    fn dismiss(&mut self) -> Step {
        Step::Cancel
    }

    /// The directory and preset choices above the feature list.
    fn header_lines(&self, canonical: &FeatureSet, palette: Palette) -> Vec<Line<'static>> {
        vec![
            Line::from(Span::styled(
                format!("tola init — {}", self.site),
                palette.emphasis_style(),
            )),
            Line::default(),
            preset_bar(canonical, palette),
            self.filter_indicator(palette),
        ]
    }

    /// The row under the preset bar: the accepted filter, or a blank that keeps the layout.
    fn filter_indicator(&self, palette: Palette) -> Line<'static> {
        if self.query.is_empty() {
            Line::default()
        } else {
            Line::from(Span::styled(
                format!("/{}", self.query),
                palette.accent_style(),
            ))
        }
    }

    /// One line per visible row; the focused row's own spans hold the selection bar.
    fn list_items(
        &self,
        visible: &[usize],
        rows: &[Row],
        palette: Palette,
        focused: Option<usize>,
    ) -> Vec<Line<'static>> {
        visible
            .iter()
            .enumerate()
            .map(|(position, index)| {
                let bar = if Some(position) == focused {
                    palette.selected_style()
                } else {
                    Style::default()
                };
                Line::from(
                    rows::line(rows[*index], &self.selected, Some(position) == focused)
                        .into_iter()
                        .map(|piece| {
                            Span::styled(piece.text, role_style(piece.role, palette).patch(bar))
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect()
    }
}

/// Checks or unchecks one feature, with the footnote naming what changed; a displaced feature
/// refuses the check and repeats what displaces it.
fn toggle_feature(selected: &mut FeatureSet, feature: Feature, footnote: &mut Option<String>) {
    let before = selected.clone();
    if selected.contains(feature) {
        *selected = selection::unselect(selected, feature);
    } else if let Some(winner) = features::replacement(selected, feature) {
        *footnote = Some(format!(
            "{} is replaced by {}",
            features::feature_name(feature),
            features::feature_name(winner)
        ));
        return;
    } else {
        *selected = selection::select(selected, feature);
    }
    *footnote = difference(&before, selected);
}

/// Replaces the selection with the preset at `position` of the offered order.
fn apply_preset(selected: &mut FeatureSet, footnote: &mut Option<String>, position: u8) {
    let Some(preset) = features::preset_choices().nth(usize::from(position)) else {
        return;
    };
    let before = selected.clone();
    *selected = canon(&features::features(preset.name));
    *footnote = difference(&before, selected);
}

/// The footnote naming how the selection just changed; `None` when nothing did.
fn difference(before: &FeatureSet, after: &FeatureSet) -> Option<String> {
    let notes = differences(before, after);
    (!notes.is_empty()).then(|| notes.join("  "))
}

impl Surface for View {
    fn draw(&mut self, frame: &mut Frame, palette: Palette) {
        let area = frame.area();
        if area.height == 0 || area.width == 0 {
            return;
        }
        let header_lines = self.header_lines(&self.selected, palette);
        let footer_rows = u16::from(area.height > 1);
        let header_rows =
            (header_lines.len() as u16).min(area.height.saturating_sub(footer_rows + 1));
        let [header, list, footer] = Layout::vertical([
            Constraint::Length(header_rows),
            Constraint::Fill(1),
            Constraint::Length(footer_rows),
        ])
        .areas(area);
        let rows = self.rows();
        frame.render_widget(Paragraph::new(Text::from(header_lines)), header);
        let visible = self.visible_rows(rows);
        let items = Self::visible_items(rows, &visible);
        self.cursor = self.cursor.min(items.len().saturating_sub(1));
        let item = items.get(self.cursor).copied();
        self.list.select(item);
        if visible.is_empty() {
            frame.render_widget(
                Paragraph::new("no matching features or files").style(palette.dim_style()),
                list,
            );
        } else {
            frame.render_stateful_widget(
                List::new(self.list_items(&visible, rows, palette, item)),
                list,
                &mut self.list,
            );
        }
        // The footer: the open filter line, the change just made, or the live keys.
        if let Some(filter) = &self.filter {
            filter.draw(frame, footer, palette);
            return;
        }
        match self.footnote.as_deref() {
            Some(footnote) => frame.render_widget(
                Paragraph::new(footnote).style(palette.notice_style()),
                footer,
            ),
            None => hints::draw(frame, footer, "", &self.live(), self.bindings(), palette),
        }
    }

    fn answer(&mut self, action: Action) -> Step {
        match action {
            Action::Quit => return Step::Done,
            Action::Dismiss => return self.dismiss(),
            _ => {}
        }
        self.answer_list(action)
    }

    fn key(&mut self, key: &KeyEvent) -> Step {
        self.footnote = None;
        if let Some(filter) = &mut self.filter {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && keymap::INIT.action(key) == Some(Action::Dismiss)
            {
                return Step::Cancel;
            }
            let reply = filter.key(key);
            self.query = filter.text().to_owned();
            if reply != Reply::Editing {
                if reply == Reply::Dismissed {
                    self.query.clear();
                }
                self.filter = None;
            }
            let rows = self.rows();
            self.cursor = 0;
            self.settle(rows);
            return Step::Continue;
        }
        answer_key(self, key)
    }

    fn paste(&mut self, text: &str) -> Step {
        if let Some(filter) = &mut self.filter {
            filter.paste(text);
            self.query = filter.text().to_owned();
            self.cursor = 0;
        }
        Step::Continue
    }

    /// The keys this screen binds.
    fn bindings(&self) -> &'static keymap::Table {
        &keymap::INIT
    }

    fn live(&self) -> Vec<Action> {
        let rows = self.rows();
        let visible = self.visible_rows(rows);
        let items = Self::visible_items(rows, &visible).len();
        let mut live = Vec::new();
        if self.current(rows).is_some_and(rows::is_item) {
            live.push(Action::Toggle);
            live.extend(
                [Action::First, Action::Last, Action::Down, Action::Up]
                    .into_iter()
                    .filter(|action| movement_changes(self.cursor, items, *action)),
            );
        }
        live.push(Action::Search);
        live.extend(SHARED);
        live.extend(presets());
        live
    }
}

/// Whether one action still moves the cursor: an end already reached earns no hint.
fn movement_changes(cursor: usize, items: usize, action: Action) -> bool {
    match action {
        Action::Down => stepped_index(cursor, 1, items) != cursor,
        Action::Up => stepped_index(cursor, -1, items) != cursor,
        Action::First => cursor != 0,
        Action::Last => cursor != items.saturating_sub(1),
        _ => true,
    }
}

/// The preset row: every offered preset, the one the selection matches highlighted, and
/// `custom` while the selection matches none.
fn preset_bar(canonical: &FeatureSet, palette: Palette) -> Line<'static> {
    let matched = features::preset_name(canonical);
    let mut spans = vec![Span::styled("preset", palette.dim_style())];
    if matched.is_none() {
        spans.push(Span::styled(" custom", palette.notice_style()));
    }
    for (position, preset) in features::preset_choices().enumerate() {
        let (shortcut, name) = if matched == Some(preset.name) {
            (palette.accent_style(), palette.emphasis_style())
        } else {
            (palette.dim_style(), palette.dim_style())
        };
        let key = keymap::INIT
            .key_spelling(Action::ApplyPreset(position as u8))
            .unwrap_or("");
        spans.push(Span::styled(format!("  [{key}] "), shortcut));
        spans.push(Span::styled(preset.name, name));
    }
    Line::from(spans)
}

/// One note per feature the selection gained or lost, each with the model's reason for it.
fn differences(before: &FeatureSet, after: &FeatureSet) -> Vec<String> {
    let mut notes = Vec::new();
    for feature in after.iter().filter(|feature| !before.contains(*feature)) {
        notes.push(match closure_reason(before, after, feature) {
            Some(requirer) => format!(
                "+ {} (required by {})",
                features::feature_name(feature),
                features::feature_name(requirer)
            ),
            None => format!("+ {}", features::feature_name(feature)),
        });
    }
    for feature in before.iter().filter(|feature| !after.contains(*feature)) {
        let name = features::feature_name(feature);
        notes.push(match features::replacement(after, feature) {
            Some(winner) => format!("− {name} ({})", rows::replaced_by(winner)),
            None => match selection::unfilled_slots(feature, after).first() {
                Some(slot) => format!("− {name} ({})", rows::requires(*slot)),
                None => format!("− {name}"),
            },
        });
    }
    notes
}

/// The selected feature whose requirement an added feature fills.
fn closure_reason(
    filled_before: &FeatureSet,
    after: &FeatureSet,
    added: Feature,
) -> Option<Feature> {
    let slot = added.slot();
    after
        .iter()
        .find(|feature| selection::unfilled_slots(*feature, filled_before).contains(&slot))
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use super::*;

    fn set(features: &[Feature]) -> FeatureSet {
        FeatureSet::new(features.iter().copied())
    }

    /// A view over `selected`, titled for a site that reads the same on any host.
    fn view(selected: &[Feature]) -> View {
        View::new(selection::complete(&set(selected)), Path::new("/tmp/site"))
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Puts the cursor on `row`, which must be one the cursor may land on.
    fn focus(view: &mut View, row: Row) {
        let rows = view.rows();
        let visible = view.visible_rows(rows);
        let index = rows
            .iter()
            .position(|candidate| *candidate == row)
            .unwrap_or_else(|| panic!("{row:?} is not a row of {rows:?}"));
        view.cursor = View::visible_items(rows, &visible)
            .iter()
            .position(|position| visible[*position] == index)
            .unwrap_or_else(|| panic!("{row:?} is a title, not an item"));
    }

    /// The footnote the last key left behind.
    fn footnote(view: &View) -> Option<&str> {
        view.footnote.as_deref()
    }

    /// The text one frame shows, line by line, with each line's right margin trimmed.
    fn drawn(view: &mut View) -> Vec<String> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 28)).unwrap();
        terminal
            .draw(|frame| view.draw(frame, Palette::new(false)))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .chunks(100)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    /// A view over the rich preset with its cursor on the row for `row`.
    fn rich_view(row: Row) -> View {
        let mut view = view(&[]);
        view.selected = canon(&features::features("rich"));
        focus(&mut view, row);
        view
    }

    #[test]
    fn short_view_keeps_selected_feature() {
        for height in [1, 2, 3, 4, 5] {
            let mut view = view(&[]);
            focus(&mut view, Row::Atom(Feature::StarterStylesheet));
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, height)).unwrap();
            terminal
                .draw(|frame| view.draw(frame, Palette::new(false)))
                .unwrap();
            let displayed = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(displayed.contains("starter-stylesheet"), "{displayed}");
        }
    }
    #[test]
    fn space_closes_over_required_slots() {
        let mut view = view(&[]);
        focus(&mut view, Row::Atom(Feature::TailwindCss));

        assert_eq!(view.key(&key(KeyCode::Char(' '))), Step::Continue);
        assert_eq!(
            view.selection(),
            set(&[Feature::DenoToolchain, Feature::TailwindCss])
        );
    }

    #[test]
    fn runner_removal_drops_dependents() {
        let mut view = view(&[Feature::DenoToolchain, Feature::TailwindCss]);
        focus(&mut view, Row::Atom(Feature::DenoToolchain));

        view.key(&key(KeyCode::Char(' ')));
        assert_eq!(view.selection(), set(&[]));
    }

    #[test]
    fn stronger_provider_replaces_stylesheet() {
        let mut view = view(&[Feature::StarterStylesheet]);
        focus(&mut view, Row::Atom(Feature::TailwindCss));

        view.key(&key(KeyCode::Char(' ')));
        assert_eq!(
            view.selection(),
            set(&[Feature::DenoToolchain, Feature::TailwindCss])
        );
    }

    #[test]
    fn displaced_provider_refuses_selection() {
        let mut view = view(&[Feature::StarterStylesheet, Feature::TailwindCss]);
        focus(&mut view, Row::Atom(Feature::StarterStylesheet));

        view.key(&key(KeyCode::Char(' ')));
        assert_eq!(
            view.selection(),
            set(&[Feature::DenoToolchain, Feature::TailwindCss])
        );
    }

    #[test]
    fn file_row_checks_its_only_writer() {
        for (file, feature) in [
            ("static/web-assets/css/site.css", Feature::StarterStylesheet),
            ("deno.json", Feature::DenoToolchain),
        ] {
            let mut view = view(&[]);
            focus(&mut view, Row::File(file));
            view.key(&key(KeyCode::Char(' ')));
            assert_eq!(view.selection(), set(&[feature]));
        }
    }

    #[test]
    fn file_row_names_writer_choices() {
        let mut view = view(&[]);
        focus(&mut view, Row::File("justfile"));
        view.key(&key(KeyCode::Char(' ')));
        assert_eq!(view.selection(), set(&[]));
        let note = footnote(&view).expect("ambiguous writers are explained");
        for writer in features::file_writers("justfile") {
            assert!(note.contains(features::feature_name(writer)));
        }
    }

    #[test]
    fn displaced_file_writer_refuses_selection() {
        let mut view = view(&[]);
        view.selected = canon(&features::features("rich"));
        focus(&mut view, Row::File("static/web-assets/css/site.css"));
        view.key(&key(KeyCode::Char(' ')));
        assert_eq!(view.selection(), canon(&features::features("rich")));
    }

    #[test]
    fn preset_keys_replace_the_selection() {
        let mut view = view(&[]);
        view.key(&key(KeyCode::Char('1')));
        assert_eq!(view.selection(), canon(&features::features("rich")));

        view.key(&key(KeyCode::Char('3')));
        assert_eq!(view.selection(), set(&[]));
    }

    #[test]
    fn y_accepts_the_selection() {
        let mut view = view(&[Feature::Feed]);
        assert_eq!(view.key(&key(KeyCode::Char('y'))), Step::Done);
        assert_eq!(view.selection(), set(&[Feature::Feed]));
    }

    #[test]
    fn cancel_keys_end_session() {
        for cancel in [
            key(KeyCode::Char('q')),
            key(KeyCode::Esc),
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        ] {
            let mut view = view(&[Feature::Feed]);
            assert_eq!(view.key(&cancel), Step::Cancel);
            assert_eq!(view.selection(), set(&[Feature::Feed]));
        }
    }

    #[test]
    fn next_key_clears_the_footnote() {
        let mut view = view(&[]);
        focus(&mut view, Row::Atom(Feature::Feed));
        view.key(&key(KeyCode::Char(' ')));
        assert!(footnote(&view).is_some());

        view.key(&key(KeyCode::Char('z')));
        assert_eq!(footnote(&view), None);
    }

    /// The row the cursor is on, as the frame would draw it.
    fn cursor_row(view: &View) -> Option<Row> {
        view.current(view.rows())
    }

    #[test]
    fn filter_keeps_items_with_their_headings() {
        let mut view = rich_view(Row::Atom(Feature::TailwindCss));
        view.key(&key(KeyCode::Char('/')));
        for character in "fee".chars() {
            view.key(&key(KeyCode::Char(character)));
        }
        let rows = view.rows();
        let visible: Vec<Row> = view
            .visible_rows(rows)
            .into_iter()
            .map(|index| rows[index])
            .collect();
        assert_eq!(
            visible,
            vec![Row::Group(1), Row::Atom(Feature::Feed)],
            "a heading draws with its matching item, and nothing else"
        );
        assert_eq!(cursor_row(&view), Some(Row::Atom(Feature::Feed)));
    }

    #[test]
    fn filter_matches_case_insensitively() {
        let mut view = rich_view(Row::Atom(Feature::TailwindCss));
        view.key(&key(KeyCode::Char('/')));
        for character in "DENO".chars() {
            view.key(&key(KeyCode::Char(character)));
        }
        assert_eq!(cursor_row(&view), Some(Row::Atom(Feature::DenoToolchain)));
    }

    #[test]
    fn filter_editing_consumes_list_keys() {
        let mut view = rich_view(Row::Atom(Feature::TailwindCss));
        view.key(&key(KeyCode::Char('/')));
        // `q` types instead of cancelling, and `Esc` clears the filter without leaving the screen.
        assert_eq!(view.key(&key(KeyCode::Char('q'))), Step::Continue);
        assert_eq!(view.query, "q");
        assert_eq!(view.key(&key(KeyCode::Esc)), Step::Continue);
        assert!(view.query.is_empty());
        assert!(view.filter.is_none());
        assert_eq!(view.key(&key(KeyCode::Char('q'))), Step::Cancel);
    }

    #[test]
    fn enter_accepts_the_filter_line() {
        let mut view = rich_view(Row::Atom(Feature::TailwindCss));
        view.key(&key(KeyCode::Char('/')));
        view.key(&key(KeyCode::Char('j')));
        assert_eq!(view.key(&key(KeyCode::Enter)), Step::Continue);
        assert_eq!(view.query, "j");
        assert!(view.filter.is_none());
        assert_eq!(cursor_row(&view), Some(Row::File("justfile")));
        view.key(&key(KeyCode::Down));
        assert_eq!(cursor_row(&view), Some(Row::File("deno.json")));
        view.key(&key(KeyCode::Up));
        assert_eq!(cursor_row(&view), Some(Row::File("justfile")));
    }

    #[test]
    fn navigation_scrolls_to_last_row() {
        let mut view = rich_view(Row::Atom(Feature::Feed));
        view.key(&key(KeyCode::End));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| view.draw(frame, Palette::new(false)))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let lines: Vec<String> = buffer
            .content()
            .chunks(80)
            .map(|row| row.iter().map(|cell| cell.symbol().to_owned()).collect())
            .collect();
        assert!(
            lines.iter().any(|line| line.contains("tailwind-sources")),
            "the last item is off screen: {lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.contains("starter-stylesheet")),
            "the window did not scroll: {lines:?}"
        );
    }

    #[test]
    fn paste_updates_filter_matches() {
        let mut view = view(&[]);
        view.key(&key(KeyCode::Char('/')));
        assert_eq!(view.paste("DENO"), Step::Continue);
        assert_eq!(cursor_row(&view), Some(Row::Atom(Feature::DenoToolchain)));
        view.key(&key(KeyCode::Enter));
        view.key(&key(KeyCode::Char(' ')));
        assert_eq!(view.selection(), set(&[Feature::DenoToolchain]));
    }

    #[test]
    fn control_d_cancels_filter_editing() {
        let mut view = view(&[]);
        view.key(&key(KeyCode::Char('/')));
        let cancel = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(view.key(&cancel), Step::Cancel);
    }

    #[test]
    fn unmatched_filter_keeps_selection() {
        let mut view = view(&[Feature::Feed]);
        view.key(&key(KeyCode::Char('/')));
        view.paste("no-matching-feature");
        assert!(
            drawn(&mut view)
                .iter()
                .any(|line| line.contains("no matching"))
        );
        assert_eq!(cursor_row(&view), None);
        for action in [
            Action::Down,
            Action::Up,
            Action::First,
            Action::Last,
            Action::Toggle,
        ] {
            assert!(!view.live().contains(&action));
        }
        view.key(&key(KeyCode::Enter));
        for action in [
            Action::Down,
            Action::Up,
            Action::First,
            Action::Last,
            Action::Toggle,
        ] {
            assert_eq!(view.answer(action), Step::Continue);
            assert_eq!(cursor_row(&view), None);
            assert_eq!(view.selection(), set(&[Feature::Feed]));
        }
        view.key(&key(KeyCode::Char('/')));
        view.key(&key(KeyCode::Esc));
        assert!(cursor_row(&view).is_some());
    }

    #[test]
    fn movement_hints_follow_the_list_ends() {
        let mut view = view(&[Feature::Feed]);
        assert!(!view.live().contains(&Action::First));
        assert!(!view.live().contains(&Action::Up));
        assert!(view.live().contains(&Action::Down));
        assert!(view.live().contains(&Action::Last));

        view.key(&key(KeyCode::End));
        assert!(view.live().contains(&Action::First));
        assert!(view.live().contains(&Action::Up));
        assert!(!view.live().contains(&Action::Down));
        assert!(!view.live().contains(&Action::Last));
    }
}
