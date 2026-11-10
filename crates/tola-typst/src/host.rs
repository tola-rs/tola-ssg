//! Typst resources and world construction, independent of any site.
//!
//! A [`TypstHost`] holds the decisions a caller has already made — fonts, package roots,
//! fetch policy, source boundary, and file cache — and builds worlds from them. It reads
//! no configuration and knows no route, page, or icon.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use typst::Library;
use typst::utils::LazyHash;

use crate::bundle::BundleCancellation;
use crate::world::file::{
    FileMap, FileProvider, FileResolver, FileTarget, FileView, LocalPackages, SharedFileCache,
};
use crate::world::font::{FontLoadError, FontOptions, FontStore};
use crate::world::package::{PackageFetchPolicy, PackageLocations, PackageSpec, PackageVerifier};
use crate::world::{ExplicitSources, SourceBoundary, TypstWorld, WorldBuildError};

/// The site-independent inputs a [`TypstHost`] is built from.
///
/// Every field is a decision the caller already made; the host only binds them together.
pub struct TypstHostParts {
    /// Loaded fonts and their options.
    pub fonts: Arc<FontStore>,
    /// File resolver, including the caller's file provider and source boundary.
    pub files: FileResolver,
    /// Effective Typst package roots.
    pub package_locations: PackageLocations,
    /// Whether missing packages may be fetched.
    pub fetch_policy: PackageFetchPolicy,
    /// Physical source restrictions for reads outside the resolver.
    pub source_boundary: SourceBoundary,
    /// Parsed-file cache shared with other consumers.
    pub file_cache: Arc<SharedFileCache>,
}

/// Typst resources for one compilation, without site concepts.
#[derive(Clone)]
pub struct TypstHost {
    package_locations: PackageLocations,
    fetch_policy: PackageFetchPolicy,
    source_boundary: SourceBoundary,
    files: FileResolver,
    local_packages: Arc<LocalPackages>,
    virtual_files: Arc<FileMap>,
    file_cache: Arc<SharedFileCache>,
    fonts: Arc<FontStore>,
}

impl TypstHost {
    /// Bind already-decided resources.
    pub fn new(parts: TypstHostParts) -> Self {
        Self {
            package_locations: parts.package_locations,
            fetch_policy: parts.fetch_policy,
            source_boundary: parts.source_boundary,
            files: parts.files,
            local_packages: Arc::default(),
            virtual_files: Arc::default(),
            file_cache: parts.file_cache,
            fonts: parts.fonts,
        }
    }

