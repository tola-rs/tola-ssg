//! Read-only runtime configuration after invocation overrides and path resolution.

use std::path::{Path, PathBuf};

use super::section::SiteSectionConfig;
use super::{BuildSectionConfig, ConfigDiagnostic};
use crate::resources::BuildResources;
use tola_address::{SiteOrigin, SiteUrlMount, UrlPath, browser_url};

/// Site configuration after invocation overrides, validation, and path resolution.
///
/// Construct this through [`super::loading::load_site_config`],
/// [`super::loading::resolve_site_config`], or [`super::SiteConfigSchema::resolve`].
/// Sections are read-only to preserve validated paths and URLs.
#[derive(Debug, Clone)]
pub struct ResolvedSiteConfig {
    /// Typst package roots frozen for this process invocation.
    pub(crate) package_locations: tola_typst::PackageLocations,

    /// Absolute logical path identifying the configuration source.
    pub(crate) config_path: PathBuf,

    /// Site root: the configuration file's parent directory.
    pub(super) root: PathBuf,

    /// Non-fatal validation warnings produced from this exact configuration.
    pub(super) warnings: Vec<ConfigDiagnostic>,

    /// Where the configuration source writes each key, so a warning can name its line.
    pub(super) positions: std::sync::Arc<crate::config::source::ConfigPositions>,

    pub(crate) site: SiteSectionConfig,

    pub(crate) build: BuildSectionConfig,

    pub(crate) assets: crate::config::section::AssetsConfig,

    pub(crate) typst: crate::config::section::TypstSectionConfig,

    pub(crate) icons: crate::config::section::IconsConfig,

    pub(crate) vendor: crate::config::section::VendorConfig,

    // Refresh and candidate configs retain the declared workspace, not a path derived from their selection.
    pub(crate) vendor_workspace: Option<PathBuf>,
    // Only with_vendor_root grants a candidate; changing a section cannot broaden that grant.
    pub(crate) vendor_candidate: Option<PathBuf>,
}

/// One directory a resolved configuration's own writing produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedExclusion {
    /// The directory itself, which a change filter excludes whole.
    pub directory: PathBuf,
    /// The subtree a read may still reach while it is prepared, absent when none is being prepared.
    pub candidate: Option<PathBuf>,
}

impl ResolvedSiteConfig {
    /// Where the configuration source writes each key.
    pub(crate) fn positions(&self) -> Option<&crate::config::source::ConfigPositions> {
        Some(&self.positions)
    }

    /// Every input this configuration owns that the output root must not contain.
    ///
    /// Producers supply dynamic Bundle reads and external inputs that configuration alone cannot
    /// enumerate; those are added by the caller that knows them.
    pub(crate) fn protected_input_paths(&self) -> Vec<(&'static str, &std::path::Path)> {
        let mut protected = vec![
            ("content-dir", self.build.content_dir.as_path()),
            ("build entry", self.build.entry.as_path()),
        ];
        if !self.config_path.as_os_str().is_empty() {
            protected.push(("config", self.config_path.as_path()));
        }
        protected.extend(self.assets.tree_sources().map(|path| ("asset", path)));
        protected.extend(self.assets.file_sources().map(|path| ("asset", path)));
        protected.extend(
            self.typst
                .fonts
                .paths
                .iter()
                .map(|path| ("font directory", path.as_path())),
        );
        protected.extend(self.icons.local_paths().map(|path| ("icon source", path)));
        if let Some(path) = &self.vendor.path {
            protected.push(("vendor", path.as_path()));
        }
        if let Some(location) = self.package_locations.data() {
            protected.push(("Typst package", location.root()));
        }
        if let Some(location) = self.package_locations.cache() {
            protected.push(("Typst package cache", location.root()));
        }
        protected
    }

    /// Absolute logical path identifying the configuration source.
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Validated site identity and template values.
    pub fn site(&self) -> &SiteSectionConfig {
        &self.site
    }

    /// Build settings whose filesystem paths have been resolved against the site root.
    pub fn build(&self) -> &BuildSectionConfig {
        &self.build
    }

    pub fn assets(&self) -> &crate::config::section::AssetsConfig {
        &self.assets
    }

    pub fn fonts(&self) -> &crate::config::section::FontsConfig {
        &self.typst.fonts
    }

    /// Whether system fonts may join a build under `resources`.
    ///
    /// The site's `[typst.fonts] system` and the invocation's scope each decide it, and either can
    /// refuse what the other allows.
    pub fn system_fonts_allowed(&self, resources: &BuildResources) -> bool {
        self.typst.fonts.system && resources.include_system_fonts()
    }

    pub fn icons(&self) -> &crate::config::section::IconsConfig {
        &self.icons
    }

    /// Vendor settings whose directory has been resolved against the site root.
    pub fn vendor(&self) -> &crate::config::section::VendorConfig {
        &self.vendor
    }

    /// Site root directory.
    pub fn get_root(&self) -> &Path {
        &self.root
    }

    /// Strip the site root, or return the path unchanged when it is outside the root.
    ///
    /// [`crate::filesystem::display_path`] is the rendering boundary; this accessor only borrows
    /// the relative path for callers that need path operations rather than text.
    pub fn root_relative(&self, path: impl AsRef<Path>) -> PathBuf {
        crate::filesystem::root_relative(path.as_ref(), &self.root).to_path_buf()
    }

