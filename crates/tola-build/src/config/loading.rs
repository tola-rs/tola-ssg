//! Load and validate site configuration from files or supplied TOML text.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::boundaries::{check_publish_dir, module_input_paths, validate_site_paths};
pub(crate) use super::boundaries::{overlapping_module_root, validate_hook_output_boundaries};
use super::diagnostic::config_error;
use crate::config::section::build::{BuildEntry, BuildSettings, MinifyConfig};
use crate::config::{
    ConfigDiagnostic, ConfigDiagnostics, ConfigError, ConfigPresence, ConfigSource, FieldPath,
    ParsedConfig, SiteConfig, SiteConfigSchema,
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
    config: SiteConfig,
    source_hash: blake3::Hash,
}

impl SiteConfigWithSourceDigest {
    pub fn config(&self) -> &SiteConfig {
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

    pub fn into_config(self) -> SiteConfig {
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
        Some(source.clone()),
        source.core_presence(),
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
    config_source: Option<ConfigSource>,
    presence: &ConfigPresence,
    mut package_locations: tola_typst::PackageLocations,
    build: &BuildOverrides,
) -> Result<SiteConfig> {
    let positions = config_source
        .as_ref()
        .map(|source| std::sync::Arc::clone(source.positions()))
        .unwrap_or_default();
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
        config_source
            .as_ref()
            .into_iter()
            .flat_map(|source| source.paths()),
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
        &config_source
            .as_ref()
            .map(|source| source.paths().collect::<Vec<_>>())
            .unwrap_or_default(),
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
    Ok(SiteConfig {
        package_locations,
        config_path,
        config_source,
        root,
        warnings,
        positions,
        declares_font_paths: presence
            .contains(crate::config::section::FontsConfig::FIELDS.paths.as_str()),
        site: site.into_settings(),
        build: BuildSettings {
            par_hooks: build_config.par_hooks,
            hook_jobs: build_config.hook_jobs,
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
        module_font_directories: Vec::new(),
        fresh_archive_roots: Vec::new(),
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
    config_paths: &[&Path],
    vendor_workspace: Option<&Path>,
    module_roots: &[PathBuf],
    presence: &ConfigPresence,
) -> Result<Vec<ConfigDiagnostic>> {
    let mut diagnostics = super::schema::settings_diagnostics(config, presence);
    validate_site_paths(
        config,
        root,
        config_path,
        config_paths,
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
    ) -> Result<SiteConfig> {
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
            None,
            &presence,
            package_locations,
            build,
        )
        .map_err(|error| crate::config::diagnostic::attach_load(error, Some(&path), None))
    }
}

#[cfg(test)]
mod tests;
