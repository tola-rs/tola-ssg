//! Validation of values written in one configuration fragment.

use serde::Serialize;
use serde::de::{DeserializeOwned, IntoDeserializer};
use std::collections::{BTreeMap, HashMap};

/// One key or array index in a TOML value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ValueStep {
    Key(String),
    Index(usize),
}

/// A problem found in a configuration fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentIssue {
    pub path: Vec<ValueStep>,
    pub kind: FragmentIssueKind,
}

/// The reason a written value cannot be accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FragmentIssueKind {
    UnknownKey,
    WrongType {
        expected: &'static str,
        actual: &'static str,
    },
    InvalidValue {
        message: String,
    },
}

/// Tracks the current path and the owner of unknown root keys.
pub struct FragmentContext<'a> {
    issues: &'a mut Vec<FragmentIssue>,
    root_unknown: Option<&'a dyn Fn(&str) -> bool>,
    path: Vec<ValueStep>,
}

impl<'a> FragmentContext<'a> {
    pub fn new(
        issues: &'a mut Vec<FragmentIssue>,
        root_unknown: Option<&'a dyn Fn(&str) -> bool>,
    ) -> Self {
        Self {
            issues,
            root_unknown,
            path: Vec::new(),
        }
    }

    pub fn key(&mut self, key: &str, f: impl FnOnce(&mut Self)) {
        self.path.push(ValueStep::Key(key.to_owned()));
        f(self);
        self.path.pop();
    }

    pub fn index(&mut self, index: usize, f: impl FnOnce(&mut Self)) {
        self.path.push(ValueStep::Index(index));
        f(self);
        self.path.pop();
    }

    pub fn issue(&mut self, kind: FragmentIssueKind) {
        if matches!(kind, FragmentIssueKind::UnknownKey)
            && self.path.len() == 1
            && let (Some(filter), Some(ValueStep::Key(key))) =
                (self.root_unknown, self.path.first())
            && !filter(key)
        {
            return;
        }
        self.issues.push(FragmentIssue {
            path: self.path.clone(),
            kind,
        });
    }
}

/// Validate only fields present in one input value.
pub trait ValidateFragment {
    fn validate_fragment(value: &toml::Value, context: &mut FragmentContext<'_>);
}

/// Decode a complete atomic value, discarding the decoded result.
pub fn validate_value<T: DeserializeOwned>(value: &toml::Value, context: &mut FragmentContext<'_>) {
    // toml::Value's Serde input can present a date as a string. Retain the
    // original TOML type, without creating synthetic source text or positions.
    let decoded = value
        .serialize(toml_edit::ser::ValueSerializer::new())
        .map_err(|error| error.to_string())
        .and_then(|value| {
            T::deserialize(value.into_deserializer()).map_err(|error| error.to_string())
        });
    if let Err(error) = decoded {
        context.issue(FragmentIssueKind::InvalidValue { message: error });
    }
}

/// Report a value whose TOML type does not match the expected shape.
pub fn wrong_type(value: &toml::Value, expected: &'static str, context: &mut FragmentContext<'_>) {
    let actual = match value {
        toml::Value::String(_) => "string",
        toml::Value::Integer(_) => "integer",
        toml::Value::Float(_) => "float",
        toml::Value::Boolean(_) => "boolean",
        toml::Value::Datetime(_) => "datetime",
        toml::Value::Array(_) => "array",
        toml::Value::Table(_) => "table",
    };
    context.issue(FragmentIssueKind::WrongType { expected, actual });
}

macro_rules! atomic {
    ($expected:literal, $pattern:pat => $($ty:ty),* $(,)?) => {$ (
        impl ValidateFragment for $ty {
            fn validate_fragment(value: &toml::Value, context: &mut FragmentContext<'_>) {
                if matches!(value, $pattern) {
                    validate_value::<Self>(value, context);
                } else {
                    wrong_type(value, $expected, context);
                }
            }
        }
    )*};
}
atomic!("boolean", toml::Value::Boolean(_) => bool);
atomic!("string", toml::Value::String(_) => String, char, std::path::PathBuf, std::net::IpAddr);
atomic!("integer", toml::Value::Integer(_) => u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize);
atomic!("number", toml::Value::Float(_) | toml::Value::Integer(_) => f32, f64);

impl ValidateFragment for toml::Value {
    fn validate_fragment(_value: &toml::Value, _context: &mut FragmentContext<'_>) {}
}

impl ValidateFragment for toml::Table {
    fn validate_fragment(value: &toml::Value, context: &mut FragmentContext<'_>) {
        if !value.is_table() {
            wrong_type(value, "table", context);
        }
    }
}

impl<T: ValidateFragment> ValidateFragment for Option<T> {
    fn validate_fragment(value: &toml::Value, context: &mut FragmentContext<'_>) {
        T::validate_fragment(value, context);
    }
}

impl<T: ValidateFragment> ValidateFragment for Vec<T> {
    fn validate_fragment(value: &toml::Value, context: &mut FragmentContext<'_>) {
        if let toml::Value::Array(values) = value {
            for (index, value) in values.iter().enumerate() {
                context.index(index, |context| T::validate_fragment(value, context));
            }
        } else {
            wrong_type(value, "array", context);
        }
    }
}

macro_rules! map {
    ($map:ident $(, $extra:ident)?) => {
        impl<T: ValidateFragment $(, $extra: std::hash::BuildHasher + Default)?> ValidateFragment for $map<String, T $(, $extra)?> {
            fn validate_fragment(value: &toml::Value, context: &mut FragmentContext<'_>) {
                if let toml::Value::Table(values) = value {
                    for (key, value) in values {
                        context.key(key, |context| T::validate_fragment(value, context));
                    }
                } else {
                    wrong_type(value, "table", context);
                }
            }
        }
    };
}
map!(BTreeMap);
map!(HashMap, S);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Fragment;

