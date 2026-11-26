//! Core site configuration: the `tola.toml` schema, its section definitions, the
//! decoding that retains source presence, and the path and URL meaning resolved
//! configuration has. Sections a host owns are left to that host.

mod boundaries;
pub mod diagnostic;
mod fragment;
pub mod loading;
mod site;
pub(crate) use site::ModuleFontDirectory;
mod schema;
pub mod section;
mod source;

pub use schema::{SiteConfigSchema, field_documentation, field_paths, field_values, section_help};
pub(crate) use site::GeneratedExclusion;
pub use site::{SiteConfig, VendorRootError};
pub use source::{
    ConfigApplication, ConfigFieldOrigins, ConfigFile, ConfigFileState, ConfigInclude,
    ConfigOrigin, ConfigSource, ConfigSourceError, ParsedConfig,
};

pub use section::build::{BuildEntry, BuildSectionConfig, BuildSettings, ReferenceLevel};
pub use section::{AssetsConfig, FontsConfig, IconCollectionSource, IconsConfig, Sha256Digest};

pub use tola_config::{
    ConfigDiagnostic, ConfigDiagnosticSeverity, ConfigDiagnosticTag, ConfigDiagnostics,
    ConfigError, ConfigPresence, ConfigSourceRefusal, FieldPath,
};

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use anyhow::Result;

    use super::*;
    use crate::config::loading::BuildOverrides;
    use crate::config::section::SiteSectionConfig;

    pub(crate) fn config_source(extra: &str) -> String {
        format!(
            "[{}]\ntitle = \"Test\"\ndescription = \"Test\"\n{extra}",
            SiteSectionConfig::TEMPLATE_SECTION
        )
    }

    /// A `[icons]` declaration that reads one local JSON collection from `path`.
    pub(crate) fn local_json_icons(path: &str) -> String {
        format!("[icons.collections.acme]\nsource-type = \"local-json\"\npath = \"{path}\"\n")
    }

    /// A `tola.toml` source that has its Typst packages under `vendor`.
    pub(crate) const VENDORED_SITE_SOURCE: &str = "[vendor]\npath = \"vendor\"\n";

    /// A resolved configuration whose resolved paths include this value's temporary site
    /// directory, so they stay valid exactly as long as the value lives.
    pub(crate) struct OwnedSiteConfig {
        pub(crate) config: SiteConfig,
        _site_directory: tempfile::TempDir,
    }

    /// Load a resolved config in a test-owned site directory.
    ///
    /// Keep the directory alive while using the config. Use `root()` for resolved
    /// paths; normalization may change the lexical form of `site_directory`.
    pub(crate) fn load_test_config(site_directory: &Path, source: &str) -> SiteConfig {
        std::fs::create_dir_all(site_directory).unwrap();
        let config_path = site_directory.join("tola.toml");
        std::fs::write(&config_path, source).unwrap();
        crate::config::loading::load_site_config(
            Some(&config_path),
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .unwrap()
        .into_config()
    }

    impl OwnedSiteConfig {
        pub(crate) fn new(source: &str) -> Self {
            let site_directory = tempfile::TempDir::new().unwrap();
            let config = load_test_config(site_directory.path(), source);
            Self {
                config,
                _site_directory: site_directory,
            }
        }

        pub(crate) fn with_invocation(
            source: &str,
            package_locations: tola_typst::PackageLocations,
            build: BuildOverrides,
        ) -> Self {
            let site_directory = tempfile::TempDir::new().unwrap();
            let config_path = site_directory.path().join("tola.toml");
            std::fs::write(&config_path, source).unwrap();
            let config = crate::config::loading::load_site_config(
                Some(&config_path),
                package_locations,
                &build,
            )
            .unwrap()
            .into_config();
            Self {
                config,
                _site_directory: site_directory,
            }
        }
    }

    fn resolved_config(source: &str, build: BuildOverrides) -> OwnedSiteConfig {
        OwnedSiteConfig::with_invocation(source, tola_typst::PackageLocations::default(), build)
    }

    fn try_load_site_config_at(config_path: &Path, extra: &str) -> Result<SiteConfig> {
        std::fs::write(config_path, config_source(extra)).unwrap();
        crate::config::loading::load_site_config(
            Some(config_path),
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .map(crate::config::loading::SiteConfigWithSourceDigest::into_config)
    }

    /// The diagnostics a configuration failure has, which every refusal renders.
    pub(crate) fn attached_diagnostics(error: &anyhow::Error) -> &[tola_diagnostic::Diagnostic] {
        tola_diagnostic::attached(error).expect("a configuration failure has diagnostics")
    }

    /// The attached diagnostic whose message contains `expected`.
    pub(crate) fn diagnostic_mentioning<'a>(
        error: &'a anyhow::Error,
        expected: &str,
    ) -> &'a tola_diagnostic::Diagnostic {
        attached_diagnostics(error)
            .iter()
            .find(|diagnostic| diagnostic.message.contains(expected))
            .unwrap_or_else(|| panic!("no diagnostic mentions {expected:?}: {error:#}"))
    }

    #[cfg(unix)]
    #[test]
    fn full_load_preserves_logical_paths() {
        use std::os::unix::fs::symlink;

        const INPUTS: &[&str] = &[
            "entry.typ",
            "content",
            "tree-assets",
            "exact.txt",
            "fonts",
            "icons",
            "packages",
            "package-cache",
        ];

        let dir = tempfile::TempDir::new().unwrap();
        let first = dir.path().join("input-set-a");
        let second = dir.path().join("input-set-b");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();

        let src = config_source(
            r#"
[build]
entry = "entry.typ"
content-dir = "content"
publish-dir = "public"

[assets]
trees = [{ source = "tree-assets", url-prefix = "/assets" }]
files = [{ source = "exact.txt", url = "/exact.txt" }]

[typst.fonts]
paths = ["fonts"]

[icons.collections.acme]
source-type = "local-svg-dir"
path = "icons"
"#,
        );

        for target in [&first, &second] {
            std::fs::write(target.join("tola.toml"), &src).unwrap();
            std::fs::write(target.join("entry.typ"), "").unwrap();
            std::fs::write(target.join("exact.txt"), "").unwrap();
            for name in [
                "content",
                "tree-assets",
                "fonts",
                "icons",
                "packages",
                "package-cache",
            ] {
                std::fs::create_dir(target.join(name)).unwrap();
            }
        }

        for name in INPUTS {
            symlink(first.join(name), dir.path().join(name)).unwrap();
        }
        let cfg_path = dir.path().join("tola.toml");
        symlink(first.join("tola.toml"), &cfg_path).unwrap();

        let pkg_locs = tola_typst::PackageLocations::discover(
            Some(dir.path().join("packages")),
            Some(dir.path().join("package-cache")),
        )
        .unwrap();
        let snapshot = |cfg: &SiteConfig| {
            vec![
                cfg.config_path.clone(),
                cfg.build.entry.clone(),
                cfg.build.content_dir.clone(),
                cfg.assets.trees[0].source().to_path_buf(),
                cfg.assets.files[0].source().to_path_buf(),
                cfg.typst.fonts.paths[0].clone(),
                cfg.icons.local_paths().next().unwrap().to_path_buf(),
                cfg.package_locations().data().unwrap().root().to_path_buf(),
                cfg.package_locations()
                    .cache()
                    .unwrap()
                    .root()
                    .to_path_buf(),
            ]
        };

        let before = crate::config::loading::load_site_config(
            Some(&cfg_path),
            pkg_locs.clone(),
            &BuildOverrides::default(),
        )
        .unwrap()
        .into_config();
        let logical_before = snapshot(&before);
        let physical_content_before = std::fs::canonicalize(&before.build.content_dir).unwrap();

        for name in INPUTS {
            std::fs::remove_file(dir.path().join(name)).unwrap();
            symlink(second.join(name), dir.path().join(name)).unwrap();
        }
        std::fs::remove_file(&cfg_path).unwrap();
        symlink(second.join("tola.toml"), &cfg_path).unwrap();

        let after = crate::config::loading::load_site_config(
            Some(&cfg_path),
            pkg_locs,
            &BuildOverrides::default(),
        )
        .unwrap()
        .into_config();

        assert_eq!(snapshot(&after), logical_before);
        assert_ne!(
            std::fs::canonicalize(&after.build.content_dir).unwrap(),
            physical_content_before
        );
        assert_eq!(
            std::fs::canonicalize(&after.build.content_dir).unwrap(),
            std::fs::canonicalize(second.join("content")).unwrap()
        );
    }

    #[test]
    fn invocation_overrides_site_identity() {
        let build = BuildOverrides {
            origin: Some("https://EXAMPLE.com:443/".into()),
            base_path: Some("/\u{9884}\u{89c8}/".into()),
            ..Default::default()
        };
        let site_cfg = resolved_config(
            "[site]\norigin = \"invalid\"\nbase-path = \"/unfinished\"",
            build,
        );

        assert_eq!(
            site_cfg.config.site.origin.as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            site_cfg.config.site.base_path.base_path(),
            "/%E9%A2%84%E8%A7%88/"
        );
        assert_eq!(
            site_cfg.config.site_url().as_deref(),
            Some("https://example.com/%E9%A2%84%E8%A7%88/")
        );
        assert_eq!(site_cfg.config.url_mount().as_str(), "%E9%A2%84%E8%A7%88");
    }

    #[test]
    fn icon_source_inside_output_rejected() {
        let root = tempfile::TempDir::new().unwrap();
        let cfg_path = root.path().join("tola.toml");
        let err = try_load_site_config_at(
            &cfg_path,
            &format!(
                "[build]\npublish-dir = \"public\"\n\n{}",
                local_json_icons("public/icons/acme.json")
            ),
        )
        .unwrap_err();
        use crate::filesystem::OutputBoundaryError;
        assert!(err.chain().any(|cause| matches!(
            cause.downcast_ref::<OutputBoundaryError>(),
            Some(OutputBoundaryError::ProtectedPath { protected, .. })
                if protected.ends_with("public/icons/acme.json")
        )));
    }

    #[cfg(all(unix, not(target_vendor = "apple")))]
    #[test]
    fn non_utf8_root_icon_source() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join(OsString::from_vec(b"site-\xff".to_vec()));
        std::fs::create_dir(&root).unwrap();
        let cfg_path = root.join("tola.toml");
        let loaded =
            try_load_site_config_at(&cfg_path, &local_json_icons("icons/acme.json")).unwrap();

        assert_eq!(
            loaded.icons.local_paths().next(),
            Some(root.join("icons/acme.json").as_path())
        );
    }

    #[cfg(unix)]
    #[test]
    fn content_symlink_to_internal_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let internal_content = root.join(".tola/content");
        std::fs::create_dir_all(&internal_content).unwrap();
        std::os::unix::fs::symlink(&internal_content, root.join("content")).unwrap();

        let err = try_load_site_config_at(&root.join("tola.toml"), "").unwrap_err();
        let diag = attached_diagnostics(&err);
        assert!(diag.iter().any(|d| {
            d.code == "config.invalid"
                && d.notes
                    .iter()
                    .any(|n| n == BuildSectionConfig::FIELDS.content_dir.as_str())
        }));
    }

    #[cfg(unix)]
    #[test]
    fn icon_symlink_to_internal_refused() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let internal = root.join(tola_input::INTERNAL_DIR);
        std::fs::create_dir_all(internal.join("icons")).unwrap();
        symlink(internal.join("icons"), root.join("icons")).unwrap();
        let cfg_path = root.join("tola.toml");

        let err = try_load_site_config_at(
            &cfg_path,
            "[icons.collections.acme]\nsource-type = \"local-svg-dir\"\npath = \"icons\"\n",
        )
        .unwrap_err();
        let diag = attached_diagnostics(&err);

        assert!(diag.iter().any(|d| {
            d.code == tola_diagnostic::codes::config::INVALID
                && d.notes
                    .iter()
                    .any(|n| n == IconsConfig::FIELDS.collections.as_str())
        }));
    }

    #[test]
    fn explicit_package_locations_precedence() {
        let dir = tempfile::TempDir::new().unwrap();
        let site_cfg = OwnedSiteConfig::with_invocation(
            "",
            tola_typst::PackageLocations::discover(
                Some(dir.path().join("packages")),
                Some(dir.path().join("cache")),
            )
            .unwrap(),
            BuildOverrides::default(),
        );
        let cfg = &site_cfg.config;

        assert_eq!(
            cfg.package_locations().data().unwrap().root(),
            dir.path().join("packages")
        );
        assert_eq!(
            cfg.package_locations().cache().unwrap().root(),
            dir.path().join("cache")
        );
        assert_eq!(
            cfg.package_locations().data().unwrap().source(),
            tola_typst::PackageLocationSource::Explicit
        );
        assert_eq!(
            cfg.package_locations().cache().unwrap().source(),
            tola_typst::PackageLocationSource::Explicit
        );
    }
}
