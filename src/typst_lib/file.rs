//! File caching with fingerprint-based invalidation.
//!
//! Files are cached globally to enable reuse across compilations.
//! Fingerprint-based invalidation ensures changed files are re-read.
//!
//! # Caching Strategy
//!
//! ```text
//! GLOBAL_FILE_CACHE (shared across all compilations)
//! └── FxHashMap<FileId, FileSlot>
//!     └── FileSlot
//!         ├── source: SlotCell<Source>  ─┐
//!         └── file: SlotCell<Bytes>     ─┼── Fingerprint-based invalidation
//!
//! Access Flow:
//! 1. If accessed=true && data.is_some() → return cached (fast path)
//! 2. Load file, compute fingerprint
//! 3. If fingerprint unchanged → return cached
//! 4. Otherwise → recompute and cache
//! ```

use std::fs;
use std::io::{self, Read};
use std::mem;
use std::path::Path;
use std::sync::LazyLock;
use std::cell::RefCell;

use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use typst::diag::{FileError, FileResult};
use typst::foundations::Bytes;
use typst::syntax::{FileId, Source, VirtualPath};
use typst_kit::download::{DownloadState, Progress};

use super::package::GLOBAL_PACKAGE_STORAGE;
use crate::data::{is_virtual_data_path, read_virtual_data};

// =============================================================================
// Constants
// =============================================================================

/// Virtual `FileId` for stdin input.
pub static STDIN_ID: LazyLock<FileId> =
    LazyLock::new(|| FileId::new_fake(VirtualPath::new("<stdin>")));

/// Virtual `FileId` for empty/no input.
pub static EMPTY_ID: LazyLock<FileId> =
    LazyLock::new(|| FileId::new_fake(VirtualPath::new("<empty>")));

/// Default typst.toml content for projects without one.
const DEFAULT_TYPST_TOML: &[u8] =
    b"[package]\nname = \"tola-project\"\nversion = \"0.0.0\"\nentrypoint = \"content/index.typ\"";

// =============================================================================
// Global File Cache
// =============================================================================

/// Global shared file cache - reused across all compilations.
pub static GLOBAL_FILE_CACHE: LazyLock<RwLock<FxHashMap<FileId, FileSlot>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

// =============================================================================
// Thread-Local Access Tracking
// =============================================================================

thread_local! {
    /// Thread-local set of accessed file IDs for the current compilation.
    /// This avoids race conditions when compiling files in parallel.
    static ACCESSED_FILES: RefCell<rustc_hash::FxHashSet<FileId>> =
        RefCell::new(rustc_hash::FxHashSet::default());
}

/// Clear the thread-local accessed files set and reset global cache access flags.
///
/// Call at the start of each file compilation.
pub fn reset_access_flags() {
    // Reset thread-local tracking
    ACCESSED_FILES.with(|files| files.borrow_mut().clear());

    // Reset global cache access flags for fingerprint re-checking
    for slot in GLOBAL_FILE_CACHE.write().values_mut() {
        slot.reset_access();
    }
}

/// Record a file access in the thread-local set.
pub fn record_file_access(id: FileId) {
    ACCESSED_FILES.with(|files| {
        files.borrow_mut().insert(id);
    });
}

/// Get all files accessed during the current compilation.
///
/// Returns a list of `FileId`s that were accessed since last `reset_access_flags()`.
/// Thread-safe: each thread has its own tracking.
pub fn get_accessed_files() -> Vec<FileId> {
    ACCESSED_FILES.with(|files| files.borrow().iter().copied().collect())
}

// =============================================================================
// SlotCell - Fingerprint-based Caching
// =============================================================================

/// Lazily processes data for a file with fingerprint-based caching.
pub struct SlotCell<T> {
    data: Option<FileResult<T>>,
    fingerprint: u128,
    pub accessed: bool,
}

impl<T: Clone> SlotCell<T> {
    pub const fn new() -> Self {
        Self {
            data: None,
            fingerprint: 0,
            accessed: false,
        }
    }

