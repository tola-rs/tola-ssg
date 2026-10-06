//! The Iconify document's wire shape and the bounded reads that admit it.

use serde::{
    de::{DeserializeSeed, Error as _, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use std::{
    collections::{btree_map::Entry, BTreeMap},
    fmt,
    marker::PhantomData,
};

use crate::identity::MAX_ICON_NAME_BYTES;

// Limits allow headroom above @iconify/json 2.2.500's 235 collections:
// https://registry.npmjs.org/@iconify/json/-/json-2.2.500.tgz
// Among files under 64 MiB, maxima were 19,992 icons, 6,363 aliases, 7,447 character mappings,
// a 26-byte prefix, 99-byte names, an 898,083-byte body, and 95-byte references.
// The three maps total at most 131,072 nodes. Input bytes bound decoded string payload
// because JSON decoding cannot expand UTF-8; entry limits bound per-node overhead.
const MAX_ICON_ENTRIES: usize = 65_536;
const MAX_COLLECTION_BYTES: usize = 64 * 1024 * 1024;
const MAX_ALIAS_ENTRIES: usize = 32_768;
const MAX_CHAR_ENTRIES: usize = 32_768;
const MAX_PREFIX_BYTES: usize = 2_048;
const MAX_CHAR_KEY_BYTES: usize = 256;
const MAX_ICON_BODY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy)]
struct BoundedStringSeed {
    description: &'static str,
    max_bytes: usize,
}

impl BoundedStringSeed {
    const fn new(description: &'static str, max_bytes: usize) -> Self {
        Self {
            description,
            max_bytes,
        }
    }
    fn validate_length<E>(&self, value: &str) -> Result<(), E>
    where
        E: serde::de::Error,
    {
        if value.len() > self.max_bytes {
            return Err(E::custom(format_args!(
                "{} exceeds the {}-byte limit",
                self.description, self.max_bytes
            )));
        }
        Ok(())
    }
}

impl<'de> Visitor<'de> for BoundedStringSeed {
    type Value = String;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} no longer than {} bytes",
            self.description, self.max_bytes
        )
    }

    fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(value)
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.validate_length(value)?;
        Ok(value.to_owned())
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.validate_length(&value)?;
        Ok(value)
    }
}

impl<'de> DeserializeSeed<'de> for BoundedStringSeed {
    type Value = String;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_string(self)
    }
}

struct TypedValueSeed<V>(PhantomData<fn() -> V>);

impl<V> Clone for TypedValueSeed<V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V> Copy for TypedValueSeed<V> {}

impl<V> TypedValueSeed<V> {
    const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<'de, V> DeserializeSeed<'de> for TypedValueSeed<V>
where
    V: Deserialize<'de>,
{
    type Value = V;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        V::deserialize(deserializer)
    }
}

#[derive(Clone, Copy)]
struct BoundedMapSeed<V, S> {
    description: &'static str,
    max_entries: usize,
    key: BoundedStringSeed,
    value: S,
    marker: PhantomData<fn() -> V>,
}

impl<V, S> BoundedMapSeed<V, S> {
    const fn new(
        description: &'static str,
        max_entries: usize,
        key: BoundedStringSeed,
        value: S,
    ) -> Self {
        Self {
            description,
            max_entries,
            key,
            value,
            marker: PhantomData,
        }
    }
}

impl<'de, V, S> Visitor<'de> for BoundedMapSeed<V, S>
where
    S: Copy + DeserializeSeed<'de, Value = V>,
{
    type Value = BTreeMap<String, V>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "an {} map with at most {} entries",
            self.description, self.max_entries
        )
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut entries = BTreeMap::new();
        while let Some(key) = map.next_key_seed(self.key)? {
            // Check duplicates and entry limits before allocating the value's payload.
            let full = entries.len() >= self.max_entries;
            match entries.entry(key) {
                Entry::Occupied(_) => {
                    return Err(A::Error::custom(format_args!(
                        "{} map contains a duplicate key",
                        self.description
                    )));
                }
                Entry::Vacant(entry) => {
                    if full {
                        return Err(A::Error::custom(format_args!(
                            "{} map exceeds the {}-entry limit",
                            self.description, self.max_entries
                        )));
                    }
                    entry.insert(map.next_value_seed(self.value)?);
                }
            }
        }
        Ok(entries)
    }
}

