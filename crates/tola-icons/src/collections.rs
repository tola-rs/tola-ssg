//! Namespace ownership: mounting collections under caller-chosen namespaces.

use std::{
    collections::{btree_map::Entry, BTreeMap},
    sync::Arc,
};

use crate::{IconCollection, IconCollectionName, IconId, InvalidIconName, SvgIcon};

/// A collection namespace is invalid or already mounted.
#[derive(Debug, thiserror::Error)]
pub enum InvalidIconNamespace {
    /// The namespace violates the identity grammar.
    #[error("invalid icon collection namespace: {0}")]
    Name(#[from] InvalidIconName),
    /// The namespace is already mounted.
    #[error("duplicate icon collection namespace `{namespace}`")]
    DuplicateNamespace {
        /// The colliding namespace.
        namespace: String,
    },
}

/// Named, shared icon collections with no implicit default collection.
#[derive(Clone, Debug, Default)]
pub struct IconCollections {
    collections: BTreeMap<IconCollectionName, Arc<IconCollection>>,
}

impl IconCollections {
    /// Create an empty collection set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mount a collection under the caller's namespace. Existing namespaces cannot be overwritten.
    pub fn mount(
        &mut self,
        namespace: impl AsRef<str>,
        collection: impl Into<Arc<IconCollection>>,
    ) -> Result<(), InvalidIconNamespace> {
        let namespace: IconCollectionName = namespace.as_ref().parse()?;
        match self.collections.entry(namespace) {
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

    /// Resolve an icon in a mounted collection.
    pub fn get(&self, namespace: &str, name: &str) -> Option<&SvgIcon> {
        self.collections.get(namespace)?.get(name)
    }

    /// Resolve a validated icon identity.
    pub fn get_id(&self, identity: &IconId) -> Option<&SvgIcon> {
        self.get(identity.collection(), identity.name())
    }

    /// Obtain a mounted collection.
    pub fn collection(&self, namespace: &str) -> Option<&IconCollection> {
        self.collections.get(namespace).map(AsRef::as_ref)
    }

    /// Iterate over mounted namespaces and collections in lexical order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&str, &IconCollection)> {
        self.collections
            .iter()
            .map(|(namespace, collection)| (namespace.as_str(), collection.as_ref()))
    }

    /// Number of mounted collections.
    pub fn len(&self) -> usize {
        self.collections.len()
    }

    /// Whether no collections have been mounted.
    pub fn is_empty(&self) -> bool {
        self.collections.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &[u8] = br#"{"prefix":"upstream","icons":{"mark":{"body":"<path/>"}}}"#;

    #[test]
    fn lookup_uses_the_mounted_namespace() {
        let mut collections = IconCollections::new();
        collections
            .mount("brand", IconCollection::from_iconify(SOURCE).unwrap())
            .unwrap();
        assert!(collections.get("brand", "mark").is_some());
        assert!(collections.get("upstream", "mark").is_none());
    }

    #[test]
    fn mount_refuses_occupied_namespace() {
        let mut collections = IconCollections::new();
        collections
            .mount("brand", IconCollection::from_iconify(SOURCE).unwrap())
            .unwrap();
        let rejection = collections.mount("brand", IconCollection::new());
        assert!(matches!(
            rejection,
            Err(InvalidIconNamespace::DuplicateNamespace { .. })
        ));
        assert!(collections.get("brand", "mark").is_some());
    }
}
