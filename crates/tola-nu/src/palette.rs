//! The config Tola installs: its default TUI palette and color decision.

use nu_protocol::{Config, IntoValue, Span, Value, config::UseAnsiColoring, engine::EngineState};

pub(super) fn apply(engine: &mut EngineState) {
    let mut config = Config::default();
    config.ls.use_ls_colors = false;
    // Nu's Auto decision owns FORCE_COLOR / NO_COLOR / CLICOLOR / TERM and the TTY check.
    config.use_ansi_coloring = UseAnsiColoring::Auto;
    // The TUI's default styles use the terminal's background and ANSI colors.
    for (name, color) in [
        ("int", "cyan"),
        ("float", "cyan"),
        ("filesize", "cyan"),
        ("bool", "magenta"),
        ("datetime", "magenta"),
        ("nothing", "default"),
        ("empty", "default"),
    ] {
        config
            .color_config
            .insert(name.into(), Value::string(color, Span::unknown()));
    }
    engine.set_config(config.clone());
    engine.add_env_var("config".into(), config.into_value(Span::unknown()));
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
        assert!(!config.ls.use_ls_colors);
        assert!(engine.get_env_var("config").is_some());
    }
}