impl<'de, V, S> DeserializeSeed<'de> for BoundedMapSeed<V, S>
where
    S: Copy + DeserializeSeed<'de, Value = V>,
{
    type Value = BTreeMap<String, V>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(self)
    }
}

fn deserialize_bounded_string<'de, D>(
    deserializer: D,
    description: &'static str,
    max_bytes: usize,
) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    BoundedStringSeed::new(description, max_bytes).deserialize(deserializer)
}

fn deserialize_prefix<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_string(deserializer, "collection prefix", MAX_PREFIX_BYTES)
}

fn deserialize_icon_body<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_string(deserializer, "icon body", MAX_ICON_BODY_BYTES)
}

fn deserialize_alias_parent<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_string(deserializer, "alias parent", MAX_ICON_NAME_BYTES)
}

fn default_dimension() -> f64 {
    16.0
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Eq, PartialEq)]
#[serde(try_from = "u8")]
pub(super) struct IconRotation(u8);

impl IconRotation {
    pub(super) const HALF_TURN: Self = Self(2);

    pub(super) const fn quarter_turns(self) -> u8 {
        self.0
    }

    pub(super) const fn combine(self, other: Self) -> Self {
        Self((self.0 + other.0) % 4)
    }
}

impl TryFrom<u8> for IconRotation {
    type Error = &'static str;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        (value <= 3)
            .then_some(Self(value))
            .ok_or("icon rotation must be an integer from 0 through 3")
    }
}

/// Per-icon geometry and orientation layered over the enclosing icon; an absent field inherits.
#[derive(Debug, Clone)]
pub(super) struct IconOverrides {
    pub(super) left: Option<f64>,
    pub(super) top: Option<f64>,
    pub(super) width: Option<f64>,
    pub(super) height: Option<f64>,
    pub(super) rotate: Option<IconRotation>,
    pub(super) h_flip: Option<bool>,
    pub(super) v_flip: Option<bool>,
}

#[derive(Debug, Clone)]
pub(super) struct IconifyIcon {
    pub(super) body: String,
    pub(super) overrides: IconOverrides,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IconWire {
    #[serde(deserialize_with = "deserialize_icon_body")]
    body: String,
    left: Option<f64>,
    top: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
    rotate: Option<IconRotation>,
    h_flip: Option<bool>,
    v_flip: Option<bool>,
}

impl<'de> Deserialize<'de> for IconifyIcon {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = IconWire::deserialize(deserializer)?;
        Ok(Self {
            body: wire.body,
            overrides: IconOverrides {
                left: wire.left,
                top: wire.top,
                width: wire.width,
                height: wire.height,
                rotate: wire.rotate,
                h_flip: wire.h_flip,
                v_flip: wire.v_flip,
            },
        })
    }
}

#[derive(Debug, Clone)]
pub(super) struct IconifyAlias {
    pub(super) parent: String,
    pub(super) overrides: IconOverrides,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AliasWire {
    #[serde(deserialize_with = "deserialize_alias_parent")]
    parent: String,
    left: Option<f64>,
    top: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
    rotate: Option<IconRotation>,
    h_flip: Option<bool>,
    v_flip: Option<bool>,
}

impl<'de> Deserialize<'de> for IconifyAlias {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = AliasWire::deserialize(deserializer)?;
        Ok(Self {
            parent: wire.parent,
            overrides: IconOverrides {
                left: wire.left,
                top: wire.top,
                width: wire.width,
                height: wire.height,
                rotate: wire.rotate,
                h_flip: wire.h_flip,
                v_flip: wire.v_flip,
            },
        })
    }
}

