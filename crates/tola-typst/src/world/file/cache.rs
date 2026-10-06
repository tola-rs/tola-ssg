//! File caching with fingerprint-based invalidation.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::{Mutex, RwLock};
use rustc_hash::FxHashMap;
use typst::diag::{FileError, FileResult};
use typst::foundations::Bytes;
use typst::syntax::{FileId, Source};

use super::FileResolver;
use super::evidence::{Loaded, ReadAttempt};
use super::read::decode_utf8;

/// Slots of one compilation root, keyed by file identity.
type RootSlots = FxHashMap<FileId, Arc<SharedFileSlot>>;

/// Shared file cache scoped by root and `FileId`.
///
/// Each file gets its own lock, so unrelated files can be processed in parallel
/// without contending on the whole cache map.
#[derive(Default)]
pub struct SharedFileCache {
    epoch: AtomicU64,
    /// A root is keyed by its own path, so a lookup hashes the path the caller already
    /// holds instead of copying it into a key first.
    roots: RwLock<FxHashMap<Arc<Path>, RootSlots>>,
}

impl SharedFileCache {
    /// Create an empty cache for sharing across worlds.
    pub fn new() -> Self {
        Self::default()
    }

    /// Evict slots that have not been used within `max_age` maintenance epochs.
    ///
    /// A zero age drops all map-owned slots immediately. Readers that already
    /// cloned a slot retain it until their current operation finishes.
    pub fn evict(&self, max_age: u64) {
        // Epoch and last-used values are eviction metadata only. Map ownership
        // and slot lifetime are synchronized by `roots`; these atomics do not
        // publish the cached file values.
        let mut observed = self.epoch.load(Ordering::Relaxed);
        let epoch = loop {
            let next = observed
                .checked_add(1)
                .expect("shared file cache epoch overflowed");
            match self.epoch.compare_exchange_weak(
                observed,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break next,
                Err(current) => observed = current,
            }
        };
        let mut roots = self.roots.write();
        if max_age == 0 {
            roots.clear();
            return;
        }
        roots.retain(|_, slots| {
            slots.retain(|_, slot| {
                Arc::strong_count(slot) > 1
                    || epoch.saturating_sub(slot.last_used.load(Ordering::Relaxed)) <= max_age
            });
            !slots.is_empty()
        });
    }

    /// Read and cache source text using an explicit file resolver.
    ///
    /// The read happens before the per-file lock, never under it.
    pub(crate) fn source_with_files(
        &self,
        id: FileId,
        root: &Path,
        files: &FileResolver,
    ) -> ReadAttempt<Loaded<Source>> {
        let observation = files.read_attempt(id, root);
        self.source_from_observation(id, root, observation)
    }

    /// Read and cache raw bytes using an explicit file resolver.
    pub(crate) fn file_with_files(
        &self,
        id: FileId,
        root: &Path,
        files: &FileResolver,
    ) -> ReadAttempt<Loaded<Bytes>> {
        let observation = files.read_attempt(id, root);
        self.file_from_observation(id, root, observation)
    }

    pub(crate) fn source_from_observation(
        &self,
        id: FileId,
        root: &Path,
        observation: ReadAttempt<Arc<[u8]>>,
    ) -> ReadAttempt<Loaded<Source>> {
        self.with_slot(root, id, |slot| slot.source_from_observation(observation))
    }

    pub(crate) fn file_from_observation(
        &self,
        id: FileId,
        root: &Path,
        observation: ReadAttempt<Arc<[u8]>>,
    ) -> ReadAttempt<Loaded<Bytes>> {
        self.with_slot(root, id, |slot| slot.file_from_observation(observation))
    }

    /// Run `use_slot` while holding this file's slot lock.
    fn with_slot<T>(
        &self,
        root: &Path,
        id: FileId,
        use_slot: impl FnOnce(&mut FileSlot) -> T,
    ) -> T {
        let slot = self.slot(root, id);
        let mut value = slot.value.lock();
        use_slot(&mut value)
    }

