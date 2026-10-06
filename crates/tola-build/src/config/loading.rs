//! Load and validate site configuration from files or supplied TOML text.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::section::AssetsConfig;
use crate::config::section::build::{HooksConfig, MinifyConfig};
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

/// Keeps configured assets disjoint from content discovery and owned output/state.
/// The content restriction applies to these mappings, not to Typst reads of colocated files:
/// Bundle assets and image producers can still use those files as inputs.
pub(crate) fn validate_asset_source_boundaries(
    assets: &AssetsConfig,
    site_root: &Path,
    content_root: &Path,
    output_root: &Path,
    diagnostics: &mut ConfigDiagnostics,
) {
    let mut declarations = assets
        .tree_sources()
        .enumerate()
        .map(|(index, source)| (AssetsConfig::FIELDS.trees, index, source))
        .chain(
            assets
                .file_sources()
                .enumerate()
                .map(|(index, source)| (AssetsConfig::FIELDS.files, index, source)),
        )
        .peekable();
    if declarations.peek().is_none() {
        return;
    }

    let internal_root = site_root.join(crate::filesystem::INTERNAL_DIR);
    let protected_boundaries = [
        ("content root", content_root),
        ("output root", output_root),
        ("Tola's `.tola` directory", internal_root.as_path()),
    ]
    .map(|(owner, protected)| {
        (
            owner,
            protected,
            crate::filesystem::FilesystemSourceIdentity::from_path(protected),
        )
    });
    for (field, index, source) in declarations {
        let source_identity = crate::filesystem::FilesystemSourceIdentity::from_path(source);
        for (owner, protected, protected_identity) in &protected_boundaries {
            if source_identity.intersects(protected_identity) {
                diagnostics.error(
                    field,
                    format!(
                        "`{}[{index}]` source `{}` overlaps the {owner} `{}`",
                        field.as_str(),
                        crate::filesystem::display_path(source, site_root),
                        crate::filesystem::display_path(protected, site_root),
                    ),
                );
            }
        }
    }
}

pub(crate) fn validate_hook_output_boundaries(
    hooks: &HooksConfig,
    root: &Path,
    configuration: &Path,
    build_output: &Path,
    vendor_workspace: Option<&Path>,
    diagnostics: &mut ConfigDiagnostics,
) {
    let boundaries = crate::filesystem::DeclaredOutputBoundaries::for_site(
        root,
        configuration,
        build_output,
        vendor_workspace,
    );
    let outputs = crate::config::section::build::BeforeBuildHookConfig::FIELDS.generates;
    // Every granted declaration, so a shared path names the entry that claimed it first.
    let mut granted = Vec::<(
        &crate::config::section::build::BeforeBuildHookConfig,
        PathBuf,
        crate::filesystem::FilesystemSourceIdentity,
    )>::new();
    for (entry, hook) in hooks.before_build.iter().enumerate() {
        // Disabled entries reserve nothing, and each entry keeps the index it was written at.
        if !hook.enable {
            continue;
        }
        // The scope has the entry's own index, so a failure names the entry the author must
        // edit rather than the first one of the array.
        diagnostics.with_array_element(
            HooksConfig::FIELDS.before_build.as_str(),
            entry,
            |diagnostics| {
                for output in &hook.generates {
                    let resolved = boundaries.resolve(output);
                    let violation = boundaries
                        .violation(&resolved, granted.iter().map(|(_, _, identity)| identity));
                    let shown = crate::filesystem::display_path(&root.join(output), root);
                    match violation {
                        None => {}
                        Some(crate::filesystem::DeclaredOutputViolation::OutsideSite) => {
                            diagnostics.error_with_help(
                                outputs,
                                format!("hook output `{shown}` is outside the site root"),
                                "Declare a path inside the site",
                            );
                        }
                        Some(crate::filesystem::DeclaredOutputViolation::InsidePublished) => {
                            diagnostics.error(
                                outputs,
                                format!(
                                    "hook output `{shown}` intersects build.publish-dir `{}`",
                                    crate::filesystem::display_path(build_output, root)
                                ),
                            );
                        }
                        Some(crate::filesystem::DeclaredOutputViolation::InsideReserved {
                            kind,
                            path,
                        }) => {
                            diagnostics.error(
                                outputs,
                                format!(
                                    "hook output `{shown}` intersects {kind} `{}`",
                                    crate::filesystem::display_path(&path, root)
                                ),
                            );
                        }
                        Some(crate::filesystem::DeclaredOutputViolation::OwnedElsewhere(
                            claimed,
                        )) => {
                            let (owner, existing, _) = &granted[claimed];
                            diagnostics.error(
                                outputs,
                                format!(
                                    "hook output `{shown}` from `{}` conflicts with `{}` from `{}`",
                                    hook.name,
                                    crate::filesystem::display_path(existing, root),
                                    owner.name
                                ),
                            );
                        }
                    }
                    granted.push((hook, root.join(output), resolved));
                }
            },
        );
    }
}

