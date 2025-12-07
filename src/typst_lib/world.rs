//! `SystemWorld` implementation - the core World trait.
//!
//! This module implements Typst's `World` trait, which provides the compilation
//! environment for Typst documents. The `SystemWorld` is a lightweight per-compilation
//! state that references globally shared resources (fonts, packages, library).
//!
//! # Architecture
//!
//! ```text
//! SystemWorld (per-compilation, ~lightweight)
//! ├── root: PathBuf          // Project root for path resolution
//! ├── main: FileId           // Entry point file ID
//! ├── fonts: &'static Fonts  // → Global shared fonts
//! ├── slots: FxHashMap       // Per-instance file cache
//! └── now: Now               // Lazy datetime
//!
//! World trait methods:
//! ├── library() → &GLOBAL_LIBRARY
//! ├── book()    → &fonts.book
//! ├── main()    → main FileId
//! ├── source()  → FileSlot cache
//! ├── file()    → FileSlot cache
//! ├── font()    → fonts.fonts[index]
//! └── today()   → Now (lazy UTC)
//! ```
//!
//! # Performance
//!
//! Creating a `SystemWorld` is cheap because:
//! - Fonts, packages, and library are globally shared (static references)
//! - Only the file slot cache (`FxHashMap`) is allocated per-instance
//! - Datetime is lazily computed on first access
//!
//! # Usage
//!
//! ```ignore
//! let world = SystemWorld::new(
//!     Path::new("/project/content/index.typ"),
//!     Path::new("/project"),
//! )?;
//!
//! // Compile the document
//! let document = typst::compile(&world).output?;
//! ```

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{DateTime, Datelike, FixedOffset, Local, Utc};
use typst::diag::FileResult;
use typst::foundations::{Bytes, Datetime};
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, World};
use typst_kit::fonts::Fonts;

use super::file::{FileSlot, GLOBAL_FILE_CACHE};
use super::font::get_fonts;
use super::library::GLOBAL_LIBRARY;

// =============================================================================
// DateTime handling
// =============================================================================

/// Lazy-captured datetime for consistent `World::today()` within a compilation.
///
/// The current time is captured on first access and reused for consistency.
struct LazyNow(OnceLock<DateTime<Utc>>);

// =============================================================================
// SystemWorld
// =============================================================================

/// A world that provides access to the operating system.
///
/// This struct is cheap to create because all expensive resources (fonts,
/// packages, library, file cache) are globally shared.
///
/// # Type Parameters
///
/// None - this is a concrete type, not generic.
///
/// # Thread Safety
///
/// `SystemWorld` is `Send + Sync` because all mutable state is behind
/// thread-safe locks in global statics.
pub struct SystemWorld {
    /// The root relative to which absolute paths are resolved.
    /// This is typically the project directory containing `tola.toml`.
    root: PathBuf,

    /// The input path (main entry point).
    /// This is the `FileId` of the file being compiled.
    main: FileId,

    /// Reference to global fonts (initialized on first use).
    /// This is a static reference to avoid allocation per compilation.
    fonts: &'static (Fonts, LazyHash<FontBook>),

    /// The current datetime if requested.
    /// Lazily initialized to ensure consistent time throughout compilation.
    now: LazyNow,
}

/// Normalize a path by canonicalizing it if possible, or returning it as-is.
fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

impl SystemWorld {
    /// Create a new world for compiling a specific file.
    ///
    /// This is cheap because fonts/packages/library/file-cache are globally shared.
    /// No per-instance allocation is needed.
    ///
    /// # Arguments
    ///
    /// * `entry_file` - Path to the `.typ` file to compile
    /// * `root_dir` - Project root directory for resolving imports
    ///
    /// # Returns
    ///
    /// A new `SystemWorld` ready for compilation.
    ///
    /// # Errors
    ///
    /// Currently infallible, but kept as separate constructor for clarity.
    pub fn new(entry_file: &Path, root_dir: &Path) -> Self {
        // Canonicalize root path for consistent path resolution
        let root = normalize_path(root_dir);

        // Resolve the virtual path of the main file within the project root.
        // Virtual paths are root-relative and use forward slashes.
        let entry_abs = normalize_path(entry_file);
        let virtual_path = VirtualPath::within_root(&entry_abs, &root)
            .unwrap_or_else(|| VirtualPath::new(entry_file.file_name().unwrap()));
        let main = FileId::new(None, virtual_path);

        // Get or initialize fonts with the project root as font path.
        // This allows projects to include custom fonts in their directory.
        let fonts = get_fonts(Some(&root));

        Self {
            root,
            main,
            fonts,
            now: LazyNow(OnceLock::new()),
        }
    }

