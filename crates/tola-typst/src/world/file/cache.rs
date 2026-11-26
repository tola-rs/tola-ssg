//! File caching with fingerprint-based invalidation.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::{Mutex, RwLock};
use rustc_hash::FxHashMap;
use typst::diag::{FileError, FileResult};
use typst::foundations::Bytes;
use typst::syntax::{FileId, Source};

use super::read::decode_utf8;
use super::reads::{Loaded, ReadAttempt};

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

    pub(crate) fn load_source(
        &self,
        fid: FileId,
        root: &Path,
        attempt: ReadAttempt<Bytes>,
    ) -> ReadAttempt<Loaded<Source>> {
        self.with_slot(root, fid, |slot| slot.load_source(attempt))
    }

    pub(crate) fn load_file(
        &self,
        fid: FileId,
        root: &Path,
        attempt: ReadAttempt<Bytes>,
    ) -> ReadAttempt<Loaded<Bytes>> {
        self.with_slot(root, fid, |slot| slot.load_file(attempt))
    }

    fn with_slot<T>(
        &self,
        root: &Path,
        fid: FileId,
        use_slot: impl FnOnce(&mut FileSlot) -> T,
    ) -> T {
        let slot = self.slot(root, fid);
        let mut value = slot.value.lock();
        use_slot(&mut value)
    }

    /// Number of file slots owned by the cache, across all compilation roots.
    ///
    /// Sources returned to callers can outlive evicted slots.
    pub fn retained_slots(&self) -> usize {
        self.roots.read().values().map(FxHashMap::len).sum()
    }

    fn slot(&self, root: &Path, fid: FileId) -> Arc<SharedFileSlot> {
        let epoch = self.epoch.load(Ordering::Relaxed);
        if let Some(slot) = self
            .roots
            .read()
            .get(root)
            .and_then(|slots| slots.get(&fid))
        {
            slot.touch(epoch);
            return Arc::clone(slot);
        }

        let mut roots = self.roots.write();
        let slot = roots
            .entry(Arc::from(root))
            .or_default()
            .entry(fid)
            .or_insert_with(|| Arc::new(SharedFileSlot::new(fid, epoch)));
        slot.touch(epoch);
        Arc::clone(slot)
    }
}

struct SharedFileSlot {
    last_used: AtomicU64,
    value: Mutex<FileSlot>,
}

impl SharedFileSlot {
    fn new(fid: FileId, epoch: u64) -> Self {
        Self {
            last_used: AtomicU64::new(epoch),
            value: Mutex::new(FileSlot::new(fid)),
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
    Successful(super::reads::ReadRecord),
    Failed(FileError),
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
        attempt: ReadAttempt<Bytes>,
        decode: impl FnOnce(Bytes, Option<T>) -> FileResult<T>,
    ) -> ReadAttempt<Loaded<T>> {
        let fp = match &attempt.result {
            Ok(_) => SlotFingerprint::Successful(
                attempt
                    .read
                    .as_ref()
                    .expect("successful resolver read has its byte evidence")
                    .evidence()
                    .clone(),
            ),
            Err(err) => SlotFingerprint::Failed(err.clone()),
        };

        if self.fingerprint.as_ref() == Some(&fp)
            && let Some(data) = &self.data
        {
            return attempt.with_loaded(data.clone());
        }
        self.fingerprint = Some(fp);

        let prev = self.data.take().and_then(Result::ok);
        let value = attempt.result.clone().and_then(|buf| decode(buf, prev));
        self.data = Some(value.clone());
        attempt.with_loaded(value)
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

    fn load_source(&mut self, attempt: ReadAttempt<Bytes>) -> ReadAttempt<Loaded<Source>> {
        self.source.get_or_init(attempt, |buf, prev| {
            let text = decode_utf8(&buf)?;
            match prev {
                Some(mut src) => {
                    src.replace(text);
                    Ok(src)
                }
                None => Ok(Source::new(self.id, text.into())),
            }
        })
    }

    fn load_file(&mut self, attempt: ReadAttempt<Bytes>) -> ReadAttempt<Loaded<Bytes>> {
        self.file.get_or_init(attempt, |buf, _| Ok(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::world::file::{FileRead, ReadLocator, ReadOrigin, ReadRecord};

    use std::path::PathBuf;

    fn loaded_bytes(bytes: &[u8]) -> ReadAttempt<Bytes> {
        let buf = Bytes::new(bytes.to_vec());
        let evidence = ReadRecord::new(ReadLocator::Root(PathBuf::from("test.typ")), &buf);
        ReadAttempt {
            result: Ok(buf),
            read: Some(FileRead::new(evidence, ReadOrigin::Provider)),
            disk_reads: Vec::new(),
            package_checks: Vec::new(),
        }
    }

    #[test]
    fn slot_reuses_unchanged_read() {
        let mut slot: SlotCell<String> = SlotCell::new();

        let first = slot
            .get_or_init(loaded_bytes(b"hello"), |buf, _| {
                Ok(String::from_utf8(buf.as_ref().to_vec()).unwrap())
            })
            .result
            .unwrap();
        assert_eq!(first.value, "hello");

        let repeated = slot
            .get_or_init(loaded_bytes(b"hello"), |_, _| {
                panic!("an unchanged read must reuse its processed value")
            })
            .result
            .unwrap();
        assert_eq!(repeated.value, "hello");
        assert_eq!(repeated.read, first.read);

        let mut failed: SlotCell<Bytes> = SlotCell::new();
        let err = FileError::NotFound(PathBuf::from("missing.typ"));
        let first = failed
            .get_or_init(ReadAttempt::bare(Err(err.clone())), |buf, _| Ok(buf))
            .result
            .unwrap_err();
        let repeated = failed
            .get_or_init(ReadAttempt::bare(Err(err.clone())), |_, _| {
                panic!("an unchanged failed read must reuse its cached failure")
            })
            .result
            .unwrap_err();
        assert_eq!(first, err);
        assert_eq!(repeated, err);
    }

    #[test]
    fn cache_evict_max_age() {
        let root = Path::new("/site");
        let cache = SharedFileCache::new();

        for idx in 0..100 {
            let fid = crate::world::file::file_id(format!("generated/{idx}.typ"));
            drop(cache.slot(root, fid));
            cache.evict(3);
            assert!(cache.retained_slots() <= 3);
        }

        let fid = crate::world::file::file_id("template.typ");
        drop(cache.slot(root, fid));
        cache.evict(2);
        for round in 0..2 {
            drop(cache.slot(root, fid));
            cache.evict(2);
            assert!(is_cached(root, fid, &cache), "round {round}");
        }
        cache.evict(2);
        assert!(is_cached(root, fid, &cache));
        cache.evict(2);
        assert!(!is_cached(root, fid, &cache));
    }

    fn is_cached(root: &Path, fid: FileId, cache: &SharedFileCache) -> bool {
        cache
            .roots
            .read()
            .get(root)
            .is_some_and(|slots| slots.contains_key(&fid))
    }
}