    /// Typst package roots fixed for this invocation.
    pub fn package_locations(&self) -> &tola_typst::PackageLocations {
        &self.package_locations
    }

    /// Validate a complete vendor candidate without reading the previously selected vendor.
    /// `None` resolves a refresh from the site's other declared and host roots.
    pub fn with_vendor_root(&self, path: Option<PathBuf>) -> anyhow::Result<Self> {
        let mut config = self.clone();
        config.vendor.path = path;
        config.vendor_candidate.clone_from(&config.vendor.path);
        if let Some(path) = &config.vendor.path {
            anyhow::ensure!(
                path.is_absolute(),
                "the vendor directory path must be absolute"
            );
            crate::resources::source_boundary(&config, crate::InputScope::Pure).check(path)?;
        }
        let mut roots = self
            .package_locations
            .declared()
            .iter()
            .map(|location| location.root().to_path_buf())
            .collect::<Vec<_>>();
        if let Some(old) = self.vendor.typst_packages()
            && let Some(index) = roots.iter().rposition(|root| root == &old)
        {
            roots.remove(index);
        }
        roots.extend(config.vendor.typst_packages());
        config.package_locations = self.package_locations.clone().with_declared_roots(roots)?;
        let boundary = crate::filesystem::OutputBoundary::resolve(
            config.get_root(),
            &config.build.publish_dir,
            config.protected_input_paths(),
        )?;
        config.build.publish_dir = boundary.output;
        Ok(config)
    }

    /// The directories this configuration's own writing produces, which no change may turn into a
    /// revision: the output tree, the publication workspace beside it, and the vendor workspace.
    ///
    /// The candidate of an exclusion is a read's grant, not a change's: the `resources` module's
    /// source boundary grants it back so a read may reach a candidate being prepared, while a
    /// change filter excludes the whole directory either way.
    pub fn generated_exclusions(&self) -> Vec<GeneratedExclusion> {
        let mut exclusions = vec![GeneratedExclusion {
            directory: self.build.publish_dir.clone(),
            candidate: None,
        }];
        if let Some(workspace) = crate::filesystem::publication_workspace(&self.build.publish_dir) {
            exclusions.push(GeneratedExclusion {
                directory: workspace,
                candidate: None,
            });
        }
        if let Some(workspace) = &self.vendor_workspace {
            exclusions.push(GeneratedExclusion {
                directory: workspace.clone(),
                candidate: self.vendor_candidate.clone(),
            });
        }
        exclusions
    }

    /// Browser deployment mount for site-root URLs.
    ///
    /// Derived from the configured base path on every read, because a reuse decision compares
    /// `site.base_path` while these URLs are rendered from the mount.
    pub fn url_mount(&self) -> SiteUrlMount {
        SiteUrlMount::from_base_path(&self.site.base_path).expect("site base path was validated")
    }

    /// Canonical site root URL when an origin is configured.
    pub fn site_url(&self) -> Option<String> {
        self.site.url()
    }

    /// Validated canonical origin, absent for a relative-only site.
    ///
    /// Derived from the configured value on every read, so the origin that renders a URL can
    /// never disagree with the origin a reuse decision compares.
    pub fn site_origin(&self) -> Option<SiteOrigin> {
        self.site
            .origin
            .as_deref()
            .map(|origin| SiteOrigin::parse(origin).expect("site origin was validated"))
    }

    /// Build the browser URL of one site-root route, mounted and absolute when configured.
    pub fn canonical_url(&self, path: &UrlPath) -> String {
        browser_url(path, &self.url_mount(), self.site_origin().as_ref())
    }

    /// Non-fatal diagnostics produced from this exact effective configuration.
    pub fn warnings(&self) -> &[ConfigDiagnostic] {
        &self.warnings
    }
}

#[cfg(test)]
mod tests {
    use crate::config::loading::{BuildOverrides, load_site_config};

    /// The exclusions name the state a build of this configuration writes: the output tree, the
    /// publication workspace beside it, and the vendor workspace.
    #[test]
    fn generated_exclusions_name_the_output_and_vendor_state() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        std::fs::write(root.join("tola.toml"), "[vendor]\npath = \"vendor\"\n").unwrap();
        std::fs::create_dir_all(root.join("content")).unwrap();
        let config = load_site_config(
            Some(&root.join("tola.toml")),
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .unwrap()
        .into_config();

        let exclusions = config.generated_exclusions();
        assert_eq!(exclusions.len(), 3, "{exclusions:?}");
        assert_eq!(exclusions[0].directory, config.build().publish_dir);
        assert_eq!(exclusions[0].candidate, None);
        assert_eq!(
            exclusions[1].directory,
            crate::filesystem::publication_workspace(&config.build().publish_dir)
                .expect("an output tree has a publication workspace beside it")
        );
        assert_eq!(exclusions[1].candidate, None);
        assert_eq!(
            exclusions[2].directory,
            config
                .vendor
                .workspace_path()
                .expect("a declared vendor path")
        );
        assert_eq!(exclusions[2].candidate, None);
    }
}