    /// Access the canonical slot for the given file id from global cache.
    ///
    /// Creates a new slot if one doesn't exist. The callback receives
    /// mutable access to the slot for reading source/file data.
    ///
    /// # Arguments
    ///
    /// * `id` - The file ID to look up
    /// * `f` - Callback to execute with the file slot
    #[allow(clippy::unused_self)] // self kept for API consistency
    fn slot<F, T>(&self, id: FileId, f: F) -> T
    where
        F: FnOnce(&mut FileSlot) -> T,
    {
        let mut cache = GLOBAL_FILE_CACHE.write();
        f(cache.entry(id).or_insert_with(|| FileSlot::new(id)))
    }
}

/// Implementation of Typst's `World` trait.
///
/// This trait provides the compilation environment:
/// - Standard library access
/// - Font discovery
/// - File system access
/// - Package management (via file resolution)
/// - Current date/time
impl World for SystemWorld {
    /// Returns the standard library.
    ///
    /// Uses globally shared library with HTML feature enabled.
    fn library(&self) -> &LazyHash<Library> {
        &GLOBAL_LIBRARY
    }

    /// Returns the font book for font lookup.
    ///
    /// The font book indexes all available fonts for name-based lookup.
    fn book(&self) -> &LazyHash<FontBook> {
        &self.fonts.1
    }

    /// Returns the main source file ID.
    ///
    /// This is the entry point for compilation.
    fn main(&self) -> FileId {
        self.main
    }

    /// Load a source file by ID.
    ///
    /// Returns the parsed source code, using the file slot cache
    /// for incremental compilation.
    fn source(&self, id: FileId) -> FileResult<Source> {
        self.slot(id, |slot| slot.source(&self.root))
    }

    /// Load a file's raw bytes by ID.
    ///
    /// Used for binary files (images, etc.) that don't need parsing.
    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.slot(id, |slot| slot.file(&self.root))
    }

    /// Load a font by index.
    ///
    /// Fonts are indexed in the order they were discovered during
    /// font search. The index comes from font book lookups.
    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.0.fonts.get(index)?.get()
    }

    /// Get the current date.
    ///
    /// Returns the date at the time of first access within this compilation.
    /// The time is captured once and reused for consistency.
    ///
    /// # Arguments
    ///
    /// * `offset` - Optional UTC offset in hours. If `None`, uses local timezone.
    fn today(&self, offset: Option<i64>) -> Option<Datetime> {
        let now = self.now.0.get_or_init(Utc::now);

        // Apply timezone offset
        let with_offset = match offset {
            None => now.with_timezone(&Local).fixed_offset(),
            Some(hours) => {
                let seconds = i32::try_from(hours).ok()?.checked_mul(3600)?;
                now.with_timezone(&FixedOffset::east_opt(seconds)?)
            }
        };

        // Convert to Typst's Datetime type
        Datetime::from_ymd(
            with_offset.year(),
            with_offset.month().try_into().ok()?,
            with_offset.day().try_into().ok()?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_create_system_world() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.typ");
        fs::write(&file_path, "= Hello").unwrap();

        let _world = SystemWorld::new(&file_path, dir.path());
        // If we get here without panicking, the world was created successfully
    }

    #[test]
    fn test_world_library_access() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.typ");
        fs::write(&file_path, "= Hello").unwrap();

        let world = SystemWorld::new(&file_path, dir.path());
        let _lib = world.library();
        // Should not panic
    }

    #[test]
    fn test_world_book_access() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.typ");
        fs::write(&file_path, "= Hello").unwrap();

        let world = SystemWorld::new(&file_path, dir.path());
        let book = world.book();
        // Should have some fonts
        assert!(book.families().count() > 0);
    }

    #[test]
    fn test_world_main_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.typ");
        fs::write(&file_path, "= Hello").unwrap();

        let world = SystemWorld::new(&file_path, dir.path());
        let main = world.main();
        // Main file should have the correct virtual path
        assert!(main.vpath().as_rootless_path().ends_with("test.typ"));
    }

    #[test]
    fn test_world_source_loading() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.typ");
        fs::write(&file_path, "= Hello World").unwrap();

        let world = SystemWorld::new(&file_path, dir.path());
        let source = world.source(world.main());
        assert!(source.is_ok());
        assert!(source.unwrap().text().contains("Hello World"));
    }

    #[test]
    fn test_world_today() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.typ");
        fs::write(&file_path, "= Hello").unwrap();

        let world = SystemWorld::new(&file_path, dir.path());

        // Test with local timezone
        let today = world.today(None);
        assert!(today.is_some());

        // Test with UTC offset
        let today_utc = world.today(Some(0));
        assert!(today_utc.is_some());
    }

    #[test]
    fn test_world_font_access() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.typ");
        fs::write(&file_path, "= Hello").unwrap();

        let world = SystemWorld::new(&file_path, dir.path());

        // Should be able to access at least one font
        let font = world.font(0);
        // May be None in minimal environments, but shouldn't panic
        let _ = font;
    }
}
