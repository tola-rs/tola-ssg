//! The Iconify collections Tola indexes, and the releases it pins their bytes to.
//!
//! A preset names a collection release instead of a URL; the index in `releases` supplies the npm
//! URL that serves it and the SHA-256 digest of its bytes. `just scripts::icons-index` regenerates
//! that table.

mod releases;

use crate::config::section::Sha256Digest;

/// One release of an indexed Iconify collection, and the digest of the bytes that serve it.
pub(crate) struct PinnedRelease {
    pub(super) version: &'static str,
    pub(super) sha256: Sha256Digest,
}

/// One Iconify collection Tola indexes, with its releases in ascending order.
pub(crate) struct IndexedCollection {
    pub(super) name: &'static str,
    pub(super) releases: &'static [PinnedRelease],
}

impl IndexedCollection {
    /// The release an author-written version names.
    pub(crate) fn release(&self, version: &str) -> Option<&'static PinnedRelease> {
        self.releases
            .iter()
            .find(|release| release.version == version)
    }

    /// The newest release Tola indexes for this collection.
    pub(crate) fn newest(&self) -> &'static PinnedRelease {
        self.releases
            .last()
            .expect("every indexed collection pins at least one release")
    }

    /// The releases Tola indexes for this collection, newest first.
    pub(crate) fn versions(&self) -> impl Iterator<Item = &'static str> {
        self.releases.iter().rev().map(|release| release.version)
    }
}

/// One resolved preset: the pinned release and where to fetch it.
pub(crate) struct ResolvedPreset {
    pub(crate) name: &'static str,
    pub(crate) version: &'static str,
    pub(crate) url: String,
    pub(crate) sha256: Sha256Digest,
}

/// The collection one author-written name refers to, when Tola indexes it.
pub(crate) fn indexed(name: &str) -> Option<&'static IndexedCollection> {
    releases::INDEXED_COLLECTIONS
        .iter()
        .find(|collection| collection.name == name)
}

/// Resolve one preset to the release Tola pins, or `None` when the index has neither.
pub(crate) fn resolve(name: &str, version: Option<&str>) -> Option<ResolvedPreset> {
    let collection = indexed(name)?;
    let release = match version {
        Some(version) => collection.release(version)?,
        None => collection.newest(),
    };
    Some(ResolvedPreset {
        name: collection.name,
        version: release.version,
        url: release_url(collection.name, release.version),
        sha256: release.sha256,
    })
}

/// The npm URL serving one indexed release's IconifyJSON.
///
/// Every indexed collection is published as an `@iconify-json` package, and a versioned CDN path
/// is immutable.
fn release_url(name: &str, version: &str) -> String {
    format!("https://cdn.jsdelivr.net/npm/@iconify-json/{name}@{version}/icons.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_collection() -> &'static IndexedCollection {
        releases::INDEXED_COLLECTIONS
            .first()
            .expect("the index has at least one collection")
    }

    #[test]
    fn index_pins_releases_in_order() {
        let names = releases::INDEXED_COLLECTIONS
            .iter()
            .map(|collection| collection.name)
            .collect::<Vec<_>>();
        let mut expected = names.clone();
        expected.sort_unstable();
        expected.dedup();
        assert_eq!(names.len(), expected.len(), "an indexed name repeats");
        assert_eq!(names, expected, "the index is not in ascending name order");
        for collection in releases::INDEXED_COLLECTIONS {
            assert!(
                !collection.releases.is_empty(),
                "`{}` pins no release",
                collection.name
            );
            let mut versions = collection
                .releases
                .iter()
                .map(|release| release.version)
                .collect::<Vec<_>>();
            let pinned = versions.len();
            versions.sort_unstable();
            versions.dedup();
            assert_eq!(
                pinned,
                versions.len(),
                "`{}` repeats a release",
                collection.name
            );
        }
    }

    #[test]
    fn preset_resolves_to_the_newest_release() {
        let collection = first_collection();
        let name = collection.name;
        let newest = collection.newest();
        let resolved = resolve(name, None).expect("an indexed collection resolves");
        assert_eq!(resolved.name, name);
        assert_eq!(resolved.version, newest.version);
        assert_eq!(
            resolved.url,
            format!(
                "https://cdn.jsdelivr.net/npm/@iconify-json/{name}@{}/icons.json",
                newest.version
            )
        );
        assert_eq!(resolved.sha256, newest.sha256);

        let pinned = resolve(name, Some(newest.version)).expect("an indexed release resolves");
        assert_eq!(pinned.version, newest.version);
    }

    #[test]
    fn unknown_collection_does_not_resolve() {
        assert!(resolve("not-a-real-collection", None).is_none());
        let collection = first_collection();
        assert!(resolve(collection.name, Some("0.0.1")).is_none());
    }
}