fn deserialize_icons<'de, D>(deserializer: D) -> Result<BTreeMap<String, IconifyIcon>, D::Error>
where
    D: Deserializer<'de>,
{
    BoundedMapSeed::new(
        "icons",
        MAX_ICON_ENTRIES,
        BoundedStringSeed::new("icon name", MAX_ICON_NAME_BYTES),
        TypedValueSeed::<IconifyIcon>::new(),
    )
    .deserialize(deserializer)
}

fn deserialize_aliases<'de, D>(deserializer: D) -> Result<BTreeMap<String, IconifyAlias>, D::Error>
where
    D: Deserializer<'de>,
{
    BoundedMapSeed::new(
        "aliases",
        MAX_ALIAS_ENTRIES,
        BoundedStringSeed::new("alias name", MAX_ICON_NAME_BYTES),
        TypedValueSeed::<IconifyAlias>::new(),
    )
    .deserialize(deserializer)
}

fn deserialize_chars<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    BoundedMapSeed::new(
        "chars",
        MAX_CHAR_ENTRIES,
        BoundedStringSeed::new("character key", MAX_CHAR_KEY_BYTES),
        BoundedStringSeed::new("character target", MAX_ICON_NAME_BYTES),
    )
    .deserialize(deserializer)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct IconifyCollection {
    #[serde(deserialize_with = "deserialize_prefix")]
    pub(super) prefix: String,

    #[serde(deserialize_with = "deserialize_icons")]
    pub(super) icons: BTreeMap<String, IconifyIcon>,

    #[serde(default, deserialize_with = "deserialize_aliases")]
    pub(super) aliases: BTreeMap<String, IconifyAlias>,

    #[serde(default, deserialize_with = "deserialize_chars")]
    pub(super) chars: BTreeMap<String, String>,

    #[serde(default)]
    pub(super) left: f64,

    #[serde(default)]
    pub(super) top: f64,

    #[serde(default = "default_dimension")]
    pub(super) width: f64,

    #[serde(default = "default_dimension")]
    pub(super) height: f64,
    #[serde(default)]
    pub(super) rotate: IconRotation,
    #[serde(default)]
    pub(super) h_flip: bool,
    #[serde(default)]
    pub(super) v_flip: bool,
    #[serde(rename = "info", default)]
    pub(super) metadata: Option<IconifyMetadata>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct IconifyMetadata {
    pub(super) license: Option<crate::IconLicense>,
}

pub(super) fn parse(bytes: &[u8]) -> Result<IconifyCollection, crate::InvalidCollection> {
    if bytes.len() > MAX_COLLECTION_BYTES {
        return Err(crate::InvalidCollection::TooLarge {
            limit: MAX_COLLECTION_BYTES,
        });
    }
    Ok(serde_json::from_slice(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InvalidCollection;

    #[test]
    fn wire_maps_reject_entry_overflow() {
        let seed = BoundedMapSeed::new(
            "icons",
            1,
            BoundedStringSeed::new("icon name", 8),
            TypedValueSeed::<IconifyIcon>::new(),
        );
        let mut deserializer = serde_json::Deserializer::from_str(
            r#"{"a":{"body":"<path/>"},"b":{"body":"<rect/>"}}"#,
        );
        assert!(seed.deserialize(&mut deserializer).is_err());
    }

    #[test]
    fn duplicate_wire_keys_are_rejected() {
        for source in [
            r#"{"prefix":"a","prefix":"b","icons":{}}"#,
            r#"{"prefix":"a","icons":{"x":{"body":"<path/>","body":"<rect/>"}}}"#,
            r#"{"prefix":"a","icons":{"x":{"body":"<path/>"},"x":{"body":"<rect/>"}}}"#,
        ] {
            assert!(matches!(
                parse(source.as_bytes()),
                Err(InvalidCollection::Json { .. })
            ));
        }
    }

    #[test]
    fn json_diagnostics_hide_source_strings() {
        let source = br#"{"prefix":"private-source-sentinel","icons":{"mark":{"body":"<path/>"}},"rotate":"secret"}"#;
        let rejection = parse(source).unwrap_err().to_string();
        assert!(rejection.len() < 128);
        assert!(!rejection.contains("private-source-sentinel"));
        assert!(!rejection.contains("secret"));
    }
}
