//! Validated icon collections and Iconify JSON import.

use std::collections::BTreeMap;

use crate::{identity::validate_name, InvalidIconName, InvalidSvg, SvgIcon};

mod iconify;

const MAX_ICONS: usize = 98_304;
const MAX_LICENSE_FIELD_BYTES: usize = 4_096;

/// License metadata from an Iconify collection's `info.license`.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize)]
pub struct IconLicense {
    #[serde(default, deserialize_with = "deserialize_license_field")]
    title: Option<String>,
    #[serde(default, deserialize_with = "deserialize_license_field")]
    spdx: Option<String>,
    #[serde(default, deserialize_with = "deserialize_license_field")]
    url: Option<String>,
}

impl IconLicense {
    /// Return the human-readable license title.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Return the SPDX license identifier.
    pub fn spdx(&self) -> Option<&str> {
        self.spdx.as_deref()
    }

    /// Return the license document URL.
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }
}

fn deserialize_license_field<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = <Option<String> as serde::Deserialize>::deserialize(deserializer)?;
    if val
        .as_ref()
        .is_some_and(|s| s.len() > MAX_LICENSE_FIELD_BYTES)
    {
        return Err(serde::de::Error::custom(format_args!(
            "license field exceeds the {MAX_LICENSE_FIELD_BYTES}-byte limit"
        )));
    }
    Ok(val)
}

/// Error returned when importing or mutating an [`IconCollection`].
#[derive(Debug, thiserror::Error)]
pub enum InvalidCollection {
    /// Input exceeds the collection byte budget.
    #[error("this file is larger than the {limit}-byte icon collection limit")]
    TooLarge {
        /// Maximum accepted input size in bytes.
        limit: usize,
    },
    /// Expanded SVG variants exceed the cumulative parsing and storage budget.
    #[error("the collection expands past the {limit}-byte limit")]
    ExpandedTooLarge {
        /// Maximum cumulative raw body plus normalized SVG bytes.
        limit: usize,
    },
    /// Input is invalid JSON or violates the bounded collection schema.
    #[error("the icon collection is not valid JSON at line {line}, column {column}")]
    Json {
        /// One-based JSON line number.
        line: usize,
        /// One-based JSON column number.
        column: usize,
    },
    /// An icon or alias name violates the identity grammar.
    #[error("an icon name in this collection is not valid: {0}")]
    Name(#[from] InvalidIconName),
    /// An icon name or alias collides with an existing entry.
    #[error("two icons in this collection are both named `{name}`")]
    DuplicateIcon {
        /// Colliding icon name.
        name: String,
    },
    /// Retained icon count exceeds the collection limit.
    #[error("the collection contains more than {limit} icons")]
    TooManyIcons {
        /// Maximum retained icon count.
        limit: usize,
    },
    /// Coordinates or dimensions are non-finite or non-positive.
    #[error("icon coordinates and sizes must be finite, and sizes and aspect ratios positive")]
    Geometry,
    /// Alias inheritance forms a cycle.
    #[error("icon alias `{name}` resolves in a cycle")]
    AliasCycle {
        /// Alias name where the cycle was detected.
        name: String,
    },
    /// Alias references a missing parent icon.
    #[error("icon alias `{name}` names no icon in this collection")]
    AliasTarget {
        /// Unresolved alias name.
        name: String,
    },
    /// Character mapping key is not hyphen-separated hexadecimal Unicode scalar values.
    #[error("a `chars` key must be hexadecimal Unicode scalar values separated by hyphens")]
    CharacterKey,
    /// Character mapping target names no icon or alias in the collection.
    #[error("a `chars` value names no icon or alias in this collection")]
    CharacterTarget,
    /// Individual icon SVG violates the static self-contained SVG contract.
    #[error("icon `{name}`: {source}")]
    Svg {
        /// Name of the icon whose SVG failed validation.
        name: String,
        /// Underlying SVG validation error.
        #[source]
        source: InvalidSvg,
    },
}

impl From<serde_json::Error> for InvalidCollection {
    fn from(err: serde_json::Error) -> Self {
        Self::Json {
            line: err.line(),
            column: err.column(),
        }
    }
}

/// Validated SVG icons with resolved aliases and optional Iconify source metadata.
#[derive(Clone, Debug, Default)]
pub struct IconCollection {
    source_prefix: Option<String>,
    source_license: Option<IconLicense>,
    icons: BTreeMap<String, SvgIcon>,
    characters: BTreeMap<String, String>,
}

impl IconCollection {
    /// Create an empty collection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Import IconifyJSON bytes, validating every icon and alias.
    pub fn from_iconify(bytes: impl AsRef<[u8]>) -> Result<Self, InvalidCollection> {
        iconify::parse(bytes.as_ref())
    }

