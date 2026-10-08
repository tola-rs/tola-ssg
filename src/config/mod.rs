//! Build, server, development, and diagnostic configuration.

mod dev;
mod diagnostics;
mod server;

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tola_build::InputScope;
use tola_build::config::loading::{
    BuildOverrides, SiteConfigWithSourceDigest, resolve_parsed_site_config,
};
use tola_build::config::{ConfigSource, FieldPath, ResolvedSiteConfig, SiteConfigSchema};
use tola_config::Config;

pub(crate) use dev::DevConfig;
pub(crate) use diagnostics::DiagnosticsConfig;
pub(crate) use server::{ServerConfig, ServerOverrides};

/// The core sections and the version declaration are read through
/// [`ConfigSource::decode_with_host`], so adding one to the core schema needs no edit here.
///
/// The derive gives each key the documentation its section's declaration writes, so `tola help`
/// and the editor answer `[server]`, `[dev]`, and `[diagnostics]` from one text.
#[derive(Debug, Default, Serialize, Deserialize, Config)]
#[serde(default)]
#[config(crate = tola_config, section = "")]
pub(crate) struct ConfigSchema {
    #[config(sub)]
    server: ServerConfig,

    #[config(sub)]
    dev: DevConfig,

    #[config(sub)]
    diagnostics: DiagnosticsConfig,
}

/// The host sections `tola.toml` is decoded through beyond the core schema, each as the field
/// list its own `Config` derive generated.
///
/// `tola-build` lists the core schema's keys; `[server]`, `[dev]`, and `[diagnostics]` are
/// decoded here, so their keys and documentation come from this list. The language server reads
/// `tola.toml` keys from it too, so a section added to [`ConfigSchema`] needs no second edit.
pub(crate) const HOST_SECTIONS: &[&[(FieldPath, Option<&'static str>)]] = &[
    ConfigSchema::DECLARED_FIELDS,
    ServerConfig::DECLARED_FIELDS,
    DevConfig::DECLARED_FIELDS,
    DiagnosticsConfig::DECLARED_FIELDS,
];

/// Every configuration key the host schema declares, in declaration order.
pub(crate) fn field_paths() -> impl Iterator<Item = FieldPath> {
    HOST_SECTIONS
        .iter()
        .flat_map(|fields| fields.iter().map(|(path, _)| *path))
}

/// The documentation a declared host key has, when its declaration writes one.
pub(crate) fn field_documentation(path: &str) -> Option<&'static str> {
    HOST_SECTIONS
        .iter()
        .flat_map(|fields| fields.iter())
        .find(|(field, _)| field.as_str() == path)
        .and_then(|(_, documentation)| *documentation)
}

/// What `tola help` adds under a host section's table.
pub(crate) fn section_help(section: &str) -> Option<&'static str> {
    [
        (ServerConfig::TEMPLATE_SECTION, ServerConfig::HELP),
        (DevConfig::TEMPLATE_SECTION, DevConfig::HELP),
        (DiagnosticsConfig::TEMPLATE_SECTION, DiagnosticsConfig::HELP),
    ]
    .into_iter()
    .find_map(|(name, help)| (name == section).then_some(help))
}

/// The invocation overrides a command applies on top of the configuration file.
///
/// The default overrides nothing.
#[derive(Debug, Clone, Default)]
pub(crate) struct ConfigOverrides {
    pub(crate) build: BuildOverrides,
    pub(crate) server: ServerOverrides,
    pub(crate) watch: Option<bool>,
}

struct ConfigInputs {
    packages: tola_typst::PackageLocations,
    overrides: ConfigOverrides,
}

pub(crate) struct LoadedConfig {
    core: SiteConfigWithSourceDigest,
    server: ServerConfig,
    dev: DevConfig,
    diagnostics: DiagnosticsConfig,
    loader: ConfigLoader,
}

impl LoadedConfig {
    pub(crate) fn config(&self) -> &ResolvedSiteConfig {
        self.core.config()
    }