impl SiteConfigWithSourceDigest {
    pub fn config(&self) -> &ResolvedSiteConfig {
        &self.config
    }

    /// Digest of the exact source text before invocation overrides or normalization.
    pub fn source_hash(&self) -> blake3::Hash {
        self.source_hash
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
        ConfigSource::load(explicit, crate::resources::InputScope::Online)?.decode_site()?,
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
        ConfigSource::parse(config_path, source, crate::resources::InputScope::Online)?
            .decode_site()?,
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
    .map_err(|error| {
        crate::config::diagnostic::attach_load(error, Some(source.path()), Some(source.positions()))
    })?;
    Ok(SiteConfigWithSourceDigest {
        config,
        source_hash: source.source_hash(),
    })
}

fn resolve_schema(
    schema: SiteConfigSchema,
    config_path: PathBuf,
    root: PathBuf,
    presence: &ConfigPresence,
    positions: std::sync::Arc<crate::config::source::ConfigPositions>,
    package_locations: tola_typst::PackageLocations,
    build: &BuildOverrides,
) -> Result<ResolvedSiteConfig> {
    schema.validate_source_paths(presence)?;
    let SiteConfigSchema {
        site,
        build: build_config,
        assets,
        typst,
        icons,
        vendor,
        ..
    } = schema;
    let mut config = ResolvedSiteConfig {
        package_locations,
        config_path,
        root,
        warnings: Vec::new(),
        positions,
        site,
        build: build_config,
        assets,
        typst,
        icons,
        vendor,
        vendor_workspace: None,
        vendor_candidate: None,
    };
    normalize_site_paths(&mut config, build.publish_dir.as_ref())?;
    // The bytes the site vendors are searched ahead of every host root.
    if let Some(path) = config.vendor.typst_packages() {
        config.package_locations = config.package_locations.clone().with_declared_root(path)?;
    }
    apply_build_overrides(&mut config, build);
    // Resolve the declared output path and reject one that overlaps an input before any semantic
    // validation runs: an unusable output root is a layout failure, not a value to interpret.
    let boundary = crate::filesystem::OutputBoundary::resolve(
        config.get_root(),
        &config.build.publish_dir,
        config.protected_input_paths(),
    )?;
    config.build.publish_dir = boundary.output.clone();
    validate_resolved_config(&mut config, presence)?;
    // Canonicalize the configured address in place — the origin's spelling and the base path's
    // percent-encoding — which `site.url()` and the derived mount both read. The mount this
    // returns is deliberately discarded: `ResolvedSiteConfig::url_mount()` derives it from
    // `site.base_path`, and this must run after validation, whose result its `expect` relies on.
    config.site.normalize_address();
    Ok(config)
}

fn normalize_site_paths(config: &mut ResolvedSiteConfig, output: Option<&PathBuf>) -> Result<()> {
    replace_when_some(&mut config.build.publish_dir, output);
    let root = config.get_root().to_path_buf();

    // Preserve logical input paths to detect symlink retargets.
    // Safety and overlap checks resolve physical paths separately.
    let declared = std::mem::take(&mut config.config_path);
    config.config_path =
        std::path::absolute(&declared).map_err(|error| unresolved_config_path(&declared, error))?;
    config.build.entry = root.join(&config.build.entry);
    config.build.content_dir = root.join(&config.build.content_dir);
    config.assets.normalize(&root);
    config.typst.fonts.normalize(&root);
    // Do not follow output symlinks: safety validation must see them.
    config.build.publish_dir =
        crate::filesystem::lexical_path_identity(&root.join(&config.build.publish_dir));
    config.icons.normalize(&root);
    config.vendor.normalize(&root);
    config.vendor_workspace = config.vendor.workspace_path();
    Ok(())
}

fn apply_build_overrides(config: &mut ResolvedSiteConfig, overrides: &BuildOverrides) {
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
    replace_when_some(&mut config.site.base_path, overrides.base_path.as_ref());
}

fn replace_when_some<T: Clone>(target: &mut T, replacement: Option<&T>) {
    if let Some(replacement) = replacement {
        *target = replacement.clone();
    }
}

fn validate_resolved_config(
    config: &mut ResolvedSiteConfig,
    presence: &ConfigPresence,
) -> Result<()> {
    let mut diagnostics = super::schema::settings_diagnostics(
        &config.site,
        &config.build,
        &config.assets,
        &config.icons,
        &config.typst,
        &config.vendor,
        presence,
    );
    config
        .vendor
        .validate_internal_boundaries(config.get_root(), &mut diagnostics);
    config
        .icons
        .validate_internal_boundaries(config.get_root(), &mut diagnostics);
    let boundary = crate::resources::source_boundary(config, crate::InputScope::Online);
    for (field, path) in [
        (FieldPath::new("build.entry"), &config.build.entry),
        (
            FieldPath::new("build.content-dir"),
            &config.build.content_dir,
        ),
    ] {
        if let Err(error) = boundary.check(path) {
            diagnostics.error(field, error.to_string());
        }
    }
    for (index, path) in config.typst.fonts.paths.iter().enumerate() {
        diagnostics.with_array_element("typst.fonts.paths", index, |diagnostics| {
            if let Err(error) = boundary.check(path) {
                diagnostics.error(FieldPath::new("typst.fonts.paths"), error.to_string());
            }
        });
    }
    if let Some(workspace) = &config.vendor_workspace {
        let workspace_identity = crate::filesystem::FilesystemSourceIdentity::from_path(workspace);
        let output_identity =
            crate::filesystem::FilesystemSourceIdentity::from_path(&config.build.publish_dir);
        if output_identity.intersects(&workspace_identity) {
            diagnostics.error(
                FieldPath::new("build.publish-dir"),
                format!(
                    "`build.publish-dir` `{}` overlaps the vendor working directory `{}`",
                    crate::filesystem::display_path(&config.build.publish_dir, config.get_root()),
                    crate::filesystem::display_path(workspace, config.get_root()),
                ),
            );
        }
    }
    validate_hook_output_boundaries(
        &config.build.hooks,
        config.get_root(),
        &config.config_path,
        &config.build.publish_dir,
        config.vendor_workspace.as_deref(),
        &mut diagnostics,
    );
    validate_asset_source_boundaries(
        &config.assets,
        config.get_root(),
        &config.build.content_dir,
        &config.build.publish_dir,
        &mut diagnostics,
    );

    config.warnings.extend(
        diagnostics
            .experimental_fields()
            .iter()
            .map(|field| ConfigDiagnostic::experimental(*field))
            .chain(diagnostics.warnings().iter().cloned()),
    );

    diagnostics
        .into_result()
        .map_err(|diagnostics| ConfigError::Diagnostics(diagnostics).into())
}

/// No conventional site configuration exists in the searched ancestor directories.
#[derive(Debug, thiserror::Error)]
#[error("no `tola.toml` found in this directory or a parent directory")]
pub struct ConfigNotFound {
    /// Absolute directory from which the search began.
    pub start: PathBuf,
}

/// The configuration file name Tola discovers in a site directory.
pub const CONFIG_FILE_NAME: &str = "tola.toml";

/// Absolute configuration file path and the site root its parent directory selects.
///
/// A configuration path has to name a file: a directory, or a path whose last component is
/// not a name, identifies a site but no document the author could correct.
pub(super) fn resolve_config_location(path: &Path) -> Result<(PathBuf, PathBuf)> {
    if path.file_name().is_none() || path.is_dir() {
        return Err(config_path_error(path));
    }
    let path = std::path::absolute(path).map_err(|error| unresolved_config_path(path, error))?;
    let root = crate::filesystem::normalize_path(path.parent().unwrap_or(Path::new("")));
    Ok((path, root))
}

/// The failure of a configuration path that names no file.
///
/// The author changes the argument, not a line in a document, so the diagnostic has the
/// next action and no location.
fn config_path_error(path: &Path) -> anyhow::Error {
    let message = format!("`{}` is not a file", written_path(path));
    let diagnostic = crate::diagnostic::Diagnostic::new(
        crate::codes::config::INVALID,
        crate::diagnostic::Severity::Error,
        message.clone(),
    )
    .with_help("pass `--config` the path of the site's `tola.toml`");
    crate::diagnostic::DiagnosticError::attach(
        ConfigError::Validation(message).into(),
        vec![diagnostic],
    )
    .into()
}

/// The failure of a configuration path Tola could not make absolute.
///
/// The author changes the invocation, not a line in a document, so the diagnostic names the path
/// and the action that avoids the current directory it was resolved against.
fn unresolved_config_path(path: &Path, error: std::io::Error) -> anyhow::Error {
    invocation_error(
        format!(
            "Tola could not resolve the configuration path `{}`",
            written_path(path)
        ),
        error,
    )
}

/// The failure of reading the working directory a command finds the site from.
fn unreadable_directory_error(error: std::io::Error) -> anyhow::Error {
    invocation_error(
        "Tola could not read the current directory".to_owned(),
        error,
    )
}

/// Report a failure the author fixes by changing the command, not a line in a document.
///
/// The diagnostic has the next action and no location, the input/output error stays in the
/// chain for the debug log, and the message stays in the error so a consumer that drops
/// diagnostics still renders a sentence.
fn invocation_error(message: String, error: std::io::Error) -> anyhow::Error {
    let diagnostic = crate::diagnostic::Diagnostic::new(
        crate::codes::config::INVALID,
        crate::diagnostic::Severity::Error,
        message.clone(),
    )
    .with_help(INVOCATION_HELP);
    let error = anyhow::Error::new(error).context(message);
    crate::diagnostic::DiagnosticError::attach(error, vec![diagnostic]).into()
}

/// The action that avoids the working directory a configuration path is resolved against.
const INVOCATION_HELP: &str = "Pass `--config` the full path of the site's `tola.toml`";

/// Spell one configuration path the way the author wrote it.
///
/// The path is not resolved into a site yet, so it has no site root to be relative to: an
/// empty root asks the one path renderer for the path's own spelling, with `/` separators
/// everywhere. A spelling with no components has nothing to show, so it renders as the
/// directory it resolves to — the path the flag cannot use either way.
fn written_path(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        let resolved = crate::filesystem::absolute_path(path);
        return crate::filesystem::display_path(&resolved, Path::new(""));
    }
    crate::filesystem::display_path(path, Path::new(""))
}

