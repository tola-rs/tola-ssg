//! Static option data shared by modules and sites.

use typst::foundations::{Array, Dict, IntoValue, Value};

use crate::fragment::{FragmentError, FragmentStep};

/// A nonempty TOML key path within the declaration owner's options.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct OptionPath(Vec<String>);

impl TryFrom<String> for OptionPath {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let keys = toml_edit::Key::parse(&value)
            .map_err(|err| format!("invalid option path `{value}`: {}", err.message()))?;
        if keys.is_empty() {
            return Err("an option path must contain a key".into());
        }
        Ok(Self(
            keys.into_iter().map(|key| key.get().to_owned()).collect(),
        ))
    }
}

impl std::fmt::Display for OptionPath {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (idx, key) in self.0.iter().enumerate() {
            if idx > 0 {
                fmt.write_str(".")?;
            }
            write!(fmt, "{}", toml_edit::Key::new(key.as_str()))?;
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
    pub(crate) fn read<'a>(&self, opts: &'a toml::Table) -> Result<&'a toml::Value, String> {
        let (leaf, parents) = self
            .0
            .split_last()
            .expect("a validated nonempty option path");
        let mut tbl = opts;
        for (idx, key) in parents.iter().enumerate() {
            let prefix = || Self(self.0[..=idx].to_vec());
            tbl = tbl
                .get(key)
                .ok_or_else(|| format!("option `{}` is missing while reading `{self}`", prefix()))?
                .as_table()
                .ok_or_else(|| format!("option `{}` must be a table to read `{self}`", prefix()))?;
        }
        tbl.get(leaf)
            .ok_or_else(|| format!("option `{self}` is missing"))
    }
}

/// Read options into the value types shared by Typst, Nu and external programs.
pub fn deserialize_options<'de, D: serde::Deserializer<'de>>(
    deser: D,
) -> Result<toml::Table, D::Error> {
    use serde::Deserialize;

    let mut opts = toml::Table::deserialize(deser)?;
    normalize_options(&mut opts).map_err(serde::de::Error::custom)?;
    Ok(opts)
}

/// Check the option protocol while preserving the authored values for merging.
pub fn validate_fragment(value: &toml::Value) -> Result<(), FragmentError> {
    let Some(opts) = value.as_table() else {
        return Err(FragmentError::invalid("options must be a table"));
    };
    normalize_options(&mut opts.clone())
}

pub(crate) fn normalize_options(opts: &mut toml::Table) -> Result<(), FragmentError> {
    for (key, val) in opts.iter_mut() {
        normalize(val).map_err(|err| err.at(FragmentStep::Key(key.clone())))?;
    }
    Ok(())
}

fn normalize(val: &mut toml::Value) -> Result<(), FragmentError> {
    match val {
        toml::Value::Datetime(dt) => *val = toml::Value::String(dt.to_string()),
        toml::Value::Float(num) if !num.is_finite() => {
            return Err(FragmentError::invalid("options require finite numbers"));
        }
        toml::Value::Array(vals) => {
            for (idx, item) in vals.iter_mut().enumerate() {
                normalize(item).map_err(|err| err.at(FragmentStep::Index(idx)))?;
            }
        }
        toml::Value::Table(tbl) => normalize_options(tbl)?,
        _ => {}
    }
    Ok(())
}

pub(crate) fn merge(dst: &mut toml::Table, src: &toml::Table) {
    for (key, val) in src {
        if let (Some(toml::Value::Table(dst_tbl)), toml::Value::Table(src_tbl)) =
            (dst.get_mut(key), val)
        {
            merge(dst_tbl, src_tbl);
        } else {
            dst.insert(key.clone(), val.clone());
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
    path.read(options)?
        .as_bool()
        .ok_or_else(|| format!("option `{path}` must be a boolean"))
}

/// Bind normalized TOML options as a Typst dictionary.
pub fn to_typst(options: &toml::Table) -> Value {
    options
        .iter()
        .map(|(key, val)| (key.as_str().into(), typst_value(val)))
        .collect::<Dict>()
        .into_value()
}

fn typst_value(val: &toml::Value) -> Value {
    match val {
        toml::Value::String(s) => s.as_str().into_value(),
        toml::Value::Integer(n) => (*n).into_value(),
        toml::Value::Float(n) => (*n).into_value(),
        toml::Value::Boolean(b) => (*b).into_value(),
        toml::Value::Datetime(dt) => dt.to_string().into_value(),
        toml::Value::Array(vals) => vals.iter().map(typst_value).collect::<Array>().into_value(),
        toml::Value::Table(tbl) => to_typst(tbl),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_by_nested_and_quoted() {
        let opts = toml::toml! {
            "images.enable" = false
            [images]
            enable = true
            quality = 80
        };
        for (key, expected) in [("images.enable", true), ("\"images.enable\"", false)] {
            let path = OptionPath::try_from(key.to_owned()).unwrap();
            assert_eq!(enabled_by(true, Some(&path), &opts), Ok(expected));
            assert_eq!(OptionPath::try_from(path.to_string()).unwrap(), path);
        }
        for key in [
            "missing.enable",
            "images.missing",
            "images.quality.enable",
            "images.quality",
        ] {
            let path = OptionPath::try_from(key.to_owned()).unwrap();
            assert!(enabled_by(true, Some(&path), &opts).is_err());
            assert_eq!(enabled_by(false, Some(&path), &opts), Ok(false));
        }
        assert_eq!(enabled_by(true, None, &opts), Ok(true));
    }

    #[test]
    fn option_path_rejects_malformed() {
        #[derive(serde::Deserialize, serde::Serialize)]
        struct Condition {
            #[serde(rename = "enabled-by")]
            path: OptionPath,
        }
        for key in ["", " ", ".images", "images.", "images..enable", "images[0]"] {
            let mut tbl = toml::Table::new();
            tbl.insert("enabled-by".into(), key.into());
            assert!(tbl.try_into::<Condition>().is_err(), "{key}");
        }
        let cond: Condition = toml::from_str("enabled-by = '\"image.tools\".enable'").unwrap();
        let encoded = toml::to_string(&cond).unwrap();
        let decoded: Condition = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.path, cond.path);
    }
}
