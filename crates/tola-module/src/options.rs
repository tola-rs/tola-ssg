//! Static option data shared by modules and sites.

use typst::foundations::{Array, Dict, IntoValue, Value};

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

/// Read an optional literal boolean key from the declaration owner's options.
pub fn enabled_by(enable: bool, key: Option<&str>, options: &toml::Table) -> Result<bool, String> {
    if !enable {
        return Ok(false);
    }
    let Some(key) = key else {
        return Ok(true);
    };
    let value = options
        .get(key)
        .ok_or_else(|| format!("option `{key}` is missing"))?;
    value
        .as_bool()
        .ok_or_else(|| format!("option `{key}` must be a boolean"))
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
