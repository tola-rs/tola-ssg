//! The Iconify collections Tola indexes and the releases it pins their bytes to.

mod releases;

use crate::config::section::Sha256Digest;

/// One release of an indexed Iconify collection, and the digest of the bytes that serve it.
pub(crate) struct PinnedRelease {
    pub(crate) version: &'static str,
    pub(crate) sha256: Sha256Digest,
}

/// One Iconify collection Tola indexes, with its releases in ascending order.
pub(crate) struct IndexedCollection {
    pub(crate) name: &'static str,
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

/// One resolved preset: the pinned release and the mirrors that serve it.
pub(crate) struct ResolvedPreset {
    pub(crate) name: &'static str,
    pub(crate) version: &'static str,
    pub(crate) urls: Vec<String>,
    pub(crate) sha256: Sha256Digest,
}

/// The collection one author-written name refers to, when Tola indexes it.
pub(crate) fn indexed(name: &str) -> Option<&'static IndexedCollection> {
    collections()
        .iter()
        .find(|collection| collection.name == name)
}

/// Every collection Tola indexes, in ascending name order.
pub(crate) fn collections() -> &'static [IndexedCollection] {
    releases::INDEXED_COLLECTIONS
}

/// The names of every collection Tola indexes, in ascending order.
pub(crate) fn names() -> impl Iterator<Item = &'static str> {
    collections().iter().map(|collection| collection.name)
}

/// The indexed collection a written name most plausibly meant.
pub(crate) fn nearest(written: &str) -> Option<&'static str> {
    if let Some(closest) = tola_typst::spelling::closest(written, names()) {
        return Some(closest);
    }
    // Both sides must be at least this long, or `bi` would claim `bitcoin-icons`.
    const SHARED: usize = 4;
    if written.len() < SHARED {
        return None;
    }
    names()
        .filter(|name| name.len() >= SHARED)
        .filter(|name| name.starts_with(written) || written.starts_with(name))
        .min_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)))
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
        urls: release_urls(collection.name, release.version),
        sha256: release.sha256,
    })
}

/// The npm mirrors serving one pinned release, in the order Tola tries them.
fn release_urls(name: &str, version: &str) -> Vec<String> {
    [
        format!("https://cdn.jsdelivr.net/npm/@iconify-json/{name}@{version}/icons.json"),
        format!("https://unpkg.com/@iconify-json/{name}@{version}/icons.json"),
    ]
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn near_miss_names_the_collection() {
        assert_eq!(nearest("lucid"), Some("lucide"));
        // Beyond typo distance, the shared prefix still answers the question.
        assert_eq!(
            nearest("material-symbols-outlined"),
            Some("material-symbols")
        );
        assert_eq!(nearest("not-a-real-collection"), None);
        // A shared prefix with a too-short name proves nothing.
        assert_eq!(nearest("bixyz"), None);
    }
}
