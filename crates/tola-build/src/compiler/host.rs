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

/// Resolve one virtual-package file against the builtin packages and bound icons.
fn package_target(
    id: typst::syntax::FileId,
    icons: Option<&tola_icons::IconCollections>,
) -> Option<tola_typst::FileTarget> {
    let typst::syntax::VirtualRoot::Package(spec) = id.root() else {
        return None;
    };
    let path = Path::new(id.vpath().get_with_slash().trim_start_matches('/'));
    if tola_packages::is_icon_file(spec, path) {
        return Some(
            icons.map_or(tola_typst::FileTarget::Missing, |collections| {
                package::icon_file_target(collections, spec, path)
            }),
        );
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
struct SitePackageFiles {
    icons: Arc<tola_icons::IconCollections>,
    virtual_files: Arc<FileMap>,
}

impl tola_typst::FileProvider for SitePackageFiles {
    fn target(&self, id: typst::syntax::FileId) -> Option<tola_typst::FileTarget> {
        self.virtual_files
            .target(id)
            .or_else(|| package_target(id, Some(&self.icons)))
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
    local_packages: Arc<LocalPackages>,
    virtual_files: Arc<FileMap>,
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
            local_packages: Arc::default(),
            virtual_files: Arc::default(),
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
        let directories = config.compiler_font_directories(resources);
        let font_dirs: Vec<&Path> = directories.iter().map(PathBuf::as_path).collect();
        Self::new_with_packages(
            &font_dirs,
            resources.package_locations(config),
            resources,
            config.system_fonts_allowed(resources),
            resources.source_boundary(config),
            cancellation,
        )
        .map(|host| host.with_local_packages(Arc::clone(&config.module_inputs.packages)))
    }

    /// Options select hooks before those hooks can produce configured fonts.
    pub(crate) fn for_module_options(
        config: &ResolvedSiteConfig,
        resources: &BuildResources,
        cancellation: &BuildCancellation,
    ) -> Result<Self, tola_typst::FontLoadError> {
        Self::new_with_packages(
            &[],
            resources.package_locations(config),
            resources,
            false,
            resources.source_boundary(config),
            cancellation,
        )
        .map(|host| host.with_local_packages(Arc::clone(&config.module_inputs.packages)))
    }

    /// Compiler results can be reused only within the same font-resource generation.
    pub(crate) fn has_same_inputs(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.fonts, &other.fonts)
            && self.package_locations == other.package_locations
            && self.network_access == other.network_access
            && self.source_boundary == other.source_boundary
            && self.local_packages == other.local_packages
            && Arc::ptr_eq(&self.virtual_files, &other.virtual_files)
    }

    /// Bind validated icons before creating any source sets or candidate files.
    pub(crate) fn with_icons(&self, icons: Arc<tola_icons::IconCollections>) -> Self {
        let mut host = self.clone();
        host.files = self.files.clone().with_provider(SitePackageFiles {
            icons: Arc::clone(&icons),
            virtual_files: Arc::clone(&self.virtual_files),
        });
        host.icons = icons;
        host
    }

    /// Bind one immutable selection of local packages without changing builtin files.
    pub(crate) fn with_local_packages(&self, packages: Arc<LocalPackages>) -> Self {
        let mut host = self.clone();
        host.files = self
            .files
            .clone()
            .with_local_packages(Arc::clone(&packages));
        host.local_packages = packages;
        host
    }

    /// Supply exact host-generated files, keeping builtin packages, icons and local bindings.
    pub(crate) fn with_virtual_files(&self, files: FileMap) -> Self {
        let mut host = self.clone();
        host.virtual_files = Arc::new(files);
        host.files = self.files.clone().with_provider(SitePackageFiles {
            icons: Arc::clone(&self.icons),
            virtual_files: Arc::clone(&host.virtual_files),
        });
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
        let directories = config.compiler_font_directories(resources);
        self.fonts
            .options()
            .custom_paths
            .iter()
            .eq(directories.iter().map(PathBuf::as_path))
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
        candidate_files: Arc<FileView>,
        cancellation: &BundleCancellation,
    ) -> Result<TypstWorld, WorldBuildError> {
        TypstWorld::builder(main, root)
            .with_file_view(candidate_files, Arc::clone(&self.file_cache))
            .with_fonts(Arc::clone(&self.fonts))
            .with_shared_library(library.shared())
            .build(cancellation)
    }

    /// Compile a native package or virtual entry while retaining the project's root.
    pub(crate) fn world_with_id(
        &self,
        root: &Path,
        main: typst::syntax::FileId,
        library: &tola_packages::library::SiteLibrary,
        candidate_files: Arc<FileView>,
        cancellation: &BundleCancellation,
    ) -> Result<TypstWorld, WorldBuildError> {
        TypstWorld::builder_with_id(main, root)
            .with_file_view(candidate_files, Arc::clone(&self.file_cache))
            .with_fonts(Arc::clone(&self.fonts))
            .with_shared_library(library.shared())
            .build(cancellation)
    }

    /// Compiles by entry file id, or by `build.entry` when the configuration names none.
    pub(crate) fn site_world(
        &self,
        config: &ResolvedSiteConfig,
        library: &tola_packages::library::SiteLibrary,
        candidate_files: Arc<FileView>,
        cancellation: &BundleCancellation,
    ) -> Result<TypstWorld, WorldBuildError> {
        match config.entry_file() {
            Some(main) => self.world_with_id(
                config.get_root(),
                main,
                library,
                candidate_files,
                cancellation,
            ),
            None => self.world(
                config.get_root(),
                &config.build.entry,
                library,
                candidate_files,
                cancellation,
            ),
        }
    }

    pub(crate) fn candidate_files(&self, sources: Arc<ExplicitSources>) -> Arc<FileView> {
        Arc::new(FileView::new(sources, Arc::new(self.files.clone())))
    }

    pub(crate) fn read_is_process_stable(&self, locator: &ReadLocator) -> bool {
        match locator {
            ReadLocator::ProvidedPackage { package, path } => {
                self.virtual_package_bytes(package, path).is_none()
                    && crate::package::package_file_is_process_stable(package, path)
            }
            ReadLocator::Root(_)
            | ReadLocator::Package { .. }
            | ReadLocator::ProvidedRoot(_)
            | ReadLocator::NonPersistent(_) => false,
        }
    }

    /// Compare replayable dynamic provider input without reading unrelated disk files.
    pub(crate) fn virtual_read_matches(&self, evidence: &ReadEvidence) -> bool {
        match evidence.locator() {
            ReadLocator::ProvidedPackage { package, path } => {
                if let Some(bytes) = self.virtual_package_bytes(package, path) {
                    return ContentDigest::of(&bytes) == evidence.digest();
                }
                match package::icon_file_target(&self.icons, package, path) {
                    tola_typst::FileTarget::Bytes(bytes) => {
                        ContentDigest::of(&bytes) == evidence.digest()
                    }
                    _ => {
                        path == Path::new("typst.toml")
                            && self
                                .local_packages
                                .package(package)
                                .is_some_and(|selected| {
                                    ContentDigest::of(selected.manifest_bytes())
                                        == evidence.digest()
                                })
                    }
                }
            }
            _ => false,
        }
    }

    fn virtual_package_bytes(&self, package: &PackageSpec, path: &Path) -> Option<Arc<[u8]>> {
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

    /// Keep parsed imports across short runs of builds that reuse their compilation.
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
    fn site_paths_have_no_target() {
        let files = TolaPackageFiles;
        assert_eq!(
            tola_typst::FileProvider::target(&files, tola_typst::file_id("images/photo.webp")),
            None
        );
    }

    #[test]
    fn site_config_keeps_host() {
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
    fn font_root_invalidates_host() {
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
    fn system_fonts_need_opt_in() {
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
    fn local_bindings_survive_host_changes() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let module = root.join("module");
        std::fs::create_dir(&module).unwrap();
        std::fs::write(module.join("lib.typ"), "module source").unwrap();
        let config = crate::config::tests::load_test_config(root, "");
        let base = compiler_host(&config).unwrap();
        let package = LocalPackage::new(
            "@tola-modules/sample:0.0.0".parse().unwrap(),
            &module,
            typst::syntax::VirtualPath::new("lib.typ").unwrap(),
        )
        .unwrap();
        let registry =
            || Arc::new(LocalPackages::new([package.clone()], ["tola-modules"]).unwrap());
        let selected = base.with_local_packages(registry());
        assert!(!base.has_same_inputs(&selected));
        assert!(selected.has_same_inputs(&base.with_local_packages(registry())));
        let source = virtual_file_id("<module-options>");
        let mut virtual_files = FileMap::new();
        virtual_files.insert(source, Arc::<[u8]>::from(&b"virtual source"[..]));
        let prepared = selected
            .with_virtual_files(virtual_files)
            .with_icons(Arc::default());
        assert_eq!(
            prepared.files.read(source, root).unwrap(),
            b"virtual source"
        );
        assert_eq!(
            prepared.files.read(package.entry_id(), root).unwrap(),
            b"module source"
        );
        let builtin = typst::syntax::FileId::new(typst::syntax::RootedPath::new(
            typst::syntax::VirtualRoot::Package("@tola/schema:0.0.0".parse().unwrap()),
            typst::syntax::VirtualPath::new("typst.toml").unwrap(),
        ));
        assert!(prepared.files.read(builtin, root).is_ok());
        let overrides = crate::filesystem::SourceOverrides::new(
            vec![(module.join("lib.typ"), Arc::<str>::from("editor source"))],
            &BuildCancellation::default(),
        )
        .unwrap();
        let editor = prepared.with_source_overrides(&overrides).unwrap();
        assert_eq!(editor.files.read(source, root).unwrap(), b"virtual source");
        assert_eq!(
            editor.files.read(package.entry_id(), root).unwrap(),
            b"editor source"
        );
        assert_eq!(
            prepared.files.read(package.entry_id(), root).unwrap(),
            b"module source"
        );

        let manifest = ReadEvidence::new(
            ReadLocator::ProvidedPackage {
                package: package.spec().clone(),
                path: "typst.toml".into(),
            },
            package.manifest_bytes(),
        );
        assert!(!selected.read_is_process_stable(manifest.locator()));
        assert!(selected.virtual_read_matches(&manifest));
        let changed = LocalPackage::new(
            package.spec().clone(),
            &module,
            typst::syntax::VirtualPath::new("another-entry.typ").unwrap(),
        )
        .unwrap();
        let changed = base.with_local_packages(Arc::new(
            LocalPackages::new([changed], ["tola-modules"]).unwrap(),
        ));
        assert!(!selected.has_same_inputs(&changed));
        assert!(!changed.virtual_read_matches(&manifest));
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
