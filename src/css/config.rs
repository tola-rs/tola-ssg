//! Atomic CSS semantic configuration.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;
use toml::Value;
use toml::map::Map;

/// Parsed `atomic.css.toml` configuration.
#[derive(Debug, Clone)]
pub struct AtomicCssFile {
    pub preflight: Option<PreflightConfig>,
    backend: Value,
}

/// Explicit preflight selection and local definitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightConfig {
    pub use_: PreflightUse,
    pub scope: Option<String>,
    pub local: BTreeMap<String, LocalPreflight>,
}

/// Selected preflight source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreflightUse {
    Profile { name: String },
    Local { name: String },
}

/// A local preflight body definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPreflight {
    pub source: LocalPreflightSource,
}

/// Inline or file-backed local preflight CSS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalPreflightSource {
    Css(String),
    File(PathBuf),
}

#[derive(Debug, Error)]
pub enum AtomicCssConfigError {
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
    #[error("{0}")]
    Invalid(String),
}

impl AtomicCssFile {
    pub fn parse_str(input: &str) -> Result<Self, AtomicCssConfigError> {
        let mut value: Value = toml::from_str(input)?;
        let Some(root) = value.as_table() else {
            return Err(AtomicCssConfigError::Invalid(
                "atomic CSS config must be a TOML table".into(),
            ));
        };
        let preflight = root.get("preflight").map(parse_preflight).transpose()?;
        if let Some(root) = value.as_table_mut() {
            root.remove("preflight");
        }
        Ok(Self {
            preflight,
            backend: value,
        })
    }

    pub fn parse_path(path: &std::path::Path) -> Result<Self, AtomicCssConfigError> {
        let input = fs::read_to_string(path).map_err(|err| {
            AtomicCssConfigError::Invalid(format!(
                "failed to read atomic CSS config '{}': {err}",
                path.display()
            ))
        })?;
        let mut config = Self::parse_str(&input)?;
        if let Some(parent) = path.parent() {
            config.resolve_paths(parent);
        }
        Ok(config)
    }

    pub fn backend_config(&self) -> Result<encre_css::Config, AtomicCssConfigError> {
        if self.backend.as_table().is_some_and(Map::is_empty) {
            return Ok(encre_css::Config::default());
        }
        Ok(self.backend.clone().try_into()?)
    }

    fn resolve_paths(&mut self, base: &std::path::Path) {
        let Some(preflight) = &mut self.preflight else {
            return;
        };

        for local in preflight.local.values_mut() {
            if let LocalPreflightSource::File(path) = &mut local.source
                && path.is_relative()
            {
                *path = base.join(&path);
            }
        }
    }
}

impl Default for AtomicCssFile {
    fn default() -> Self {
        Self {
            preflight: None,
            backend: Value::Table(Map::new()),
        }
    }
}

fn parse_preflight(value: &Value) -> Result<PreflightConfig, AtomicCssConfigError> {
    let table = expect_table(value, "preflight")?;
    let use_value = table.get("use").ok_or_else(|| {
        AtomicCssConfigError::Invalid(
            "`preflight.use` is required when `[preflight]` is present".into(),
        )
    })?;
    let use_ = parse_preflight_use(use_value)?;
    let scope = match table.get("scope") {
        Some(Value::String(scope)) if scope == "global" => {
            return Err(AtomicCssConfigError::Invalid(
                "omit `preflight.scope` for global preflight".into(),
            ));
        }
        Some(Value::String(scope)) if scope.trim().is_empty() => {
            return Err(AtomicCssConfigError::Invalid(
                "`preflight.scope` must not be empty".into(),
            ));
        }
        Some(Value::String(scope)) => Some(scope.clone()),
        Some(_) => {
            return Err(AtomicCssConfigError::Invalid(
                "`preflight.scope` must be a string".into(),
            ));
        }
        None => None,
    };

    let mut local = BTreeMap::new();
    for (name, item) in table {
        if name == "use" || name == "scope" {
            continue;
        }
        validate_local_name(name)?;
        let definition = parse_local_preflight(name, item)?;
        local.insert(name.clone(), definition);
    }

    if let PreflightUse::Local { name } = &use_
        && !local.contains_key(name)
    {
        return Err(AtomicCssConfigError::Invalid(format!(
            "local preflight `{name}` is not defined"
        )));
    }

    Ok(PreflightConfig { use_, scope, local })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, tag = "source", rename_all = "lowercase")]
enum RawPreflightUse {
    Profile { name: String },
    Local { name: String },
}

fn parse_preflight_use(value: &Value) -> Result<PreflightUse, AtomicCssConfigError> {
    let raw: RawPreflightUse = value.clone().try_into()?;
    match raw {
        RawPreflightUse::Profile { name } => {
            validate_use_name(&name, "profile")?;
            Ok(PreflightUse::Profile { name })
        }
        RawPreflightUse::Local { name } => {
            validate_use_name(&name, "local")?;
            Ok(PreflightUse::Local { name })
        }
    }
}

