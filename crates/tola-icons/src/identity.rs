//! The icon identity grammar: the namespace and name every icon is mounted and named by.

use std::{borrow::Borrow, fmt, str::FromStr};

pub(crate) const MAX_ICON_NAME_BYTES: usize = 512;

/// An icon collection namespace chosen by the caller.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IconCollectionName(String);

/// An icon name within a collection, independent of its resource filename.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IconName(String);

/// A name cannot be represented by the icon identity grammar.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error(
    "an icon name must be 1–{} ASCII letters, digits, hyphens, or underscores, start with a letter or digit, and not end with a hyphen",
    MAX_ICON_NAME_BYTES
)]
pub struct InvalidIconName;

pub(crate) fn validate_name(value: &str) -> Result<(), InvalidIconName> {
    if value.is_empty()
        || value.len() > MAX_ICON_NAME_BYTES
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || value.ends_with('-')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(InvalidIconName);
    }
    Ok(())
}

macro_rules! icon_name {
    ($name:ident) => {
        impl $name {
            /// The name, which always satisfies the identity grammar.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl FromStr for $name {
            type Err = InvalidIconName;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                validate_name(value)?;
                Ok(Self(value.to_owned()))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
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

/// An unambiguous collection and icon identity, written as `collection:name`.
///
/// Consumers that map this pair into another spelling must detect collisions in that mapping.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IconId {
    collection: IconCollectionName,
    name: IconName,
}

impl IconId {
    /// Validate both parts of an icon identity.
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

    /// Return the icon's name within its collection.
    pub fn name(&self) -> &str {
        self.name.as_str()
    }
}

impl FromStr for IconId {
    type Err = InvalidIconName;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (collection, name) = value.split_once(':').ok_or(InvalidIconName)?;
        Self::new(collection, name)
    }
}

impl fmt::Display for IconId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.collection, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_separates_collection_from_name() {
        let first: IconId = "a-b:c".parse().unwrap();
        let second: IconId = "a:b-c".parse().unwrap();
        assert_ne!(first, second);
        assert_eq!(first.collection(), "a-b");
        assert_eq!(first.name(), "c");
        assert_eq!(first.to_string(), "a-b:c");
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
