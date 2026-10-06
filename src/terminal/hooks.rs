//! The hooks a run will execute, grouped by the stage that owns them.

use tola_build::build::BuildMode;
use tola_build::config::section::build::hooks::HookStage;
use tola_build::hooks::ConfiguredHook;
use unicode_width::UnicodeWidthStr;

use super::Palette;

/// Stages a command that never publishes schedules.
pub(crate) const PRE_PUBLICATION_STAGES: &[HookStage] =
    &[HookStage::BeforeBuild, HookStage::GenerateOutputs];

/// Every stage a command that publishes schedules.
pub(crate) const EVERY_STAGE: &[HookStage] = &[
    HookStage::BeforeBuild,
    HookStage::GenerateOutputs,
    HookStage::AfterPublish,
];

/// Columns assumed when output is redirected instead of sized by a terminal.
const REDIRECTED_COLUMNS: usize = 100;

/// Narrowest block this renderer produces, whatever the terminal reports.
const MINIMUM_COLUMNS: usize = 40;

const NAME_SEPARATOR: &str = ", ";
const SKIPPED_MARK: &str = " (skipped in this session)";

/// Render the hooks this run executes, one line per stage.
///
/// Each line names the entries of one stage in declaration order, so the reader sees at a
/// glance what this build will do. A hook the build mode leaves out is named with the reason
/// instead of disappearing.
pub(crate) fn overview(
    hooks: &[ConfiguredHook<'_>],
    mode: BuildMode,
    stages: &[HookStage],
    columns: Option<usize>,
    palette: Palette,
) -> String {
    let listed = hooks
        .iter()
        .filter(|hook| stages.contains(&hook.stage))
        .collect::<Vec<_>>();
    if listed.is_empty() {
        return String::new();
    }
    let columns = columns.unwrap_or(REDIRECTED_COLUMNS).max(MINIMUM_COLUMNS);
    let mut rendered = String::from("Hooks:\n");
    let mut open: Option<HookStage> = None;
    let mut line = String::new();
    // The visible width of the line; styling never counts toward the terminal's columns, and a
    // grapheme covers the columns it draws in rather than the bytes or the characters it holds.
    let mut width = 0;
    let mut indent = 0;
    for hook in listed {
        let name = crate::terminal::text::single_line(hook.label);
        let skipped = if hook.participates(mode) {
            ""
        } else {
            SKIPPED_MARK
        };
        let entry_width = name.width() + skipped.width();
        if open == Some(hook.stage) {
            if width + NAME_SEPARATOR.width() + entry_width > columns {
                line.push('\n');
                rendered.push_str(&line);
                line = " ".repeat(indent);
                width = indent;
            } else {
                line.push_str(NAME_SEPARATOR);
                width += NAME_SEPARATOR.width();
            }
        } else {
            if !line.is_empty() {
                line.push('\n');
                rendered.push_str(&line);
            }
            open = Some(hook.stage);
            let stage = hook.stage.as_str();
            let prefix = format!("{}{stage}: ", super::INDENT_UNIT);
            indent = prefix.width();
            line = format!("{}{}: ", super::INDENT_UNIT, palette.hook_stage(stage));
            width = indent;
        }
        line.push_str(&palette.hook_name(&name));
        line.push_str(skipped);
        width += entry_width;
    }
    line.push('\n');
    rendered.push_str(&line);
    rendered.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tola_build::config::section::build::{BeforeBuildHookConfig, HooksConfig};

    #[test]
    fn wrapped_entries_keep_stage_indent() {
        let mut hooks = HooksConfig::default();
        for label in ["alpha", "bravo", "charlie", "delta", "echo"] {
            hooks.before_build.push(BeforeBuildHookConfig {
                name: label.to_owned(),
                command: vec!["tool".into()],
                ..BeforeBuildHookConfig::default()
            });
        }
        let configured = tola_build::hooks::configured_hooks(&hooks).collect::<Vec<_>>();

        assert_eq!(
            overview(
                &configured,
                BuildMode::Production,
                EVERY_STAGE,
                Some(40),
                Palette::new(false)
            ),
            [
                "Hooks:",
                "  before-build: alpha, bravo, charlie",
                "                delta, echo",
            ]
            .join("\n")
        );
    }

    #[test]
    fn hook_overview_fits_terminal_columns() {
        let mut hooks = HooksConfig::default();
        for label in ["文档", "图片", "样式", "脚本", "图标"] {
            hooks.before_build.push(BeforeBuildHookConfig {
                name: label.to_owned(),
                command: vec!["tool".into()],
                ..BeforeBuildHookConfig::default()
            });
        }
        let configured = tola_build::hooks::configured_hooks(&hooks).collect::<Vec<_>>();
        // Five names of two graphemes each: four columns each, so the line they share reaches 44
        // columns and only wrapping keeps it inside a terminal that reports 40.
        let rendered = overview(
            &configured,
            BuildMode::Production,
            EVERY_STAGE,
            Some(40),
            Palette::new(false),
        );
        for line in rendered.lines() {
            assert!(line.width() <= 40, "{line:?}");
        }
    }

    #[test]
    fn color_leaves_overview_layout_unchanged() {
        let mut hooks = HooksConfig::default();
        for label in ["alpha", "bravo", "charlie", "delta", "echo"] {
            hooks.before_build.push(BeforeBuildHookConfig {
                name: label.to_owned(),
                command: vec!["tool".into()],
                ..BeforeBuildHookConfig::default()
            });
        }
        let configured = tola_build::hooks::configured_hooks(&hooks).collect::<Vec<_>>();

        let plain = overview(
            &configured,
            BuildMode::Production,
            EVERY_STAGE,
            Some(40),
            Palette::new(false),
        );
        let colored = overview(
            &configured,
            BuildMode::Production,
            EVERY_STAGE,
            Some(40),
            Palette::new(true),
        );

        assert!(colored.contains('\u{1b}'), "{colored:?}");
        assert_eq!(visible_text(&colored), plain);
    }

    /// The characters a terminal draws, with SGR escape sequences removed.
    fn visible_text(styled: &str) -> String {
        let mut visible = String::new();
        let mut escaped = false;
        for character in styled.chars() {
            if escaped {
                escaped = !character.is_ascii_alphabetic();
            } else if character == '\u{1b}' {
                escaped = true;
            } else {
                visible.push(character);
            }
        }
        visible
    }
}
