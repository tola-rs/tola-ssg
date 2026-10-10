//! Typst fonts, file resolution, and compiler construction.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tola_typst::prelude::*;

use crate::{
    cancellation::{BuildCancellation, BuildCancelled},
    config::ResolvedSiteConfig,
    package,
    resources::{BuildResources, NetworkAccess},
};

/// File provider for Tola's versioned virtual packages.
#[derive(Clone, Copy)]
pub struct TolaPackageFiles;

impl tola_typst::FileProvider for TolaPackageFiles {
    fn target(&self, id: typst::syntax::FileId) -> Option<tola_typst::FileTarget> {
        package_target(id, None)
    }

    /// The `tola` namespace belongs to Tola, so only the packages above exist in it.
    fn owned_namespaces(&self) -> &'static [&'static str] {
        &[tola_packages::TOLA_NAMESPACE]
    }
}

/// Resolve one virtual-package file against the builtin packages and, when the
/// host has bound icon collections, their captured bytes.
fn package_target(
    id: typst::syntax::FileId,
    icons: Option<&tola_icons::IconCollections>,
) -> Option<tola_typst::FileTarget> {
    let typst::syntax::VirtualRoot::Package(spec) = id.root() else {
        return None;
    };
    let path = Path::new(id.vpath().get_with_slash().trim_start_matches('/'));
    if tola_packages::is_icon_file(spec, path) {
        return Some(match icons {
            Some(collections) => tola_packages::icon_file_bytes(collections, spec, path)
                .map(|bytes| tola_typst::FileTarget::Bytes(Arc::from(bytes)))
                .unwrap_or(tola_typst::FileTarget::Missing),
            None => tola_typst::FileTarget::Missing,
        });
    }
    if tola_packages::owns_observation(spec, id.vpath().get_with_slash()) {
        return Some(
            package::read_package(spec, id.vpath().get_with_slash())
                .map(|bytes| tola_typst::FileTarget::Bytes(bytes.into()))
                .unwrap_or(tola_typst::FileTarget::Missing),
        );
    }
    package::read_package(spec, id.vpath().get_with_slash())
        .map(|bytes| tola_typst::FileTarget::Bytes(Arc::from(bytes)))
}

/// Immutable dynamic files for one prepared attempt, not a live icon collection handle.
struct SitePackageFiles(Arc<tola_icons::IconCollections>);

impl tola_typst::FileProvider for SitePackageFiles {
    fn target(&self, id: typst::syntax::FileId) -> Option<tola_typst::FileTarget> {
        package_target(id, Some(&self.0))
    }

    /// The `tola` namespace belongs to Tola, so only the packages above exist in it.
    fn owned_namespaces(&self) -> &'static [&'static str] {
        &[tola_packages::TOLA_NAMESPACE]
    }
}

/// Typst resources for one site configuration.
#[derive(Clone)]
pub struct TypstHost {
    package_locations: tola_typst::PackageLocations,
    network_access: NetworkAccess,
    source_boundary: tola_typst::SourceBoundary,
    files: FileResolver,
    icons: Arc<tola_icons::IconCollections>,
    file_cache: Arc<SharedFileCache>,
    fonts: Arc<FontStore>,
}

impl TypstHost {
    fn new_with_packages(
        font_dirs: &[&Path],
        package_locations: tola_typst::PackageLocations,
        resources: &BuildResources,
        system_fonts: bool,
        source_boundary: tola_typst::SourceBoundary,
        cancellation: &BuildCancellation,
    ) -> Result<Self, tola_typst::FontLoadError> {
        let fonts = Arc::new(FontStore::with_options(
            FontOptions::new()
                .with_custom_paths(font_dirs)
                .with_system_fonts(system_fonts)
                .with_source_boundary(source_boundary.clone()),
        ));
        fonts.load(&cancellation.bundle_cancellation())?;
        Ok(Self {
            files: file_resolver(package_locations.clone(), resources.network_access())
                .with_source_boundary(source_boundary.clone()),
            icons: Arc::default(),
            package_locations,
            network_access: resources.network_access(),
            source_boundary,
            file_cache: resources.file_cache(),
            fonts,
        })
    }

    pub(crate) fn for_config_with_resources(
        config: &ResolvedSiteConfig,
        resources: &BuildResources,
        cancellation: &BuildCancellation,
    ) -> Result<Self, tola_typst::FontLoadError> {
        // A vendored font directory holds system faces the site chose to include, so it joins the
        // configured directories and is searched before system discovery.
        let vendored = config.vendor.fonts().filter(|path| path.is_dir());
        let mut font_dirs: Vec<&Path> = config
            .typst
            .fonts
            .paths
            .iter()
            .map(PathBuf::as_path)
            .collect();
        font_dirs.extend(vendored.as_deref());
        Self::new_with_packages(
            &font_dirs,
            resources.package_locations(config),
            resources,
            config.system_fonts_allowed(resources),
            resources.source_boundary(config),
            cancellation,
        )
    }