    pub(crate) fn server(&self) -> &ServerConfig {
        &self.server
    }

    pub(crate) fn dev(&self) -> &DevConfig {
        &self.dev
    }

    pub(crate) fn diagnostics(&self) -> &DiagnosticsConfig {
        &self.diagnostics
    }

    pub(crate) fn into_config(self) -> ResolvedSiteConfig {
        self.core.into_config()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        Arc<ResolvedSiteConfig>,
        ServerConfig,
        DevConfig,
        ConfigLoader,
    ) {
        (
            Arc::new(self.core.into_config()),
            self.server,
            self.dev,
            self.loader,
        )
    }
}

impl std::fmt::Debug for LoadedConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoadedConfig")
            .field("config", self.core.config())
            .field("server", &self.server)
            .field("dev", &self.dev)
            .finish_non_exhaustive()
    }
}

pub(crate) fn load(
    explicit: Option<&Path>,
    scope: InputScope,
    packages: tola_typst::PackageLocations,
    overrides: &ConfigOverrides,
) -> Result<LoadedConfig> {
    let source = ConfigSource::load(explicit, scope).map_err(|error| {
        if !error
            .chain()
            .any(|cause| cause.is::<tola_build::config::loading::ConfigNotFound>())
        {
            return error;
        }
        let diagnostics = tola_build::diagnostic::attached(&error)
            .map(<[_]>::to_vec)
            .unwrap_or_else(|| {
                vec![tola_build::diagnostic::fallback(
                    crate::codes::config::LOAD,
                    &error,
                )]
            })
            .into_iter()
            .map(|diagnostic| diagnostic.with_help("Run `tola init` to create a site"))
            .collect();
        tola_build::diagnostic::DiagnosticError::attach(error, diagnostics).into()
    })?;
    resolve(source, packages, overrides)
}

pub(crate) fn resolve(
    source: ConfigSource,
    packages: tola_typst::PackageLocations,
    overrides: &ConfigOverrides,
) -> Result<LoadedConfig> {
    let inputs = Arc::new(ConfigInputs {
        packages,
        overrides: overrides.clone(),
    });
    let (core, server, dev, diagnostics) = resolve_source(&source, &inputs)?;
    Ok(LoadedConfig {
        core,
        server,
        dev,
        diagnostics,
        loader: ConfigLoader { source, inputs },
    })
}

fn resolve_source(
    source: &ConfigSource,
    inputs: &ConfigInputs,
) -> Result<(
    SiteConfigWithSourceDigest,
    ServerConfig,
    DevConfig,
    DiagnosticsConfig,
)> {
    let (core, host) = source.decode_with_host::<ConfigSchema>()?;
    let mut server = host.schema().server.clone();
    let mut dev = host.schema().dev.clone();
    let diagnostics = host.schema().diagnostics.clone();
    if let Some(interface) = inputs.overrides.server.interface {
        server.interface = interface;
    }
    if let Some(port) = inputs.overrides.server.port {
        server.port = port;
    }
    if let Some(watch) = inputs.overrides.watch {
        dev.watch = watch;
    }
    let core = resolve_parsed_site_config(core, inputs.packages.clone(), &inputs.overrides.build)?;
    Ok((core, server, dev, diagnostics))
}

pub(crate) fn validate_settings(
    path: &Path,
    text: &str,
    scope: InputScope,
) -> Result<SiteConfigSchema> {
    let (parsed, _) = ConfigSource::parse(path, text, scope)?.decode_with_host::<ConfigSchema>()?;
    parsed.validate_settings()?;
    Ok(parsed.into_schema())
}

/// A reloaded configuration awaiting acceptance by the running session.
pub(crate) struct ConfigCandidate {
    config: Arc<ResolvedSiteConfig>,
    server: ServerConfig,
    dev: DevConfig,
    diagnostics: DiagnosticsConfig,
    source: ConfigSource,
    inputs: Arc<ConfigInputs>,
}

