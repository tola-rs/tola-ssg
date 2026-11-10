//! Site Typst host: Typst resources plus the icons bound for one site.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tola_input::NetworkAccess;
use tola_typst::prelude::*;

use crate::{
    cancellation::{BuildCancellation, BuildCancelled},
    config::ResolvedSiteConfig,
    package,
    resources::BuildResources,
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

/// Builtin package files with the icons bound for this site.
struct SitePackageFiles {
    icons: Arc<tola_icons::IconCollections>,
}

impl tola_typst::FileProvider for SitePackageFiles {
    fn target(&self, id: typst::syntax::FileId) -> Option<tola_typst::FileTarget> {
        package_target(id, Some(&self.icons))
    }

    /// The `tola` namespace belongs to Tola, so only the packages above exist in it.
    fn owned_namespaces(&self) -> &'static [&'static str] {
        &[tola_packages::TOLA_NAMESPACE]
    }
}

/// Typst resources for one site: a Typst host plus the icons bound to it.
#[derive(Clone)]
pub struct SiteHost {
    typst: tola_typst::TypstHost,
    icons: Arc<tola_icons::IconCollections>,
}

impl SiteHost {
    fn build(
        font_dirs: &[&Path],
        package_locations: tola_typst::PackageLocations,
        resources: &BuildResources,
        system_fonts: bool,
        source_boundary: tola_typst::SourceBoundary,
        local_packages: Arc<LocalPackages>,
        cancellation: &BuildCancellation,
    ) -> Result<Self, tola_typst::FontLoadError> {
        let fetch_policy = fetch_policy(resources.network_access());
        let files = FileResolver::from_package_locations(package_locations, fetch_policy)
            .with_provider(TolaPackageFiles)
            .with_source_boundary(source_boundary.clone())
            .with_local_packages(local_packages);
        let fonts = Arc::new(FontStore::with_options(
            FontOptions::new()
                .with_custom_paths(font_dirs)
                .with_system_fonts(system_fonts)
                .with_source_boundary(source_boundary.clone()),
        ));
        fonts.load(&cancellation.bundle_cancellation())?;
        Ok(Self {
            typst: tola_typst::TypstHost::new(fonts, files, resources.file_cache()),
            icons: Arc::default(),
        })
    }

    pub(crate) fn for_config(
        config: &ResolvedSiteConfig,
        resources: &BuildResources,
        local_packages: Arc<tola_typst::LocalPackages>,
        cancellation: &BuildCancellation,
    ) -> Result<Self, tola_typst::FontLoadError> {
        let directories = config.compiler_font_directories(resources);
        let font_dirs: Vec<&Path> = directories.iter().map(PathBuf::as_path).collect();
        Self::build(
            &font_dirs,
            resources.package_locations(config),
            resources,
            config.system_fonts_allowed(resources),
            resources.source_boundary(config),
            local_packages,
            cancellation,
        )
    }

    /// Refuse a package directory the lock does not describe before any byte is read.
    pub(crate) fn with_package_verifier(self, verifier: tola_typst::PackageVerifier) -> Self {
        Self {
            typst: self.typst.with_package_verifier(verifier),
            icons: self.icons,
        }
    }

    /// Whether this host may serve one build of `config` with `resources`.
    pub(crate) fn matches(&self, config: &ResolvedSiteConfig, resources: &BuildResources) -> bool {
        let directories = config.compiler_font_directories(resources);
        self.typst
            .font_options()
            .custom_paths
            .iter()
            .eq(directories.iter().map(PathBuf::as_path))
            && self
                .typst
                .package_locations()
                .is_ok_and(|locations| locations == &resources.package_locations(config))
            && self.typst.font_options().include_system_fonts
                == config.system_fonts_allowed(resources)
            && self.typst.fetch_policy() == fetch_policy(resources.network_access())
            && self.typst.source_boundary() == &resources.source_boundary(config)
            && Arc::ptr_eq(self.typst.file_cache(), &resources.file_cache())
    }

    /// Compiler results can be reused only within the same font-resource generation.
    pub(crate) fn has_same_inputs(&self, other: &Self) -> bool {
        self.typst.has_same_inputs(&other.typst) && Arc::ptr_eq(&self.icons, &other.icons)
    }

    /// Bind validated icons before creating any source sets or candidate files.
    pub(crate) fn with_icons(&self, icons: Arc<tola_icons::IconCollections>) -> Self {
        Self {
            typst: self.typst.with_file_provider(SitePackageFiles {
                icons: Arc::clone(&icons),
            }),
            icons,
        }
    }

