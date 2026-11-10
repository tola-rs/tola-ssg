//! Load and validate site configuration from files or supplied TOML text.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::boundaries::{check_publish_dir, module_input_paths, validate_site_paths};
pub(crate) use super::boundaries::{overlapping_module_root, validate_hook_output_boundaries};
use super::diagnostic::config_error;
use crate::config::section::build::{BuildEntry, MinifyConfig, ResolvedBuildConfig};
use crate::config::{
    ConfigDiagnostic, ConfigDiagnostics, ConfigError, ConfigPresence, ConfigSource, FieldPath,
    ParsedConfig, ResolvedSiteConfig, SiteConfigSchema,
};

/// Per-invocation build settings applied before effective configuration validation.
#[derive(Debug, Clone, Default)]
pub struct BuildOverrides {
    /// Published directory, relative to the site root unless absolute.
    pub publish_dir: Option<PathBuf>,
    /// Override HTML, CSS, and JavaScript minification together.
    pub minify: Option<bool>,
    /// Canonical HTTP(S) origin without a deployment path.
    pub origin: Option<String>,
    /// Deployment URL prefix, beginning and ending with `/`.
    pub base_path: Option<String>,
}

/// A validated configuration and the digest of the exact TOML text that produced it.
#[derive(Debug)]
pub struct SiteConfigWithSourceDigest {
    config: ResolvedSiteConfig,
    source_hash: blake3::Hash,
}

impl SiteConfigWithSourceDigest {
    pub fn config(&self) -> &ResolvedSiteConfig {
        &self.config
    }

    /// Digest of the exact source text before invocation overrides or normalization.
    pub fn source_hash(&self) -> blake3::Hash {
        self.source_hash
    }

    /// Install the application runner while keeping the digest of the loaded source.
    pub fn with_hook_runner(mut self, runner: crate::hooks::HookRunner) -> Self {
        self.config = self.config.with_hook_runner(runner);
        self
    }

    pub fn into_config(self) -> ResolvedSiteConfig {
        self.config
    }
}

/// Read, parse, and resolve one core site configuration.
///
/// No resources were chosen at this layer, so the read takes the permissive scope.
pub fn load_site_config(
    explicit: Option<&Path>,
    package_locations: tola_typst::PackageLocations,
    build: &BuildOverrides,
) -> Result<SiteConfigWithSourceDigest> {
    resolve_parsed_site_config(
        ConfigSource::load(explicit, tola_input::InputScope::Online)?.decode_site()?,
        package_locations,
        build,
    )
}

/// Resolve supplied text without reading or writing its configuration path.
pub fn resolve_site_config(
    config_path: &Path,
    source: &str,
    package_locations: tola_typst::PackageLocations,
    build: &BuildOverrides,
) -> Result<SiteConfigWithSourceDigest> {
    resolve_parsed_site_config(
        ConfigSource::parse(config_path, source, tola_input::InputScope::Online)?.decode_site()?,
        package_locations,
        build,
    )
}

/// Resolve core sections projected from one already parsed host document.
pub fn resolve_parsed_site_config(
    parsed: ParsedConfig<SiteConfigSchema>,
    package_locations: tola_typst::PackageLocations,
    build: &BuildOverrides,
) -> Result<SiteConfigWithSourceDigest> {
    let source = parsed.source;
    let config = resolve_schema(
        parsed.schema,
        source.path().to_path_buf(),
        source.root().to_path_buf(),
        source.core_presence(),
        std::sync::Arc::clone(source.positions()),
        package_locations,
        build,
    )
    .map_err(|error| source.attach_load(error))?;
    Ok(SiteConfigWithSourceDigest {
        config,
        source_hash: source.source_hash(),
    })
}

