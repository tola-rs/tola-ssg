//! Static option data shared by modules and sites.

use typst::foundations::{Array, Dict, IntoValue, Value};

/// A nonempty TOML key path within the declaration owner's options.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct OptionPath(Vec<String>);

impl TryFrom<String> for OptionPath {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let keys = toml_edit::Key::parse(&value)
            .map_err(|error| format!("invalid option path `{value}`: {}", error.message()))?;
        if keys.is_empty() {
            return Err("an option path must contain a key".into());
        }
        Ok(Self(
            keys.into_iter().map(|key| key.get().to_owned()).collect(),
        ))
    }
}

impl std::fmt::Display for OptionPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, key) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(".")?;
            }
            write!(f, "{}", toml_edit::Key::new(key.as_str()))?;
        }
        Ok(())
    }
}

impl From<OptionPath> for String {
    fn from(path: OptionPath) -> Self {
        path.to_string()
    }
}

impl OptionPath {
    fn boolean(&self, options: &toml::Table) -> Result<bool, String> {
        let (leaf, parents) = self
            .0
            .split_last()
            .expect("a validated nonempty option path");
        let mut table = options;
        for (index, key) in parents.iter().enumerate() {
            let prefix = || Self(self.0[..=index].to_vec());
            table = table
                .get(key)
                .ok_or_else(|| format!("option `{}` is missing while reading `{self}`", prefix()))?
                .as_table()
                .ok_or_else(|| format!("option `{}` must be a table to read `{self}`", prefix()))?;
        }
        table
            .get(leaf)
            .ok_or_else(|| format!("option `{self}` is missing"))?
            .as_bool()
            .ok_or_else(|| format!("option `{self}` must be a boolean"))
    }
}

/// Read options into the value types shared by Typst, Nu and external programs.
pub fn deserialize_options<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<toml::Table, D::Error> {
    use serde::Deserialize;

    let mut options = toml::Table::deserialize(deserializer)?;
    for (_, value) in options.iter_mut() {
        normalize(value).map_err(serde::de::Error::custom)?;
    }
    Ok(options)
}

fn normalize(value: &mut toml::Value) -> Result<(), &'static str> {
    match value {
        toml::Value::Datetime(datetime) => *value = toml::Value::String(datetime.to_string()),
        toml::Value::Float(number) if !number.is_finite() => {
            return Err("options require finite numbers");
        }
        toml::Value::Array(values) => values.iter_mut().try_for_each(normalize)?,
        toml::Value::Table(values) => {
            values
                .iter_mut()
                .try_for_each(|(_, value)| normalize(value))?;
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn merge(defaults: &mut toml::Table, overrides: &toml::Table) {
    for (key, value) in overrides {
        if let (Some(toml::Value::Table(default)), toml::Value::Table(value)) =
            (defaults.get_mut(key), value)
        {
            merge(default, value);
        } else {
            defaults.insert(key.clone(), value.clone());
        }
    }
}

/// Read a boolean at the selected path, unless the declaration is disabled.
pub fn enabled_by(
    enable: bool,
    path: Option<&OptionPath>,
    options: &toml::Table,
) -> Result<bool, String> {
    if !enable {
        return Ok(false);
    }
    let Some(path) = path else {
        return Ok(true);
    };
    path.boolean(options)
}

/// Bind normalized TOML options as a Typst dictionary.
pub fn to_typst(options: &toml::Table) -> Value {
    options
        .iter()
        .map(|(key, value)| (key.as_str().into(), typst_value(value)))
        .collect::<Dict>()
        .into_value()
}

fn typst_value(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(value) => value.as_str().into_value(),
        toml::Value::Integer(value) => (*value).into_value(),
        toml::Value::Float(value) => (*value).into_value(),
        toml::Value::Boolean(value) => (*value).into_value(),
        toml::Value::Datetime(value) => value.to_string().into_value(),
        toml::Value::Array(values) => values
            .iter()
            .map(typst_value)
            .collect::<Array>()
            .into_value(),
        toml::Value::Table(values) => to_typst(values),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_by_reads_nested_and_quoted_keys() {
        let options = toml::toml! {
            "images.enable" = false
            [images]
            enable = true
            quality = 80
        };
        for (key, expected) in [("images.enable", true), ("\"images.enable\"", false)] {
            let path = OptionPath::try_from(key.to_owned()).unwrap();
            assert_eq!(enabled_by(true, Some(&path), &options), Ok(expected));
            assert_eq!(OptionPath::try_from(path.to_string()).unwrap(), path);
        }
        for key in [
            "missing.enable",
            "images.missing",
            "images.quality.enable",
            "images.quality",
        ] {
            let path = OptionPath::try_from(key.to_owned()).unwrap();
            assert!(enabled_by(true, Some(&path), &options).is_err());
            assert_eq!(enabled_by(false, Some(&path), &options), Ok(false));
        }
        assert_eq!(enabled_by(true, None, &options), Ok(true));
    }

    #[test]
    fn enabled_by_rejects_malformed_paths_during_deserialization() {
        #[derive(serde::Deserialize, serde::Serialize)]
        struct Condition {
            #[serde(rename = "enabled-by")]
            path: OptionPath,
        }
        for key in ["", " ", ".images", "images.", "images..enable", "images[0]"] {
            let mut table = toml::Table::new();
            table.insert("enabled-by".into(), key.into());
            assert!(table.try_into::<Condition>().is_err(), "{key}");
        }
        let condition: Condition = toml::from_str("enabled-by = '\"image.tools\".enable'").unwrap();
        let encoded = toml::to_string(&condition).unwrap();
        let decoded: Condition = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.path, condition.path);
    }
}