    /// How many file slots this cache's map currently owns, across every root it has seen.
    ///
    /// A slot a reader has already cloned stays counted here after an eviction that dropped it from
    /// the map, so this reports the cache's own reachable entries, not every live `Source`.
    pub fn retained_slots(&self) -> usize {
        self.roots.read().values().map(FxHashMap::len).sum()
    }

    fn slot(&self, root: &Path, id: FileId) -> Arc<SharedFileSlot> {
        let epoch = self.epoch.load(Ordering::Relaxed);
        if let Some(slot) = self.roots.read().get(root).and_then(|slots| slots.get(&id)) {
            slot.touch(epoch);
            return Arc::clone(slot);
        }

        let mut roots = self.roots.write();
        let slot = roots
            .entry(Arc::from(root))
            .or_default()
            .entry(id)
            .or_insert_with(|| Arc::new(SharedFileSlot::new(id, epoch)));
        slot.touch(epoch);
        Arc::clone(slot)
    }
}

struct SharedFileSlot {
    last_used: AtomicU64,
    value: Mutex<FileSlot>,
}

impl SharedFileSlot {
    fn new(id: FileId, epoch: u64) -> Self {
        Self {
            last_used: AtomicU64::new(epoch),
            value: Mutex::new(FileSlot::new(id)),
        }
    }

    fn touch(&self, epoch: u64) {
        self.last_used.fetch_max(epoch, Ordering::Relaxed);
    }
}

/// Processed value for one file, reused while a fresh read has the same fingerprint.
struct SlotCell<T> {
    data: Option<FileResult<T>>,
    fingerprint: Option<SlotFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SlotFingerprint {
    Successful(super::evidence::ReadEvidence),
    Failed(FileError),
}

impl<T: Clone> Default for SlotCell<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> SlotCell<T> {
    const fn new() -> Self {
        Self {
            data: None,
            fingerprint: None,
        }
    }

    fn get_or_init(
        &mut self,
        load: impl FnOnce() -> ReadAttempt<Arc<[u8]>>,
        process: impl FnOnce(Arc<[u8]>, Option<T>) -> FileResult<T>,
    ) -> ReadAttempt<Loaded<T>> {
        let attempt = load();
        let fingerprint = match &attempt.result {
            Ok(_) => SlotFingerprint::Successful(
                attempt
                    .reads
                    .as_ref()
                    .expect("successful resolver read has its byte evidence")
                    .evidence()
                    .clone(),
            ),
            Err(error) => SlotFingerprint::Failed(error.clone()),
        };

        if self.fingerprint.as_ref() == Some(&fingerprint)
            && let Some(data) = &self.data
        {
            return attempt.with_loaded_result(data.clone());
        }
        self.fingerprint = Some(fingerprint);

        let prev = self.data.take().and_then(Result::ok);
        let value = attempt
            .result
            .clone()
            .and_then(|bytes| process(bytes, prev));
        self.data = Some(value.clone());
        attempt.with_loaded_result(value)
    }
}

struct FileSlot {
    id: FileId,
    source: SlotCell<Source>,
    file: SlotCell<Bytes>,
}

impl FileSlot {
    const fn new(id: FileId) -> Self {
        Self {
            id,
            source: SlotCell::new(),
            file: SlotCell::new(),
        }
    }

    fn source_from_observation(
        &mut self,
        observation: ReadAttempt<Arc<[u8]>>,
    ) -> ReadAttempt<Loaded<Source>> {
        self.source.get_or_init(
            || observation,
            |data, prev| {
                let text = decode_utf8(&data)?;
                match prev {
                    Some(mut source) => {
                        source.replace(text);
                        Ok(source)
                    }
                    None => Ok(Source::new(self.id, text.into())),
                }
            },
        )
    }