    /// Compiler results can be reused only within the same font-resource generation.
    pub(crate) fn has_same_inputs(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.fonts, &other.fonts)
            && self.package_locations == other.package_locations
            && self.network_access == other.network_access
            && self.source_boundary == other.source_boundary
    }

    /// Bind validated icons before creating any source or candidate snapshots.
    /// Older hosts and worlds keep their own immutable icon collections.
    pub(crate) fn with_icons(&self, icons: Arc<tola_icons::IconCollections>) -> Self {
        let mut host = self.clone();
        host.files = self
            .files
            .clone()
            .with_provider(SitePackageFiles(Arc::clone(&icons)));
        host.icons = icons;
        host
    }

    pub(crate) fn with_source_overrides(
        &self,
        sources: &crate::filesystem::SourceOverrides,
    ) -> typst::diag::FileResult<Self> {
        let mut host = self.clone();
        if sources.is_empty() {
            return Ok(host);
        }
        for path in sources.paths() {
            self.source_boundary.check(path)?;
        }
        host.files = self
            .files
            .clone()
            .with_disk_overrides(sources.sources().map(|(path, source)| {
                (
                    path.to_path_buf(),
                    Arc::<[u8]>::from(source.text.as_bytes()),
                )
            }))?;
        Ok(host)
    }

    /// Whether this host may serve one build of `config` with `resources`.
    pub(crate) fn matches(&self, config: &ResolvedSiteConfig, resources: &BuildResources) -> bool {
        let vendored = config.vendor.fonts().filter(|path| path.is_dir());
        self.fonts.options().custom_paths.iter().eq(config
            .typst
            .fonts
            .paths
            .iter()
            .map(PathBuf::as_path)
            .chain(vendored.as_deref()))
            && self.package_locations == resources.package_locations(config)
            && self.fonts.options().include_system_fonts == config.system_fonts_allowed(resources)
            && self.network_access == resources.network_access()
            && self.source_boundary == resources.source_boundary(config)
            && Arc::ptr_eq(&self.file_cache, &resources.file_cache())
    }

    pub(crate) fn font_inventory_is_fresh(
        &self,
        cancellation: &BuildCancellation,
    ) -> Result<bool, BuildCancelled> {
        match self.fonts.is_current(&cancellation.bundle_cancellation()) {
            Ok(current) => Ok(current),
            Err(tola_typst::FontLoadError::Cancelled) => Err(BuildCancelled),
            Err(_) => Ok(false),
        }
    }

    pub(crate) fn font_read_paths(&self) -> Vec<PathBuf> {
        self.fonts.read_paths()
    }

    pub(crate) fn source_boundary(&self) -> &tola_typst::SourceBoundary {
        &self.source_boundary
    }

    /// The resolver every source read of this host uses.
    pub(crate) fn file_resolver(&self) -> Arc<FileResolver> {
        Arc::new(self.files.clone())
    }

    pub(crate) fn world(
        &self,
        root: &Path,
        main: &Path,
        library: &tola_packages::library::SiteLibrary,
        candidate_files: Arc<FileSnapshot>,
        cancellation: &BundleCancellation,
    ) -> Result<TypstWorld, WorldBuildError> {
        TypstWorld::builder(main, root)
            .with_file_snapshot(candidate_files, Arc::clone(&self.file_cache))
            .with_fonts(Arc::clone(&self.fonts))
            .with_shared_library(library.shared())
            .build(cancellation)
    }

    pub(crate) fn candidate_files(&self, snapshot: Arc<SourceSnapshot>) -> Arc<FileSnapshot> {
        Arc::new(FileSnapshot::new(snapshot, Arc::new(self.files.clone())))
    }

    pub(crate) fn read_is_process_stable(&self, locator: &ReadLocator) -> bool {
        match locator {
            ReadLocator::ProvidedRoot(_) => false,
            ReadLocator::ProvidedPackage { package, path } => {
                crate::package::package_file_is_process_stable(package, path)
            }
            ReadLocator::Root(_) | ReadLocator::Package { .. } => false,
            ReadLocator::NonPersistent(_) => false,
        }
    }

    /// Compare replayable dynamic provider input without reading unrelated disk files.
    pub(crate) fn virtual_read_matches(&self, evidence: &ReadEvidence) -> bool {
        match evidence.locator() {
            ReadLocator::ProvidedPackage { package, path } => {
                tola_packages::icon_file_bytes(&self.icons, package, path)
                    .is_some_and(|bytes| ContentDigest::of(bytes) == evidence.digest())
            }
            _ => false,
        }
    }

    /// Keep parsed imports across short runs of builds that reuse their compilation.
    /// Explicit content sources and the entry program live in the source snapshot.
    pub(crate) const RETAINED_FILE_CACHE_EPOCHS: usize = 10;

    pub(crate) fn evict_stale_file_cache_entries(&self, max_age: usize) {
        self.file_cache.evict(max_age as u64);
    }
}

