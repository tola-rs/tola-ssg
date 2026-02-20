//! Page route - source to output path mapping.

use std::path::PathBuf;

use crate::core::UrlPath;

/// Source -> output path mapping for a page
///
/// Contains all path information needed for:
/// - Link resolution (knowing where the page lives)
/// - Asset copying (knowing source and output directories)
/// - URL generation (knowing the permalink)
///
/// # Example
///
/// ```text
/// Source: content/posts/hello.typ
/// Output: public/blog/posts/hello/index.html
///
/// PageRoute {
///     source:      content/posts/hello.typ
///     is_index:    false
///     permalink:   /blog/posts/hello/
///     output_file: public/blog/posts/hello/index.html
///     output_dir:  public/blog/posts/hello/
///     full_url:    https://example.com/blog/posts/hello/
/// }
/// ```
///
/// # Content Assets
///
/// Non-content files in the content directory are automatically copied
/// to the corresponding output location:
///
/// ```text
/// content/posts/
/// ├── hello.typ           -> public/posts/hello/index.html
/// └── hello/
///     └── image.png       -> public/posts/hello/image.png
/// ```
#[derive(Debug, Clone, Default)]
pub struct PageRoute {
    // === Source ===
    /// Source file path (e.g., content/posts/hello.typ)
    pub source: PathBuf,
    /// Whether this is an index file (index.typ or index.md)
    pub is_index: bool,
    /// Whether this is the 404 page (configured via `build.not_found`)
    pub is_404: bool,

    // === Output ===
    /// URL path / permalink (e.g., /blog/posts/hello/)
    pub permalink: UrlPath,
    /// Output HTML file (e.g., public/blog/posts/hello/index.html)
    pub output_file: PathBuf,
    /// Output directory (e.g., public/blog/posts/hello/)
    pub output_dir: PathBuf,

    // === URLs ===
    /// Full URL including base (e.g., https://example.com/blog/posts/hello/)
    pub full_url: String,

    // === Legacy (for compatibility, consider removing) ===
    /// Relative path without extension (for logging)
    pub relative: String,
}
