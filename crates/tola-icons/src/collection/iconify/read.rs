//! Bounded Iconify JSON wire types and deserializers.

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

// Headroom above `@iconify/json` 2.2.500 maxima (19,992 icons, 6,363 aliases, 7,447 chars).
const MAX_ICON_ENTRIES: usize = 65_536;
const MAX_COLLECTION_BYTES: usize = 64 * 1024 * 1024;
const MAX_ALIAS_ENTRIES: usize = 32_768;
const MAX_CHAR_ENTRIES: usize = 32_768;
const MAX_PREFIX_BYTES: usize = 2_048;
const MAX_CHAR_KEY_BYTES: usize = 256;
const MAX_ICON_BODY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy)]
struct BoundedStringSeed {
    desc: &'static str,
    max_bytes: usize,
}

impl BoundedStringSeed {
    const fn new(desc: &'static str, max_bytes: usize) -> Self {
        Self { desc, max_bytes }
    }

    fn check_len<E: serde::de::Error>(&self, val: &str) -> Result<(), E> {
        if val.len() > self.max_bytes {
            return Err(E::custom(format_args!(
                "{} exceeds the {}-byte limit",
                self.desc, self.max_bytes
            )));
        }
        Ok(())
    }
}

impl<'de> Visitor<'de> for BoundedStringSeed {
    type Value = String;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} no longer than {} bytes", self.desc, self.max_bytes)
    }

    fn visit_borrowed_str<E: serde::de::Error>(self, val: &'de str) -> Result<Self::Value, E> {
        self.visit_str(val)
    }

    fn visit_str<E: serde::de::Error>(self, val: &str) -> Result<Self::Value, E> {
        self.check_len(val)?;
        Ok(val.to_owned())
    }

    fn visit_string<E: serde::de::Error>(self, val: String) -> Result<Self::Value, E> {
        self.check_len(&val)?;
        Ok(val)
    }
}

impl<'de> DeserializeSeed<'de> for BoundedStringSeed {
    type Value = String;

    fn deserialize<D: Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        de.deserialize_string(self)
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

impl<'de, V: Deserialize<'de>> DeserializeSeed<'de> for TypedValueSeed<V> {
    type Value = V;

    fn deserialize<D: Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        V::deserialize(de)
    }
}

#[derive(Clone, Copy)]
struct BoundedMapSeed<V, S> {
    desc: &'static str,
    max_entries: usize,
    key: BoundedStringSeed,
    val: S,
    marker: PhantomData<fn() -> V>,
}

impl<V, S> BoundedMapSeed<V, S> {
    const fn new(desc: &'static str, max_entries: usize, key: BoundedStringSeed, val: S) -> Self {
        Self {
            desc,
            max_entries,
            key,
            val,
            marker: PhantomData,
        }
    }
}

impl<'de, V, S> Visitor<'de> for BoundedMapSeed<V, S>
where
    S: Copy + DeserializeSeed<'de, Value = V>,
{
    type Value = BTreeMap<String, V>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "an {} map with at most {} entries",
            self.desc, self.max_entries
        )
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out = BTreeMap::new();
        while let Some(key) = map.next_key_seed(self.key)? {
            // Check duplicates and entry limits before allocating the value payload.
            let full = out.len() >= self.max_entries;
            match out.entry(key) {
                Entry::Occupied(_) => {
                    return Err(A::Error::custom(format_args!(
                        "{} map contains a duplicate key",
                        self.desc
                    )));
                }
                Entry::Vacant(entry) => {
                    if full {
                        return Err(A::Error::custom(format_args!(
                            "{} map exceeds the {}-entry limit",
                            self.desc, self.max_entries
                        )));
                    }
                    entry.insert(map.next_value_seed(self.val)?);
                }
            }
        }
        Ok(out)
    }
}

impl<'de, V, S> DeserializeSeed<'de> for BoundedMapSeed<V, S>
where
    S: Copy + DeserializeSeed<'de, Value = V>,
{
    type Value = BTreeMap<String, V>;

    fn deserialize<D: Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        de.deserialize_map(self)
    }
}

