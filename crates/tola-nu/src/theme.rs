//! The `$env.config` Tola installs: its TUI palette and color decision.

use nu_protocol::{
    Config, IntoValue, Record, Span, Value, config::UseAnsiColoring, engine::EngineState,
};
use tola_highlight::ThemeChoice;

pub(super) fn apply(engine: &mut EngineState) {
    let choice = environment(engine, "TOLA_THEME")
        .as_deref()
        .and_then(ThemeChoice::parse)
        .unwrap_or(ThemeChoice::Dark);
    let mut config = Config::default();
    config.ls.use_ls_colors = false;
    // Nu's Auto decision owns FORCE_COLOR / NO_COLOR / CLICOLOR / TERM and the TTY check.
    config.use_ansi_coloring = if choice == ThemeChoice::Plain {
        UseAnsiColoring::False
    } else {
        UseAnsiColoring::Auto
    };
    if choice != ThemeChoice::Plain {
        let (tui, cells) = palette(choice);
        for (name, value) in tui {
            config.tui.insert(name.into(), value);
        }
        for (name, value) in cells {
            config.color_config.insert(name.into(), value);
        }
    }
    engine.set_config(config.clone());
    engine.add_env_var("config".into(), config.into_value(Span::unknown()));
}

fn environment(engine: &EngineState, name: &str) -> Option<String> {
    engine
        .get_env_var(name)
        .and_then(|value| value.as_str().ok())
        .map(str::to_owned)
}

fn style(fg: Option<&str>, bg: Option<&str>, attr: Option<&str>) -> Value {
    let span = Span::unknown();
    let mut record = Record::new();
    if let Some(fg) = fg {
        record.insert("fg", Value::string(fg, span));
    }
    if let Some(bg) = bg {
        record.insert("bg", Value::string(bg, span));
    }
    if let Some(attr) = attr {
        record.insert("attr", Value::string(attr, span));
    }
    Value::record(record, span)
}

fn string(value: &str) -> Value {
    Value::string(value, Span::unknown())
}

type Slot = (&'static str, Value);
type Slots = (Vec<Slot>, Vec<Slot>);

fn palette(choice: ThemeChoice) -> Slots {
    let dark = choice == ThemeChoice::Dark;
    let tui = vec![
        (
            "title_bar",
            if dark {
                style(Some("#0f1115"), Some("#7aa2f7"), Some("b"))
            } else {
                style(Some("#ffffff"), Some("#2563eb"), Some("b"))
            },
        ),
        (
            "status_bar",
            if dark {
                style(Some("#c9d1d9"), Some("#161b22"), None)
            } else {
                style(Some("#4b5563"), Some("#eef1f5"), None)
            },
        ),
        (
            "border",
            style(Some(if dark { "#30363d" } else { "#d0d5dd" }), None, None),
        ),
        (
            "border_focused",
            style(Some(if dark { "#7aa2f7" } else { "#2563eb" }), None, None),
        ),
        (
            "selected",
            if dark {
                style(Some("#ffffff"), Some("#1f6feb"), Some("b"))
            } else {
                style(Some("#1f2937"), Some("#dbeafe"), None)
            },
        ),
        (
            "header",
            style(
                Some(if dark { "#7aa2f7" } else { "#1d4ed8" }),
                None,
                Some("b"),
            ),
        ),
        (
            "muted",
            style(Some(if dark { "#8b949e" } else { "#6b7280" }), None, None),
        ),
        (
            "highlight",
            style(
                Some(if dark { "#ffd33d" } else { "#b45309" }),
                None,
                Some("b"),
            ),
        ),
        (
            "tab_active",
            style(
                Some(if dark { "#7aa2f7" } else { "#1d4ed8" }),
                None,
                Some("bu"),
            ),
        ),
        (
            "tab_inactive",
            style(Some(if dark { "#8b949e" } else { "#6b7280" }), None, None),
        ),
        (
            "progress",
            style(Some(if dark { "#3fb950" } else { "#15803d" }), None, None),
        ),
        (
            "button",
            if dark {
                style(Some("#ffffff"), Some("#1f6feb"), None)
            } else {
                style(Some("#ffffff"), Some("#2563eb"), None)
            },
        ),
        (
            "surface",
            style(None, Some(if dark { "#0f1115" } else { "#fbfbfd" }), None),
        ),
        (
            "backdrop",
            style(None, Some(if dark { "#0a0b0f" } else { "#f2f3f5" }), None),
        ),
        ("border_type", string("rounded")),
    ];
    let cells = vec![
        ("string", string("default")),
        ("int", string(if dark { "#79c0ff" } else { "#1d4ed8" })),
        ("float", string(if dark { "#79c0ff" } else { "#1d4ed8" })),
        ("filesize", string(if dark { "#79c0ff" } else { "#1d4ed8" })),
        ("bool", string(if dark { "#ff7b72" } else { "#b91c1c" })),
        ("datetime", string(if dark { "#d2a8ff" } else { "#7e22ce" })),
        ("duration", string("default")),
        ("nothing", string(if dark { "#8b949e" } else { "#6b7280" })),
        ("empty", string(if dark { "#8b949e" } else { "#6b7280" })),
        ("header", string(if dark { "#7aa2f7" } else { "#1d4ed8" })),
    ];
    (tui, cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_installs_surface_and_slots() {
        let mut engine = EngineState::new();
        engine.add_env_var("TOLA_THEME".into(), Value::string("dark", Span::unknown()));
        apply(&mut engine);
        let config = engine.get_config();
        assert_eq!(config.use_ansi_coloring, UseAnsiColoring::Auto);
        assert!(config.tui.contains_key("surface"));
        assert!(config.tui.contains_key("border_focused"));
        assert!(!config.ls.use_ls_colors);
        assert!(engine.get_env_var("config").is_some());
    }

    #[test]
    fn plain_theme_disables_colors() {
        let mut engine = EngineState::new();
        engine.add_env_var("TOLA_THEME".into(), Value::string("plain", Span::unknown()));
        apply(&mut engine);
        assert_eq!(
            engine.get_config().use_ansi_coloring,
            UseAnsiColoring::False
        );
    }
}