fn parse_local_preflight(
    name: &str,
    value: &Value,
) -> Result<LocalPreflight, AtomicCssConfigError> {
    let table = expect_table(value, &format!("preflight.{name}"))?;
    reject_unknown_local_fields(name, table)?;

    let css = table.get("css");
    let file = table.get("file");
    match (css, file) {
        (Some(Value::String(css)), None) => Ok(LocalPreflight {
            source: LocalPreflightSource::Css(css.clone()),
        }),
        (None, Some(Value::String(file))) => Ok(LocalPreflight {
            source: LocalPreflightSource::File(PathBuf::from(file)),
        }),
        (Some(_), None) => Err(AtomicCssConfigError::Invalid(format!(
            "`preflight.{name}.css` must be a string"
        ))),
        (None, Some(_)) => Err(AtomicCssConfigError::Invalid(format!(
            "`preflight.{name}.file` must be a string"
        ))),
        _ => Err(AtomicCssConfigError::Invalid(format!(
            "`preflight.{name}` must specify exactly one of `css` or `file`"
        ))),
    }
}

fn reject_unknown_local_fields(
    name: &str,
    table: &Map<String, Value>,
) -> Result<(), AtomicCssConfigError> {
    for key in table.keys() {
        if key != "css" && key != "file" {
            return Err(AtomicCssConfigError::Invalid(format!(
                "`preflight.{name}.{key}` is invalid; local preflight definitions only accept `css` or `file`"
            )));
        }
    }
    Ok(())
}

fn expect_table<'a>(
    value: &'a Value,
    path: &str,
) -> Result<&'a Map<String, Value>, AtomicCssConfigError> {
    value
        .as_table()
        .ok_or_else(|| AtomicCssConfigError::Invalid(format!("`{path}` must be a TOML table")))
}

fn validate_use_name(name: &str, source: &str) -> Result<(), AtomicCssConfigError> {
    if valid_name(name) {
        Ok(())
    } else {
        Err(AtomicCssConfigError::Invalid(format!(
            "`preflight.use.name` has invalid {source} preflight name `{name}`"
        )))
    }
}

fn validate_local_name(name: &str) -> Result<(), AtomicCssConfigError> {
    if name == "use" || name == "scope" {
        return Err(AtomicCssConfigError::Invalid(format!(
            "`preflight.{name}` is reserved by the TOML control field `{name}`"
        )));
    }
    if valid_name(name) {
        Ok(())
    } else {
        Err(AtomicCssConfigError::Invalid(format!(
            "invalid local preflight name `{name}`"
        )))
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn preflight_selects_profile_source() {
        let file = AtomicCssFile::parse_str(
            r#"
[preflight]
use = { source = "profile", name = "tailwind-v4" }
"#,
        )
        .unwrap();

        let preflight = file.preflight.unwrap();
        assert!(matches!(
            preflight.use_,
            PreflightUse::Profile { ref name } if name == "tailwind-v4"
        ));
        assert!(preflight.scope.is_none());
        assert!(preflight.local.is_empty());
    }

    #[test]
    fn preflight_selects_local_definition() {
        let file = AtomicCssFile::parse_str(
            r#"
[preflight]
use = { source = "local", name = "site-reset" }
scope = ".app"

[preflight.site-reset]
file = "styles/preflight.css"
"#,
        )
        .unwrap();

        let preflight = file.preflight.unwrap();
        assert!(matches!(
            preflight.use_,
            PreflightUse::Local { ref name } if name == "site-reset"
        ));
        assert_eq!(preflight.scope.as_deref(), Some(".app"));
        assert_eq!(
            preflight.local.get("site-reset").unwrap().source,
            LocalPreflightSource::File("styles/preflight.css".into())
        );
    }

    #[test]
    fn local_preflight_requires_exactly_one_body_source() {
        let err = AtomicCssFile::parse_str(
            r#"
[preflight]
use = { source = "local", name = "bad" }

[preflight.bad]
css = "body {}"
file = "styles/preflight.css"
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("exactly one of `css` or `file`"));
    }

    #[test]
    fn local_preflight_selection_must_exist() {
        let err = AtomicCssFile::parse_str(
            r#"
[preflight]
use = { source = "local", name = "missing" }
"#,
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("local preflight `missing` is not defined")
        );
    }

    #[test]
    fn local_preflight_file_is_resolved_from_config_dir() {
        let temp = TempDir::new().unwrap();
        let config_dir = temp.path().join("config");
        fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("atomic.css.toml");
        fs::write(
            &config_path,
            r#"
[preflight]
use = { source = "local", name = "site" }

[preflight.site]
file = "preflight.css"
"#,
        )
        .unwrap();

        let file = AtomicCssFile::parse_path(&config_path).unwrap();
        let preflight = file.preflight.unwrap();

        assert_eq!(
            preflight.local.get("site").unwrap().source,
            LocalPreflightSource::File(config_dir.join("preflight.css"))
        );
    }
}
