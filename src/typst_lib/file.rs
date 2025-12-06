//! File slot caching for incremental compilation.
//!
//! This module provides efficient file caching with fingerprint-based invalidation.
//! Files are cached globally to enable reuse across multiple compilations.
//!
//! # Module Structure
//!
//! - [`virtual_id`] - Virtual file ID constants (stdin, empty)
//! - [`slot`] - Fingerprint-based caching primitive (`SlotCell`)
//! - [`reader`] - File reading utilities (disk, stdin, UTF-8)
//!
//! # Caching Strategy
//!
//! ```text
//! GLOBAL_FILE_CACHE (shared across all compilations)
//! └── FxHashMap<FileId, FileSlot>
//!     └── FileSlot
//!         ├── source: SlotCell<Source>  ─┐
//!         └── file: SlotCell<Bytes>     ─┼── Fingerprint-based invalidation
//!                                        │
//!     ┌──────────────────────────────────┘
//!     ▼
//! SlotCell<T>
//! ├── data: Option<FileResult<T>>   // Cached result
//! ├── fingerprint: u128             // Hash of raw file content
//! └── accessed: bool                // Fast path for same-compilation access
//!
//! Access Flow:
//! 1. If accessed=true && data.is_some() → return cached (fast path)
//! 2. Load file, compute fingerprint
//! 3. If fingerprint unchanged → return cached
//! 4. Otherwise → recompute and cache
//! ```

use std::path::Path;
use std::sync::LazyLock;

use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use typst::diag::FileResult;
use typst::foundations::Bytes;
use typst::syntax::{FileId, Source};

use reader::{decode_utf8, read};
use slot::SlotCell;

// =============================================================================
// Virtual File IDs
// =============================================================================

/// Virtual file ID constants for special input sources.
mod virtual_id {
    use std::sync::LazyLock;
    use typst::syntax::{FileId, VirtualPath};

    /// Static `FileId` for stdin input.
    pub static STDIN_ID: LazyLock<FileId> =
        LazyLock::new(|| FileId::new_fake(VirtualPath::new("<stdin>")));

    /// Static `FileId` for empty/no input.
    pub static EMPTY_ID: LazyLock<FileId> =
        LazyLock::new(|| FileId::new_fake(VirtualPath::new("<empty>")));
}

// =============================================================================
// Slot Cell - Fingerprint-based Caching Primitive
// =============================================================================

/// Fingerprint-based caching primitive.
mod slot {
    use std::mem;
    use typst::diag::FileResult;

    /// Lazily processes data for a file with fingerprint-based caching.
    ///
    /// Tracks:
    /// - Cached result (if any)
    /// - 128-bit fingerprint of raw file content
    /// - Whether slot was accessed in current compilation
    pub struct SlotCell<T> {
        data: Option<FileResult<T>>,
        fingerprint: u128,
        pub accessed: bool,
    }

    impl<T: Clone> SlotCell<T> {
        /// Create a new empty slot cell.
        pub fn new() -> Self {
            Self {
                data: None,
                fingerprint: 0,
                accessed: false,
            }
        }

        /// Reset access flag for a new compilation.
        pub fn reset_access(&mut self) {
            self.accessed = false;
        }

        /// Get or initialize cached data using fingerprint-based invalidation.
        ///
        /// # Algorithm
        /// 1. Fast path: if accessed && data exists → return cached
        /// 2. Load raw content, compute fingerprint
        /// 3. If fingerprint unchanged → return cached
        /// 4. Otherwise → process and cache new data
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
}

// =============================================================================
// File Reader - Reading Utilities
// =============================================================================

/// File reading utilities for various sources.
mod reader {
    use std::fs;
    use std::io::{self, Read};
    use std::path::Path;

    use typst::diag::{FileError, FileResult};
    use typst::syntax::FileId;
    use typst_kit::download::{DownloadState, Progress};

    use super::virtual_id::{EMPTY_ID, STDIN_ID};
    use crate::typst_lib::package::GLOBAL_PACKAGE_STORAGE;

