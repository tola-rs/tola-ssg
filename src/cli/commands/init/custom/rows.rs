//! The rows the init screen's list shows, and the text each one has.

use std::sync::LazyLock;

use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use crate::terminal::style::Palette;

use super::super::features::{self, Feature, FeatureSet, GROUPS, SlotId};
use super::super::selection;

/// One row of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Row {
    /// A group of [`GROUPS`] at this position.
    Group(usize),
    /// One provider of a group's slots.
    Atom(Feature),
    /// The files the selection writes.
    Files,
    /// One file the selection writes, by the path the row shows.
    File(&'static str),
}

/// The state mark one row has: `[■]` when the row is on, `[ ]` when it is not.
fn mark(row: Row, selected: &FeatureSet) -> StyledText {
    let on = match row {
        Row::Group(_) | Row::Files => return styled(String::new(), Role::Plain),
        Row::Atom(feature) => selected.contains(feature),
        Row::File(file) => features::file_selected(file, selected),
    };
    styled(
        format!("{} ", if on { "[■]" } else { "[ ]" }),
        if on { Role::Accent } else { Role::Dim },
    )
}

/// The leading field one row has: `❯ ` on the row the reader is on, blanks on every other row.
/// Every row reserves the two columns so the marks line up.
fn leading(focused: bool) -> StyledText {
    if focused {
        styled("❯ ", Role::Accent)
    } else {
        styled("  ", Role::Plain)
    }
}

/// The phrase naming the feature that displaces `winner`'s slot peer, for the row hint and the
/// footnote alike.
pub(super) fn replaced_by(winner: Feature) -> String {
    format!("replaced by {}", features::feature_name(winner))
}

/// The phrase naming the features that fill `slot`, for the row hint and the footnote alike.
pub(super) fn requires(slot: SlotId) -> String {
    let providers = features::providers(slot);
    match providers {
        [only] => format!("requires {}", features::feature_name(*only)),
        many => format!(
            "requires one of {}",
            many.iter()
                .map(|provider| features::feature_name(*provider))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The name one row has, when the cursor may land on it; a title has none.
pub(super) fn name(row: Row) -> Option<&'static str> {
    match row {
        Row::Atom(feature) => Some(features::feature_name(feature)),
        Row::File(file) => Some(file),
        Row::Group(_) | Row::Files => None,
    }
}

/// Whether the cursor may land on this row: the atoms and the files, not the titles.
pub(super) fn is_item(row: Row) -> bool {
    name(row).is_some()
}

/// The column every item's hint starts at: after the leading field, the mark, and the longest name
/// the model has.
fn hint_column() -> usize {
    *HINT_COLUMN
}

/// The hint column, computed once from the model's longest name.
static HINT_COLUMN: LazyLock<usize> = LazyLock::new(|| {
    let longest = features::selectable_features()
        .map(|feature| features::feature_name(feature).width())
        .chain(
            features::decided_files()
                .into_iter()
                .map(UnicodeWidthStr::width),
        )
        .max()
        .unwrap_or(0);
    leading(false).text.width() + "[■]".width() + longest + 1
});

/// The palette style one styled of a row or card draws under.
pub(super) fn role_style(role: Role, palette: Palette) -> Style {
    match role {
        Role::Plain => Style::default(),
        Role::Dim => palette.dim_style(),
        Role::Emphasis => palette.emphasis_style(),
        Role::Accent => palette.accent_style(),
        Role::Pending => palette.notice_style(),
    }
}

/// The palette role one styled of a row or card is drawn under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Role {
    /// A styled that paints nothing: the columns an unfocused row reserves.
    Plain,
    /// A reading aid: a mark that is not in effect, or a name that is not.
    Dim,
    /// A heading: the group and Files titles.
    Emphasis,
    /// A mark of a row that is in effect.
    Accent,
    /// A hint of a row that is not yet in effect.
    Pending,
}

/// The role an item's name draws under: plain when the item is on, dim when it is not. The mark
/// pair has the state; only the headings are bold.
fn name_role(in_effect: bool) -> Role {
    if in_effect { Role::Plain } else { Role::Dim }
}

/// `name` padded to `field`, so the row's hint starts in the hint column.
fn fielded(name: &str, field: usize) -> String {
    format!("{name:<field$}")
}

fn styled(text: impl Into<String>, role: Role) -> StyledText {
    StyledText {
        text: text.into(),
        role,
    }
}

/// One styled of a row line.
pub(super) struct StyledText {
    pub(super) text: String,
    pub(super) role: Role,
}

/// The rows the list shows: every group with its providers, then the files.
pub(super) fn rows() -> &'static [Row] {
    &ROWS
}