    /// Parse and insert an SVG under `name`.
    pub fn insert_svg(
        &mut self,
        name: impl AsRef<str>,
        bytes: impl AsRef<[u8]>,
    ) -> Result<(), InvalidCollection> {
        let name = name.as_ref();
        self.validate_insertion(name)?;
        let icon = SvgIcon::parse(bytes).map_err(|source| InvalidCollection::Svg {
            name: name.to_owned(),
            source,
        })?;
        self.icons.insert(name.to_owned(), icon);
        Ok(())
    }

    /// Insert an already validated icon under `name`, leaving the collection unchanged on error.
    pub fn insert(
        &mut self,
        name: impl AsRef<str>,
        icon: SvgIcon,
    ) -> Result<(), InvalidCollection> {
        let name = name.as_ref();
        self.validate_insertion(name)?;
        self.icons.insert(name.to_owned(), icon);
        Ok(())
    }

    fn validate_insertion(&self, name: &str) -> Result<(), InvalidCollection> {
        validate_name(name)?;
        if self.icons.contains_key(name) || self.characters.contains_key(name) {
            return Err(InvalidCollection::DuplicateIcon {
                name: name.to_owned(),
            });
        }
        if self.icons.len() >= MAX_ICONS {
            return Err(InvalidCollection::TooManyIcons { limit: MAX_ICONS });
        }
        Ok(())
    }

    /// Resolve an icon, alias, or Iconify character mapping.
    pub fn get(&self, name: &str) -> Option<&SvgIcon> {
        self.icons.get(name).or_else(|| {
            self.characters
                .get(name)
                .and_then(|target| self.icons.get(target))
        })
    }

    /// Iterate over icon and alias names in lexical order.
    pub fn names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.icons.keys().map(String::as_str)
    }

    /// Iterate over character mappings in lexical order.
    pub fn characters(&self) -> impl ExactSizeIterator<Item = (&str, &str)> {
        self.characters
            .iter()
            .map(|(ch, target)| (ch.as_str(), target.as_str()))
    }

    /// Return the source JSON prefix, independent of the mounted namespace.
    pub fn source_prefix(&self) -> Option<&str> {
        self.source_prefix.as_deref()
    }

    /// Return license metadata from the source IconifyJSON.
    pub fn license(&self) -> Option<&IconLicense> {
        self.source_license.as_ref()
    }

    /// Return the number of icons and aliases, excluding extra character mappings.
    pub fn len(&self) -> usize {
        self.icons.len()
    }

    /// Return `true` when the collection contains no icons.
    pub fn is_empty(&self) -> bool {
        self.icons.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_insert_keeps_first() {
        let mut col = IconCollection::new();
        col.insert_svg("mark", br#"<svg viewBox="0 0 12 24"/>"#)
            .unwrap();
        let err = col.insert_svg("mark", br#"<svg viewBox="0 0 24 12"/>"#);
        assert!(matches!(err, Err(InvalidCollection::DuplicateIcon { .. })));
        assert_eq!(col.get("mark").unwrap().aspect_ratio(), 0.5);
    }
}