fn resolve_schema(
    mut schema: SiteConfigSchema,
    config_path: PathBuf,
    root: PathBuf,
    presence: &ConfigPresence,
    positions: std::sync::Arc<tola_diagnostic::SourcePositions>,
    mut package_locations: tola_typst::PackageLocations,
    build: &BuildOverrides,
) -> Result<ResolvedSiteConfig> {
    schema.validate_source_paths()?;
    schema.validate_entry()?;
    super::schema::validate_hook_options(&mut schema.hooks, &schema.options)?;
    apply_build_overrides(&mut schema, build);
    bind_site_paths(&mut schema, &root);
    // Vendored package bytes precede host roots before configuration semantics are checked.
    if let Some(path) = schema.vendor.typst_packages() {
        package_locations = package_locations.with_declared_root(path)?;
    }
    let module_roots = module_input_paths(&schema, &root);
    let vendor_workspace = schema
        .vendor
        .path
        .as_deref()
        .and_then(crate::filesystem::swap_workspace);
    let boundary = check_publish_dir(
        &schema,
        &config_path,
        &root,
        &package_locations,
        &module_roots,
        vendor_workspace.as_deref(),
    )?;
    schema.build.publish_dir = boundary.output;
    let warnings = validate_effective_config(
        &schema,
        &root,
        &config_path,
        vendor_workspace.as_deref(),
        &module_roots,
        presence,
    )?;
    let SiteConfigSchema {
        site,
        build: build_config,
        assets,
        typst,
        icons,
        vendor,
        formatter,
        modules,
        hooks,
        options,
        ..
    } = schema;
    let (entry, entry_module) = match build_config.entry {
        BuildEntry::Project(path) => (path, None),
        BuildEntry::Module { module } => (PathBuf::new(), Some(module)),
    };
    Ok(ResolvedSiteConfig {
        package_locations,
        config_path,
        root,
        warnings,
        positions,
        declares_font_paths: presence
            .contains(crate::config::section::FontsConfig::FIELDS.paths.as_str()),
        site: site.into_resolved(),
        build: ResolvedBuildConfig {
            entry,
            content_dir: build_config.content_dir,
            publish_dir: build_config.publish_dir,
            minify: build_config.minify,
            references: build_config.references,
        },
        assets,
        typst,
        icons,
        vendor,
        formatter,
        modules,
        hooks,
        options,
        entry_module,
        hook_runner: None,
        module_roots,
        entry_file: None,
        vendor_workspace,
        vendor_candidate: None,
    })
}

fn bind_site_paths(config: &mut SiteConfigSchema, root: &Path) {
    // Preserve logical input paths to detect symlink retargets.
    // Safety and overlap checks resolve physical paths separately.
    if let BuildEntry::Project(entry) = &mut config.build.entry {
        *entry = root.join(&*entry);
    }
    config.build.content_dir = root.join(&config.build.content_dir);
    config.assets.normalize(root);
    config.typst.fonts.normalize(root);
    // Do not follow output symlinks: safety validation must see them.
    config.build.publish_dir =
        tola_input::lexical_path_identity(&root.join(&config.build.publish_dir));
    config.icons.normalize(root);
    config.vendor.normalize(root);
}

fn apply_build_overrides(config: &mut SiteConfigSchema, overrides: &BuildOverrides) {
    if let Some(path) = &overrides.publish_dir {
        config.build.publish_dir.clone_from(path);
    }
    if let Some(enabled) = overrides.minify {
        config.build.minify = MinifyConfig {
            html: enabled,
            css: enabled,
            javascript: enabled,
        };
    }
    if let Some(origin) = &overrides.origin {
        config.site.origin = Some(origin.clone());
    }
    if let Some(path) = &overrides.base_path {
        config.site.base_path.clone_from(path);
    }
}

fn validate_effective_config(
    config: &SiteConfigSchema,
    root: &Path,
    config_path: &Path,
    vendor_workspace: Option<&Path>,
    module_roots: &[PathBuf],
    presence: &ConfigPresence,
) -> Result<Vec<ConfigDiagnostic>> {
    let mut diagnostics = super::schema::settings_diagnostics(config, presence);
    validate_site_paths(
        config,
        root,
        config_path,
        vendor_workspace,
        module_roots,
        &mut diagnostics,
    );

    let warnings = diagnostics
        .experimental_fields()
        .iter()
        .map(|field| ConfigDiagnostic::experimental(*field))
        .chain(diagnostics.warnings().iter().cloned())
        .collect();

    diagnostics
        .into_result()
        .map(|()| warnings)
        .map_err(config_error)
}

/// The configuration file name Tola discovers in a site directory.
pub const CONFIG_FILE_NAME: &str = "tola.toml";

/// Absolute configuration file path and the site root its parent directory selects.
pub(super) fn resolve_config_location(
    path: &Path,
) -> Result<(PathBuf, PathBuf), super::ConfigSourceError> {
    if path.file_name().is_none() || path.is_dir() {
        return Err(config_path_error(path));
    }
    let path = std::path::absolute(path).map_err(|error| unresolved_config_path(path, error))?;
    Ok(config_location(path))
}