impl ConfigCandidate {
    pub(crate) fn config(&self) -> Arc<ResolvedSiteConfig> {
        Arc::clone(&self.config)
    }

    pub(crate) fn server(&self) -> &ServerConfig {
        &self.server
    }

    pub(crate) fn dev(&self) -> &DevConfig {
        &self.dev
    }

    pub(crate) fn diagnostics(&self) -> &DiagnosticsConfig {
        &self.diagnostics
    }

    /// Whether a reloaded configuration leaves every build setting unchanged.
    ///
    /// A `true` answer keeps the caller's prepared session and resources. `server`, `dev`, and
    /// `diagnostics` are host settings, applied separately from this comparison.
    pub(crate) fn has_same_build_settings(&self, config: &ResolvedSiteConfig) -> Result<bool> {
        Ok(self.config.get_root() == config.get_root()
            && self.config.package_locations() == config.package_locations()
            && same_section(self.config.site(), config.site())?
            && same_section(self.config.build(), config.build())?
            && same_section(self.config.assets(), config.assets())?
            && same_section(self.config.fonts(), config.fonts())?
            && same_section(self.config.icons(), config.icons())?
            && same_section(self.config.vendor(), config.vendor())?
            && self.config.warnings().len() == config.warnings().len()
            && self
                .config
                .warnings()
                .iter()
                .zip(config.warnings())
                .all(|(left, right)| {
                    left.field == right.field
                        && left.message == right.message
                        && left.help == right.help
                        && left.severity() == right.severity()
                        && left.tag() == right.tag()
                }))
    }
}

/// Whether two configuration sections hold the same values.
fn same_section(left: &impl Serialize, right: &impl Serialize) -> Result<bool> {
    Ok(same_setting_value(
        &toml::Value::try_from(left)?,
        &toml::Value::try_from(right)?,
    ))
}

fn same_setting_value(left: &toml::Value, right: &toml::Value) -> bool {
    match (left, right) {
        (toml::Value::Float(left), toml::Value::Float(right)) => {
            !left.is_nan() && !right.is_nan() && left.to_bits() == right.to_bits()
        }
        (toml::Value::Array(left), toml::Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| same_setting_value(left, right))
        }
        (toml::Value::Table(left), toml::Value::Table(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|((left_key, left), (right_key, right))| {
                        left_key == right_key && same_setting_value(left, right)
                    })
        }
        _ => left == right,
    }
}

impl std::fmt::Debug for ConfigCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfigCandidate")
            .field("config", &self.config)
            .field("server", &self.server)
            .field("dev", &self.dev)
            .finish_non_exhaustive()
    }
}

/// Reloads the same configuration source with the original overrides.
pub(crate) struct ConfigLoader {
    source: ConfigSource,
    inputs: Arc<ConfigInputs>,
}

impl ConfigLoader {
    pub(crate) fn path(&self) -> &Path {
        self.source.path()
    }

    pub(crate) fn load_candidate(&self) -> Result<Option<ConfigCandidate>> {
        self.resolve_changed(self.source.read_changed()?)
    }

    pub(crate) fn load_text_candidate(&self, text: &str) -> Result<Option<ConfigCandidate>> {
        self.resolve_changed(self.source.changed_text(text)?)
    }

    fn resolve_changed(&self, source: Option<ConfigSource>) -> Result<Option<ConfigCandidate>> {
        let Some(source) = source else {
            return Ok(None);
        };
        let (core, server, dev, diagnostics) = resolve_source(&source, &self.inputs)?;
        Ok(Some(ConfigCandidate {
            config: Arc::new(core.into_config()),
            server,
            dev,
            diagnostics,
            source,
            inputs: Arc::clone(&self.inputs),
        }))
    }

    pub(crate) fn acknowledge(&mut self, candidate: &ConfigCandidate) {
        assert!(
            Arc::ptr_eq(&self.inputs, &candidate.inputs),
            "configuration candidate belongs to another invocation"
        );
        self.source = candidate.source.clone();
    }
}

