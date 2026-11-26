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
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let FragmentErrorKind::InvalidValue(msg) = &self.kind {
            return fmt.write_str(msg);
        }
        fmt.write_str("unknown field `")?;
        for (idx, step) in self.path.iter().enumerate() {
            match step {
                FragmentStep::Key(name) => {
                    if idx != 0 {
                        fmt.write_char('.')?;
                    }
                    write!(fmt, "{}", toml_edit::Key::new(name.as_str()))?;
                }
                FragmentStep::Index(idx) => write!(fmt, "[{idx}]")?,
            }
        }
        fmt.write_char('`')
    }
}

pub(crate) fn push_pointer_steps(pointer: &mut String, steps: &[FragmentStep]) {
    for step in steps {
        match step {
            FragmentStep::Key(key) => tola_diagnostic::push_pointer(pointer, key),
            FragmentStep::Index(idx) => tola_diagnostic::push_pointer(pointer, &idx.to_string()),
        }
    }
}

enum ReadError<E> {
    UnknownKey(Vec<FragmentStep>),
    InvalidValue(serde_path_to_error::Error<E>),
}

struct TableFields<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for TableFields<T> {
    fn deserialize<D: Deserializer<'de>>(deser: D) -> Result<Self, D::Error> {
        struct TableVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for TableVisitor<T> {
            type Value = TableFields<T>;

            fn expecting(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt.write_str("a table")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(TableFields)
            }
        }

        deser.deserialize_map(TableVisitor(PhantomData))
    }
}

pub(crate) fn optional_table<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deser: D,
) -> Result<Option<T>, D::Error> {
    Option::<TableFields<T>>::deserialize(deser).map(|fields| fields.map(|fields| fields.0))
}

fn read<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deser: D,
) -> Result<T, ReadError<D::Error>> {
    let mut unknown = None;
    let parsed = serde_path_to_error::deserialize::<_, TableFields<T>>(
        serde_ignored::Deserializer::new(deser, &mut |path| {
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
    parsed
        .map(|fields| fields.0)
        .map_err(ReadError::InvalidValue)
}

pub(crate) fn read_fields<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deser: D,
) -> Result<T, D::Error> {
    match read(deser) {
        Ok(fields) => Ok(fields),
        Err(ReadError::UnknownKey(path)) => Err(D::Error::custom(FragmentError {
            path,
            kind: FragmentErrorKind::UnknownKey,
        })),
        Err(ReadError::InvalidValue(err)) => Err(err.into_inner()),
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
        .map_err(|err| FragmentError::invalid(err.to_string()))?;
    read(fields.into_deserializer()).map_err(|err| match err {
        ReadError::UnknownKey(path) => FragmentError {
            path,
            kind: FragmentErrorKind::UnknownKey,
        },
        ReadError::InvalidValue(err) => FragmentError {
            path: err
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
            kind: FragmentErrorKind::InvalidValue(err.inner().message().to_owned()),
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