fn config_location(path: PathBuf) -> (PathBuf, PathBuf) {
    let root = tola_input::normalize_path(path.parent().unwrap_or(Path::new("")));
    (path, root)
}

/// The failure of a configuration path that names no file.
fn config_path_error(path: &Path) -> super::ConfigSourceError {
    let error = ConfigError::PathNotAFile {
        path: written_path(path),
    };
    let diagnostic = tola_diagnostic::Diagnostic::new(
        tola_diagnostic::codes::config::INVALID,
        tola_diagnostic::Severity::Error,
        error.to_string(),
    )
    .with_help("Pass `--config` the path of the site's `tola.toml`");
    super::ConfigSourceError::Path(tola_diagnostic::DiagnosticError::attach(
        error.into(),
        vec![diagnostic],
    ))
}

/// The failure of a configuration path Tola could not make absolute.
fn unresolved_config_path(path: &Path, error: std::io::Error) -> super::ConfigSourceError {
    invocation_error(InvocationFailure::ConfigPath(path.to_path_buf()), error)
}

/// The failure of reading the working directory a command finds the site from.
fn unreadable_directory_error(error: std::io::Error) -> super::ConfigSourceError {
    invocation_error(InvocationFailure::WorkingDirectory, error)
}

/// A failure the author fixes by changing the command, not a line in a document.
#[derive(Debug, thiserror::Error)]
enum InvocationFailure {
    /// The configuration path could not be made absolute.
    #[error("Tola could not resolve the configuration path `{}`", written_path(.0))]
    ConfigPath(PathBuf),
    /// The working directory a command finds the site from could not be read.
    #[error("Tola could not read the current directory")]
    WorkingDirectory,
}

/// Report one invocation failure: its sentence comes from the variant, the input/output error
/// stays in the chain for the debug log.
fn invocation_error(failure: InvocationFailure, cause: std::io::Error) -> super::ConfigSourceError {
    let message = failure.to_string();
    let diagnostic = tola_diagnostic::Diagnostic::new(
        tola_diagnostic::codes::config::INVALID,
        tola_diagnostic::Severity::Error,
        message.clone(),
    )
    .with_help(INVOCATION_HELP);
    let error = anyhow::Error::new(cause).context(message);
    super::ConfigSourceError::Path(tola_diagnostic::DiagnosticError::attach(
        error,
        vec![diagnostic],
    ))
}

/// The action that avoids the working directory a configuration path is resolved against.
const INVOCATION_HELP: &str = "Pass `--config` the full path of the site's `tola.toml`";

/// Spell one configuration path the way the author wrote it.
fn written_path(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        let resolved = crate::filesystem::absolute_path(path);
        return tola_diagnostic::display_path(&resolved, Path::new(""));
    }
    tola_diagnostic::display_path(path, Path::new(""))
}

pub(super) fn select_config_location(
    explicit: Option<&Path>,
) -> Result<(PathBuf, PathBuf), super::ConfigSourceError> {
    if let Some(explicit) = explicit {
        // Keep the logical path so the watcher can detect config symlink retargets.
        return resolve_config_location(explicit);
    }
    let cwd = std::env::current_dir().map_err(unreadable_directory_error)?;
    find_default_config(&cwd)
        .map(config_location)
        .ok_or_else(|| {
            let message = "no `tola.toml` found in this directory or a parent directory";
            let diagnostic = tola_diagnostic::Diagnostic::new(
                tola_diagnostic::codes::config::LOAD,
                tola_diagnostic::Severity::Error,
                message,
            );
            super::ConfigSourceError::NotFound {
                start: cwd,
                diagnostic: tola_diagnostic::DiagnosticError::new(message, vec![diagnostic]),
            }
        })
}

/// The site root one configuration path selects.
pub fn config_site_root(explicit: Option<&Path>) -> Result<PathBuf, super::ConfigSourceError> {
    match explicit {
        Some(path) => resolve_config_location(path).map(|(_, root)| root),
        None => {
            let cwd = std::env::current_dir().map_err(unreadable_directory_error)?;
            let root = find_default_config(&cwd)
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .unwrap_or(cwd);
            Ok(tola_input::normalize_path(&root))
        }
    }
}