fn file_resolver(
    package_locations: tola_typst::PackageLocations,
    network: NetworkAccess,
) -> FileResolver {
    FileResolver::from_package_locations(
        package_locations,
        match network {
            NetworkAccess::Allowed => tola_typst::PackageFetchPolicy::AllowNetwork,
            NetworkAccess::Denied => tola_typst::PackageFetchPolicy::LocalOnly,
        },
    )
    .with_provider(TolaPackageFiles)
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::compiler::tests::{compiler_host, compiler_host_with, configure_site};
    use tempfile::TempDir;

    fn host(font_dirs: &[&Path]) -> TypstHost {
        TypstHost::new_with_packages(
            font_dirs,
            tola_typst::PackageLocations::default(),
            &BuildResources::default(),
            true,
            tola_typst::SourceBoundary::default(),
            &BuildCancellation::default(),
        )
        .unwrap()
    }

    #[test]
    fn pure_hosts_ignore_machine_packages() {
        let directory = TempDir::new().unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        let machine = config.get_root().join("machine-packages");
        let package = machine.join("local/demo/1.0.0");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("value.typ"), "host-only").unwrap();
        config.package_locations =
            tola_typst::PackageLocations::from_absolute_roots(Some(machine), None).unwrap();
        let resources = BuildResources::new()
            .without_system_fonts()
            .with_network_access(NetworkAccess::Denied);
        let ordinary = compiler_host_with(&config, &resources).unwrap();
        let id = typst::syntax::FileId::new(typst::syntax::RootedPath::new(
            typst::syntax::VirtualRoot::Package("@local/demo:1.0.0".parse().unwrap()),
            typst::syntax::VirtualPath::new("value.typ").unwrap(),
        ));
        assert_eq!(
            ordinary.files.read(id, config.get_root()).unwrap(),
            b"host-only"
        );
        let pure_resources = resources.with_input_scope(crate::InputScope::Pure);
        assert!(!ordinary.matches(&config, &pure_resources));
        let pure = compiler_host_with(&config, &pure_resources).unwrap();
        assert!(pure.files.read(id, config.get_root()).is_err());
    }

    #[test]
    fn provider_ignores_site_paths() {
        let files = TolaPackageFiles;
        assert_eq!(
            tola_typst::FileProvider::target(&files, tola_typst::file_id("images/photo.webp")),
            None
        );
    }

    #[test]
    fn site_config_changes_keep_the_host() {
        let dir = TempDir::new().unwrap();
        let mut config = crate::config::tests::load_test_config(dir.path(), "");
        configure_site(
            &mut config,
            &dir.path().join("content"),
            &dir.path().join("site.typ"),
        );
        let resources = BuildResources::default();
        let host = compiler_host_with(&config, &resources).unwrap();

        config.site.title = "Changed title".into();
        config.build.minify.html = !config.build.minify.html;

        assert!(host.matches(&config, &resources));
    }

    #[test]
    fn font_root_change_invalidates_the_host() {
        let dir = TempDir::new().unwrap();
        let mut config = crate::config::tests::load_test_config(dir.path(), "");
        configure_site(
            &mut config,
            &dir.path().join("content"),
            &dir.path().join("site.typ"),
        );
        let resources = BuildResources::default();
        let host = compiler_host_with(&config, &resources).unwrap();

        config.typst.fonts.paths = vec![dir.path().join("fonts")];

        assert!(!host.matches(&config, &resources));
    }

    /// System fonts join a build only when the site opts in and the invocation permits them;
    /// either decision alone can keep them out.
    #[test]
    fn system_fonts_need_site_opt_in_and_permission() {
        let dir = TempDir::new().unwrap();
        let mut config = crate::config::tests::load_test_config(dir.path(), "");
        let resources = BuildResources::default();

        assert!(!config.system_fonts_allowed(&resources));

        config.typst.fonts.system = true;
        assert!(config.system_fonts_allowed(&resources));
        assert!(!config.system_fonts_allowed(&resources.without_system_fonts()));
    }

    #[test]
    fn clones_share_font_resources() {
        let dir = TempDir::new().unwrap();
        let first = host(&[dir.path()]);
        let cloned = first.clone();
        let second = host(&[dir.path()]);

        assert!(first.has_same_inputs(&cloned));
        assert!(!first.has_same_inputs(&second));
    }

    #[test]
    fn font_operations_keep_cancellation() {
        let directory = TempDir::new().unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), "");
        let loaded = compiler_host(&config).unwrap();
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        canceller.cancel();
        assert!(loaded.font_inventory_is_fresh(&cancellation).is_err());
        assert!(matches!(
            TypstHost::for_config_with_resources(
                &config,
                &BuildResources::default(),
                &cancellation
            ),
            Err(tola_typst::FontLoadError::Cancelled)
        ));
    }
}