impl std::fmt::Debug for ConfigLoader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfigLoader")
            .field("path", &self.source.path())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(title: &str) -> String {
        format!(
            "[site]\ntitle = {}\n",
            serde_json::to_string(title).unwrap()
        )
    }

    fn load_at(path: &Path) -> LoadedConfig {
        load(
            Some(path),
            InputScope::Online,
            tola_typst::PackageLocations::default(),
            &ConfigOverrides::default(),
        )
        .unwrap()
    }

    #[test]
    fn single_document_supplies_all_sections() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        let source = "[server]\nport = 9000\n[dev]\nwatch = false\n[diagnostics]\nmax_errors = 7\nmax_warnings = 1\n[build.minify]\nhtml = false\n";
        let parsed = ConfigSource::parse(&path, source, InputScope::Online).unwrap();
        assert!(parsed.presence().contains("server.port"));
        assert!(parsed.presence().contains("build.minify.html"));

        let loaded = resolve(
            parsed,
            tola_typst::PackageLocations::default(),
            &ConfigOverrides::default(),
        )
        .unwrap();
        assert_eq!(loaded.server().port, 9000);
        assert!(!loaded.dev().watch);
        assert_eq!(loaded.diagnostics().max_errors, Some(7));
        assert_eq!(loaded.diagnostics().max_warnings, Some(1));
        assert!(!loaded.config().build().minify.html);
        assert_eq!(loaded.core.source_hash(), blake3::hash(source.as_bytes()));
        assert!(loaded.config().warnings().is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn server_defaults_bind_loopback() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(&path, text("Site")).unwrap();
        let loaded = load_at(&path);

        assert_eq!(loaded.server().interface.to_string(), "127.0.0.1");
        assert_eq!(loaded.server().port, 5277);
    }

    #[test]
    fn unknown_root_section_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let source = ConfigSource::parse(
            &directory.path().join("tola.toml"),
            "[unknown]\nvalue = true\n",
            InputScope::Online,
        )
        .unwrap();
        let error = resolve(
            source,
            tola_typst::PackageLocations::default(),
            &ConfigOverrides::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown"));
        assert!(tola_build::diagnostic::attached(&error).is_some());
    }

    #[test]
    fn misspelled_core_key_stops_the_load() {
        let directory = tempfile::tempdir().unwrap();
        let source = ConfigSource::parse(
            &directory.path().join("tola.toml"),
            "[assets]\ncache_busting = true\n\n[site]\ntitel = \"Misspelled\"\n",
            InputScope::Online,
        )
        .unwrap();
        let error = resolve(
            source,
            tola_typst::PackageLocations::default(),
            &ConfigOverrides::default(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("site.titel"), "{error:#}");
        assert!(tola_build::diagnostic::attached(&error).is_some());
    }

    #[test]
    fn type_error_keeps_its_source_line() {
        let directory = tempfile::tempdir().unwrap();
        let source = ConfigSource::parse(
            &directory.path().join("tola.toml"),
            "[server]\nport = \"wrong\"\n",
            InputScope::Online,
        )
        .unwrap();
        let error = resolve(
            source,
            tola_typst::PackageLocations::default(),
            &ConfigOverrides::default(),
        )
        .unwrap_err();
        let diagnostic = &tola_build::diagnostic::attached(&error).unwrap()[0];
        assert_eq!(diagnostic.code, "config.toml");
        let location = diagnostic.location.as_ref().unwrap();
        assert_eq!(location.line, Some(2));
        assert_eq!(location.source_lines[0].text, "port = \"wrong\"");
    }

    #[test]
    fn reload_reuses_initial_overrides() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(&path, text("First")).unwrap();
        let mut overrides = ConfigOverrides {
            build: BuildOverrides {
                publish_dir: Some("dist".into()),
                ..BuildOverrides::default()
            },
            server: ServerOverrides {
                interface: Some("0.0.0.0".parse().unwrap()),
                port: Some(8000),
            },
            watch: Some(false),
        };
        let loaded = load(
            Some(&path),
            InputScope::Online,
            tola_typst::PackageLocations::default(),
            &overrides,
        )
        .unwrap();
        let (first, server, _, mut loader) = loaded.into_parts();
        overrides.build.publish_dir = Some("other-output".into());
        overrides.server.port = Some(9000);
        overrides.watch = Some(true);
        let changed = text("Second");
        std::fs::write(&path, &changed).unwrap();

        let candidate = loader.load_candidate().unwrap().unwrap();
        assert_eq!(first.site().title, "First");
        assert_eq!(candidate.config().site().title, "Second");
        assert_eq!(server.port, 8000);
        assert_eq!(
            candidate.config().build().publish_dir,
            first.get_root().join("dist")
        );
        assert_eq!(candidate.server().interface.to_string(), "0.0.0.0");
        assert_eq!(candidate.server().port, 8000);
        assert!(!candidate.dev().watch);
        assert_eq!(
            candidate.source.source_hash(),
            blake3::hash(changed.as_bytes())
        );
        assert_eq!(
            overrides.build.publish_dir,
            Some(std::path::PathBuf::from("other-output"))
        );
        assert_eq!(overrides.server.port, Some(9000));
        assert_eq!(overrides.watch, Some(true));
        loader.acknowledge(&candidate);
        assert!(loader.load_candidate().unwrap().is_none());
    }

    #[test]
    fn failed_reload_keeps_the_source() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(&path, text("First")).unwrap();
        let (_, _, _, mut loader) = load_at(&path).into_parts();
        let accepted = loader.source.source_hash();
        std::fs::write(&path, "[\n").unwrap();
        assert!(loader.load_candidate().is_err());
        assert_eq!(loader.source.source_hash(), accepted);
        std::fs::write(&path, text("Repaired")).unwrap();
        let candidate = loader.load_candidate().unwrap().unwrap();
        loader.acknowledge(&candidate);
        assert_eq!(candidate.config().site().title, "Repaired");
        assert!(loader.load_candidate().unwrap().is_none());
    }

    #[test]
    fn unsaved_text_leaves_disk_source_alone() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(&path, text("Disk")).unwrap();
        let loaded = load(
            Some(&path),
            InputScope::Online,
            tola_typst::PackageLocations::default(),
            &ConfigOverrides {
                build: BuildOverrides {
                    publish_dir: Some("dist".into()),
                    ..BuildOverrides::default()
                },
                ..ConfigOverrides::default()
            },
        )
        .unwrap();
        let (config, _, _, mut loader) = loaded.into_parts();
        let candidate = loader
            .load_text_candidate(&text("Buffer"))
            .unwrap()
            .unwrap();
        assert_eq!(candidate.config().site().title, "Buffer");
        assert_eq!(candidate.config().get_root(), config.get_root());
        assert_eq!(
            candidate.config().build().publish_dir,
            config.get_root().join("dist")
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text("Disk"));
        loader.acknowledge(&candidate);
        assert!(
            loader
                .load_text_candidate(&text("Buffer"))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            loader
                .load_candidate()
                .unwrap()
                .unwrap()
                .config()
                .site()
                .title,
            "Disk"
        );
    }

    #[test]
    fn edited_section_changes_build_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        // Every row edits a section that decides the source check's inputs, so a
        // reload keeping its session would answer from that section's previous values.
        let rows = [
            ("assets", "[assets]\ncache-busting = true\n"),
            ("fonts", "[typst.fonts]\nsystem = true\n"),
            (
                "icons",
                "[icons.collections.local]\nsource-type = \"local-json\"\npath = \"icons.json\"\n",
            ),
            ("vendor", "[vendor]\npath = \"vendor\"\n"),
        ];
        for (section, added) in rows {
            std::fs::write(&path, "# base\n").unwrap();
            let (config, _, _, loader) = load_at(&path).into_parts();
            std::fs::write(&path, format!("# base\n{added}")).unwrap();
            let candidate = loader.load_candidate().unwrap().unwrap();
            assert!(
                config.warnings().is_empty() && candidate.config().warnings().is_empty(),
                "{section} must load without a warning of its own"
            );
            assert!(
                !candidate.has_same_build_settings(&config).unwrap(),
                "{section} must be a build setting"
            );
        }
    }

    #[test]
    fn comment_edit_leaves_build_settings_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(&path, text("Same")).unwrap();
        let (config, _, _, loader) = load_at(&path).into_parts();
        let candidate = loader
            .load_text_candidate(&format!("{}# note\n", text("Same")))
            .unwrap()
            .unwrap();
        assert!(candidate.has_same_build_settings(&config).unwrap());
    }

    #[test]
    fn float_equality_preserves_representation() {
        assert!(!same_setting_value(
            &toml::Value::Float(0.0),
            &toml::Value::Float(-0.0)
        ));
        assert!(!same_setting_value(
            &toml::Value::Float(f64::NAN),
            &toml::Value::Float(f64::NAN)
        ));
        assert!(!same_setting_value(
            &toml::Value::Integer(1),
            &toml::Value::Float(1.0)
        ));
        assert!(same_setting_value(
            &toml::Value::Float(1.0),
            &toml::Value::Float(1.0)
        ));
    }

    #[test]
    fn loaders_acknowledge_independently() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_path = first.path().join("tola.toml");
        let second_path = second.path().join("tola.toml");
        std::fs::write(&first_path, text("First")).unwrap();
        std::fs::write(&second_path, text("Second")).unwrap();
        let (_, _, _, mut first_loader) = load_at(&first_path).into_parts();
        let (_, _, _, second_loader) = load_at(&second_path).into_parts();
        std::fs::write(&first_path, text("Changed")).unwrap();
        let candidate = first_loader.load_candidate().unwrap().unwrap();
        first_loader.acknowledge(&candidate);
        assert!(first_loader.load_candidate().unwrap().is_none());
        assert!(second_loader.load_candidate().unwrap().is_none());
    }

    #[test]
    #[cfg(unix)]
    fn symlinked_config_keeps_site_root() {
        let directory = tempfile::tempdir().unwrap();
        let site = directory.path().join("site");
        let shared = directory.path().join("shared");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        let target = shared.join("tola.toml");
        std::fs::write(&target, text("First")).unwrap();
        let path = site.join("tola.toml");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let (first, _, _, loader) = load_at(&path).into_parts();
        std::fs::write(&target, text("Second")).unwrap();
        let candidate = loader.load_candidate().unwrap().unwrap();
        assert_eq!(candidate.config().get_root(), first.get_root());
        assert_eq!(candidate.config().build().entry, first.build().entry);
    }

    #[test]
    fn host_accessors_answer_their_keys() {
        let paths = field_paths().map(|path| path.as_str()).collect::<Vec<_>>();
        for (section, key, help) in [
            ("server", "server.port", ServerConfig::HELP),
            ("dev", "dev.watch", DevConfig::HELP),
            (
                "diagnostics",
                "diagnostics.max_errors",
                DiagnosticsConfig::HELP,
            ),
        ] {
            assert!(paths.contains(&section), "`{section}` is not a field path");
            assert!(paths.contains(&key), "`{key}` is not a field path");
            assert!(field_documentation(key).is_some(), "`{key}` has no meaning");
            assert_eq!(section_help(section), Some(help));
        }
    }

    /// The core sections stay `tola-build`'s, which `tola help` consults first.
    #[test]
    fn host_accessors_ignore_core_keys() {
        for section in ["site", "build", "icons", "typst"] {
            assert!(section_help(section).is_none(), "`{section}` answers here");
            assert!(!field_paths().any(|path| path.as_str() == section));
        }
        for key in ["build.entry", "icons.collections", "typst.fonts.system"] {
            assert!(field_documentation(key).is_none(), "`{key}` answers here");
        }
    }
}