/// The rows the list shows, built once: every group with its providers, then the files.
static ROWS: LazyLock<Vec<Row>> = LazyLock::new(|| {
    let mut rows = Vec::new();
    for (index, group) in GROUPS.iter().enumerate() {
        rows.push(Row::Group(index));
        for slot in group.slots {
            rows.extend(features::providers(*slot).iter().copied().map(Row::Atom));
        }
    }
    rows.push(Row::Files);
    rows.extend(features::decided_files().into_iter().map(Row::File));
    rows
});

/// Feature marks and file marks describe the same completed selection.
pub(super) fn line(row: Row, selected: &FeatureSet, focused: bool, width: u16) -> Vec<StyledText> {
    let mut pieces = vec![leading(focused), mark(row, selected)];
    let used = pieces
        .iter()
        .map(|styled| styled.text.width())
        .sum::<usize>();
    let field = hint_column().saturating_sub(used);
    match row {
        Row::Group(index) => {
            pieces.push(styled(GROUPS[index].name, Role::Emphasis));
        }
        Row::Atom(feature) => {
            let in_effect = selected.contains(feature);
            let hint = hint(selected, feature).map(|hint| (hint.text(), hint.role()));
            let name = features::feature_name(feature);
            let field = match &hint {
                Some((text, _)) if hint_column() + text.width() > usize::from(width) => {
                    name.width() + 1
                }
                _ => field,
            };
            pieces.push(styled(fielded(name, field), name_role(in_effect)));
            if let Some((text, role)) = hint {
                pieces.push(styled(text, role));
            }
        }
        Row::Files => {
            pieces.push(styled("Files", Role::Emphasis));
        }
        Row::File(file) => {
            pieces.push(styled(
                fielded(file, field),
                name_role(features::file_selected(file, selected)),
            ));
        }
    }
    pieces
}

/// Why one feature is not in effect, when it is not.
enum Hint {
    /// Another provider of its slot displaces it.
    Replaced(Feature),
    /// Selecting it needs these slots filled first.
    Requires(Vec<SlotId>),
}

/// Why `feature` is not in effect under `selected`, when it is not.
fn hint(selected: &FeatureSet, feature: Feature) -> Option<Hint> {
    if selected.contains(feature) {
        return None;
    }
    if let Some(winner) = features::replacement(selected, feature) {
        return Some(Hint::Replaced(winner));
    }
    let unfilled = selection::unfilled_slots(feature, selected);
    (!unfilled.is_empty()).then_some(Hint::Requires(unfilled))
}

impl Hint {
    /// The hint as the row shows it.
    fn text(&self) -> String {
        match self {
            Hint::Replaced(winner) => replaced_by(*winner),
            Hint::Requires(slots) => slots
                .iter()
                .map(|slot| requires(*slot))
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    /// The role the hint draws under.
    fn role(&self) -> Role {
        match self {
            Hint::Replaced(_) => Role::Dim,
            Hint::Requires(_) => Role::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark_text(row: Row, selected: &FeatureSet) -> String {
        mark(row, selected).text
    }

    #[test]
    fn marks_follow_effective_features() {
        let selected = selection::complete(&FeatureSet::new([
            Feature::StarterStylesheet,
            Feature::TailwindCss,
        ]));
        for (row, checked) in [
            (Row::Atom(Feature::StarterStylesheet), false),
            (Row::Atom(Feature::TailwindCss), true),
            (Row::Atom(Feature::DenoToolchain), true),
            (Row::File("static/web-assets/css/site.css"), false),
            (Row::File("static/tailwind-sources/site.css"), true),
            (Row::File("deno.json"), true),
            (Row::File("justfile"), true),
        ] {
            assert_eq!(mark_text(row, &selected).contains('■'), checked, "{row:?}");
        }
        assert!(matches!(
            hint(&selected, Feature::StarterStylesheet),
            Some(Hint::Replaced(Feature::TailwindCss))
        ));
    }

    #[test]
    fn unmet_requirements_name_provider() {
        let selected = FeatureSet::default();
        let hint = hint(&selected, Feature::TailwindCss)
            .expect("the disabled feature names its missing requirement");
        assert!(
            hint.text()
                .contains(features::feature_name(Feature::DenoToolchain))
        );
    }
}
