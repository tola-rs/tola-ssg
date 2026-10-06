//! One collection's validated icon values and its Iconify import.

use std::collections::BTreeMap;

use crate::{identity::validate_name, InvalidIconName, InvalidSvg, SvgIcon};

mod iconify;

const MAX_ICONS: usize = 98_304;
const MAX_LICENSE_FIELD_BYTES: usize = 4_096;

/// License metadata in an Iconify collection's `info.license`.
///
/// Each field is retained as written by the source, up to the per-field byte limit.
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
    /// Human-readable license name.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
    /// SPDX expression.
    pub fn spdx(&self) -> Option<&str> {
        self.spdx.as_deref()
    }
    /// License document URL, retained but not fetched.
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }
}

fn deserialize_license_field<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = <Option<String> as serde::Deserialize>::deserialize(deserializer)?;
    if value
        .as_ref()
        .is_some_and(|value| value.len() > MAX_LICENSE_FIELD_BYTES)
    {
        return Err(serde::de::Error::custom(format_args!(
            "license field exceeds the {MAX_LICENSE_FIELD_BYTES}-byte limit"
        )));
    }
    Ok(value)
}

/// Invalid collection data or an exceeded import limit.
#[derive(Debug, thiserror::Error)]
pub enum InvalidCollection {
    /// The input exceeds the collection byte budget.
    #[error("this file is larger than the {limit}-byte icon collection limit")]
    TooLarge {
        /// Maximum accepted input size.
        limit: usize,
    },
    /// Unique SVG variants exceed the aggregate parsing-work and frozen-storage budget.
    #[error("the collection expands past the {limit}-byte limit")]
    ExpandedTooLarge {
        /// Maximum cumulative raw body bytes parsed plus normalized SVG bytes retained.
        limit: usize,
    },
    /// Invalid JSON or a violation of the bounded collection schema.
    #[error("the icon collection is not valid JSON at line {line}, column {column}")]
    Json {
        /// One-based JSON line.
        line: usize,
        /// One-based JSON column.
        column: usize,
    },
    /// An icon or alias has an invalid identity.
    #[error("an icon name in this collection is not valid: {0}")]
    Name(#[from] InvalidIconName),
    /// An icon name is already in use.
    #[error("two icons in this collection are both named `{name}`")]
    DuplicateIcon {
        /// The colliding name.
        name: String,
    },
    /// The number of retained icons would exceed the collection budget.
    #[error("the collection contains more than {limit} icons")]
    TooManyIcons {
        /// Maximum retained icon count.
        limit: usize,
    },
    /// Non-finite, non-positive or otherwise unrepresentable geometry.
    #[error("icon coordinates and sizes must be finite, and sizes and aspect ratios positive")]
    Geometry,
    /// Alias inheritance is cyclic.
    #[error("icon alias `{name}` resolves in a cycle")]
    AliasCycle {
        /// The alias at which a cycle was found.
        name: String,
    },
    /// An alias has no base icon.
    #[error("icon alias `{name}` names no icon in this collection")]
    AliasTarget {
        /// The unresolved alias.
        name: String,
    },
    /// A character mapping key is not a hyphen-separated hexadecimal Unicode sequence.
    #[error("a `chars` key must be hexadecimal Unicode scalar values separated by hyphens")]
    CharacterKey,
    /// A character mapping does not name an icon or alias.
    #[error("a `chars` value names no icon or alias in this collection")]
    CharacterTarget,
    /// An individual icon violates the self-contained static SVG contract.
    #[error("icon `{name}`: {source}")]
    Svg {
        /// The icon whose SVG is invalid.
        name: String,
        /// Structured SVG diagnostic.
        #[source]
        source: InvalidSvg,
    },
}

impl From<serde_json::Error> for InvalidCollection {
    fn from(error: serde_json::Error) -> Self {
        Self::Json {
            line: error.line(),
            column: error.column(),
        }
    }
}

/// Validated icons with resolved aliases and optional Iconify source metadata.
///
/// Icons are immutable and cheaply cloned.
#[derive(Clone, Debug, Default)]
pub struct IconCollection {
    source_prefix: Option<String>,
    source_license: Option<IconLicense>,
    icons: BTreeMap<String, SvgIcon>,
    characters: BTreeMap<String, String>,
}

impl IconCollection {
    /// Create an empty collection for caller-supplied SVG icons.
    pub fn new() -> Self {
        Self::default()
    }

    /// Import IconifyJSON bytes, validating every icon and alias.
    pub fn from_iconify(bytes: impl AsRef<[u8]>) -> Result<Self, InvalidCollection> {
        iconify::parse(bytes.as_ref())
    }

    /// Parse and insert an SVG without requiring its bytes to have a static lifetime.
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

    /// Insert an already validated icon. A failed insertion leaves the collection unchanged.
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

    /// Resolve an icon, alias or Iconify character mapping.
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
            .map(|(character, target)| (character.as_str(), target.as_str()))
    }

    /// The source JSON prefix, independent of the collection namespace.
    pub fn source_prefix(&self) -> Option<&str> {
        self.source_prefix.as_deref()
    }

    /// License metadata from the source IconifyJSON.
    pub fn license(&self) -> Option<&IconLicense> {
        self.source_license.as_ref()
    }

    /// Number of icons and aliases, excluding additional character lookup names.
    pub fn len(&self) -> usize {
        self.icons.len()
    }

    /// Whether the collection has no icons.
    pub fn is_empty(&self) -> bool {
        self.icons.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_insertion_keeps_first_icon() {
        let mut collection = IconCollection::new();
        collection
            .insert_svg("mark", br#"<svg viewBox="0 0 12 24"/>"#)
            .unwrap();
        let rejection = collection.insert_svg("mark", br#"<svg viewBox="0 0 24 12"/>"#);
        assert!(matches!(
            rejection,
            Err(InvalidCollection::DuplicateIcon { .. })
        ));
        assert_eq!(collection.get("mark").unwrap().aspect_ratio(), 0.5);
    }
}
