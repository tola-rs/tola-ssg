//! Preflight rendering.

use anyhow::{Result, bail};

use crate::css::config::{LocalPreflightSource, PreflightConfig, PreflightUse};

/// Render selected preflight into the backend representation.
pub fn render(config: &PreflightConfig) -> Result<encre_css::Preflight> {
    match &config.use_ {
        PreflightUse::Profile { name } => render_profile(name, config.scope.as_deref()),
        PreflightUse::Local { name } => render_local(config, name),
    }
}

fn render_profile(name: &str, scope: Option<&str>) -> Result<encre_css::Preflight> {
    if name != "tailwind-v4" {
        bail!("profile preflight `{name}` is not available");
    }
    let preflight = encre_css::Preflight::new_full();
    if let Some(scope) = scope {
        let mut config = encre_css::Config::default();
        config.preflight = preflight;
        let css = encre_css::generate(std::iter::empty::<&str>(), &config);
        return Ok(encre_css::Preflight::new_custom(scope_css(scope, &css)));
    }
    Ok(preflight)
}

fn render_local(config: &PreflightConfig, name: &str) -> Result<encre_css::Preflight> {
    let local = config
        .local
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("local preflight `{name}` is not defined"))?;
    let css = match &local.source {
        LocalPreflightSource::Css(css) => css.clone(),
        LocalPreflightSource::File(path) => std::fs::read_to_string(path)?,
    };
    let css = if let Some(scope) = &config.scope {
        scope_css(scope, &css)
    } else {
        css
    };
    Ok(encre_css::Preflight::new_custom(css))
}

fn scope_css(scope: &str, css: &str) -> String {
    format!("@scope ({scope}) {{\n{css}\n}}\n")
}
