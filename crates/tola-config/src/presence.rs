//! Explicit field and section presence in a TOML source.

use rustc_hash::FxHashMap;
use std::sync::Arc;

/// Tracks which TOML paths were explicitly present in user config.
///
/// Dot paths (e.g. `assets.files`) can address array-table elements by index
/// or match across them. Clones share the TOML tree without expanding path combinations.
#[derive(Debug, Clone, Default)]
pub struct ConfigPresence {
    root: Option<Arc<PresenceNode>>,
}

#[derive(Debug)]
enum PresenceNode {
    Table(FxHashMap<String, PresenceNode>),
    // Store only tables, keyed by their original array positions.
    Array(FxHashMap<usize, PresenceNode>),
    Scalar,
}

impl ConfigPresence {
    pub fn from_toml(content: &str) -> Result<Self, toml::de::Error> {
        let value: toml::Value = toml::from_str(content)?;
        Ok(Self::from_value(&value))
    }

    /// Build presence from a parsed TOML value in linear time and space.
    pub fn from_value(value: &toml::Value) -> Self {
        Self {
            root: match PresenceNode::from_value(value) {
                PresenceNode::Table(table) if table.is_empty() => None,
                PresenceNode::Array(items) if items.is_empty() => None,
                PresenceNode::Scalar => None,
                root => Some(Arc::new(root)),
            },
        }
    }

    /// Check whether a field or section path was explicitly present.
    #[inline]
    pub fn contains(&self, path: &str) -> bool {
        self.contains_scoped(path, &[])
    }

    /// Check a field in one array-table element. Unlike
    /// `contains("site.seo.feeds.url")`, this cannot match another feed's URL.
    #[inline]
    pub fn contains_indexed(&self, array_path: &str, index: usize, field: &str) -> bool {
        if array_path.is_empty() || field.is_empty() {
            return false;
        }
        let path = format!("{array_path}.{index}.{field}");
        self.contains(&path)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    pub(crate) fn contains_scoped(&self, path: &str, scopes: &[(&str, usize)]) -> bool {
        !path.is_empty()
            && self
                .root
                .as_ref()
                .is_some_and(|root| root.contains(Some(path), path, scopes))
    }
}

impl PresenceNode {
    fn from_value(value: &toml::Value) -> Self {
        match value {
            toml::Value::Table(table) => Self::Table(
                table
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from_value(value)))
                    .collect(),
            ),
            toml::Value::Array(items) => Self::Array(
                items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| matches!(item, toml::Value::Table(_)))
                    .map(|(index, item)| (index, Self::from_value(item)))
                    .collect(),
            ),
            _ => Self::Scalar,
        }
    }

    fn contains(&self, remaining: Option<&str>, full_path: &str, scopes: &[(&str, usize)]) -> bool {
        if let Self::Array(items) = self {
            // Scope paths are canonical and unindexed. Select the scoped array element
            // before checking its section or fields.
            let array_path_end = remaining.map_or(full_path.len(), |remaining| {
                (full_path.len() - remaining.len()).saturating_sub(1)
            });
            if let Some((_, index)) = scopes
                .iter()
                .find(|(path, _)| *path == &full_path[..array_path_end])
            {
                return items
                    .get(index)
                    .is_some_and(|item| item.contains(remaining, full_path, scopes));
            }
        }
        let Some(remaining) = remaining else {
            return true;
        };
        match self {
            Self::Table(table) => table_key_candidates(remaining).any(|key| {
                table.get(key).is_some_and(|child| {
                    let rest = remaining.get(key.len() + 1..);
                    child.contains(rest, full_path, scopes)
                })
            }),
            Self::Array(items) => {
                split_array_index(remaining).is_some_and(|(index, rest)| {
                    items
                        .get(&index)
                        .is_some_and(|item| item.contains(rest, full_path, scopes))
                }) || items
                    .values()
                    .any(|item| item.contains(Some(remaining), full_path, scopes))
            }
            Self::Scalar => false,
        }
    }
}

/// Try complete TOML keys, including keys containing literal dots.
fn table_key_candidates(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('.')
        .map(|(offset, _)| offset)
        .chain(std::iter::once(path.len()))
        .map(move |end| &path[..end])
}

