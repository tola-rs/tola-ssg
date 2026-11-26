//! Build, server, development, and diagnostic configuration.

mod dev;
mod diagnostics;
mod server;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tola_build::config::loading::{
    BuildOverrides, SiteConfigWithSourceDigest, resolve_parsed_site_config,
};
use tola_build::config::{ConfigSource, FieldPath, SiteConfig, SiteConfigSchema};
use tola_config::Config;
use tola_input::InputScope;

pub(crate) use dev::DevConfig;
pub(crate) use diagnostics::DiagnosticsConfig;
pub(crate) use server::{ServerConfig, ServerOverrides};

/// Host-only configuration sections decoded alongside the core schema.
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

/// Host sections decoded beyond the core schema, in declaration order.
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

/// Documentation for a declared host key, when available.
pub(crate) fn field_documentation(path: &str) -> Option<&'static str> {
    HOST_SECTIONS
        .iter()
        .flat_map(|fields| fields.iter())
        .find(|(field, _)| field.as_str() == path)
        .and_then(|(_, doc)| *doc)
}

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

impl std::fmt::Debug for LoadedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedConfig")
            .field("config", self.core.config())
            .field("server", &self.server)
            .field("dev", &self.dev)
            .finish_non_exhaustive()
    }
}

impl LoadedConfig {
    pub(crate) fn config(&self) -> &SiteConfig {
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

    pub(crate) fn into_config(self) -> SiteConfig {
        self.core.into_config()
    }

    pub(crate) fn into_parts(self) -> (Arc<SiteConfig>, ServerConfig, DevConfig, ConfigLoader) {
        (
            Arc::new(self.core.into_config()),
            self.server,
            self.dev,
            self.loader,
        )
    }
}

pub(crate) fn load(
    explicit: Option<&Path>,
    scope: InputScope,
    packages: tola_typst::PackageLocations,
    overrides: &ConfigOverrides,
) -> Result<LoadedConfig> {
    let src = ConfigSource::load(explicit, scope).map_err(|err| {
        if !matches!(err, tola_build::config::ConfigSourceError::NotFound { .. }) {
            return anyhow::Error::new(err);
        }
        let diags = err
            .diagnostics()
            .iter()
            .cloned()
            .map(|diag| diag.with_help("Run `tola init` to create a site"))
            .collect();
        tola_diagnostic::DiagnosticError::attach(err.into(), diags).into()
    })?;
    resolve(src, packages, overrides)
}

pub(crate) fn resolve(
    src: ConfigSource,
    packages: tola_typst::PackageLocations,
    overrides: &ConfigOverrides,
) -> Result<LoadedConfig> {
    let inputs = Arc::new(ConfigInputs {
        packages,
        overrides: overrides.clone(),
    });
    let (core, server, dev, diagnostics) = resolve_source(&src, &inputs)?;
    Ok(LoadedConfig {
        core,
        server,
        dev,
        diagnostics,
        loader: ConfigLoader {
            source: src,
            inputs,
        },
    })
}

fn resolve_source(
    src: &ConfigSource,
    inputs: &ConfigInputs,
) -> Result<(
    SiteConfigWithSourceDigest,
    ServerConfig,
    DevConfig,
    DiagnosticsConfig,
)> {
    let (core, host) = src.decode_with_host::<ConfigSchema>()?;
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
    let core = resolve_parsed_site_config(core, inputs.packages.clone(), &inputs.overrides.build)?
        .with_hook_runner(crate::cli::hook_runner());
    Ok((core, server, dev, diagnostics))
}

pub(crate) fn validate_settings(
    path: &Path,
    text: &str,
    scope: InputScope,
    overlays: &[(PathBuf, Arc<str>)],
) -> Result<(SiteConfigSchema, Vec<PathBuf>)> {
    let src = ConfigSource::parse_with_sources(path, text, scope, overlays, None)?;
    let paths = src.paths().map(Path::to_path_buf).collect();
    let (parsed, _) = src.decode_with_host::<ConfigSchema>()?;
    parsed.validate_settings()?;
    Ok((parsed.into_schema(), paths))
}

/// A reloaded configuration awaiting acceptance by the running session.
pub(crate) struct ConfigCandidate {
    config: Arc<SiteConfig>,
    server: ServerConfig,
    dev: DevConfig,
    diagnostics: DiagnosticsConfig,
    source: ConfigSource,
    inputs: Arc<ConfigInputs>,
}

impl ConfigCandidate {
    pub(crate) fn config(&self) -> Arc<SiteConfig> {
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

    #[cfg(test)]
    pub(crate) fn has_same_effective_settings(&self, cfg: &SiteConfig) -> Result<bool> {
        Ok(self.config.root() == cfg.root()
            && self.config.package_locations() == cfg.package_locations()
            && same_section(self.config.site(), cfg.site())?
            && same_section(self.config.build(), cfg.build())?
            && self.config.module_entry() == cfg.module_entry()
            && self.config.modules() == cfg.modules()
            && self.config.hooks() == cfg.hooks()
            && self.config.options() == cfg.options()
            && same_section(self.config.assets(), cfg.assets())?
            && same_section(self.config.fonts(), cfg.fonts())?
            && same_section(self.config.icons(), cfg.icons())?
            && same_section(self.config.vendor(), cfg.vendor())?
            && same_section(self.config.formatter(), cfg.formatter())?
            && self.config.warnings().len() == cfg.warnings().len()
            && self
                .config
                .warnings()
                .iter()
                .zip(cfg.warnings())
                .all(|(left, right)| {
                    left.field == right.field
                        && left.message == right.message
                        && left.help == right.help
                        && left.severity() == right.severity()
                        && left.tag() == right.tag()
                }))
    }
}

#[cfg(test)]
fn same_section(left: &impl Serialize, right: &impl Serialize) -> Result<bool> {
    Ok(toml::Value::try_from(left)? == toml::Value::try_from(right)?)
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

    #[cfg(test)]
    fn load_text_candidate(&self, text: &str) -> Result<Option<ConfigCandidate>> {
        self.load_sources_candidate(&[(self.path().to_path_buf(), Arc::from(text))], None)
    }

    pub(crate) fn load_sources_candidate(
        &self,
        overlays: &[(PathBuf, Arc<str>)],
        cancellation: Option<&tola_build::cancellation::BuildCancellation>,
    ) -> Result<Option<ConfigCandidate>> {
        self.resolve_changed(self.source.changed_sources(overlays, cancellation)?)
    }

    fn resolve_changed(&self, src: Option<ConfigSource>) -> Result<Option<ConfigCandidate>> {
        let Some(src) = src else {
            return Ok(None);
        };
        let (core, server, dev, diagnostics) = resolve_source(&src, &self.inputs)?;
        Ok(Some(ConfigCandidate {
            config: Arc::new(core.into_config()),
            server,
            dev,
            diagnostics,
            source: src,
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
    fn single_document_all_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tola.toml");
        let src = "[server]\nport = 9000\n[dev]\nwatch = false\n[diagnostics]\nmax_errors = 7\nmax_warnings = 1\n[build.minify]\nhtml = false\n";
        let parsed = ConfigSource::parse(&path, src, InputScope::Online).unwrap();
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
        assert_eq!(
            loaded.config().source().unwrap().sources()[0].hash(),
            blake3::hash(src.as_bytes())
        );
        assert!(loaded.config().warnings().is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn server_defaults_loopback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tola.toml");
        std::fs::write(&path, text("Site")).unwrap();
        let loaded = load_at(&path);

        assert_eq!(loaded.server().interface.to_string(), "127.0.0.1");
        assert_eq!(loaded.server().port, 5277);
    }

    #[test]
    fn unknown_keys_rejected() {
        let dir = tempfile::tempdir().unwrap();
        for (toml, key) in [
            ("[unknown]\nvalue = true\n", "unknown"),
            (
                "[assets]\ncache_busting = true\n\n[site]\ntitel = \"Misspelled\"\n",
                "site.titel",
            ),
        ] {
            let src = ConfigSource::parse(&dir.path().join("tola.toml"), toml, InputScope::Online)
                .unwrap();
            let err = resolve(
                src,
                tola_typst::PackageLocations::default(),
                &ConfigOverrides::default(),
            )
            .unwrap_err();
            assert!(err.to_string().contains(key), "{err:#}");
            assert!(tola_diagnostic::attached(&err).is_some());
        }
    }

    #[test]
    fn reload_reuses_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tola.toml");
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
            first.root().join("dist")
        );
        assert_eq!(candidate.server().interface.to_string(), "0.0.0.0");
        assert_eq!(candidate.server().port, 8000);
        assert!(!candidate.dev().watch);
        assert_eq!(
            candidate.source.sources()[0].hash(),
            blake3::hash(changed.as_bytes())
        );
        loader.acknowledge(&candidate);
        assert!(loader.load_candidate().unwrap().is_none());
    }

    #[test]
    fn failed_reload_retains_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tola.toml");
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
    fn unsaved_text_preserves_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tola.toml");
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
        let (cfg, _, _, mut loader) = loaded.into_parts();
        let candidate = loader
            .load_text_candidate(&text("Buffer"))
            .unwrap()
            .unwrap();
        assert_eq!(candidate.config().site().title, "Buffer");
        assert_eq!(candidate.config().root(), cfg.root());
        assert_eq!(
            candidate.config().build().publish_dir,
            cfg.root().join("dist")
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
    fn mount_identity_equivalence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tola.toml");
        std::fs::write(&path, "[site]\nbase-path = \"/文档/\"\n").unwrap();
        let (cfg, _, _, loader) = load_at(&path).into_parts();

        for (base_path, unchanged) in [("/%e6%96%87%e6%a1%a3/", true), ("/notes/", false)] {
            let src = format!("[site]\nbase-path = \"{base_path}\"\n");
            let candidate = loader.load_text_candidate(&src).unwrap().unwrap();
            assert_eq!(
                candidate.has_same_effective_settings(&cfg).unwrap(),
                unchanged
            );
        }
    }

    #[test]
    fn edited_section_changes_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tola.toml");
        let rows = [
            ("assets", "[assets]\ncache-busting = true\n"),
            ("fonts", "[typst.fonts]\nsystem = true\n"),
            (
                "icons",
                "[icons.collections.local]\nsource-type = \"local-json\"\npath = \"icons.json\"\n",
            ),
            ("vendor", "[vendor]\npath = \"vendor\"\n"),
            ("formatter", "[formatter]\nindent = 4\n"),
            (
                "hooks",
                "[[hooks.before-build]]\nname = 'prepare'\nargv = ['tool']\n",
            ),
            ("options", "[options]\nsearch = true\n"),
            ("modules", "[modules.notes]\npath = 'theme'\n"),
        ];
        for (section, added) in rows {
            std::fs::write(&path, "# base\n").unwrap();
            let (cfg, _, _, loader) = load_at(&path).into_parts();
            std::fs::write(&path, format!("# base\n{added}")).unwrap();
            let candidate = loader.load_candidate().unwrap().unwrap();
            assert!(
                cfg.warnings().is_empty() && candidate.config().warnings().is_empty(),
                "{section}"
            );
            assert!(
                !candidate.has_same_effective_settings(&cfg).unwrap(),
                "{section}"
            );
        }
    }

    #[test]
    fn module_entry_alias_changes_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tola.toml");
        let modules = "[modules.first]\npath = 'theme'\n[modules.second]\npath = 'theme'\n";
        let src = |alias| format!("[build]\nentry = {{ module = '{alias}' }}\n{modules}");
        std::fs::write(&path, src("first")).unwrap();
        let (cfg, _, _, loader) = load_at(&path).into_parts();
        let candidate = loader.load_text_candidate(&src("second")).unwrap().unwrap();

        assert!(!candidate.has_same_effective_settings(&cfg).unwrap());
    }

    #[test]
    fn comment_edit_preserves_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tola.toml");
        std::fs::write(&path, text("Same")).unwrap();
        let (cfg, _, _, loader) = load_at(&path).into_parts();
        let candidate = loader
            .load_text_candidate(&format!("{}# note\n", text("Same")))
            .unwrap()
            .unwrap();
        assert!(candidate.has_same_effective_settings(&cfg).unwrap());
    }

