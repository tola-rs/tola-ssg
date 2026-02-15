//! AddressSpace - the core bidirectional mapping between sources and URLs.
//!
//! This is the single source of truth for all addressable resources in the site.

use rustc_hash::{FxHashMap, FxHashSet};
use std::path::{Path, PathBuf};

use crate::asset::AssetRoute;
use crate::config::SlugConfig;
use crate::core::{LinkKind, UrlPath};
use crate::page::PageRoute;
use crate::utils::path::route::split_path_fragment;
use crate::utils::path::slug::slugify_path;

use super::resolve::{resolve_physical_path, resolve_relative_url};
use super::{ResolveContext, ResolveResult, Resource};

/// Result of updating a page's permalink during hot-reload.
///
/// This is a lightweight result type for single-page permalink changes,
/// complementing the batch `conflict::detect_conflicts` used during build.
#[derive(Debug, Clone)]
pub enum PermalinkUpdate {
    /// Permalink unchanged.
    Unchanged,
    /// Permalink changed successfully.
    Changed {
        /// The old URL before the change.
        old_url: UrlPath,
    },
    /// New permalink conflicts with an existing resource.
    Conflict {
        /// The conflicting URL.
        url: UrlPath,
        /// Source path of the resource that already owns this URL.
        existing_source: PathBuf,
    },
}

/// Site address space - bidirectional mapping between sources and URLs.
///
/// This is the single source of truth for all addressable resources in the site.
/// Build it once after metadata collection, then use it for O(1) link validation.
#[derive(Debug, Default)]
pub struct AddressSpace {
    /// URL -> Resource mapping
    pub(super) by_url: FxHashMap<UrlPath, Resource>,
    /// Source path -> URL mapping (for reverse lookup)
    pub(super) by_source: FxHashMap<PathBuf, UrlPath>,
    /// Page URL -> heading IDs (for fragment validation)
    headings: FxHashMap<UrlPath, FxHashSet<String>>,
    /// Assets directory prefix (e.g., "assets")
    assets_prefix: String,
    /// Slug configuration for URL normalization
    slug_config: Option<SlugConfig>,
}

impl AddressSpace {
    /// Create a new empty address space.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all data from the address space.
    pub fn clear(&mut self) {
        self.by_url.clear();
        self.by_source.clear();
        self.headings.clear();
        self.assets_prefix.clear();
        self.slug_config = None;
    }