    pub const fn reset_access(&mut self) {
        self.accessed = false;
    }

    /// Get or initialize cached data using fingerprint-based invalidation.
    pub fn get_or_init(
        &mut self,
        load: impl FnOnce() -> FileResult<Vec<u8>>,
        process: impl FnOnce(Vec<u8>, Option<T>) -> FileResult<T>,
    ) -> FileResult<T> {
        // Fast path: already accessed in this compilation
        if mem::replace(&mut self.accessed, true)
            && let Some(data) = &self.data
        {
            return data.clone();
        }

        let result = load();
        let fingerprint = typst::utils::hash128(&result);

        // Fingerprint unchanged: reuse previous result
        if mem::replace(&mut self.fingerprint, fingerprint) == fingerprint
            && let Some(data) = &self.data
        {
            return data.clone();
        }

        // Process and cache new data
        let prev = self.data.take().and_then(Result::ok);
        let value = result.and_then(|data| process(data, prev));
        self.data = Some(value.clone());
        value
    }
}

// =============================================================================
// File Reading
// =============================================================================

/// Read file content from a `FileId`.
///
/// Handles special cases:
/// - `EMPTY_ID`: Returns empty bytes
/// - `STDIN_ID`: Reads from stdin
/// - Virtual data files (`/_data/*.json`): Returns dynamically generated JSON
/// - Package files: Downloads package if needed
/// - Missing `typst.toml`: Generates default
pub fn read(id: FileId, project_root: &Path) -> FileResult<Vec<u8>> {
    // Handle virtual file IDs
    if id == *EMPTY_ID {
        return Ok(Vec::new());
    }
    if id == *STDIN_ID {
        return read_stdin();
    }

    // Handle virtual data files (/_data/*.json)
    // These are generated dynamically from GLOBAL_SITE_DATA
    // Record access for dependency tracking (so we know which pages use virtual data)
    let vpath = id.vpath().as_rooted_path();
    if is_virtual_data_path(vpath) {
        record_file_access(id);
        return read_virtual_data(vpath)
            .ok_or_else(|| FileError::NotFound(vpath.to_path_buf()));
    }

    // Resolve path with typst.toml fallback
    let path = resolve_path(project_root, id).or_else(|e| {
        id.vpath()
            .resolve(project_root)
            .filter(|p| p.ends_with("typst.toml") && !p.exists())
            .ok_or(e)
    })?;

    // Generate default typst.toml if missing
    if path.ends_with("typst.toml") && !path.exists() {
        return Ok(DEFAULT_TYPST_TOML.to_vec());
    }

    read_disk(&path)
}

/// Decode bytes as UTF-8, stripping BOM if present.
pub fn decode_utf8(buf: &[u8]) -> FileResult<&str> {
    let buf = buf.strip_prefix(b"\xef\xbb\xbf").unwrap_or(buf);
    std::str::from_utf8(buf).map_err(|_| FileError::InvalidUtf8)
}

/// Resolve file path, downloading package if needed.
fn resolve_path(project_root: &Path, id: FileId) -> FileResult<std::path::PathBuf> {
    let root = id
        .package()
        .map(|spec| GLOBAL_PACKAGE_STORAGE.prepare_package(spec, &mut SilentProgress))
        .transpose()?
        .unwrap_or_else(|| project_root.to_path_buf());

    id.vpath().resolve(&root).ok_or(FileError::AccessDenied)
}

/// Read file from disk.
fn read_disk(path: &Path) -> FileResult<Vec<u8>> {
    let map_err = |e| FileError::from_io(e, path);
    fs::metadata(path).map_err(map_err).and_then(|m| {
        if m.is_dir() {
            Err(FileError::IsDirectory)
        } else {
            fs::read(path).map_err(map_err)
        }
    })
}

/// Read all data from stdin.
fn read_stdin() -> FileResult<Vec<u8>> {
    let mut buf = Vec::new();
    io::stdin()
        .read_to_end(&mut buf)
        .or_else(|e| {
            if e.kind() == io::ErrorKind::BrokenPipe {
                Ok(0)
            } else {
                Err(FileError::from_io(e, Path::new("<stdin>")))
            }
        })?;
    Ok(buf)
}

