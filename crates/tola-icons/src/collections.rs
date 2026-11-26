//! Collection mounting under caller-chosen namespaces.

use std::{
    collections::{btree_map::Entry, BTreeMap},
    sync::Arc,
};

use crate::{IconCollection, IconCollectionKey, IconId, InvalidIconName, SvgIcon};

/// A collection namespace is invalid or already mounted.
#[derive(Debug, thiserror::Error)]
pub enum InvalidIconNamespace {
    /// The namespace violates the icon identity grammar.
    #[error("invalid icon collection namespace: {0}")]
    Name(#[from] InvalidIconName),
    /// The namespace is already mounted.
    #[error("duplicate icon collection namespace `{namespace}`")]
    DuplicateNamespace {
        /// Colliding namespace string.
        namespace: String,
    },
}

/// Named, shared icon collections with no implicit default collection.
#[derive(Clone, Debug, Default)]
pub struct IconCollections {
    collections: BTreeMap<IconCollectionKey, Arc<IconCollection>>,
}

impl IconCollections {
    /// Create an empty collection set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mount a collection under `namespace`, failing if that namespace is already occupied.
    pub fn mount(
        &mut self,
        namespace: impl AsRef<str>,
        collection: impl Into<Arc<IconCollection>>,
    ) -> Result<(), InvalidIconNamespace> {
        let ns: IconCollectionKey = namespace.as_ref().parse()?;
        match self.collections.entry(ns) {
            Entry::Occupied(entry) => {
                return Err(InvalidIconNamespace::DuplicateNamespace {
                    namespace: entry.key().to_string(),
                });
            }
            Entry::Vacant(entry) => {
                entry.insert(collection.into());
            }
        }
        Ok(())
    }

    /// Look up an icon in a mounted collection by namespace and name.
    pub fn get(&self, namespace: &str, name: &str) -> Option<&SvgIcon> {
        self.collections.get(namespace)?.get(name)
    }

    /// Look up an icon by validated identity.
    pub fn get_id(&self, id: &IconId) -> Option<&SvgIcon> {
        self.get(id.collection(), id.name())
    }

    /// Return a mounted collection by namespace.
    pub fn collection(&self, namespace: &str) -> Option<&IconCollection> {
        self.collections.get(namespace).map(AsRef::as_ref)
    }

    /// Iterate over mounted namespaces and collections in lexical order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&str, &IconCollection)> {
        self.collections
            .iter()
            .map(|(ns, col)| (ns.as_str(), col.as_ref()))
    }

    /// Return the number of mounted collections.
    pub fn len(&self) -> usize {
        self.collections.len()
    }

    /// Return `true` when no collections are mounted.
    pub fn is_empty(&self) -> bool {
        self.collections.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &[u8] = br#"{"prefix":"upstream","icons":{"mark":{"body":"<path/>"}}}"#;

    #[test]
    fn lookup_uses_mounted_ns() {
        let mut cols = IconCollections::new();
        cols.mount("brand", IconCollection::from_iconify(SRC).unwrap())
            .unwrap();
        assert!(cols.get("brand", "mark").is_some());
        assert!(cols.get("upstream", "mark").is_none());
    }

    #[test]
    fn mount_rejects_duplicate_ns() {
        let mut cols = IconCollections::new();
        cols.mount("brand", IconCollection::from_iconify(SRC).unwrap())
            .unwrap();
        let err = cols.mount("brand", IconCollection::new());
        assert!(matches!(
            err,
            Err(InvalidIconNamespace::DuplicateNamespace { .. })
        ));
        assert!(cols.get("brand", "mark").is_some());
    }
}