    /// Bind the local package tree the site reads through this host.
    pub(crate) fn with_local_packages(self, packages: Arc<LocalPackages>) -> Self {
        Self {
            typst: self.typst.with_local_packages(packages),
            icons: self.icons,
        }
    }

    pub(crate) fn with_source_overrides(
        &self,
        sources: &crate::filesystem::SourceOverrides,
    ) -> typst::diag::FileResult<Self> {
        if sources.is_empty() {
            return Ok(self.clone());
        }
        for path in sources.paths() {
            self.typst.source_boundary().check(path)?;
        }
        Ok(Self {
            typst: self
                .typst
                .with_disk_overrides(sources.sources().map(|(path, source)| {
                    (
                        path.to_path_buf(),
                        Arc::<[u8]>::from(source.text.as_bytes()),
                    )
                }))?,
            icons: Arc::clone(&self.icons),
        })
    }

    pub(crate) fn font_inventory_is_fresh(
        &self,
        cancellation: &BuildCancellation,
    ) -> Result<bool, BuildCancelled> {
        match self
            .typst
            .font_inventory_is_fresh(&cancellation.bundle_cancellation())
        {
            Ok(current) => Ok(current),
            Err(tola_typst::FontLoadError::Cancelled) => Err(BuildCancelled),
            Err(_) => Ok(false),
        }
    }

    pub(crate) fn font_read_paths(&self) -> Vec<PathBuf> {
        self.typst.font_read_paths()
    }

    pub(crate) fn source_boundary(&self) -> &tola_typst::SourceBoundary {
        self.typst.source_boundary()
    }

    /// The resolver every source read of this host uses.
    pub(crate) fn file_resolver(&self) -> Arc<FileResolver> {
        self.typst.file_resolver()
    }

    pub(crate) fn world(
        &self,
        root: &Path,
        main: &Path,
        library: &tola_packages::library::SiteLibrary,
        candidate_files: Arc<FileView>,
        cancellation: &BundleCancellation,
    ) -> Result<TypstWorld, WorldBuildError> {
        self.typst
            .world(root, main, library.shared(), candidate_files, cancellation)
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
        self.typst
            .world_with_id(root, main, library.shared(), candidate_files, cancellation)
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
        self.typst.candidate_files(sources)
    }

    pub(crate) fn read_is_process_stable(&self, locator: &ReadLocator) -> bool {
        match locator {
            ReadLocator::ProvidedPackage { package, path } => {
                self.typst.virtual_bytes(package, path).is_none()
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
                if let Some(bytes) = self.typst.virtual_bytes(package, path) {
                    return ContentDigest::of(&bytes) == evidence.digest();
                }
                match package::icon_file_target(&self.icons, package, path) {
                    tola_typst::FileTarget::Bytes(bytes) => {
                        ContentDigest::of(&bytes) == evidence.digest()
                    }
                    _ => {
                        path == Path::new("typst.toml")
                            && self.typst.local_packages().package(package).is_some_and(
                                |selected| {
                                    ContentDigest::of(selected.manifest_bytes())
                                        == evidence.digest()
                                },
                            )
                    }
                }
            }
            _ => false,
        }
    }

    /// Keep parsed imports across short runs of builds that reuse their compilation.
    pub(crate) const RETAINED_FILE_CACHE_EPOCHS: usize =
        tola_typst::TypstHost::RETAINED_FILE_CACHE_EPOCHS;

    pub(crate) fn evict_stale_file_cache_entries(&self, max_age: usize) {
        self.typst.evict_stale_file_cache_entries(max_age);
    }
}

fn fetch_policy(network: NetworkAccess) -> tola_typst::PackageFetchPolicy {
    match network {
        NetworkAccess::Allowed => tola_typst::PackageFetchPolicy::AllowNetwork,
        NetworkAccess::Denied => tola_typst::PackageFetchPolicy::LocalOnly,
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::compiler::tests::compiler_host_with;
    use tempfile::TempDir;

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
            ordinary
                .file_resolver()
                .read(id, config.get_root())
                .unwrap(),
            b"host-only"
        );
        let pure_resources = resources.with_input_scope(tola_input::InputScope::Pure);
        assert!(!ordinary.matches(&config, &pure_resources));
        let pure = compiler_host_with(&config, &pure_resources).unwrap();
        assert!(pure.file_resolver().read(id, config.get_root()).is_err());
    }
}