    fn file_from_observation(
        &mut self,
        observation: ReadAttempt<Arc<[u8]>>,
    ) -> ReadAttempt<Loaded<Bytes>> {
        self.file
            .get_or_init(|| observation, |data, _| Ok(Bytes::new(data)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::file::{FileRead, ReadEvidence, ReadLocator, ReadOrigin};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn loaded_bytes(bytes: &[u8]) -> ReadAttempt<Arc<[u8]>> {
        let value: Arc<[u8]> = Arc::from(bytes);
        let evidence = ReadEvidence::new(ReadLocator::Root(PathBuf::from("test.typ")), &value);
        let read = FileRead::new(evidence, ReadOrigin::Provider);
        ReadAttempt {
            result: Ok(value),
            reads: Some(read),
            disk_reads: Vec::new(),
            package_checks: Vec::new(),
        }
    }

    #[test]
    fn unchanged_read_reuses_cached_value() {
        let mut slot: SlotCell<String> = SlotCell::new();

        let first = slot
            .get_or_init(
                || loaded_bytes(b"hello"),
                |data, _| Ok(String::from_utf8(data.as_ref().to_vec()).unwrap()),
            )
            .result
            .unwrap();
        assert_eq!(first.value, "hello");

        let repeated = slot
            .get_or_init(
                || loaded_bytes(b"hello"),
                |_, _| panic!("an unchanged read must reuse its processed value"),
            )
            .result
            .unwrap();
        assert_eq!(repeated.value, "hello");
        assert_eq!(repeated.read, first.read);

        let mut failed: SlotCell<Arc<[u8]>> = SlotCell::new();
        let error = FileError::NotFound(PathBuf::from("missing.typ"));
        let first = failed
            .get_or_init(
                || ReadAttempt::without_inputs(Err(error.clone())),
                |bytes, _| Ok(bytes),
            )
            .result
            .unwrap_err();
        let repeated = failed
            .get_or_init(
                || ReadAttempt::without_inputs(Err(error.clone())),
                |_, _| panic!("an unchanged failed read must reuse its cached failure"),
            )
            .result
            .unwrap_err();
        assert_eq!(first, error);
        assert_eq!(repeated, error);
    }

    #[test]
    fn same_file_id_under_roots_differs() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        fs::write(first.path().join("same.typ"), "= First").unwrap();
        fs::write(second.path().join("same.typ"), "= Second").unwrap();

        let id = crate::world::file::file_id("same.typ");
        let cache = SharedFileCache::new();
        let files = FileResolver::new();

        let first_source = cache
            .source_with_files(id, first.path(), &files)
            .result
            .unwrap();
        let second_source = cache
            .source_with_files(id, second.path(), &files)
            .result
            .unwrap();

        assert!(first_source.value.text().contains("First"));
        assert!(second_source.value.text().contains("Second"));
    }

    #[test]
    fn cache_retention_follows_max_age() {
        let root = Path::new("/site");
        let cache = SharedFileCache::new();

        for index in 0..100 {
            let id = crate::world::file::file_id(format!("generated/{index}.typ"));
            drop(cache.slot(root, id));
            cache.evict(3);
            assert!(cached_slots(&cache) <= 3);
        }

        let id = crate::world::file::file_id("template.typ");
        drop(cache.slot(root, id));
        cache.evict(2);
        for round in 0..2 {
            drop(cache.slot(root, id));
            cache.evict(2);
            assert!(caches(root, id, &cache), "round {round}");
        }
        cache.evict(2);
        assert!(caches(root, id, &cache));
        cache.evict(2);
        assert!(!caches(root, id, &cache));
    }

    #[test]
    fn zero_age_keeps_cloned_slots_alive() {
        let root = Path::new("/site");
        let cache = SharedFileCache::new();
        let id = crate::world::file::file_id("active.typ");
        let active = cache.slot(root, id);

        cache.evict(0);

        assert!(cache.roots.read().is_empty());
        assert_eq!(active.value.lock().id, id);
    }

    /// Number of file slots the cache currently owns for every root.
    fn cached_slots(cache: &SharedFileCache) -> usize {
        cache.roots.read().values().map(FxHashMap::len).sum()
    }

    /// Whether the cache holds a slot for `id` under `root`.
    fn caches(root: &Path, id: FileId, cache: &SharedFileCache) -> bool {
        cache
            .roots
            .read()
            .get(root)
            .is_some_and(|slots| slots.contains_key(&id))
    }
}