    /// Replace the file provider, keeping virtual files as an overlay above it.
    pub fn with_file_provider<P: FileProvider + 'static>(&self, provider: P) -> Self {
        let mut host = self.clone();
        host.files = self.files.clone().with_provider(provider);
        host
    }

    /// Require a caller's check of every package root before it is read.
    pub fn with_package_verifier(mut self, verifier: PackageVerifier) -> Self {
        self.files = self.files.with_package_verifier(verifier);
        self
    }

    /// Bind one immutable selection of local packages without changing builtin files.
    pub fn with_local_packages(&self, packages: Arc<LocalPackages>) -> Self {
        let mut host = self.clone();
        host.files = self
            .files
            .clone()
            .with_local_packages(Arc::clone(&packages));
        host.local_packages = packages;
        host
    }

    /// Supply exact host-generated files, replacing any previous set.
    ///
    /// Virtual files are an overlay: they take precedence over the base provider but do
    /// not replace it.
    pub fn with_virtual_files(&self, files: FileMap) -> Self {
        let mut host = self.clone();
        host.virtual_files = Arc::new(files);
        host
    }

    /// Use immutable bytes for selected physical files without changing file selection.
    pub fn with_disk_overrides(
        &self,
        overrides: impl IntoIterator<Item = (PathBuf, Arc<[u8]>)>,
    ) -> typst::diag::FileResult<Self> {
        let mut host = self.clone();
        host.files = self.files.clone().with_disk_overrides(overrides)?;
        Ok(host)
    }

    /// Effective Typst package roots.
    pub fn package_locations(&self) -> &PackageLocations {
        &self.package_locations
    }

    /// Whether missing packages may be fetched.
    pub fn fetch_policy(&self) -> PackageFetchPolicy {
        self.fetch_policy
    }

    /// Physical source restrictions applied to disk reads.
    pub fn source_boundary(&self) -> &SourceBoundary {
        &self.source_boundary
    }

    /// Font options this host loaded with.
    pub fn font_options(&self) -> &FontOptions {
        self.fonts.options()
    }

    /// Parsed-file cache shared with other consumers.
    pub fn file_cache(&self) -> &Arc<SharedFileCache> {
        &self.file_cache
    }

    /// Declared local packages bound to this host.
    pub fn local_packages(&self) -> &Arc<LocalPackages> {
        &self.local_packages
    }

    /// The resolver every source read of this host uses.
    ///
    /// Virtual files, when present, are layered over the base provider.
    pub fn file_resolver(&self) -> Arc<FileResolver> {
        Arc::new(self.resolved_files())
    }

    fn resolved_files(&self) -> FileResolver {
        if self.virtual_files.is_empty() {
            self.files.clone()
        } else {
            self.files
                .clone()
                .with_overlay(Arc::clone(&self.virtual_files))
        }
    }

    /// The bytes this host supplies for one package file, if any.
    pub fn virtual_bytes(&self, package: &PackageSpec, path: &Path) -> Option<Arc<[u8]>> {
        let path = typst::syntax::VirtualPath::new(path.to_str()?).ok()?;
        let id = typst::syntax::FileId::new(typst::syntax::RootedPath::new(
            typst::syntax::VirtualRoot::Package(package.clone()),
            path,
        ));
        match self.virtual_files.target(id) {
            Some(FileTarget::Bytes(bytes)) => Some(bytes),
            _ => None,
        }
    }

    /// Every physical path the font inventory read.
    pub fn font_read_paths(&self) -> Vec<PathBuf> {
        self.fonts.read_paths()
    }

    /// Whether this host and `other` would compile from the same inputs.
    pub fn has_same_inputs(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.fonts, &other.fonts)
            && self.package_locations == other.package_locations
            && self.fetch_policy == other.fetch_policy
            && self.source_boundary == other.source_boundary
            && self.local_packages == other.local_packages
            && Arc::ptr_eq(&self.virtual_files, &other.virtual_files)
    }

    /// Whether the loaded font inventory still matches the files on disk.
    pub fn font_inventory_is_fresh(
        &self,
        cancellation: &BundleCancellation,
    ) -> Result<bool, FontLoadError> {
        self.fonts.is_current(cancellation)
    }

    /// Drop parsed-file cache entries older than `max_age` generations.
    pub fn evict_stale_file_cache_entries(&self, max_age: usize) {
        self.file_cache.evict(max_age as u64);
    }

    /// Keep parsed imports across short runs of builds that reuse their compilation.
    pub const RETAINED_FILE_CACHE_EPOCHS: usize = 10;

    /// Candidate files for one explicit source set.
    pub fn candidate_files(&self, sources: Arc<ExplicitSources>) -> Arc<FileView> {
        Arc::new(FileView::new(sources, Arc::new(self.resolved_files())))
    }

    /// Build a world from a main path below `root`.
    pub fn world(
        &self,
        root: &Path,
        main: &Path,
        library: Arc<LazyHash<Library>>,
        candidate_files: Arc<FileView>,
        cancellation: &BundleCancellation,
    ) -> Result<TypstWorld, WorldBuildError> {
        TypstWorld::builder(main, root)
            .with_file_view(candidate_files, Arc::clone(&self.file_cache))
            .with_fonts(Arc::clone(&self.fonts))
            .with_shared_library(library)
            .build(cancellation)
    }

    /// Build a world from a main file id while retaining `root`.
    pub fn world_with_id(
        &self,
        root: &Path,
        main: typst::syntax::FileId,
        library: Arc<LazyHash<Library>>,
        candidate_files: Arc<FileView>,
        cancellation: &BundleCancellation,
    ) -> Result<TypstWorld, WorldBuildError> {
        TypstWorld::builder_with_id(main, root)
            .with_file_view(candidate_files, Arc::clone(&self.file_cache))
            .with_fonts(Arc::clone(&self.fonts))
            .with_shared_library(library)
            .build(cancellation)
    }
}