    #[test]
    fn fragment_comment_updates_digest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = root.join("tola.toml");
        let fragment = root.join("site.toml");
        std::fs::write(&path, "include = ['site.toml']\n").unwrap();
        std::fs::write(&fragment, text("Same")).unwrap();
        let (cfg, _, _, mut loader) = load_at(&path).into_parts();
        let prev = loader.source.source_hash();
        std::fs::write(&fragment, format!("{}# note\n", text("Same"))).unwrap();
        let candidate = loader.load_candidate().unwrap().unwrap();
        assert!(candidate.has_same_effective_settings(&cfg).unwrap());
        assert_ne!(candidate.source.source_hash(), prev);
        loader.acknowledge(&candidate);
        assert!(loader.load_candidate().unwrap().is_none());
    }

    #[test]
    #[cfg(unix)]
    fn symlinked_config_preserves_root() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path().join("site");
        let shared = dir.path().join("shared");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        let target = shared.join("tola.toml");
        std::fs::write(&target, text("First")).unwrap();
        let path = site.join("tola.toml");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let (first, _, _, loader) = load_at(&path).into_parts();
        std::fs::write(&target, text("Second")).unwrap();
        let candidate = loader.load_candidate().unwrap().unwrap();
        assert_eq!(candidate.config().root(), first.root());
        assert_eq!(candidate.config().build().entry, first.build().entry);
    }

    #[test]
    fn host_accessors_cover_keys() {
        let paths = field_paths().map(|path| path.as_str()).collect::<Vec<_>>();
        for (section, key) in [
            ("server", "server.port"),
            ("dev", "dev.watch"),
            ("diagnostics", "diagnostics.max_errors"),
        ] {
            assert!(paths.contains(&section));
            assert!(paths.contains(&key));
            assert!(field_documentation(key).is_some());
        }
    }

    #[test]
    fn host_accessors_ignore_core() {
        for section in ["site", "build", "icons", "typst"] {
            assert!(!field_paths().any(|path| path.as_str() == section));
        }
        for key in ["build.entry", "icons.collections", "typst.fonts.system"] {
            assert!(field_documentation(key).is_none());
        }
    }
}