    /// Set the assets directory prefix.
    pub fn with_assets_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.assets_prefix = prefix.into();
        self
    }

    /// Get the assets directory prefix.
    pub fn assets_prefix(&self) -> &str {
        &self.assets_prefix
    }

    /// Set the assets directory prefix.
    pub fn set_assets_prefix(&mut self, prefix: impl Into<String>) {
        self.assets_prefix = prefix.into();
    }

    /// Set the slug configuration for URL normalization.
    pub fn set_slug_config(&mut self, config: SlugConfig) {
        self.slug_config = Some(config);
    }

    /// Register a page in the address space.
    pub fn register_page(&mut self, route: PageRoute, title: Option<String>) {
        let permalink = route.permalink.clone();
        let source = route.source.clone();
        let resource = Resource::Page { route, title };
        self.by_url.insert(permalink.clone(), resource);
        self.by_source.insert(source, permalink);
    }

    /// Register an asset in the address space.
    pub fn register_asset(&mut self, route: AssetRoute) {
        let url = route.url.clone();
        let source = route.source.clone();
        let resource = Resource::Asset { route };
        self.by_url.insert(url.clone(), resource);
        self.by_source.insert(source, url);
    }

    /// Register heading IDs for a page.
    pub fn register_headings(
        &mut self,
        permalink: &UrlPath,
        ids: impl IntoIterator<Item = String>,
    ) {
        self.headings
            .entry(permalink.clone())
            .or_default()
            .extend(ids);
    }

    /// Register a single heading ID for a page.
    pub fn register_heading(&mut self, permalink: &UrlPath, id: String) {
        self.headings
            .entry(permalink.clone())
            .or_default()
            .insert(id);
    }

    /// Remove a URL entry and its associated data.
    ///
    /// This is a low-level operation. Use `update_source_url` for permalink change handling.
    pub fn remove_url(&mut self, url: &UrlPath) {
        self.by_url.remove(url);
        self.headings.remove(url);
    }

    /// Set source -> URL mapping (low-level, no change detection).
    ///
    /// This is a low-level operation. Use `update_source_url` for permalink change handling.
    pub fn set_source_url(&mut self, source: PathBuf, url: UrlPath) {
        self.by_source.insert(source, url);
    }

    /// Update a page's URL mapping with full Resource data.
    ///
    /// Returns the old URL if the permalink changed, None otherwise.
    /// Properly cleans up old entries when permalink changes.
    ///
    /// Use this during build when full PageRoute is available.
    pub fn update_page(&mut self, route: PageRoute, title: Option<String>) -> Option<UrlPath> {
        let old_url = self.detect_permalink_change(&route.source, &route.permalink);

        // Always register (in case other fields changed)
        self.register_page(route, title);

        old_url
    }

    /// Update source -> URL mapping for hot-reload with conflict detection.
    ///
    /// Returns:
    /// - `Unchanged` if permalink didn't change
    /// - `Changed { old_url }` if permalink changed successfully
    /// - `Conflict { url, existing_source }` if new URL conflicts with another resource
    ///
    /// Use this during hot-reload when full PageMeta is not available.
    pub fn update_source_url(&mut self, source: &Path, new_url: &UrlPath) -> PermalinkUpdate {
        let old_url = self.by_source.get(source).cloned();

        // Check if permalink actually changed
        if let Some(ref old) = old_url
            && old == new_url
        {
            return PermalinkUpdate::Unchanged;
        }

        // Check for conflict: new URL already owned by a different source
        if let Some(resource) = self.by_url.get(new_url) {
            let existing_source = resource.source();
            if existing_source != source {
                return PermalinkUpdate::Conflict {
                    url: new_url.clone(),
                    existing_source: existing_source.to_path_buf(),
                };
            }
        }

        // No conflict - proceed with update
        if let Some(ref old) = old_url {
            // Clean up old entries
            self.remove_url(old);
        }

        // Update mapping
        self.by_source.insert(source.to_path_buf(), new_url.clone());

        match old_url {
            Some(old) => PermalinkUpdate::Changed { old_url: old },
            None => PermalinkUpdate::Unchanged, // First time seeing this source
        }
    }

    /// Detect permalink change and clean up old entries if changed.
    ///
    /// Returns Some(old_url) if permalink changed, None otherwise.
    fn detect_permalink_change(&mut self, source: &Path, new_url: &UrlPath) -> Option<UrlPath> {
        let old_url = self.by_source.get(source).cloned();

        if let Some(ref old) = old_url
            && old != new_url
        {
            // Permalink changed - clean up old entries
            self.remove_url(old);
            return Some(old.clone());
        }

        None
    }

    /// Check if a URL exists in the address space.
    pub fn contains_url(&self, url: &UrlPath) -> bool {
        self.by_url.contains_key(url)
    }

    /// Get a resource by URL.
    pub fn get_by_url(&self, url: &UrlPath) -> Option<&Resource> {
        self.by_url.get(url)
    }

    /// Get URL for a source file.
    pub fn url_for_source(&self, source: &Path) -> Option<&UrlPath> {
        self.by_source.get(source)
    }

    /// Get source file path for a URL (reverse lookup, pages only).
    ///
    /// Used by on-demand compilation to find the source file for a requested URL.
    pub fn source_for_url(&self, url: &UrlPath) -> Option<PathBuf> {
        self.by_url.get(url).and_then(|r| match r {
            Resource::Page { route, .. } => Some(route.source.clone()),
            Resource::Asset { .. } => None,
        })
    }

    /// Get URL -> source path mapping for cache persistence.
    ///
    /// This is computed on-demand from `by_source` (no additional storage).
    /// Only call this when persisting cache (e.g., on shutdown).
    ///
    /// Complexity: O(n) where n is the number of pages.
    pub fn source_paths(&self) -> rustc_hash::FxHashMap<UrlPath, PathBuf> {
        self.by_source
            .iter()
            .map(|(source, url)| (url.clone(), source.clone()))
            .collect()
    }

    /// Get heading IDs for a page.
    pub fn headings_for(&self, permalink: &UrlPath) -> Option<&FxHashSet<String>> {
        self.headings.get(permalink)
    }

    /// Check if a URL path is in the assets directory.
    pub fn is_asset_path(&self, path: &str) -> bool {
        let path = path.trim_start_matches('/');
        if self.assets_prefix.is_empty() {
            return false;
        }
        path.starts_with(&self.assets_prefix)
            && path
                .get(self.assets_prefix.len()..)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    }

    /// Get total number of resources.
    pub fn len(&self) -> usize {
        self.by_url.len()
    }

    /// Check if the address space is empty.
    pub fn is_empty(&self) -> bool {
        self.by_url.is_empty()
    }

    /// Get number of pages.
    pub fn page_count(&self) -> usize {
        self.by_url.values().filter(|r| r.is_page()).count()
    }

    /// Get number of assets.
    pub fn asset_count(&self) -> usize {
        self.by_url.values().filter(|r| r.is_asset()).count()
    }

    /// Iterate over all resources.
    pub fn iter(&self) -> impl Iterator<Item = (&UrlPath, &Resource)> {
        self.by_url.iter()
    }

    /// Iterate over all pages.
    pub fn pages(&self) -> impl Iterator<Item = &Resource> {
        self.by_url.values().filter(|r| r.is_page())
    }

    /// Iterate over all assets.
    pub fn assets(&self) -> impl Iterator<Item = &Resource> {
        self.by_url.values().filter(|r| r.is_asset())
    }

    /// Resolve a link to its target resource.
    ///
    /// This is the main entry point for link validation. It handles:
    /// - External links (https://, mailto:, etc.)
    /// - Fragment links (#section)
    /// - Site-root links (/about, /posts/hello/)
    /// - File-relative links (./image.png, ../other/)
    ///
    /// # Arguments
    /// - `link`: The link to resolve
    /// - `ctx`: Context including current page's permalink and source path
    ///
    /// # Returns
    /// A [`ResolveResult`] indicating the resolution outcome.
    pub fn resolve(&self, link: &str, ctx: &ResolveContext<'_>) -> ResolveResult {
        // Empty link is an error
        if link.is_empty() {
            return ResolveResult::Error {
                message: "Empty link".to_string(),
            };
        }

        // Use LinkKind for syntactic classification, then resolve semantically
        match LinkKind::parse(link) {
            LinkKind::External(url) => ResolveResult::External(url.to_string()),

            LinkKind::Fragment(fragment) => {
                self.resolve_fragment(ctx.current_permalink.as_str(), fragment)
            }

            LinkKind::SiteRoot(path) => {
                let (path, fragment) = split_path_fragment(path);
                self.resolve_absolute(path, fragment)
            }

            LinkKind::FileRelative(path) => self.resolve_relative(path, ctx),
        }
    }

    /// Resolve a fragment on the current page.
    fn resolve_fragment(&self, current_url: &str, fragment: &str) -> ResolveResult {
        // Empty fragment is technically valid (links to top of page)
        if fragment.is_empty()
            && let Some(resource) = self.by_url.get(current_url)
        {
            return ResolveResult::Found(resource.clone());
        }

        // Check if the fragment exists on the current page
        if let Some(headings) = self.headings.get(current_url) {
            if headings.contains(fragment) {
                // Fragment exists, return the page resource
                if let Some(resource) = self.by_url.get(current_url) {
                    return ResolveResult::Found(resource.clone());
                }
            }
            // Fragment doesn't exist, but we know what's available
            return ResolveResult::FragmentNotFound {
                page: current_url.to_string(),
                fragment: fragment.to_string(),
                available: headings.iter().cloned().collect(),
            };
        }

        // No heading info for this page (fragments not indexed)
        // Return Found with a warning that we can't verify
        if let Some(resource) = self.by_url.get(current_url) {
            return ResolveResult::Found(resource.clone());
        }

        ResolveResult::NotFound {
            target: format!("{}#{}", current_url, fragment),
            tried: vec![current_url.to_string()],
        }
    }

    /// Resolve an absolute (site-root) path.
    fn resolve_absolute(&self, path: &str, fragment: &str) -> ResolveResult {
        // Apply slug transformation if configured (for consistency with link.rs)
        // Use from_asset to preserve the path without trailing slash
        let slugified_path = if let Some(ref slug_config) = self.slug_config {
            let path_without_slash = path.trim_start_matches('/');
            let slugified = slugify_path(path_without_slash, slug_config);
            UrlPath::from_asset(&format!("/{}", slugified.to_string_lossy()))
        } else {
            UrlPath::from_asset(path)
        };

        // Create version with trailing slash for page lookup
        let normalized = UrlPath::from_page(slugified_path.as_str());

        // Try to find the resource (with trailing slash for pages)
        if let Some(resource) = self.by_url.get(&normalized) {
            // If there's a fragment, verify it exists
            if !fragment.is_empty() {
                return self.check_fragment_on_resource(resource, &normalized, fragment);
            }
            return ResolveResult::Found(resource.clone());
        }

        // Try without trailing slash (for assets)
        if let Some(resource) = self.by_url.get(&slugified_path) {
            if !fragment.is_empty() {
                return self.check_fragment_on_resource(resource, &slugified_path, fragment);
            }
            return ResolveResult::Found(resource.clone());
        }

        ResolveResult::NotFound {
            target: if fragment.is_empty() {
                path.to_string()
            } else {
                format!("{}#{}", path, fragment)
            },
            tried: vec![normalized.to_string(), slugified_path.to_string()],
        }
    }

    /// Check if a fragment exists on a resource.
    fn check_fragment_on_resource(
        &self,
        resource: &Resource,
        url: &UrlPath,
        fragment: &str,
    ) -> ResolveResult {
        // Only pages can have fragments
        if !resource.is_page() {
            return ResolveResult::Warning {
                resolved: Some(url.to_string()),
                message: format!(
                    "Fragment '{}' specified on asset '{}'. Assets don't have fragments.",
                    fragment, url
                ),
            };
        }

        // Check if fragment is indexed
        if let Some(headings) = self.headings.get(url) {
            if headings.contains(fragment) {
                return ResolveResult::Found(resource.clone());
            }
            return ResolveResult::FragmentNotFound {
                page: url.to_string(),
                fragment: fragment.to_string(),
                available: headings.iter().cloned().collect(),
            };
        }

        // Fragments not indexed, assume OK
        ResolveResult::Found(resource.clone())
    }

    /// Resolve a file-relative link.
    ///
    /// This handles both colocated assets (./image.png) and relative page links (../other/).
    fn resolve_relative(&self, link: &str, ctx: &ResolveContext<'_>) -> ResolveResult {
        let (path, fragment) = split_path_fragment(link);

        // Asset attributes (src, poster, data) -> resolve as colocated asset
        if ctx.is_asset_attr() {
            return self.resolve_colocated_asset(path, fragment, ctx);
        }

        // href attribute -> could be page or asset, use smart resolution
        self.resolve_relative_page(path, fragment, ctx)
    }

    /// Resolve a colocated asset reference.
    fn resolve_colocated_asset(
        &self,
        path: &str,
        fragment: &str,
        ctx: &ResolveContext<'_>,
    ) -> ResolveResult {
        // Compute the physical path
        let source_dir = ctx.source_path.parent().unwrap_or(Path::new(""));
        let clean_path = path.trim_start_matches("./");
        let physical_path = source_dir.join(clean_path);

        // Check if there's a colocated directory
        if let Some(colocated_dir) = ctx.colocated_dir {
            let asset_path = colocated_dir.join(clean_path);

            // Check if asset is registered
            if let Some(url) = self.by_source.get(&asset_path)
                && let Some(resource) = self.by_url.get(url)
            {
                if !fragment.is_empty() {
                    return ResolveResult::Warning {
                        resolved: Some(url.to_string()),
                        message: format!(
                            "Fragment '{}' specified on asset. Assets don't have fragments.",
                            fragment
                        ),
                    };
                }
                return ResolveResult::Found(resource.clone());
            }
        }

        // Check physical path directly
        if let Some(url) = self.by_source.get(&physical_path)
            && let Some(resource) = self.by_url.get(url)
        {
            return ResolveResult::Found(resource.clone());
        }

        // Asset not found in address space - this might be OK if it exists on disk
        // The caller should verify file existence
        ResolveResult::NotFound {
            target: path.to_string(),
            tried: vec![
                physical_path.display().to_string(),
                ctx.colocated_dir
                    .map(|d| d.join(clean_path).display().to_string())
                    .unwrap_or_default(),
            ],
        }
    }

    /// Resolve a relative page link using smart bidirectional resolution.
    ///
    /// This implements the intelligent resolution algorithm that:
    /// - Tries URL-space resolution (relative to current permalink)
    /// - Tries physical-space resolution (relative to source file)
    /// - Compares results and reports inconsistencies
    fn resolve_relative_page(
        &self,
        path: &str,
        fragment: &str,
        ctx: &ResolveContext<'_>,
    ) -> ResolveResult {
        // Step 1: Compute URL-space target
        let url_target = resolve_relative_url(ctx.current_permalink, path);

        // Step 2: Compute physical-space target
        let source_dir = ctx.source_path.parent().unwrap_or(Path::new(""));
        let physical_target = resolve_physical_path(source_dir, path);

        // Step 3: Try URL-space match
        let url_match = self.by_url.get(&url_target);

        // Step 4: Try physical-space match
        let physical_match = self.find_page_by_physical_path(&physical_target);

        // Step 5: Analyze results
        match (url_match, physical_match) {
            // Case 1: URL matches, check consistency
            (Some(resource), _) => {
                if let Resource::Page { route, .. } = resource {
                    if self.source_matches_physical(&physical_target, &route.source) {
                        // Perfect: both URL and physical path agree
                        if !fragment.is_empty() {
                            return self.check_fragment_on_resource(
                                resource,
                                &url_target,
                                fragment,
                            );
                        }
                        return ResolveResult::Found(resource.clone());
                    }
                    // URL matches but physical path differs - coincidental match
                    return ResolveResult::Warning {
                        resolved: Some(url_target.to_string()),
                        message: format!(
                            "Relative link '{}' resolves to '{}' via URL matching,\n\
                             but physical path '{}' points elsewhere.\n\
                             Consider using absolute path '{}' for clarity.",
                            path,
                            url_target,
                            physical_target.display(),
                            url_target
                        ),
                    };
                }
                // URL matches an asset, not a page
                ResolveResult::Warning {
                    resolved: Some(url_target.to_string()),
                    message: format!(
                        "Relative link '{}' resolves to asset '{}', not a page.\n\
                         Use src attribute for assets.",
                        path, url_target
                    ),
                }
            }

            // Case 2: URL doesn't match, but physical path finds a page
            (None, Some((found_url, _))) => {
                // Physical path is correct, but that page has a different permalink
                ResolveResult::Error {
                    message: format!(
                        "Relative link '{}' physically points to '{}',\n\
                         but that page's permalink is '{}'.\n\
                         The link will not work. Use absolute path '{}'.",
                        path,
                        physical_target.display(),
                        found_url,
                        found_url
                    ),
                }
            }

            // Case 3: Neither matches
            (None, None) => ResolveResult::NotFound {
                target: path.to_string(),
                tried: vec![
                    format!("URL: {}", url_target),
                    format!("Physical: {}", physical_target.display()),
                ],
            },
        }
    }

    /// Find a page by its physical source path (trying common extensions).
    fn find_page_by_physical_path(&self, path: &Path) -> Option<(&UrlPath, &Resource)> {
        // Try exact path and common extensions
        let candidates = [
            path.to_path_buf(),
            path.with_extension("typ"),
            path.with_extension("md"),
            path.join("index.typ"),
            path.join("index.md"),
        ];

        for candidate in &candidates {
            if let Some(url) = self.by_source.get(candidate)
                && let Some(resource) = self.by_url.get(url)
                && resource.is_page()
            {
                return Some((url, resource));
            }
        }
        None
    }

    /// Check if physical path matches the target source.
    fn source_matches_physical(&self, physical: &Path, target_source: &Path) -> bool {
        let candidates = [
            physical.to_path_buf(),
            physical.with_extension("typ"),
            physical.with_extension("md"),
            physical.join("index.typ"),
            physical.join("index.md"),
        ];
        candidates.iter().any(|c| c == target_source)
    }

    /// Dump the address space for debugging.
    pub fn dump(&self) -> String {
        use std::fmt::Write;
        let mut output = String::new();

        writeln!(output, "=== Pages ({}) ===", self.page_count()).unwrap();
        let mut pages: Vec<_> = self.pages().collect();
        pages.sort_by_key(|r| r.url());
        for resource in pages {
            if let Resource::Page { route, .. } = resource {
                writeln!(output, "  {} ← {}", route.permalink, route.source.display()).unwrap();
            }
        }

        writeln!(output, "\n=== Assets ({}) ===", self.asset_count()).unwrap();
        let mut assets: Vec<_> = self.assets().collect();
        assets.sort_by_key(|r| r.url());
        for resource in assets {
            if let Resource::Asset { route } = resource {
                use crate::asset::AssetKind;
                let kind_str = match route.kind {
                    AssetKind::Global => "global",
                    AssetKind::Colocated => "colocated",
                };
                writeln!(
                    output,
                    "  {} ← {} ({})",
                    route.url,
                    route.source.display(),
                    kind_str
                )
                .unwrap();
            }
        }

        if !self.headings.is_empty() {
            writeln!(output, "\n=== Headings ===").unwrap();
            let mut urls: Vec<_> = self.headings.keys().collect();
            urls.sort();
            for url in urls {
                let ids = self.headings.get(url).unwrap();
                writeln!(output, "  {} ({} headings)", url, ids.len()).unwrap();
            }
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test PageRoute with minimal required fields.
    fn test_route(source: &str, permalink: &str, output: &str) -> PageRoute {
        PageRoute {
            source: PathBuf::from(source),
            permalink: UrlPath::from_page(permalink),
            output_file: PathBuf::from(output),
            is_index: false,
            is_404: false,
            colocated_dir: None,
            output_dir: PathBuf::new(),
            full_url: String::new(),
            relative: String::new(),
        }
    }

    #[test]
    fn test_register_page() {
        let mut space = AddressSpace::new();
        let route = test_route("content/hello.typ", "/hello/", "public/hello/index.html");
        let permalink = route.permalink.clone();
        space.register_page(route, Some("Hello".to_string()));

        assert!(space.contains_url(&permalink));
        assert_eq!(
            space.url_for_source(Path::new("content/hello.typ")),
            Some(&permalink)
        );
        assert_eq!(space.page_count(), 1);
    }

    /// Create a test AssetRoute.
    fn test_asset_route(source: &str, url: &str, output: &str) -> AssetRoute {
        use crate::asset::AssetKind;
        AssetRoute {
            source: PathBuf::from(source),
            url: UrlPath::from_asset(url),
            output: PathBuf::from(output),
            kind: AssetKind::Global,
        }
    }

    #[test]
    fn test_register_asset() {
        let mut space = AddressSpace::new().with_assets_prefix("assets");
        let route = test_asset_route(
            "assets/logo.png",
            "/assets/logo.png",
            "public/assets/logo.png",
        );
        let url = route.url.clone();
        space.register_asset(route);

        assert!(space.contains_url(&url));
        assert!(space.is_asset_path("/assets/logo.png"));
        assert!(!space.is_asset_path("/posts/hello/"));
        assert_eq!(space.asset_count(), 1);
    }

    #[test]
    fn test_register_headings() {
        let mut space = AddressSpace::new();
        let route = test_route("content/hello.typ", "/hello/", "public/hello/index.html");
        let permalink = route.permalink.clone();
        space.register_page(route, None);
        space.register_headings(&permalink, ["intro".to_string(), "conclusion".to_string()]);

        let headings = space.headings_for(&permalink).unwrap();
        assert!(headings.contains("intro"));
        assert!(headings.contains("conclusion"));
        assert_eq!(headings.len(), 2);
    }

    #[test]
    fn test_is_asset_path() {
        let space = AddressSpace::new().with_assets_prefix("assets");

        assert!(space.is_asset_path("/assets/logo.png"));
        assert!(space.is_asset_path("/assets/images/photo.jpg"));
        assert!(space.is_asset_path("assets/logo.png"));
        assert!(!space.is_asset_path("/assetsxyz/logo.png")); // not a segment match
        assert!(!space.is_asset_path("/posts/hello/"));
        assert!(!space.is_asset_path("/about/"));
    }

    #[test]
    fn test_dump() {
        let mut space = AddressSpace::new().with_assets_prefix("assets");
        let route = test_route("content/hello.typ", "/hello/", "public/hello/index.html");
        space.register_page(route, Some("Hello".to_string()));
        space.register_asset(test_asset_route(
            "assets/logo.png",
            "/assets/logo.png",
            "public/assets/logo.png",
        ));

        let dump = space.dump();
        assert!(dump.contains("Pages (1)"));
        assert!(dump.contains("/hello/"));
        assert!(dump.contains("Assets (1)"));
        assert!(dump.contains("/assets/logo.png"));
    }

    #[test]
    fn test_update_source_url_unchanged() {
        let mut space = AddressSpace::new();
        let route = test_route("content/hello.typ", "/hello/", "public/hello/index.html");
        let url = route.permalink.clone();
        space.register_page(route, None);

        // Same URL - should be unchanged
        let result = space.update_source_url(Path::new("content/hello.typ"), &url);
        assert!(matches!(result, PermalinkUpdate::Unchanged));
    }

    #[test]
    fn test_update_source_url_changed() {
        let mut space = AddressSpace::new();
        let route = test_route("content/hello.typ", "/hello/", "public/hello/index.html");
        let old_url = route.permalink.clone();
        space.register_page(route, None);
        let new_url = UrlPath::from_page("/world/");

        // Different URL - should be changed
        let result = space.update_source_url(Path::new("content/hello.typ"), &new_url);
        match result {
            PermalinkUpdate::Changed { old_url: old } => {
                assert_eq!(old, old_url);
            }
            _ => panic!("Expected Changed, got {:?}", result),
        }
    }

    #[test]
    fn test_update_source_url_conflict() {
        let mut space = AddressSpace::new();

        // Register page A at /hello/
        let route_a = test_route("content/a.typ", "/hello/", "public/hello/index.html");
        space.register_page(route_a, None);

        // Register page B at /world/
        let route_b = test_route("content/b.typ", "/world/", "public/world/index.html");
        space.register_page(route_b, None);

        // Try to change page B's URL to /hello/ (conflict with page A)
        let result =
            space.update_source_url(Path::new("content/b.typ"), &UrlPath::from_page("/hello/"));
        match result {
            PermalinkUpdate::Conflict {
                url,
                existing_source,
            } => {
                assert_eq!(url, "/hello/");
                assert_eq!(existing_source, PathBuf::from("content/a.typ"));
            }
            _ => panic!("Expected Conflict, got {:?}", result),
        }
    }
}