fn de_bounded_str<'de, D: Deserializer<'de>>(
    de: D,
    desc: &'static str,
    max_bytes: usize,
) -> Result<String, D::Error> {
    BoundedStringSeed::new(desc, max_bytes).deserialize(de)
}

fn de_prefix<'de, D: Deserializer<'de>>(de: D) -> Result<String, D::Error> {
    de_bounded_str(de, "collection prefix", MAX_PREFIX_BYTES)
}

fn de_icon_body<'de, D: Deserializer<'de>>(de: D) -> Result<String, D::Error> {
    de_bounded_str(de, "icon body", MAX_ICON_BODY_BYTES)
}

fn de_alias_parent<'de, D: Deserializer<'de>>(de: D) -> Result<String, D::Error> {
    de_bounded_str(de, "alias parent", MAX_ICON_NAME_BYTES)
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

    fn try_from(val: u8) -> Result<Self, Self::Error> {
        (val <= 3)
            .then_some(Self(val))
            .ok_or("icon rotation must be an integer from 0 through 3")
    }
}

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
    #[serde(deserialize_with = "de_icon_body")]
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
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let wire = IconWire::deserialize(de)?;
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
    #[serde(deserialize_with = "de_alias_parent")]
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
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let wire = AliasWire::deserialize(de)?;
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

fn de_icons<'de, D: Deserializer<'de>>(de: D) -> Result<BTreeMap<String, IconifyIcon>, D::Error> {
    BoundedMapSeed::new(
        "icons",
        MAX_ICON_ENTRIES,
        BoundedStringSeed::new("icon name", MAX_ICON_NAME_BYTES),
        TypedValueSeed::<IconifyIcon>::new(),
    )
    .deserialize(de)
}

fn de_aliases<'de, D: Deserializer<'de>>(
    de: D,
) -> Result<BTreeMap<String, IconifyAlias>, D::Error> {
    BoundedMapSeed::new(
        "aliases",
        MAX_ALIAS_ENTRIES,
        BoundedStringSeed::new("alias name", MAX_ICON_NAME_BYTES),
        TypedValueSeed::<IconifyAlias>::new(),
    )
    .deserialize(de)
}

fn de_chars<'de, D: Deserializer<'de>>(de: D) -> Result<BTreeMap<String, String>, D::Error> {
    BoundedMapSeed::new(
        "chars",
        MAX_CHAR_ENTRIES,
        BoundedStringSeed::new("character key", MAX_CHAR_KEY_BYTES),
        BoundedStringSeed::new("character target", MAX_ICON_NAME_BYTES),
    )
    .deserialize(de)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct IconifyCollection {
    #[serde(deserialize_with = "de_prefix")]
    pub(super) prefix: String,
    #[serde(deserialize_with = "de_icons")]
    pub(super) icons: BTreeMap<String, IconifyIcon>,
    #[serde(default, deserialize_with = "de_aliases")]
    pub(super) aliases: BTreeMap<String, IconifyAlias>,
    #[serde(default, deserialize_with = "de_chars")]
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

    #[test]
    fn wire_map_overflow() {
        let seed = BoundedMapSeed::new(
            "icons",
            1,
            BoundedStringSeed::new("icon name", 8),
            TypedValueSeed::<IconifyIcon>::new(),
        );
        let mut de = serde_json::Deserializer::from_str(
            r#"{"a":{"body":"<path/>"},"b":{"body":"<rect/>"}}"#,
        );
        assert!(seed.deserialize(&mut de).is_err());
    }

    #[test]
    fn json_errors_hide_strings() {
        let src = br#"{"prefix":"private-source-sentinel","icons":{"mark":{"body":"<path/>"}},"rotate":"secret"}"#;
        let err = parse(src).unwrap_err().to_string();
        assert!(err.len() < 128);
        assert!(!err.contains("private-source-sentinel"));
        assert!(!err.contains("secret"));
    }
}
