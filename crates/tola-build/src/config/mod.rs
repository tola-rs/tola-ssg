//! Core site configuration: the `tola.toml` schema, its section definitions, the
//! decoding that retains source presence, and the path and URL meaning resolved
//! configuration has. Sections a host owns are left to that host.

pub mod diagnostic;
pub mod loading;
mod resolved;
mod schema;
pub mod section;
mod source;

pub use resolved::ResolvedSiteConfig;
pub use schema::{SiteConfigSchema, field_documentation, field_paths, field_values, section_help};
pub use source::{ConfigSource, ParsedConfig};

pub use section::build::{BuildSectionConfig, ReferenceLevel};
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
    use crate::config::diagnostic::UNKNOWN_FIELD_HELP;
    use crate::config::loading::BuildOverrides;
    use crate::config::section::SiteSectionConfig;

    pub(crate) fn config_source(extra: &str) -> String {
        format!(
            "[{}]\ntitle = \"Test\"\ndescription = \"Test\"\n{extra}",
            SiteSectionConfig::TEMPLATE_SECTION
        )
    }

    pub(crate) fn config_schema(extra: &str) -> SiteConfigSchema {
        let config = config_source(extra);
        SiteConfigSchema::parse(
            Path::new("tola.toml"),
            &config,
            crate::resources::InputScope::Online,
        )
        .unwrap()
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
        pub(crate) config: ResolvedSiteConfig,
        _site_directory: tempfile::TempDir,
    }

    /// Load a resolved config in a test-owned site directory.
    ///
    /// Keep the directory alive while using the config. Use `get_root()` for resolved
    /// paths; normalization may change the lexical form of `site_directory`.
    pub(crate) fn load_test_config(site_directory: &Path, source: &str) -> ResolvedSiteConfig {
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

    fn try_load_site_config_at(config_path: &Path, extra: &str) -> Result<ResolvedSiteConfig> {
        std::fs::write(config_path, config_source(extra)).unwrap();
        crate::config::loading::load_site_config(
            Some(config_path),
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .map(crate::config::loading::SiteConfigWithSourceDigest::into_config)
    }

    /// The diagnostics a configuration failure has, which every refusal renders.
    pub(crate) fn attached_diagnostics(error: &anyhow::Error) -> &[crate::diagnostic::Diagnostic] {
        crate::diagnostic::attached(error).expect("a configuration failure has diagnostics")
    }

    /// The attached diagnostic whose message contains `expected`.
    pub(crate) fn diagnostic_mentioning<'a>(
        error: &'a anyhow::Error,
        expected: &str,
    ) -> &'a crate::diagnostic::Diagnostic {
        attached_diagnostics(error)
            .iter()
            .find(|diagnostic| diagnostic.message.contains(expected))
            .unwrap_or_else(|| panic!("no diagnostic mentions {expected:?}: {error:#}"))
    }

    /// The message an author reads, which only the attached diagnostic has.
    pub(crate) fn diagnostic_message(error: &anyhow::Error) -> &str {
        crate::diagnostic::attached(error)
            .and_then(|diagnostics| diagnostics.first())
            .map(|diagnostic| diagnostic.message.as_str())
            .unwrap_or_default()
    }

    /// The code of the first diagnostic `error` has.
    #[cfg(unix)]
    pub(crate) fn diagnostic_code(
        error: &anyhow::Error,
    ) -> Option<crate::diagnostic::DiagnosticCode> {
        crate::diagnostic::attached(error)?
            .first()
            .map(|diagnostic| diagnostic.code)
    }

    #[test]
    fn unset_fields_take_schema_defaults() {
        let site_config = OwnedSiteConfig::new("");

        assert!(site_config.config.site.title.is_empty());
        assert!(site_config.config.site.description.is_empty());
        assert!(site_config.config.build.minify.html);
        assert!(site_config.config.build.minify.css);
        assert!(site_config.config.build.minify.javascript);
    }

    #[test]
    fn declared_minify_disables_every_language() {
        let site_config =
            OwnedSiteConfig::new("[build.minify]\nhtml = false\ncss = false\njavascript = false\n");

        assert!(!site_config.config.build.minify.html);
        assert!(!site_config.config.build.minify.css);
        assert!(!site_config.config.build.minify.javascript);
    }

    #[cfg(unix)]
    #[test]
    fn full_load_keeps_logical_input_paths() {
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

        let directory = tempfile::TempDir::new().unwrap();
        let first = directory.path().join("input-set-a");
        let second = directory.path().join("input-set-b");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();

        let config_source = config_source(
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
            std::fs::write(target.join("tola.toml"), &config_source).unwrap();
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
            symlink(first.join(name), directory.path().join(name)).unwrap();
        }
        let config_path = directory.path().join("tola.toml");
        symlink(first.join("tola.toml"), &config_path).unwrap();

        let package_locations = tola_typst::PackageLocations::discover(
            Some(directory.path().join("packages")),
            Some(directory.path().join("package-cache")),
        )
        .unwrap();
        let snapshot = |config: &ResolvedSiteConfig| {
            vec![
                config.config_path.clone(),
                config.build.entry.clone(),
                config.build.content_dir.clone(),
                config.assets.trees[0].source().to_path_buf(),
                config.assets.files[0].source().to_path_buf(),
                config.typst.fonts.paths[0].clone(),
                config.icons.local_paths().next().unwrap().to_path_buf(),
                config
                    .package_locations()
                    .data()
                    .unwrap()
                    .root()
                    .to_path_buf(),
                config
                    .package_locations()
                    .cache()
                    .unwrap()
                    .root()
                    .to_path_buf(),
            ]
        };

        let before = crate::config::loading::load_site_config(
            Some(&config_path),
            package_locations.clone(),
            &BuildOverrides::default(),
        )
        .unwrap()
        .into_config();
        let logical_before = snapshot(&before);
        let physical_content_before = std::fs::canonicalize(&before.build.content_dir).unwrap();

        for name in INPUTS {
            std::fs::remove_file(directory.path().join(name)).unwrap();
            symlink(second.join(name), directory.path().join(name)).unwrap();
        }
        std::fs::remove_file(&config_path).unwrap();
        symlink(second.join("tola.toml"), &config_path).unwrap();

        let after = crate::config::loading::load_site_config(
            Some(&config_path),
            package_locations,
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
    fn parse_reports_unknown_fields() {
        // An unknown key has two reports: one inside a core section is collected by field path
        // and answered by the root report — which a whole unknown section or a removed root key
        // also reaches — while one inside a section that denies unknown fields fails that
        // table's own deserialization and is rewritten by `schema_error` (see
        // `unknown_hook_field_names_allowed_fields_and_help`).
        let content = config_source("[build]\nunknown_field = \"value\"");
        let error = SiteConfigSchema::parse(
            Path::new("tola.toml"),
            &content,
            crate::resources::InputScope::Online,
        )
        .unwrap_err();
        let message = error.to_string();

        assert!(
            message.contains("unknown configuration fields:"),
            "{message}"
        );
        assert!(message.contains("build.unknown_field"), "{message}");

        let diagnostic = &attached_diagnostics(&error)[0];
        let [help] = diagnostic.help.as_slice() else {
            panic!("an unknown field has one help: {diagnostic:?}");
        };
        assert_eq!(help.message, UNKNOWN_FIELD_HELP);
    }

    #[test]
    fn unknown_hook_field_names_allowed_fields_and_help() {
        let content = config_source("[build.hooks]\nunknown_stage = true");
        let error = SiteConfigSchema::parse(
            Path::new("tola.toml"),
            &content,
            crate::resources::InputScope::Online,
        )
        .unwrap_err();
        let diagnostic = &attached_diagnostics(&error)[0];

        assert!(
            diagnostic.message.contains("`unknown_stage`"),
            "{}",
            diagnostic.message
        );
        assert!(
            diagnostic.message.contains("`before-build`"),
            "{}",
            diagnostic.message
        );
        let [help] = diagnostic.help.as_slice() else {
            panic!("an unknown field has one help: {diagnostic:?}");
        };
        assert_eq!(help.message, UNKNOWN_FIELD_HELP);
    }

    #[test]
    fn misspelled_site_field_is_reported() {
        let content = r#"[site]
title = "Test"
titel = "misspelled"

[site.extra]
theme = "night"
"#;
        let error = SiteConfigSchema::parse(
            Path::new("tola.toml"),
            content,
            crate::resources::InputScope::Online,
        )
        .unwrap_err();
        assert!(error.to_string().contains("site.titel"));
        let schema = SiteConfigSchema::parse(
            Path::new("tola.toml"),
            &content.replace("titel = \"misspelled\"\n", ""),
            crate::resources::InputScope::Online,
        )
        .unwrap();

        assert_eq!(
            schema.site.extra.get("theme"),
            Some(&toml::Value::String("night".into()))
        );
    }

    #[test]
    fn declared_paths_parse_from_toml() {
        let config = config_schema(
            r#"
[build]
entry = "entries/post.typ"
content-dir = "content"
"#,
        );

        assert_eq!(config.build.entry, Path::new("entries/post.typ"));
        assert_eq!(config.build.content_dir, Path::new("content"));
    }

    #[test]
    fn declared_paths_reject_other_forms() {
        for declaration in [
            "[build]\nentry = [\"site.typ\"]\n",
            "[build]\ncontent = [\"content\"]\n",
            "[build]\ncontent = { root = \"content\" }\n",
        ] {
            let content = config_source(declaration);
            assert!(
                SiteConfigSchema::parse(
                    Path::new("tola.toml"),
                    &content,
                    crate::resources::InputScope::Online,
                )
                .is_err(),
                "accepted non-path declaration: {declaration}"
            );
        }
    }

    #[test]
    fn url_mount_follows_the_site_base_path() {
        for (base_path, expected) in [("/", None), ("/docs/blog/", Some("docs/blog"))] {
            let site_config = resolved_config(
                &format!("[site]\norigin = \"https://example.com\"\nbase-path = \"{base_path}\""),
                BuildOverrides::default(),
            );

            match expected {
                Some(mount) => assert_eq!(site_config.config.url_mount().as_str(), mount),
                None => assert!(site_config.config.url_mount().is_root()),
            }
        }
    }

    #[test]
    fn invocation_overrides_fix_site_identity() {
        let build = BuildOverrides {
            origin: Some("https://EXAMPLE.com:443/".into()),
            base_path: Some("/\u{9884}\u{89c8}/".into()),
            ..Default::default()
        };
        let site_config = resolved_config(
            "[site]\norigin = \"https://old.example\"\nbase-path = \"/old/\"",
            build,
        );

        assert_eq!(
            site_config.config.site.origin.as_deref(),
            Some("https://example.com")
        );
        assert_eq!(site_config.config.site.base_path, "/%E9%A2%84%E8%A7%88/");
        assert_eq!(
            site_config.config.site_url().as_deref(),
            Some("https://example.com/%E9%A2%84%E8%A7%88/")
        );
        assert_eq!(
            site_config.config.url_mount().as_str(),
            "%E9%A2%84%E8%A7%88"
        );
    }

    #[test]
    fn full_load_keeps_local_icon_sources() {
        let root = tempfile::TempDir::new().unwrap();
        let config_path = root.path().join("tola.toml");
        let loaded =
            try_load_site_config_at(&config_path, &local_json_icons("icons/acme.json")).unwrap();
        let expected = loaded.get_root().join("icons/acme.json");

        assert_eq!(loaded.icons.local_paths().next(), Some(expected.as_path()));
    }

    #[test]
    fn icon_source_inside_output_is_rejected() {
        let root = tempfile::TempDir::new().unwrap();
        let config_path = root.path().join("tola.toml");
        let error = try_load_site_config_at(
            &config_path,
            &format!(
                "[build]\npublish-dir = \"public\"\n\n{}",
                local_json_icons("public/icons/acme.json")
            ),
        )
        .unwrap_err();
        let message = diagnostic_message(&error);

        assert!(message.contains("overlap the icon source at"), "{message}");
    }

    #[cfg(all(unix, not(target_vendor = "apple")))]
    #[test]
    fn non_utf8_root_survives_in_icon_source() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let directory = tempfile::TempDir::new().unwrap();
        let root = directory
            .path()
            .join(OsString::from_vec(b"site-\xff".to_vec()));
        std::fs::create_dir(&root).unwrap();
        let config_path = root.join("tola.toml");
        let loaded =
            try_load_site_config_at(&config_path, &local_json_icons("icons/acme.json")).unwrap();

        assert_eq!(
            loaded.icons.local_paths().next(),
            Some(root.join("icons/acme.json").as_path())
        );
    }

    #[cfg(unix)]
    #[test]
    fn content_symlink_into_internal_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let internal_content = root.join(".tola/content");
        std::fs::create_dir_all(&internal_content).unwrap();
        std::os::unix::fs::symlink(&internal_content, root.join("content")).unwrap();

        let error = try_load_site_config_at(&root.join("tola.toml"), "").unwrap_err();
        let diagnostics = attached_diagnostics(&error);
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "config.invalid"
                && diagnostic
                    .notes
                    .iter()
                    .any(|note| note == BuildSectionConfig::FIELDS.content_dir.as_str())
        }));
    }

    #[cfg(unix)]
    #[test]
    fn input_symlinks_into_internal_are_refused() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let internal = root.join(crate::filesystem::INTERNAL_DIR);
        std::fs::create_dir_all(internal.join("icons")).unwrap();
        symlink(internal.join("icons"), root.join("icons")).unwrap();
        let config_path = root.join("tola.toml");

        let error = try_load_site_config_at(
            &config_path,
            "[icons.collections.acme]\nsource-type = \"local-svg-dir\"\npath = \"icons\"\n",
        )
        .unwrap_err();
        let diagnostics = attached_diagnostics(&error);

        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == crate::codes::config::INVALID
                && diagnostic
                    .notes
                    .iter()
                    .any(|note| note == IconsConfig::FIELDS.collections.as_str())
        }));
    }

    #[test]
    fn minify_override_controls_every_language() {
        let build = BuildOverrides {
            minify: Some(false),
            ..Default::default()
        };

        let site_config = resolved_config("", build);

        assert!(!site_config.config.build.minify.html);
        assert!(!site_config.config.build.minify.css);
        assert!(!site_config.config.build.minify.javascript);
    }

    #[test]
    fn explicit_package_locations_win() {
        let directory = tempfile::TempDir::new().unwrap();
        let site_config = OwnedSiteConfig::with_invocation(
            "",
            tola_typst::PackageLocations::discover(
                Some(directory.path().join("packages")),
                Some(directory.path().join("cache")),
            )
            .unwrap(),
            BuildOverrides::default(),
        );
        let config = &site_config.config;

        assert_eq!(
            config.package_locations().data().unwrap().root(),
            directory.path().join("packages")
        );
        assert_eq!(
            config.package_locations().cache().unwrap().root(),
            directory.path().join("cache")
        );
        assert_eq!(
            config.package_locations().data().unwrap().source(),
            tola_typst::PackageLocationSource::Explicit
        );
        assert_eq!(
            config.package_locations().cache().unwrap().source(),
            tola_typst::PackageLocationSource::Explicit
        );
    }
}
