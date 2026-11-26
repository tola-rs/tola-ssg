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
        let val: toml::Value = toml::from_str(content)?;
        Ok(Self::from_value(&val))
    }

    pub fn from_value(val: &toml::Value) -> Self {
        Self {
            root: match PresenceNode::from_value(val) {
                PresenceNode::Table(tbl) if tbl.is_empty() => None,
                PresenceNode::Array(items) if items.is_empty() => None,
                PresenceNode::Scalar => None,
                root => Some(Arc::new(root)),
            },
        }
    }

    #[inline]
    pub fn contains(&self, path: &str) -> bool {
        self.contains_scoped(path, &[])
    }

    #[inline]
    pub fn contains_indexed(&self, array_path: &str, idx: usize, field: &str) -> bool {
        if array_path.is_empty() || field.is_empty() {
            return false;
        }
        self.contains(&format!("{array_path}.{idx}.{field}"))
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
    fn from_value(val: &toml::Value) -> Self {
        match val {
            toml::Value::Table(tbl) => Self::Table(
                tbl.iter()
                    .map(|(k, v)| (k.clone(), Self::from_value(v)))
                    .collect(),
            ),
            toml::Value::Array(items) => Self::Array(
                items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| matches!(item, toml::Value::Table(_)))
                    .map(|(idx, item)| (idx, Self::from_value(item)))
                    .collect(),
            ),
            _ => Self::Scalar,
        }
    }

    fn contains(&self, rem: Option<&str>, full_path: &str, scopes: &[(&str, usize)]) -> bool {
        if let Self::Array(items) = self {
            // Scope paths are unindexed; select the active element before descending.
            let array_end = rem.map_or(full_path.len(), |r| {
                (full_path.len() - r.len()).saturating_sub(1)
            });
            if let Some((_, idx)) = scopes
                .iter()
                .find(|(path, _)| *path == &full_path[..array_end])
            {
                return items
                    .get(idx)
                    .is_some_and(|item| item.contains(rem, full_path, scopes));
            }
        }
        let Some(rem) = rem else {
            return true;
        };
        match self {
            Self::Table(tbl) => table_key_candidates(rem).any(|k| {
                tbl.get(k).is_some_and(|child| {
                    let rest = rem.get(k.len() + 1..);
                    child.contains(rest, full_path, scopes)
                })
            }),
            Self::Array(items) => {
                split_array_index(rem).is_some_and(|(idx, rest)| {
                    items
                        .get(&idx)
                        .is_some_and(|item| item.contains(rest, full_path, scopes))
                }) || items
                    .values()
                    .any(|item| item.contains(Some(rem), full_path, scopes))
            }
            Self::Scalar => false,
        }
    }
}

fn table_key_candidates(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('.')
        .map(|(off, _)| off)
        .chain(std::iter::once(path.len()))
        .map(move |end| &path[..end])
}

/// Only canonical decimal segments address an array element.
fn split_array_index(path: &str) -> Option<(usize, Option<&str>)> {
    let (seg, rest) = path
        .split_once('.')
        .map_or((path, None), |(s, r)| (s, Some(r)));
    if seg.is_empty()
        || (seg != "0" && seg.starts_with('0'))
        || !seg.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    seg.parse().ok().map(|idx| (idx, rest))
}

#[cfg(test)]
mod tests {
    use super::ConfigPresence;

    #[test]
    fn indexed_array_paths() {
        let raw = r#"
[[site.seo.feeds]]
format = "rss"

[[site.seo.feeds]]
format = "atom"
url = "/atom.xml"
"#;

        let p = ConfigPresence::from_toml(raw).unwrap();

        assert!(p.contains("site.seo.feeds"));
        assert!(p.contains("site.seo.feeds.format"));
        assert!(p.contains("site.seo.feeds.url"));
        assert!(p.contains("site.seo.feeds.0.format"));
        assert!(!p.contains("site.seo.feeds.0.url"));
        assert!(p.contains("site.seo.feeds.1.format"));
        assert!(p.contains("site.seo.feeds.1.url"));
        assert!(p.contains_indexed("site.seo.feeds", 1, "url"));
        assert!(!p.contains_indexed("site.seo.feeds", 0, "url"));
    }

    #[test]
    fn empty_roots() {
        for val in [
            toml::Value::String(String::new()),
            toml::Value::Table(Default::default()),
            toml::Value::Array(Vec::new()),
            toml::Value::Array(vec![toml::Value::Integer(0)]),
        ] {
            let p = ConfigPresence::from_value(&val);
            assert!(p.is_empty());
            assert!(!p.contains("0"));
            assert!(!p.contains("field"));
        }

        let p = ConfigPresence::from_value(&toml::Value::Array(vec![toml::Value::Table(
            Default::default(),
        )]));
        assert!(!p.is_empty());
        assert!(p.contains("0"));
        assert!(!p.contains("0.field"));
    }

    #[test]
    fn literal_table_keys() {
        let p = ConfigPresence::from_toml(
            "\"literal.dot\" = { value = true }\nempty = []\nscalars = [1, 2]\nmixed = [0, { field = true }]\n",
        )
        .unwrap();
        assert!(p.contains("literal.dot.value"));
        assert!(p.contains("empty"));
        assert!(!p.contains("scalars.0"));
        assert!(!p.contains_indexed("mixed", 0, "field"));
        assert!(p.contains_indexed("mixed", 1, "field"));
        assert!(!p.contains("mixed.01.field"));
        assert!(!p.contains("mixed.1."));
        assert!(!p.contains("literal.dot.value."));
    }
}