/// No-op progress reporter for silent package downloads.
struct SilentProgress;

impl Progress for SilentProgress {
    fn print_start(&mut self) {}
    fn print_progress(&mut self, _: &DownloadState) {}
    fn print_finish(&mut self, _: &DownloadState) {}
}

// =============================================================================
// FileSlot - Per-file Caching
// =============================================================================

/// Holds cached data for a file ID.
pub struct FileSlot {
    id: FileId,
    source: SlotCell<Source>,
    file: SlotCell<Bytes>,
}

impl FileSlot {
    pub const fn new(id: FileId) -> Self {
        Self {
            id,
            source: SlotCell::new(),
            file: SlotCell::new(),
        }
    }

    pub const fn reset_access(&mut self) {
        self.source.reset_access();
        self.file.reset_access();
    }

    /// Retrieve parsed source for this file.
    pub fn source(&mut self, project_root: &Path) -> FileResult<Source> {
        record_file_access(self.id);
        self.source.get_or_init(
            || read(self.id, project_root),
            |data, prev| {
                let text = decode_utf8(&data)?;
                match prev {
                    Some(mut src) => {
                        src.replace(text);
                        Ok(src)
                    }
                    None => Ok(Source::new(self.id, text.into())),
                }
            },
        )
    }

    /// Retrieve raw bytes for this file.
    pub fn file(&mut self, project_root: &Path) -> FileResult<Bytes> {
        record_file_access(self.id);
        self.file
            .get_or_init(|| read(self.id, project_root), |data, _| Ok(Bytes::new(data)))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_decode_utf8_valid() {
        let text = "Hello, 世界!";
        assert_eq!(decode_utf8(text.as_bytes()).unwrap(), text);
    }

    #[test]
    fn test_decode_utf8_strips_bom() {
        let mut bytes = vec![0xef, 0xbb, 0xbf];
        bytes.extend_from_slice(b"Hello");
        assert_eq!(decode_utf8(&bytes).unwrap(), "Hello");
    }

    #[test]
    fn test_decode_utf8_invalid() {
        let invalid = vec![0xff, 0xfe];
        assert!(decode_utf8(&invalid).is_err());
    }

    #[test]
    fn test_read_disk() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "test content").unwrap();

        assert_eq!(read_disk(&path).unwrap(), b"test content");
    }

    #[test]
    fn test_read_disk_directory() {
        let dir = TempDir::new().unwrap();
        assert!(read_disk(dir.path()).is_err());
    }

    #[test]
    fn test_read_disk_nonexistent() {
        assert!(read_disk(Path::new("/nonexistent/file.txt")).is_err());
    }

    #[test]
    fn test_slot_cell_fingerprint() {
        let mut slot: SlotCell<String> = SlotCell::new();

        let result1 = slot.get_or_init(
            || Ok(b"hello".to_vec()),
            |data, _| Ok(String::from_utf8(data).unwrap()),
        );
        assert_eq!(result1.unwrap(), "hello");

        slot.accessed = false;
        let result2 = slot.get_or_init(
            || Ok(b"hello".to_vec()),
            |_, _| panic!("Should not reprocess"),
        );
        assert_eq!(result2.unwrap(), "hello");
    }

    #[test]
    fn test_file_slot_caching() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.typ");
        fs::write(&path, "= Hello").unwrap();

        let vpath = VirtualPath::new("test.typ");
        let id = FileId::new(None, vpath);
        let mut slot = FileSlot::new(id);

        let result1 = slot.file(dir.path());
        let result2 = slot.file(dir.path());

        assert!(result1.is_ok());
        assert_eq!(result1.unwrap(), result2.unwrap());
    }

    #[test]
    fn test_empty_id() {
        let dir = TempDir::new().unwrap();
        let result = read(*EMPTY_ID, dir.path());
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
