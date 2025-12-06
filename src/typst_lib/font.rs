//! Global shared font management.
//!
//! Fonts are expensive to load (~100ms+), so we load them once at startup
//! and share across all compilations via `OnceLock`.
//!
//! # Design Rationale
//!
//! Font loading involves:
//! 1. Scanning system font directories (platform-specific)
//! 2. Parsing font metadata (family, weight, style, etc.)
//! 3. Building a searchable font book index
//!
//! This is done once and shared via `OnceLock` to ensure:
//! - Single initialization (first caller wins)
//! - Zero-cost subsequent access (just a pointer dereference)
//! - Thread-safe sharing across compilations
//!
//! # Font Sources
//!
//! Fonts are searched in order:
//! 1. Custom paths provided at initialization (e.g., project fonts)
//! 2. System fonts (if enabled)
//!
//! # Usage
//!
//! ```ignore
//! // Initialize with project fonts
//! let fonts = get_fonts(Some(Path::new("/project/fonts")));
//!
//! // Access font book
//! let book: &FontBook = &fonts.1;
//!
//! // Get font by index
//! if let Some(font) = fonts.0.fonts.get(0) {
//!     let font: Font = font.get()?;
//! }
//! ```

use std::path::Path;
use std::sync::OnceLock;

use typst::text::FontBook;
use typst::utils::LazyHash;
use typst_kit::fonts::Fonts;

/// Global shared fonts - initialized once with custom font paths.
///
/// Uses `OnceLock` for thread-safe, one-time initialization.
/// The first call to `get_fonts` determines the font paths for all
/// subsequent compilations.
static GLOBAL_FONTS: OnceLock<(Fonts, LazyHash<FontBook>)> = OnceLock::new();

/// Initialize fonts with custom font paths.
///
/// # Arguments
///
/// * `font_paths` - Additional directories to search for fonts
///
/// # Returns
///
/// A tuple of:
/// - `Fonts`: The font collection with lazy-loaded font data
/// - `LazyHash<FontBook>`: The font book index wrapped for comemo caching
fn init_fonts(font_paths: &[&Path]) -> (Fonts, LazyHash<FontBook>) {
    let mut searcher = Fonts::searcher();
    // Include system fonts (platform-specific locations)
    searcher.include_system_fonts(true);
    // Search custom paths and system fonts
    let fonts = searcher.search_with(font_paths);
    // Wrap font book in LazyHash for comemo caching
    let book = LazyHash::new(fonts.book.clone());
    (fonts, book)
}

/// Get or initialize global fonts.
///
/// The first call determines the font paths used for all subsequent compilations.
/// This is intentional: fonts rarely change during a program's lifetime, and
/// sharing them saves ~100ms per compilation.
///
/// # Arguments
///
/// * `font_path` - Optional project root to include project-specific fonts.
///   Pass `Some(root)` on the first call to include fonts from `{root}/fonts/`.
///
/// # Returns
///
/// A static reference to the shared font collection and book.
///
/// # Thread Safety
///
/// This function is thread-safe. If called concurrently, only one thread
/// performs initialization; others wait and receive the shared result.
pub fn get_fonts(font_path: Option<&Path>) -> &'static (Fonts, LazyHash<FontBook>) {
    GLOBAL_FONTS.get_or_init(|| {
        font_path.map_or_else(|| init_fonts(&[]), |path| init_fonts(&[path]))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_fonts_initialized() {
        let fonts = get_fonts(None);
        // Should find at least some system fonts on most systems
        // Note: This test may fail in minimal container environments
        assert!(!fonts.0.fonts.is_empty(), "Should find system fonts");
    }

    #[test]
    fn test_font_book_not_empty() {
        let fonts = get_fonts(None);
        // FontBook should have indexed the fonts
        assert!(fonts.1.families().count() > 0, "Font book should have families");
    }

    #[test]
    fn test_fonts_are_shared() {
        let fonts1 = get_fonts(None);
        let fonts2 = get_fonts(None);
        // Should return the same static reference
        assert!(std::ptr::eq(fonts1, fonts2), "Fonts should be shared");
    }

    #[test]
    fn test_subsequent_calls_ignore_path() {
        // First call initializes (may have been done by other tests)
        let fonts1 = get_fonts(None);
        // Second call with different path should return same fonts
        let fonts2 = get_fonts(Some(Path::new("/nonexistent")));
        assert!(std::ptr::eq(fonts1, fonts2), "Path ignored after initialization");
    }
}
