//! The config Tola installs: its default TUI palette and color decision.

use nu_protocol::{Config, IntoValue, Span, Value, config::UseAnsiColoring, engine::EngineState};

pub(super) fn apply(engine: &mut EngineState) {
    let mut cfg = Config::default();
    cfg.ls.use_ls_colors = false;
    // Nu's Auto decision owns FORCE_COLOR / NO_COLOR / CLICOLOR / TERM and the TTY check.
    cfg.use_ansi_coloring = UseAnsiColoring::Auto;
    for (name, color) in [
        ("int", "cyan"),
        ("float", "cyan"),
        ("filesize", "cyan"),
        ("bool", "magenta"),
        ("datetime", "magenta"),
        ("nothing", "default"),
        ("empty", "default"),
    ] {
        cfg.color_config
            .insert(name.into(), Value::string(color, Span::unknown()));
    }
    engine.set_config(cfg.clone());
    engine.add_env_var("config".into(), cfg.into_value(Span::unknown()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_palette() {
        let mut engine = EngineState::new();
        apply(&mut engine);
        let cfg = engine.get_config();
        assert_eq!(cfg.use_ansi_coloring, UseAnsiColoring::Auto);
        assert!(!cfg.ls.use_ls_colors);
        assert!(engine.get_env_var("config").is_some());
    }
}