pub(super) fn resolve_config_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(explicit) = explicit {
        // Keep the logical path so the watcher can detect config symlink retargets.
        let (path, _) = resolve_config_location(explicit)?;
        return Ok(path);
    }
    let cwd = std::env::current_dir().map_err(unreadable_directory_error)?;
    find_default_config(&cwd).ok_or_else(|| ConfigNotFound { start: cwd }.into())
}

/// The site root one configuration path selects.
///
/// An explicit path names the configuration file, so the directory that holds it is the root.
/// Without one, the nearest `tola.toml` in the current directory or an ancestor selects its
/// own directory; a site without a configuration file roots at the current directory.
pub fn config_site_root(explicit: Option<&Path>) -> Result<PathBuf> {
    match explicit {
        Some(path) => resolve_config_location(path).map(|(_, root)| root),
        None => {
            let cwd = std::env::current_dir().map_err(unreadable_directory_error)?;
            let root = find_default_config(&cwd)
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .unwrap_or(cwd);
            Ok(crate::filesystem::normalize_path(&root))
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
    ///
    /// `scope` reaches the boundary the source path is checked against even though nothing is read
    /// from disk: an editable schema for a path this scope refuses is refused here too.
    pub fn parse(path: &Path, content: &str, scope: crate::resources::InputScope) -> Result<Self> {
        Self::parse_at(path, content, scope).map(ParsedConfig::into_schema)
    }

    /// Resolve an explicitly constructed schema without a TOML round trip.
    /// All fields present in its serialized value count as explicit for status diagnostics.
    pub fn resolve(
        self,
        config_path: &Path,
        package_locations: tola_typst::PackageLocations,
        build: &BuildOverrides,
    ) -> Result<ResolvedSiteConfig> {
        let value = toml::Value::try_from(&self)
            .map_err(anyhow::Error::from)
            .map_err(|error| {
                crate::config::diagnostic::attach_load(error, Some(config_path), None)
            })?;
        let presence = ConfigPresence::from_value(&value);
        let (path, root) = resolve_config_location(config_path)
            .map_err(|error| crate::config::diagnostic::attach_load(error, None, None))?;
        resolve_schema(
            self,
            path.clone(),
            root,
            &presence,
            std::sync::Arc::new(crate::config::source::ConfigPositions::default()),
            package_locations,
            build,
        )
        .map_err(|error| crate::config::diagnostic::attach_load(error, Some(&path), None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::{
        VENDORED_SITE_SOURCE, attached_diagnostics, diagnostic_mentioning, diagnostic_message,
    };

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

    /// The validated configuration at `config_path`.
    fn load_config(config_path: &Path) -> SiteConfigWithSourceDigest {
        load_site_config(
            Some(config_path),
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .unwrap()
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
        for (source, expected) in [
            (
                "[site]\ntitle = \"Site\"\nlanguage = \"nope\"\n",
                ["site.language", "not a language tag"],
            ),
            (
                "[build]\nentry = \"site.typ\"\nminfy = true\n",
                ["unknown configuration fields", "`build.minfy`"],
            ),
        ] {
            let directory = tempfile::TempDir::new().unwrap();
            let config_path = write_config(directory.path(), source);

            let error = load_error(&config_path);

            let diagnostic = diagnostic_mentioning(&error, expected[0]);
            assert_eq!(diagnostic.code, crate::codes::config::INVALID);
            for substring in expected {
                assert!(diagnostic.message.contains(substring), "{diagnostic:?}");
            }
            let location = diagnostic.location.as_ref().unwrap();
            assert_eq!(location.path, "tola.toml");
            assert_eq!((location.line, location.column), (Some(3), Some(1)));
        }
    }

    #[test]
    fn unknown_fields_name_their_written_lines() {
        // A key and a table header are two spellings of the same field, and both are where the
        // author changes it: the span covers the key's own text, never the table it opens.
        for (source, written) in [
            (
                "[build]\nentry = \"site.typ\"\nminfy = true\n",
                [("build.minfy", 3, 2, 0, 5)],
            ),
            (
                "[build]\nentry = \"site.typ\"\n\n[build.minfy]\nunknown = 1\n",
                [("build.minfy", 4, 3, 7, 12)],
            ),
        ] {
            let directory = tempfile::TempDir::new().unwrap();
            let config_path = write_config(directory.path(), source);

            let error = load_error(&config_path);
            let diagnostic = diagnostic_mentioning(&error, "unknown configuration fields");

            assert_eq!(
                diagnostic.cause,
                Some(
                    crate::diagnostic::DiagnosticCause::UnknownConfigurationFields {
                        fields: written
                            .into_iter()
                            .map(|(name, line, written_line, start, end)| {
                                crate::diagnostic::UnknownField {
                                    name: name.to_owned(),
                                    line: Some(line),
                                    written: Some(crate::diagnostic::SourceRange {
                                        start: crate::diagnostic::SourcePosition {
                                            line: written_line,
                                            character: start,
                                        },
                                        end: crate::diagnostic::SourcePosition {
                                            line: written_line,
                                            character: end,
                                        },
                                    }),
                                }
                            })
                            .collect(),
                    }
                ),
                "{source:?}"
            );
        }
    }

    #[test]
    fn language_shape_error_locates_value() {
        let directory = tempfile::TempDir::new().unwrap();
        let config_path = write_config(
            directory.path(),
            "[site]\nlanguage = { lang = \"zh\", tag = \"zh-Hans-CN\" }\n",
        );

        let error = load_error(&config_path);

        let diagnostics = attached_diagnostics(&error);
        let [diagnostic] = diagnostics else {
            panic!("one language shape failure is reported: {diagnostics:?}");
        };
        assert_eq!(diagnostic.code, crate::codes::config::TOML);
        assert_eq!(diagnostic.message.matches("`site.language`").count(), 1);
        let location = diagnostic.location.as_ref().unwrap();
        assert_eq!(location.path, "tola.toml");
        assert_eq!((location.line, location.column), (Some(2), Some(12)));
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
    fn supplied_text_applies_overrides() {
        let directory = tempfile::TempDir::new().unwrap();
        let config_path = directory.path().join("tola.toml");
        let build = BuildOverrides {
            publish_dir: Some("dist".into()),
            minify: Some(false),
            origin: Some("https://EXAMPLE.test:443/".into()),
            base_path: Some("/docs/".into()),
        };

        let resolved = resolve_site_config(
            &config_path,
            &site_source("Site"),
            tola_typst::PackageLocations::default(),
            &build,
        )
        .unwrap();
        let config = resolved.config();

        assert_eq!(config.build().publish_dir, config.get_root().join("dist"));
        assert!(!config.build().minify.html);
        assert_eq!(
            config.site_url().as_deref(),
            Some("https://example.test/docs/")
        );
        assert!(!config_path.exists());
        assert!(!config.build().publish_dir.exists());
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
                .contains(directory.path().to_str().unwrap())
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
    fn supplied_text_needs_existing_site() {
        let directory = tempfile::TempDir::new().unwrap();
        let config_path = directory.path().join("missing/tola.toml");
        let error = resolve_site_config(
            &config_path,
            &site_source("Site"),
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .unwrap_err();

        let diagnostics = attached_diagnostics(&error);
        let diagnostic = diagnostic_mentioning(&error, "the site directory could not be used");

        assert_eq!(diagnostic.code, crate::codes::config::LOAD);
        assert!(diagnostics.iter().all(|diagnostic| {
            diagnostic
                .location
                .as_ref()
                .is_some_and(|location| location.path == "tola.toml")
        }));
    }

    #[test]
    fn config_path_selects_its_directory() {
        let directory = tempfile::TempDir::new().unwrap();
        let config_path = write_config(directory.path(), &site_source("Site"));

        let root = config_site_root(Some(&config_path)).unwrap();

        assert_eq!(root, crate::filesystem::normalize_path(directory.path()));
    }

    #[test]
    fn initial_load_resolves_output_path() {
        let directory = tempfile::TempDir::new().unwrap();
        let config_path = write_config(directory.path(), &site_source("Site"));

        let config = load_config(&config_path);

        assert_eq!(
            config.config().build.publish_dir,
            std::fs::canonicalize(directory.path())
                .unwrap()
                .join("public")
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
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("points outside the site directory")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn output_resolution_precedes_value_checks() {
        let directory = tempfile::TempDir::new().unwrap();
        let config_path = write_config(
            directory.path(),
            r#"[site]
title = "Site"
origin = "invalid"

[build]
publish-dir = "."
"#,
        );

        let error = load_error(&config_path);
        let message = diagnostic_message(&error);

        assert!(
            message.contains("points outside the site directory"),
            "{message}"
        );
    }

    #[test]
    fn base_path_rejects_encoded_separators() {
        let directory = tempfile::TempDir::new().unwrap();
        let config_path =
            write_config(directory.path(), "[site]\nbase-path = \"/docs%2fadmin/\"\n");

        let error = load_error(&config_path);
        let message = diagnostic_message(&error);
        let diagnostic = crate::diagnostic::attached(&error)
            .and_then(|diagnostics| diagnostics.first())
            .expect("the refusal has a diagnostic");

        assert!(message.contains("site.base-path"), "{message}");
        assert!(
            diagnostic
                .help
                .iter()
                .any(|help| help.message.contains("percent-encoded")),
            "{diagnostic:?}"
        );
    }

    #[test]
    fn default_discovery_uses_tola_toml() {
        let root = tempfile::TempDir::new().unwrap();
        let nested = root.path().join("content/posts");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.path().join("tola.toml"), "").unwrap();

        assert_eq!(
            find_default_config(&nested),
            Some(root.path().join("tola.toml"))
        );
    }

    #[test]
    fn invalid_nearest_config_stops_the_search() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("tola.toml"), "").unwrap();
        let nested = root.path().join("nested");
        let path = nested.join("tola.toml");
        std::fs::create_dir_all(&path).unwrap();

        assert_eq!(find_default_config(&nested), Some(path.clone()));
        assert!(ConfigSource::load(Some(&path), crate::resources::InputScope::Online).is_err());
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
        let error =
            ConfigSource::load(Some(&path), crate::resources::InputScope::Online).unwrap_err();
        assert!(!error.chain().any(|cause| cause.is::<ConfigNotFound>()));
        assert!(error.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        }));
    }

    #[test]
    fn schema_resolves_without_config_file() {
        let site_directory = tempfile::tempdir().unwrap();
        let config_path = site_directory.path().join("tola.toml");
        let mut schema = SiteConfigSchema::default();
        schema.site.origin = Some("https://EXAMPLE.test:443/".into());
        schema.site.base_path = "/文档/".into();
        schema.build.publish_dir = "dist".into();

        let config = schema
            .resolve(
                &config_path,
                tola_typst::PackageLocations::default(),
                &BuildOverrides::default(),
            )
            .unwrap();

        assert_eq!(config.build().publish_dir, config.get_root().join("dist"));
        assert_eq!(config.build().entry, config.get_root().join("site.typ"));
        assert_eq!(
            config.site_url().as_deref(),
            Some("https://example.test/%E6%96%87%E6%A1%A3/")
        );
        assert!(!config_path.exists());
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
        let cases: [SchemaCase<'_>; 5] = [
            (
                |schema| schema.build.entry = "../site.typ".into(),
                "`build.entry` is `../site.typ`",
            ),
            (
                |schema| schema.build.publish_dir = ".".into(),
                "`build.publish-dir` points outside the site directory",
            ),
            (
                |schema| schema.build.entry = ".tola/site.typ".into(),
                "`.tola/site.typ`",
            ),
            (
                |schema| schema.typst.fonts.paths = vec![".tola/fonts".into()],
                "`.tola/fonts`",
            ),
            (
                |schema| {
                    schema.vendor.path = Some("vendor".into());
                    schema.build.publish_dir = ".vendor-vendor/site".into();
                },
                "vendor working directory",
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

            diagnostic_mentioning(&error, expected);
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
        schema.build.hooks.before_build =
            vec![crate::config::section::build::BeforeBuildHookConfig {
                name: "generator".into(),
                command: vec!["generator".into()],
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
    fn later_hook_output_boundary_names_its_own_entry() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = write_config(
            directory.path(),
            r#"[[build.hooks.before-build]]
name = "first"
command = ["first.sh"]
generates = ["generated/first"]

[[build.hooks.before-build]]
name = "second"
command = ["second.sh"]
generates = [".tola/cache"]
"#,
        );

        let error = load_error(&config_path);

        let diagnostic = diagnostic_mentioning(&error, ".tola/cache");
        assert_eq!(diagnostic.code, crate::codes::config::INVALID);
        // Line 9 writes the second entry's own `generates`, not the first entry's line 4.
        let location = diagnostic
            .location
            .as_ref()
            .expect("the refusal is located");
        assert_eq!(
            (location.line, location.source_lines[0].text.as_str()),
            (Some(9), "generates = [\".tola/cache\"]"),
        );
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
        let candidate = config.get_root().join(".vendor-vendor/candidate");
        std::fs::create_dir_all(&candidate).unwrap();
        let config = config.with_vendor_root(Some(candidate.clone())).unwrap();
        let pure = crate::BuildResources::new().with_input_scope(crate::InputScope::Pure);
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
        let workspace = config.vendor.workspace_path().unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("old.typ"), "generated previous source").unwrap();
        let refreshed = config.with_vendor_root(None).unwrap();
        let files = tola_typst::FileResolver::new()
            .with_source_boundary(crate::BuildResources::new().source_boundary(&refreshed));
        assert!(
            files
                .read(
                    tola_typst::file_id(".vendor-vendor/old.typ"),
                    refreshed.get_root(),
                )
                .is_err()
        );
    }
}
