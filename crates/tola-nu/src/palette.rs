//! The config Tola installs: its default TUI palette and color decision.

use nu_protocol::{
    Config, IntoValue, Record, Span, Value, config::UseAnsiColoring, engine::EngineState,
};

pub(super) fn apply(engine: &mut EngineState) {
    let mut config = Config::default();
    config.ls.use_ls_colors = false;
    // Nu's Auto decision owns FORCE_COLOR / NO_COLOR / CLICOLOR / TERM and the TTY check.
    config.use_ansi_coloring = UseAnsiColoring::Auto;
    let (tui, cells) = palette();
    for (name, value) in tui {
        config.tui.insert(name.into(), value);
    }
    for (name, value) in cells {
        config.color_config.insert(name.into(), value);
    }
    engine.set_config(config.clone());
    engine.add_env_var("config".into(), config.into_value(Span::unknown()));
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

fn palette() -> Slots {
    let tui = vec![
        (
            "title_bar",
            style(Some("#0f1115"), Some("#7aa2f7"), Some("b")),
        ),
        ("status_bar", style(Some("#c9d1d9"), Some("#161b22"), None)),
        ("border", style(Some("#30363d"), None, None)),
        ("border_focused", style(Some("#7aa2f7"), None, None)),
        (
            "selected",
            style(Some("#ffffff"), Some("#1f6feb"), Some("b")),
        ),
        ("header", style(Some("#7aa2f7"), None, Some("b"))),
        ("muted", style(Some("#8b949e"), None, None)),
        ("highlight", style(Some("#ffd33d"), None, Some("b"))),
        ("tab_active", style(Some("#7aa2f7"), None, Some("bu"))),
        ("tab_inactive", style(Some("#8b949e"), None, None)),
        ("progress", style(Some("#3fb950"), None, None)),
        ("button", style(Some("#ffffff"), Some("#1f6feb"), None)),
        ("surface", style(None, Some("#0f1115"), None)),
        ("backdrop", style(None, Some("#0a0b0f"), None)),
        ("border_type", string("rounded")),
    ];
    let cells = vec![
        ("string", string("default")),
        ("int", string("#79c0ff")),
        ("float", string("#79c0ff")),
        ("filesize", string("#79c0ff")),
        ("bool", string("#ff7b72")),
        ("datetime", string("#d2a8ff")),
        ("duration", string("default")),
        ("nothing", string("#8b949e")),
        ("empty", string("#8b949e")),
        ("header", string("#7aa2f7")),
    ];
    (tui, cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_default_palette() {
        let mut engine = EngineState::new();
        apply(&mut engine);
        let config = engine.get_config();
        assert_eq!(config.use_ansi_coloring, UseAnsiColoring::Auto);
        assert!(config.tui.contains_key("surface"));
        assert!(config.tui.contains_key("border_focused"));
        assert!(!config.ls.use_ls_colors);
        assert!(engine.get_env_var("config").is_some());
    }
}