/// Find the conventional site configuration from a known start directory.
/// Explicit paths never enter this discovery function.
/// An existing or inaccessible path stops the search so loading can report its error.
pub fn find_default_config(start: &Path) -> Option<PathBuf> {
    let mut current = start;
    loop {
        let candidate = current.join(CONFIG_FILE_NAME);
        if !matches!(std::fs::symlink_metadata(&candidate), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        {
            return Some(candidate);
        }

        current = current.parent()?;
    }
}

impl SiteConfigSchema {
    /// Decode an editable core schema without resolving filesystem paths.
    pub fn parse(
        path: &Path,
        content: &str,
        scope: tola_input::InputScope,
    ) -> Result<Self, super::ConfigSourceError> {
        Self::parse_at(path, content, scope).map(ParsedConfig::into_schema)
    }

    /// Resolve an explicitly constructed schema without a TOML round trip.
    /// All fields present in its serialized value count as explicit for status diagnostics.
    pub fn resolve(
        mut self,
        config_path: &Path,
        package_locations: tola_typst::PackageLocations,
        build: &BuildOverrides,
    ) -> Result<ResolvedSiteConfig> {
        self.options = tola_module::options::deserialize_options(std::mem::take(&mut self.options))
            .map_err(|error| {
                let mut diagnostics = ConfigDiagnostics::new();
                diagnostics.error(FieldPath::new("options"), error.to_string());
                super::diagnostic::attach_load(config_error(diagnostics), Some(config_path), None)
            })?;
        let value = toml::Value::try_from(&self)
            .map_err(anyhow::Error::from)
            .map_err(|error| {
                crate::config::diagnostic::attach_load(error, Some(config_path), None)
            })?;
        let presence = ConfigPresence::from_value(&value);
        let (path, root) = resolve_config_location(config_path)?;
        resolve_schema(
            self,
            path.clone(),
            root,
            &presence,
            std::sync::Arc::new(tola_diagnostic::SourcePositions::default()),
            package_locations,
            build,
        )
        .map_err(|error| crate::config::diagnostic::attach_load(error, Some(&path), None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::{VENDORED_SITE_SOURCE, attached_diagnostics, diagnostic_mentioning};

    /// A `tola.toml` source with `site.title` and the default build entry and content root.
    fn site_source(title: &str) -> String {
        format!(
            r#"[site]
title = "{title}"

[build]
entry = "site.typ"
content-dir = "content"
"#
        )
    }

    /// Write `source` as the `tola.toml` of `directory` and return its path.
    fn write_config(directory: &Path, source: &str) -> PathBuf {
        let config_path = directory.join("tola.toml");
        std::fs::write(&config_path, source).unwrap();
        config_path
    }

    /// The failure of loading the configuration at `config_path`.
    fn load_error(config_path: &Path) -> anyhow::Error {
        load_site_config(
            Some(config_path),
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .unwrap_err()
    }

    #[test]
    fn diagnostics_name_the_written_line() {
        for (source, field) in [
            (
                "[site]\ntitle = \"Site\"\nlanguage = \"nope\"\n",
                "site.language",
            ),
            (
                "[build]\nentry = \"site.typ\"\nminfy = true\n",
                "build.minfy",
            ),
        ] {
            let directory = tempfile::TempDir::new().unwrap();
            let config_path = write_config(directory.path(), source);

            let error = load_error(&config_path);

            let diagnostic = diagnostic_mentioning(&error, field);
            assert_eq!(diagnostic.code, tola_diagnostic::codes::config::INVALID);
            let location = diagnostic.location.as_ref().unwrap();
            assert_eq!(location.path, "tola.toml");
            assert_eq!((location.line, location.column), (Some(3), Some(1)));
        }
    }

    #[test]
    fn supplied_text_outranks_the_config_file() {
        let directory = tempfile::TempDir::new().unwrap();
        let config_path = write_config(directory.path(), &site_source("Disk"));
        let source = site_source("Supplied");
        let parsed = resolve_site_config(
            &config_path,
            &source,
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .unwrap();

        assert_eq!(parsed.config().site().title, "Supplied");
        assert_eq!(parsed.source_hash(), blake3::hash(source.as_bytes()));
        assert!(parsed.config().warnings().is_empty());
        assert_eq!(
            std::fs::read_to_string(config_path).unwrap(),
            site_source("Disk")
        );
    }

    #[test]
    fn directory_config_path_is_refused() {
        let directory = tempfile::TempDir::new().unwrap();
        let error = resolve_site_config(
            directory.path(),
            &site_source("Site"),
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .unwrap_err();

        let diagnostics = attached_diagnostics(&error);
        assert!(
            diagnostics[0].message.contains("is not a file"),
            "{diagnostics:?}"
        );
        assert!(
            diagnostics[0]
                .message
                .contains(&written_path(directory.path())),
            "{diagnostics:?}"
        );
        assert!(
            diagnostics[0]
                .help
                .iter()
                .any(|help| help.message.contains("--config")),
            "{diagnostics:?}"
        );
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.location.is_none())
        );
    }

    #[test]
    fn output_errors_name_the_config_path() {
        let directory = tempfile::TempDir::new().unwrap();
        let config_path = write_config(
            directory.path(),
            &format!("{}publish-dir = \".\"\n", site_source("Site")),
        );

        let error = load_error(&config_path);
        let diagnostics = attached_diagnostics(&error);

        assert!(!diagnostics.is_empty());
        assert!(diagnostics.iter().all(|diagnostic| {
            diagnostic
                .location
                .as_ref()
                .is_some_and(|location| location.path == "tola.toml")
        }));
        assert!(error.chain().any(|cause| matches!(
            cause.downcast_ref::<crate::filesystem::OutputBoundaryError>(),
            Some(crate::filesystem::OutputBoundaryError::OutsideSite)
        )));
    }

    #[cfg(unix)]
    #[test]
    fn broken_symlink_stops_the_search() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("tola.toml"), "").unwrap();
        let nested = root.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let path = nested.join("tola.toml");
        std::os::unix::fs::symlink("missing.toml", &path).unwrap();

        assert_eq!(find_default_config(&nested), Some(path.clone()));
        let error = ConfigSource::load(Some(&path), tola_input::InputScope::Online).unwrap_err();
        assert!(matches!(error, super::super::ConfigSourceError::Read(_)));
        let error = anyhow::Error::new(error);
        assert!(error.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        }));
    }

    #[test]
    fn declared_vendor_packages_precede_host_roots() {
        let site_directory = tempfile::tempdir().unwrap();
        let config_path = site_directory.path().join("tola.toml");
        let locations = || {
            tola_typst::PackageLocations::from_absolute_roots(
                Some(site_directory.path().join("host-packages")),
                Some(site_directory.path().join("host-cache")),
            )
            .unwrap()
        };
        let mut declared_schema = SiteConfigSchema::default();
        declared_schema.vendor.path = Some("vendor".into());

        let declared = declared_schema
            .resolve(&config_path, locations(), &BuildOverrides::default())
            .unwrap();

        assert_eq!(
            declared
                .package_locations()
                .declared()
                .first()
                .unwrap()
                .root(),
            declared.get_root().join("vendor/typst-packages")
        );

        let undeclared = SiteConfigSchema::default()
            .resolve(&config_path, locations(), &BuildOverrides::default())
            .unwrap();

        assert!(undeclared.package_locations().declared().is_empty());
        assert!(undeclared.package_locations().data().is_some());
    }

    #[test]
    fn schema_cannot_bypass_path_boundaries() {
        type SchemaCase<'a> = (fn(&mut SiteConfigSchema), &'a str);

        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("tola.toml");
        let cases: [SchemaCase<'_>; 7] = [
            (
                |schema| schema.build.entry = "../site.typ".into(),
                "build.entry",
            ),
            (
                |schema| schema.build.entry = "C:site.typ".into(),
                "build.entry",
            ),
            (
                |schema| schema.build.entry = r"\\server\share\site.typ".into(),
                "build.entry",
            ),
            (
                |schema| schema.build.publish_dir = ".".into(),
                "build.publish-dir",
            ),
            (
                |schema| schema.build.entry = ".tola/site.typ".into(),
                "build.entry",
            ),
            (
                |schema| schema.typst.fonts.paths = vec![".tola/fonts".into()],
                "typst.fonts.paths",
            ),
            (
                |schema| {
                    schema.vendor.path = Some("vendor".into());
                    schema.build.publish_dir = ".vendor-swap/site".into();
                },
                "build.publish-dir",
            ),
        ];

        for (edit, expected) in cases {
            let mut schema = SiteConfigSchema::default();
            edit(&mut schema);

            let error = schema
                .resolve(
                    &config_path,
                    tola_typst::PackageLocations::default(),
                    &BuildOverrides::default(),
                )
                .unwrap_err();

            assert!(attached_diagnostics(&error).iter().any(|diagnostic| {
                diagnostic.notes.iter().any(|note| note == expected)
                    || diagnostic.message.contains(expected)
            }));
        }
    }

    #[cfg(unix)]
    #[test]
    fn hook_output_cannot_reach_the_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let config_path = write_config(root, "");
        std::os::unix::fs::symlink("tola.toml", root.join("alias.toml")).unwrap();
        std::os::unix::fs::symlink(".", root.join("linked")).unwrap();
        let mut schema = SiteConfigSchema::default();
        schema.hooks.before_build = vec![crate::config::section::build::BeforeBuildHookConfig {
            name: "generator".into(),
            argv: Some(vec!["generator".into()]),
            generates: vec!["tola.toml".into(), "alias.toml".into(), "linked".into()],
            ..crate::config::section::build::BeforeBuildHookConfig::default()
        }];

        let error = schema
            .resolve(
                &config_path,
                tola_typst::PackageLocations::default(),
                &BuildOverrides::default(),
            )
            .unwrap_err();

        let diagnostics = attached_diagnostics(&error);
        assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
        for diagnostic in diagnostics {
            assert!(
                diagnostic.message.contains("site configuration"),
                "{diagnostic:?}"
            );
        }
    }

    #[test]
    fn hook_output_boundary_names_its_entry() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = write_config(
            directory.path(),
            r#"[[hooks.before-build]]
name = "first"
argv = ["first.sh"]
generates = ["generated/first"]

[[hooks.before-build]]
name = "second"
argv = ["second.sh"]
generates = [".tola/cache"]
"#,
        );

        let error = load_error(&config_path);

        let diagnostic = diagnostic_mentioning(&error, ".tola/cache");
        assert_eq!(diagnostic.code, tola_diagnostic::codes::config::INVALID);
        // Line 9 writes the second entry's own `generates`, not the first entry's line 4.
        let location = diagnostic
            .location
            .as_ref()
            .expect("the refusal is located");
        assert_eq!(location.line, Some(9));
    }

    #[test]
    fn vendor_candidate_never_reads_old_copy() {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), VENDORED_SITE_SOURCE);
        let old = config
            .vendor
            .typst_packages()
            .unwrap()
            .join("local/demo/1.0.0");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("value.typ"), "old vendor").unwrap();
        let candidate = config.get_root().join(".vendor-swap/candidate");
        std::fs::create_dir_all(&candidate).unwrap();
        let config = config.with_vendor_root(Some(candidate.clone())).unwrap();
        let pure = crate::BuildResources::new().with_input_scope(tola_input::InputScope::Pure);
        let files = tola_typst::FileResolver::from_package_locations(
            pure.package_locations(&config),
            tola_typst::PackageFetchPolicy::LocalOnly,
        )
        .with_source_boundary(pure.source_boundary(&config));
        let id = typst::syntax::FileId::new(typst::syntax::RootedPath::new(
            typst::syntax::VirtualRoot::Package("@local/demo:1.0.0".parse().unwrap()),
            typst::syntax::VirtualPath::new("value.typ").unwrap(),
        ));
        assert!(files.read(id, config.get_root()).is_err());
        let package = candidate.join("typst-packages/local/demo/1.0.0");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("value.typ"), "candidate vendor").unwrap();
        assert_eq!(
            files.read(id, config.get_root()).unwrap(),
            b"candidate vendor"
        );
    }

    #[test]
    fn refresh_keeps_workspace_exclusions() {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::config::tests::load_test_config(directory.path(), VENDORED_SITE_SOURCE);
        let workspace = config
            .vendor
            .path
            .as_deref()
            .and_then(crate::filesystem::swap_workspace)
            .unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("old.typ"), "generated previous source").unwrap();
        let refreshed = config.with_vendor_root(None).unwrap();
        let files = tola_typst::FileResolver::new()
            .with_source_boundary(crate::BuildResources::new().source_boundary(&refreshed));
        assert!(
            files
                .read(
                    tola_typst::file_id(".vendor-swap/old.typ"),
                    refreshed.get_root(),
                )
                .is_err()
        );
    }
}