    /// Default typst.toml content for projects without one.
    const DEFAULT_TYPST_TOML: &[u8] =
        b"[package]\nname = \"tola-project\"\nversion = \"0.0.0\"\nentrypoint = \"content/index.typ\"";

    /// Read file content from a `FileId`.
    ///
    /// Handles special cases:
    /// - `EMPTY_ID`: Returns empty bytes
    /// - `STDIN_ID`: Reads from stdin
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
    pub fn read_disk(path: &Path) -> FileResult<Vec<u8>> {
        let map_err = |e| FileError::from_io(e, path);
        fs::metadata(path).map_err(map_err).and_then(|m| {
            if m.is_dir() {
                Err(FileError::IsDirectory)
            } else {
                fs::read(path).map_err(map_err)
            }
        })
    }

    /// Read all data from stdin, handling BrokenPipe gracefully.
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
}

// =============================================================================
// Global File Cache
// =============================================================================

/// Global shared file cache - reused across all compilations.
///
/// Template files, common imports, etc. are read once and reused.
pub static GLOBAL_FILE_CACHE: LazyLock<RwLock<FxHashMap<FileId, FileSlot>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

/// Reset access flags for all cached files.
///
/// Call at compilation start to enable fingerprint re-checking.
pub fn reset_access_flags() {
    for slot in GLOBAL_FILE_CACHE.write().values_mut() {
        slot.reset_access();
    }
}

// =============================================================================
// FileSlot - Per-file Caching
// =============================================================================

/// Holds cached data for a file ID.
///
/// Each file has two representations:
/// - `source`: Parsed source code (for `.typ` files)
/// - `file`: Raw bytes (for binary files)
pub struct FileSlot {
    id: FileId,
    source: SlotCell<Source>,
    file: SlotCell<Bytes>,
}

impl FileSlot {
    /// Create a new empty file slot.
    pub fn new(id: FileId) -> Self {
        Self {
            id,
            source: SlotCell::new(),
            file: SlotCell::new(),
        }
    }

    /// Reset access flags for a new compilation.
    pub fn reset_access(&mut self) {
        self.source.reset_access();
        self.file.reset_access();
    }

    /// Retrieve parsed source for this file.
    pub fn source(&mut self, project_root: &Path) -> FileResult<Source> {
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
    use typst::syntax::VirtualPath;

    mod decode_utf8_tests {
        use super::*;

        #[test]
        fn valid_utf8() {
            let text = "Hello, 世界!";
            assert_eq!(decode_utf8(text.as_bytes()).unwrap(), text);
        }

        #[test]
        fn strips_bom() {
            let mut bytes = vec![0xef, 0xbb, 0xbf];
            bytes.extend_from_slice(b"Hello");
            assert_eq!(decode_utf8(&bytes).unwrap(), "Hello");
        }

        #[test]
        fn invalid_utf8_errors() {
            let invalid = vec![0xff, 0xfe];
            assert!(decode_utf8(&invalid).is_err());
        }
    }

    mod read_disk_tests {
        use super::*;

        #[test]
        fn reads_file() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("test.txt");
            fs::write(&path, "test content").unwrap();

            assert_eq!(reader::read_disk(&path).unwrap(), b"test content");
        }

        #[test]
        fn directory_errors() {
            let dir = TempDir::new().unwrap();
            assert!(reader::read_disk(dir.path()).is_err());
        }

        #[test]
        fn nonexistent_errors() {
            assert!(reader::read_disk(Path::new("/nonexistent/file.txt")).is_err());
        }
    }

    mod slot_cell_tests {
        use super::*;

        #[test]
        fn fingerprint_caching() {
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
    }

    mod file_slot_tests {
        use super::*;

        #[test]
        fn caches_file() {
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
    }

    mod virtual_id_tests {
        use super::*;
        use super::virtual_id::EMPTY_ID;

        #[test]
        fn empty_id_returns_empty() {
            let dir = TempDir::new().unwrap();
            let result = read(*EMPTY_ID, dir.path());
            assert!(result.is_ok());
            assert!(result.unwrap().is_empty());
        }
    }
}