/// Only canonical decimal segments address an array element.
fn split_array_index(path: &str) -> Option<(usize, Option<&str>)> {
    let (segment, rest) = path
        .split_once('.')
        .map_or((path, None), |(segment, rest)| (segment, Some(rest)));
    if segment.is_empty()
        || (segment != "0" && segment.starts_with('0'))
        || !segment.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    segment.parse().ok().map(|index| (index, rest))
}

#[cfg(test)]
mod tests {
    use super::ConfigPresence;

    #[test]
    fn indexed_paths_select_one_array_element() {
        let raw = r#"
[[site.seo.feeds]]
format = "rss"

[[site.seo.feeds]]
format = "atom"
url = "/atom.xml"
"#;

        let presence = ConfigPresence::from_toml(raw).unwrap();

        assert!(presence.contains("site.seo.feeds"));
        assert!(presence.contains("site.seo.feeds.format"));
        assert!(presence.contains("site.seo.feeds.url"));
        assert!(presence.contains("site.seo.feeds.0.format"));
        assert!(!presence.contains("site.seo.feeds.0.url"));
        assert!(presence.contains("site.seo.feeds.1.format"));
        assert!(presence.contains("site.seo.feeds.1.url"));
        assert!(presence.contains_indexed("site.seo.feeds", 1, "url"));
        assert!(!presence.contains_indexed("site.seo.feeds", 0, "url"));
    }

    #[test]
    fn empty_roots_have_no_presence() {
        for value in [
            toml::Value::String(String::new()),
            toml::Value::Table(Default::default()),
            toml::Value::Array(Vec::new()),
            toml::Value::Array(vec![toml::Value::Integer(0)]),
        ] {
            let presence = ConfigPresence::from_value(&value);
            assert!(presence.is_empty());
            assert!(!presence.contains("0"));
            assert!(!presence.contains("field"));
        }

        let presence = ConfigPresence::from_value(&toml::Value::Array(vec![toml::Value::Table(
            Default::default(),
        )]));
        assert!(!presence.is_empty());
        assert!(presence.contains("0"));
        assert!(!presence.contains("0.field"));
    }

    #[test]
    fn nesting_depth_does_not_multiply_paths() {
        let mut value = toml::Value::Table(toml::map::Map::from_iter([(
            "leaf".into(),
            toml::Value::Boolean(true),
        )]));
        for _ in 0..40 {
            value = toml::Value::Table(toml::map::Map::from_iter([(
                "items".into(),
                toml::Value::Array(vec![value]),
            )]));
        }
        let presence = ConfigPresence::from_value(&value);
        assert!(presence.contains(&format!("{}leaf", "items.".repeat(40))));
        assert!(presence.contains(&format!("{}leaf", "items.0.".repeat(40))));
        assert!(presence.contains(&format!("{}leaf", "items.0.items.".repeat(20))));
        assert!(!presence.contains(&format!("{}missing", "items.".repeat(40))));
    }

    #[test]
    fn literal_table_keys_stay_addressable() {
        let presence = ConfigPresence::from_toml(
            "\"literal.dot\" = { value = true }\nempty = []\nscalars = [1, 2]\nmixed = [0, { field = true }]\n",
        )
        .unwrap();
        assert!(presence.contains("literal.dot.value"));
        assert!(presence.contains("empty"));
        assert!(!presence.contains("scalars.0"));
        assert!(!presence.contains_indexed("mixed", 0, "field"));
        assert!(presence.contains_indexed("mixed", 1, "field"));
        assert!(!presence.contains("mixed.01.field"));
        assert!(!presence.contains("mixed.1."));
        assert!(!presence.contains("literal.dot.value."));
    }

    #[test]
    fn failed_index_tries_table_keys() {
        let presence = ConfigPresence::from_toml(
            r#"
[[rows]]
"0" = { field = true }
"01" = { field = true }
"#,
        )
        .unwrap();

        assert!(presence.contains("rows.0.field"));
        assert!(presence.contains("rows.01.field"));
        assert!(!presence.contains("rows.1.field"));
    }
}
