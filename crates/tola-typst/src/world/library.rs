//! Shared Typst standard library with the `Html` and `Bundle` features enabled.
//!
//! [`GLOBAL_LIBRARY`] is initialized once and uses `LazyHash` for comemo caching.
//! Use [`create_library_with_inputs`] for compilation-specific `sys.inputs`.
//!
//! `context { target() }` reports the rendering target, including `"paged"`
//! inside `html.frame()`, but is not evaluated during eager scans. A caller-supplied
//! `sys.inputs.at("format")` is available during scans but does not change inside
//! frames. Use `target()` when a show rule must distinguish frame rendering.

use std::sync::LazyLock;

use typst::foundations::Dict;
use typst::utils::LazyHash;
use typst::{Feature, Features, Library, LibraryExt};

/// Lazily initialized standard library with HTML and Bundle support, without inputs.
pub static GLOBAL_LIBRARY: LazyLock<LazyHash<Library>> = LazyLock::new(|| {
    let library = Library::builder()
        .with_features(Features::from_iter([Feature::Html, Feature::Bundle]))
        .build();
    LazyHash::new(library)
});

/// Create a library with custom `sys.inputs`.
///
/// Creates a new library on each call. Its immutable input dictionary is shared
/// by the complete compilation world, including all Bundle child documents.
///
/// # Example
///
/// ```ignore
/// use std::sync::Arc;
/// use tola_typst::typst::foundations::{Dict, IntoValue};
/// use tola_typst::{BundleCancellation, FontStore, SharedFileCache, TypstWorld, create_library_with_inputs};
///
/// let mut inputs = Dict::new();
/// inputs.insert("title".into(), "My Document".into_value());
/// inputs.insert("author".into(), "Alice".into_value());
///
/// let library = create_library_with_inputs(inputs);
///
/// let cancellation = BundleCancellation::default();
/// let world = TypstWorld::builder(path, root)
///     .with_shared_library(Arc::new(library))
///     .with_shared_cache(Arc::new(SharedFileCache::new()))
///     .with_fonts(Arc::new(FontStore::new()))
///     .build(&cancellation)?;
/// ```
///
/// In your Typst document:
/// ```typst
/// #let title = sys.inputs.at("title", default: "Untitled")
/// #let author = sys.inputs.at("author", default: "Unknown")
///
/// = #title
/// by #author
/// ```
pub fn create_library_with_inputs(inputs: Dict) -> LazyHash<Library> {
    Library::builder()
        .with_inputs(inputs)
        .with_features(Features::from_iter([Feature::Html, Feature::Bundle]))
        .build()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The elements a site program calls by name come from the Bundle this library builds with.
    #[test]
    fn bundle_library_supplies_site_elements() {
        let library = create_library_with_inputs(Dict::new());
        let scope = library.global.scope();
        assert!(
            scope.get("document").is_some(),
            "document is missing from {:?}",
            scope
                .iter()
                .map(|(name, _)| name.as_str())
                .take(12)
                .collect::<Vec<_>>()
        );
    }
}