    #[test]
    fn atomic_validation_does_not_coerce_dates_to_strings() {
        let value: toml::Table = toml::from_str("date = 2024-01-01").unwrap();
        let mut issues = Vec::new();
        validate_value::<String>(&value["date"], &mut FragmentContext::new(&mut issues, None));
        assert!(!issues.is_empty());
    }

    // These fixtures describe fields for static validation; they are never constructed.
    #[allow(dead_code)]
    #[derive(Fragment)]
    struct Child {
        required: String,
        flag: bool,
    }

    #[allow(dead_code)]
    #[derive(Fragment)]
    struct Root {
        children: Vec<Child>,
        values: BTreeMap<String, u8>,
        #[serde(skip)]
        internal: bool,
    }

    #[test]
    fn validates_only_written_fields_with_structured_paths() {
        let value: toml::Value = toml::from_str(
            "children = [{ flag = 1 }, { unknown = true }]\n[values]\n'quoted.key' = 300\n",
        )
        .unwrap();
        let mut issues = Vec::new();
        Root::validate_fragment(&value, &mut FragmentContext::new(&mut issues, None));
        assert_eq!(issues.len(), 3);
        assert!(issues.iter().any(|issue| issue.path
            == vec![
                ValueStep::Key("children".into()),
                ValueStep::Index(0),
                ValueStep::Key("flag".into())
            ]
            && matches!(issue.kind, FragmentIssueKind::WrongType { .. })));
        assert!(issues.iter().any(|issue| issue.path
            == vec![
                ValueStep::Key("children".into()),
                ValueStep::Index(1),
                ValueStep::Key("unknown".into())
            ]
            && matches!(issue.kind, FragmentIssueKind::UnknownKey)));
        assert!(issues.iter().any(|issue| issue.path
            == vec![
                ValueStep::Key("values".into()),
                ValueStep::Key("quoted.key".into())
            ]
            && matches!(issue.kind, FragmentIssueKind::InvalidValue { .. })));
    }

    #[test]
    fn filters_only_unknown_root_keys() {
        let value: toml::Value = toml::from_str("host = true\ninternal = true\n").unwrap();
        let mut issues = Vec::new();
        Root::validate_fragment(
            &value,
            &mut FragmentContext::new(&mut issues, Some(&|key| key != "host")),
        );
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, vec![ValueStep::Key("internal".into())]);
    }
}
