//! Icon identity grammar for collection namespaces and icon names.

use std::{borrow::Borrow, fmt, str::FromStr};

pub(crate) const MAX_ICON_NAME_BYTES: usize = 512;

/// Caller-chosen icon collection namespace.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IconCollectionName(String);

/// Icon name within a collection, independent of its source filename.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IconName(String);

/// Error returned when an icon or collection name violates the identity grammar.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error(
    "an icon name must be 1–{} ASCII letters, digits, hyphens, or underscores, start with a letter or digit, and not end with a hyphen",
    MAX_ICON_NAME_BYTES
)]
pub struct InvalidIconName;

pub(crate) fn validate_name(val: &str) -> Result<(), InvalidIconName> {
    if val.is_empty()
        || val.len() > MAX_ICON_NAME_BYTES
        || !val.as_bytes()[0].is_ascii_alphanumeric()
        || val.ends_with('-')
        || !val
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(InvalidIconName);
    }
    Ok(())
}

macro_rules! icon_name {
    ($name:ident) => {
        impl $name {
            /// Return the validated name slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl FromStr for $name {
            type Err = InvalidIconName;

            fn from_str(val: &str) -> Result<Self, Self::Err> {
                validate_name(val)?;
                Ok(Self(val.to_owned()))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                self.as_str()
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }
    };
}

icon_name!(IconCollectionName);
icon_name!(IconName);

/// Site collection key or a collection belonging to a configured module instance.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IconCollectionKey(String);

impl IconCollectionKey {
    /// Bind a collection name to an optional module alias.
    pub fn new(module: Option<&str>, collection: &str) -> Result<Self, InvalidIconName> {
        validate_name(collection)?;
        match module {
            Some(module) => {
                validate_name(module)?;
                Ok(Self(format!("modules/{module}/{collection}")))
            }
            None => Ok(Self(collection.to_owned())),
        }
    }

    /// Return the canonical key string used in virtual reads, lock entries, and vendored paths.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return the owning module alias, or `None` for a site collection.
    pub fn module(&self) -> Option<&str> {
        self.0
            .strip_prefix("modules/")
            .and_then(|path| path.split_once('/'))
            .map(|(module, _)| module)
    }

    /// Return the collection name within its owner.
    pub fn name(&self) -> &str {
        self.0.rsplit('/').next().expect("a collection has a name")
    }
}

impl FromStr for IconCollectionKey {
    type Err = InvalidIconName;

    fn from_str(val: &str) -> Result<Self, Self::Err> {
        if let Some(path) = val.strip_prefix("modules/") {
            let (module, col) = path.split_once('/').ok_or(InvalidIconName)?;
            Self::new(Some(module), col)
        } else {
            Self::new(None, val)
        }
    }
}

impl fmt::Display for IconCollectionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Borrow<str> for IconCollectionKey {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// Unambiguous collection and icon identity formatted as `collection:name`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IconId {
    collection: IconCollectionKey,
    name: IconName,
}

impl IconId {
    /// Validate and construct an icon identity from collection and name parts.
    pub fn new(collection: &str, name: &str) -> Result<Self, InvalidIconName> {
        Ok(Self {
            collection: collection.parse()?,
            name: name.parse()?,
        })
    }

    /// Return the collection namespace.
    pub fn collection(&self) -> &str {
        self.collection.as_str()
    }

    /// Return the icon name within its collection.
    pub fn name(&self) -> &str {
        self.name.as_str()
    }
}

impl FromStr for IconId {
    type Err = InvalidIconName;

    fn from_str(val: &str) -> Result<Self, Self::Err> {
        let (col, name) = val.split_once(':').ok_or(InvalidIconName)?;
        Self::new(col, name)
    }
}

impl fmt::Display for IconId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.collection, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_splits_collection_and_name() {
        let a: IconId = "a-b:c".parse().unwrap();
        let b: IconId = "a:b-c".parse().unwrap();
        assert_ne!(a, b);
        assert_eq!(a.collection(), "a-b");
        assert_eq!(a.name(), "c");
        assert_eq!(a.to_string(), "a-b:c");
    }

    #[test]
    fn grammar_rejects_invalid_names() {
        for name in ["", "../mark", "mark.svg", "mark:small", "a b", "a?b"] {
            assert!(name.parse::<IconName>().is_err(), "{name}");
            assert!(name.parse::<IconCollectionName>().is_err(), "{name}");
        }
        assert!("Company_2026".parse::<IconCollectionName>().is_ok());
        assert!("a".repeat(512).parse::<IconName>().is_ok());
        assert!("a".repeat(513).parse::<IconName>().is_err());
    }
}
