//! Structured errors from reading fields of site module and asset declarations.

use std::fmt::{self, Write as _};
use std::marker::PhantomData;

use serde::de::{
    DeserializeOwned, Error, IntoDeserializer, MapAccess, Visitor, value::MapAccessDeserializer,
};
use serde::{Deserialize, Deserializer, Serialize};

/// One component of an authored value's path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FragmentStep {
    Key(String),
    Index(usize),
}

/// A field present in a partial declaration that could not be read.
#[derive(Debug)]
pub struct FragmentError {
    pub path: Vec<FragmentStep>,
    pub kind: FragmentErrorKind,
}

/// Why one field could not be read.
#[derive(Debug)]
pub enum FragmentErrorKind {
    UnknownKey,
    InvalidValue(String),
}

impl FragmentError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self {
            path: Vec::new(),
            kind: FragmentErrorKind::InvalidValue(message.into()),
        }
    }

    pub(crate) fn at(mut self, step: FragmentStep) -> Self {
        self.path.insert(0, step);
        self
    }
}

impl fmt::Display for FragmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let FragmentErrorKind::InvalidValue(message) = &self.kind {
            return formatter.write_str(message);
        }
        formatter.write_str("unknown field `")?;
        for (index, step) in self.path.iter().enumerate() {
            match step {
                FragmentStep::Key(name) => {
                    if index != 0 {
                        formatter.write_char('.')?;
                    }
                    write!(formatter, "{}", toml_edit::Key::new(name.as_str()))?;
                }
                FragmentStep::Index(index) => write!(formatter, "[{index}]")?,
            }
        }
        formatter.write_char('`')
    }
}

enum ReadError<E> {
    UnknownKey(Vec<FragmentStep>),
    InvalidValue(serde_path_to_error::Error<E>),
}

struct TableFields<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for TableFields<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TableVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for TableVisitor<T> {
            type Value = TableFields<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a table")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(TableFields)
            }
        }

        deserializer.deserialize_map(TableVisitor(PhantomData))
    }
}

pub(crate) fn optional_table<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<TableFields<T>>::deserialize(deserializer).map(|fields| fields.map(|fields| fields.0))
}

fn read<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<T, ReadError<D::Error>> {
    let mut unknown = None;
    let read_fields = serde_path_to_error::deserialize::<_, TableFields<T>>(
        serde_ignored::Deserializer::new(deserializer, &mut |path| {
            unknown.get_or_insert_with(|| {
                let mut steps = Vec::new();
                ignored_steps(&path, &mut steps);
                steps
            });
        }),
    );
    if let Some(path) = unknown {
        return Err(ReadError::UnknownKey(path));
    }
    read_fields
        .map(|fields| fields.0)
        .map_err(ReadError::InvalidValue)
}

pub(crate) fn read_fields<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<T, D::Error> {
    match read(deserializer) {
        Ok(fields) => Ok(fields),
        Err(ReadError::UnknownKey(path)) => Err(D::Error::custom(FragmentError {
            path,
            kind: FragmentErrorKind::UnknownKey,
        })),
        Err(ReadError::InvalidValue(error)) => Err(error.into_inner()),
    }
}

pub(crate) fn read_fragment<T: DeserializeOwned>(value: &toml::Value) -> Result<T, FragmentError> {
    if !value.is_table() {
        return Err(FragmentError::invalid(format!(
            "expected a table, found {}",
            value.type_str()
        )));
    }
    // A toml::Value used as a Serde deserializer presents dates as strings.
    let fields = value
        .serialize(toml_edit::ser::ValueSerializer::new())
        .map_err(|error| FragmentError::invalid(error.to_string()))?;
    read(fields.into_deserializer()).map_err(|error| match error {
        ReadError::UnknownKey(path) => FragmentError {
            path,
            kind: FragmentErrorKind::UnknownKey,
        },
        ReadError::InvalidValue(error) => FragmentError {
            path: error
                .path()
                .iter()
                .filter_map(|step| match step {
                    serde_path_to_error::Segment::Map { key } => {
                        Some(FragmentStep::Key(key.clone()))
                    }
                    serde_path_to_error::Segment::Seq { index } => {
                        Some(FragmentStep::Index(*index))
                    }
                    serde_path_to_error::Segment::Enum { variant } => {
                        Some(FragmentStep::Key(variant.clone()))
                    }
                    serde_path_to_error::Segment::Unknown => None,
                })
                .collect(),
            kind: FragmentErrorKind::InvalidValue(error.inner().message().to_owned()),
        },
    })
}

fn ignored_steps(path: &serde_ignored::Path<'_>, steps: &mut Vec<FragmentStep>) {
    use serde_ignored::Path;

    match path {
        Path::Root => {}
        Path::Map { parent, key } => {
            ignored_steps(parent, steps);
            steps.push(FragmentStep::Key(key.clone()));
        }
        Path::Seq { parent, index } => {
            ignored_steps(parent, steps);
            steps.push(FragmentStep::Index(*index));
        }
        Path::Some { parent }
        | Path::NewtypeStruct { parent }
        | Path::NewtypeVariant { parent } => {
            ignored_steps(parent, steps);
        }
    }
}
