//! One parsed configuration source shared by schema decoding and resolution.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::ConfigSourceRefusal;
use super::{ConfigError, ConfigPresence};
use tola_diagnostic::{Diagnostic, DiagnosticError, SourcePositions};
use tola_input::InputScope;

/// Why a configuration source could not be found, read, or decoded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigSourceError {
    /// No configuration file was found in the current directory or its ancestors.
    #[error("{diagnostic}")]
    NotFound {
        /// Directory from which the search began.
        start: PathBuf,
        #[source]
        diagnostic: DiagnosticError,
    },
    /// The invocation could not select a configuration path.
    #[error("{0}")]
    Path(#[source] DiagnosticError),
    /// A configuration file could not be read or is outside the allowed source paths.
    #[error("{0}")]
    Read(#[source] DiagnosticError),
    /// The configuration text could not be parsed as TOML.
    #[error("{0}")]
    Syntax(#[source] DiagnosticError),
    /// The parsed values could not be decoded as configuration fields.
    #[error("{0}")]
    Schema(#[source] DiagnosticError),
    /// The source boundary could not determine where this file resolves.
    #[error("{0}")]
    SourcePath(#[from] typst::diag::FileError),
}

impl ConfigSourceError {
    /// The configuration diagnostics recorded when the operation failed.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        match self {
            Self::NotFound { diagnostic, .. }
            | Self::Path(diagnostic)
            | Self::Read(diagnostic)
            | Self::Syntax(diagnostic)
            | Self::Schema(diagnostic) => diagnostic.diagnostics(),
            Self::SourcePath(_) => &[],
        }
    }
}

/// Exact TOML bytes, their parsed document, and the site root and input scope selected at loading.
///
/// A host decodes its own sections beside the core ones without reading or parsing the
/// document again. Reloads retain the original root and scope.
#[derive(Debug, Clone)]
pub struct ConfigSource {
    path: PathBuf,
    root: PathBuf,
    scope: InputScope,
    document: Arc<toml_edit::Document<Arc<str>>>,
    positions: Arc<SourcePositions>,
    presence: Arc<ConfigPresence>,
    core_presence: Arc<ConfigPresence>,
    hash: blake3::Hash,
}

impl ConfigSource {
    /// Parse supplied text. The path supplies diagnostic identity and its parent the root.
    ///
    /// `scope` decides which physical paths count as sources, so unsaved editor text for a file
    /// this scope refuses is rejected exactly as a disk read would be.
    pub fn parse(path: &Path, source: &str, scope: InputScope) -> Result<Self, ConfigSourceError> {
        let (path, root) = super::loading::resolve_config_location(path)?;
        check_source_path(&root, &path, scope, None)?;
        Self::parse_at_root(path, root, source.into(), scope)
    }

    /// Read and parse one configuration document under `scope`.
    pub fn load(explicit: Option<&Path>, scope: InputScope) -> Result<Self, ConfigSourceError> {
        let (path, root) = super::loading::select_config_location(explicit)?;
        let source = read_source(&root, &path, scope, None)?;
        Self::parse_at_root(path, root, source.into(), scope)
    }

    /// The configuration a workspace that holds no site configuration resolves through.
    ///
    /// The schema's defaults apply at the workspace's own root, and the document reports the path
    /// where a site configuration for that root would live. Nothing is read.
    pub fn defaults(root: &Path, scope: InputScope) -> Result<Self, ConfigSourceError> {
        let root = tola_input::normalize_path(root);
        let path = root.join(super::loading::CONFIG_FILE_NAME);
        check_source_path(&root, &path, scope, None)?;
        Self::parse_at_root(path, root, Arc::from(""), scope)
    }

    /// Parse bytes whose source path has already been checked.
    fn parse_at_root(
        path: PathBuf,
        root: PathBuf,
        source: Arc<str>,
        scope: InputScope,
    ) -> Result<Self, ConfigSourceError> {
        let document = toml_edit::Document::parse(Arc::clone(&source)).map_err(|error| {
            let diagnostic =
                super::diagnostic::syntax_error(&path, &source, error.message(), error.span());
            ConfigSourceError::Syntax(DiagnosticError::attach(error.into(), vec![diagnostic]))
        })?;
        let mut value: toml::Value = toml::Value::deserialize(toml_edit::de::Deserializer::from(
            document.clone(),
        ))
        .map_err(|error| {
            let diagnostic =
                super::diagnostic::schema_error(&path, &document, error.message(), error.span());
            ConfigSourceError::Schema(DiagnosticError::attach(error.into(), vec![diagnostic]))
        })?;
        let positions = Arc::new(SourcePositions::of(&document, Arc::clone(&source)));
        let presence = Arc::new(ConfigPresence::from_value(&value));
        // Select real top-level keys before presence flattens paths. A host key
        // containing dots must not impersonate a nested core configuration field.
        value
            .as_table_mut()
            .expect("a parsed configuration document has a table root")
            .retain(|field, _| is_core_key(field));
        let core_presence = Arc::new(ConfigPresence::from_value(&value));
        Ok(Self {
            path,
            root,
            scope,
            document: Arc::new(document),
            positions,
            presence,
            core_presence,
            hash: blake3::hash(source.as_bytes()),
        })
    }

    /// Decode a host schema from this parsed document, rejecting unknown fields.
    pub fn decode<T: DeserializeOwned>(&self) -> Result<ParsedConfig<T>, ConfigSourceError> {
        self.decode_reporting_unknown_fields(|_| true)
    }

    /// Read the formatter section without resolving site inputs.
    pub fn formatter(&self) -> Result<super::section::FormatterConfig, ConfigSourceError> {
        use super::section::FormatterConfig;

        #[derive(Deserialize)]
        struct FormatterSection {
            #[serde(default)]
            formatter: FormatterConfig,
        }

        let parsed: ParsedConfig<FormatterSection> =
            self.decode_reporting_unknown_fields(|path| {
                root_field(path) == Some(FormatterConfig::TEMPLATE_SECTION)
            })?;
        Ok(parsed.into_schema().formatter)
    }

    /// Decode the core sections beside a host schema that declares only the rest.
    pub fn decode_with_host<T: DeserializeOwned>(
        &self,
    ) -> Result<(ParsedConfig<super::SiteConfigSchema>, ParsedConfig<T>), ConfigSourceError> {
        let core = self.decode_site()?;
        let host = self.decode_reporting_unknown_fields(|path| {
            root_field(path).is_none_or(|field| !is_core_key(field))
        })?;
        Ok((core, host))
    }

    pub(super) fn decode_site(
        &self,
    ) -> Result<ParsedConfig<super::SiteConfigSchema>, ConfigSourceError> {
        self.decode_reporting_unknown_fields(|path| root_field(path).is_some_and(is_core_key))
    }

    fn decode_reporting_unknown_fields<T: DeserializeOwned>(
        &self,
        report_unknown: impl Fn(&serde_ignored::Path<'_>) -> bool,
    ) -> Result<ParsedConfig<T>, ConfigSourceError> {
        // The declaration is classified before the document's unknown fields are reported.
        let mut ignored = Vec::new();
        let schema = serde_ignored::deserialize(
            toml_edit::de::Deserializer::from(self.document.as_ref().clone()),
            |path: serde_ignored::Path| {
                if report_unknown(&path) {
                    ignored.push(config_field_name(&path));
                }
            },
        )
        .map_err(|error: toml_edit::de::Error| {
            let diagnostic = super::diagnostic::schema_error(
                &self.path,
                &self.document,
                error.message(),
                error.span(),
            );
            ConfigSourceError::Schema(DiagnosticError::attach(error.into(), vec![diagnostic]))
        })?;
        if !ignored.is_empty() {
            let error = ConfigError::UnknownFields { fields: ignored };
            return Err(ConfigSourceError::Schema(
                super::diagnostic::report_source_error(
                    error,
                    Some(&self.path),
                    Some(&self.positions),
                ),
            ));
        }
        Ok(ParsedConfig {
            schema,
            source: self.clone(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn source_hash(&self) -> blake3::Hash {
        self.hash
    }

    pub fn presence(&self) -> &ConfigPresence {
        &self.presence
    }

    pub(super) fn core_presence(&self) -> &ConfigPresence {
        &self.core_presence
    }

    /// Where this document writes each configuration key.
    pub(super) fn positions(&self) -> &Arc<SourcePositions> {
        &self.positions
    }

    /// The load diagnostic an error from this document carries, located in this source.
    pub(super) fn attach_load(&self, error: anyhow::Error) -> anyhow::Error {
        super::diagnostic::attach_load(
            error,
            Some(self.path.as_path()),
            Some(self.positions.as_ref()),
        )
    }

    /// Read the same source, parsing only changed bytes and retaining its original root and scope.
    pub fn read_changed(&self) -> Result<Option<Self>, ConfigSourceError> {
        let source = read_source(&self.root, &self.path, self.scope, Some(&self.positions))?;
        self.parse_changed(&source)
    }

    /// Parse changed text from the same source while retaining its bound root and scope.
    pub fn changed_text(&self, source: &str) -> Result<Option<Self>, ConfigSourceError> {
        check_source_path(&self.root, &self.path, self.scope, Some(&self.positions))?;
        self.parse_changed(source)
    }

    fn parse_changed(&self, source: &str) -> Result<Option<Self>, ConfigSourceError> {
        if blake3::hash(source.as_bytes()) == self.hash {
            return Ok(None);
        }
        Self::parse_at_root(
            self.path.clone(),
            self.root.clone(),
            source.into(),
            self.scope,
        )
        .map(Some)
    }
}

fn read_source(
    root: &Path,
    path: &Path,
    scope: InputScope,
    positions: Option<&SourcePositions>,
) -> Result<String, ConfigSourceError> {
    check_source_path(root, path, scope, positions)?;
    std::fs::read_to_string(path).map_err(|error| {
        ConfigSourceError::Read(super::diagnostic::report_source_error(
            ConfigError::Io(path.to_owned(), error),
            Some(path),
            positions,
        ))
    })
}

fn check_source_path(
    root: &Path,
    path: &Path,
    scope: InputScope,
    positions: Option<&SourcePositions>,
) -> Result<(), ConfigSourceError> {
    let boundary = crate::resources::base_source_boundary(root, scope);
    let Some(refusal) = boundary.refusal(path)? else {
        return Ok(());
    };
    let reason = match refusal {
        tola_typst::SourceRefusal::GeneratedState => ConfigSourceRefusal::GeneratedState,
        tola_typst::SourceRefusal::OutsideSite => ConfigSourceRefusal::OutsideSite,
    };
    Err(ConfigSourceError::Read(
        super::diagnostic::report_source_error(
            ConfigError::Refused {
                path: path.to_owned(),
                reason,
            },
            Some(path),
            positions,
        ),
    ))
}

/// Whether `field` names a top-level key the core schema reads: one of its sections.
fn is_core_key(field: &str) -> bool {
    super::SiteConfigSchema::SECTIONS.contains(&field)
}

/// Spell map keys and sequence indices without Serde's wrapper markers.
fn config_field_name(path: &serde_ignored::Path<'_>) -> String {
    use serde_ignored::Path;
    use std::borrow::Cow;

    fn append(path: &Path<'_>, name: &mut String) -> bool {
        let (parent, segment) = match path {
            Path::Root => return false,
            Path::Map { parent, key } => (parent, Cow::Borrowed(key.as_str())),
            Path::Seq { parent, index } => (parent, Cow::Owned(index.to_string())),
            Path::Some { parent }
            | Path::NewtypeStruct { parent }
            | Path::NewtypeVariant { parent } => return append(parent, name),
        };
        if append(parent, name) {
            name.push('.');
        }
        name.push_str(&segment);
        true
    }

    let mut name = String::new();
    append(path, &mut name);
    name
}

fn root_field<'a>(path: &'a serde_ignored::Path<'_>) -> Option<&'a str> {
    match path {
        serde_ignored::Path::Root => None,
        serde_ignored::Path::Map { parent, key } => root_field(parent).or(Some(key.as_str())),
        serde_ignored::Path::Seq { parent, .. }
        | serde_ignored::Path::Some { parent }
        | serde_ignored::Path::NewtypeStruct { parent }
        | serde_ignored::Path::NewtypeVariant { parent } => root_field(parent),
    }
}

/// Typed sections decoded from a configuration source, retaining its field presence.
#[derive(Debug)]
pub struct ParsedConfig<T> {
    pub(super) schema: T,
    pub(super) source: ConfigSource,
}

impl<T> ParsedConfig<T> {
    pub fn schema(&self) -> &T {
        &self.schema
    }

    pub fn source(&self) -> &ConfigSource {
        &self.source
    }

    /// Project host sections while retaining their original source identity and presence.
    pub fn map_schema<U>(self, project: impl FnOnce(T) -> U) -> ParsedConfig<U> {
        ParsedConfig {
            schema: project(self.schema),
            source: self.source,
        }
    }

    pub fn into_schema(self) -> T {
        self.schema
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::loading::{BuildOverrides, resolve_parsed_site_config};

    #[test]
    #[cfg(unix)]
    fn changed_text_retains_the_bound_root() {
        let directory = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let alias = directory.path().join("site");
        std::os::unix::fs::symlink(first.path(), &alias).unwrap();
        let source = ConfigSource::parse(&alias.join("tola.toml"), "", InputScope::Online).unwrap();
        assert!(source.changed_text("").unwrap().is_none());
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(second.path(), &alias).unwrap();

        let changed = source
            .changed_text("[site]\ntitle = 'Changed'\n")
            .unwrap()
            .unwrap();

        assert_eq!(changed.path(), source.path());
        assert_eq!(changed.root(), source.root());
        assert_ne!(
            changed.root(),
            tola_input::normalize_existing_prefix(&alias)
        );
        assert_eq!(
            changed.decode_site().unwrap().schema().site.title,
            "Changed"
        );
    }

    #[test]
    fn dotted_host_sections_stay_host_owned() {
        #[derive(Default, serde::Deserialize)]
        #[serde(default)]
        struct HostConfig {
            site: crate::config::section::SiteSectionConfig,
            build: crate::config::BuildSectionConfig,
            #[serde(rename = "host.example.section")]
            dotted_host: std::collections::BTreeMap<String, toml::Value>,
        }

        let directory = tempfile::tempdir().unwrap();
        let text = "[\"host.example.section\"]\nenable = true\n";
        let source = ConfigSource::parse(
            &directory.path().join("tola.toml"),
            text,
            InputScope::Online,
        )
        .unwrap();
        assert!(source.presence().contains("host.example.section"));
        let direct = source.decode_site().unwrap();
        let host = source.decode::<HostConfig>().unwrap();
        assert_eq!(
            host.schema().dotted_host["enable"],
            toml::Value::Boolean(true)
        );
        let projected = host.map_schema(|host| crate::config::SiteConfigSchema {
            site: host.site,
            build: host.build,
            ..crate::config::SiteConfigSchema::default()
        });

        for parsed in [direct, projected] {
            let core = resolve_parsed_site_config(
                parsed,
                tola_typst::PackageLocations::default(),
                &BuildOverrides::default(),
            )
            .unwrap();
            assert!(core.config().warnings().is_empty());
            assert_eq!(core.source_hash(), blake3::hash(text.as_bytes()));
        }
    }

    #[derive(Debug, serde::Deserialize)]
    struct NumericSettings {
        #[serde(rename = "server")]
        _server: NumericFields,
    }

    #[derive(Debug, serde::Deserialize)]
    struct NumericFields {
        #[serde(rename = "port")]
        _port: u16,
        #[serde(rename = "limit")]
        _limit: usize,
    }

    #[test]
    fn numeric_schema_errors_key_value() {
        for (field, value, maximum) in [
            ("port", "70000", u16::MAX.to_string()),
            ("port", "-1", u16::MAX.to_string()),
            ("limit", "-1", usize::MAX.to_string()),
        ] {
            let port = if field == "port" { value } else { "5277" };
            let limit = if field == "limit" { value } else { "2" };
            let text = format!("[server]\nport = {port}\nlimit = {limit}\n");
            let directory = tempfile::tempdir().unwrap();
            let source = ConfigSource::parse(
                &directory.path().join("tola.toml"),
                &text,
                InputScope::Online,
            )
            .unwrap();
            let error = source.decode::<NumericSettings>().unwrap_err();
            let diagnostic = &error.diagnostics()[0];
            assert_eq!(diagnostic.code, tola_diagnostic::codes::config::TOML);
            assert!(
                diagnostic.message.contains(&format!("`server.{field}`")),
                "{}",
                diagnostic.message
            );
            assert!(
                diagnostic.message.contains(&format!("`{value}`")),
                "{}",
                diagnostic.message
            );
            assert!(
                diagnostic.message.contains(&maximum),
                "{}",
                diagnostic.message
            );
            let location = diagnostic.location.as_ref().unwrap();
            let line = &location.source_lines[0];
            let (start, end) = line.highlight.unwrap();
            assert_eq!(&line.text[start..end], value);
        }
    }

    #[test]
    fn table_value_error_keeps_owning_key() {
        for (text, key, highlighted) in [
            (
                "[server]\nport = { x = 1 }\nlimit = 2\n",
                "server.port",
                "{ x = 1 }",
            ),
            (
                "server = { port = { x = 1 }, limit = 2 }\n",
                "server.port",
                "{ x = 1 }",
            ),
            (
                "[server]\nport = 5277\nlimit = { x = 1 }\n",
                "server.limit",
                "{ x = 1 }",
            ),
            (
                "[server]\nlimit = 2\n[server.port]\nx = 1\n",
                "server.port",
                "[server.port]",
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let source = ConfigSource::parse(
                &directory.path().join("tola.toml"),
                text,
                InputScope::Online,
            )
            .unwrap();
            let error = source.decode::<NumericSettings>().unwrap_err();
            let diagnostic = &error.diagnostics()[0];
            assert_eq!(diagnostic.code, tola_diagnostic::codes::config::TOML);
            assert!(
                diagnostic.message.contains(&format!("`{key}`")),
                "{}",
                diagnostic.message
            );
            let line = &diagnostic.location.as_ref().unwrap().source_lines[0];
            let (start, end) = line.highlight.unwrap();
            assert_eq!(&line.text[start..end], highlighted);
        }
    }

    #[test]
    #[cfg(unix)]
    fn pure_refuses_configuration_outside_the_site() {
        let directory = tempfile::tempdir().unwrap();
        let site = directory.path().join("site");
        let outside = tempfile::tempdir().unwrap();
        let text = "[site]\ntitle = \"Site\"\n";
        std::fs::create_dir(&site).unwrap();
        std::fs::write(outside.path().join("tola.toml"), text).unwrap();
        let path = site.join("tola.toml");
        std::fs::write(&path, text).unwrap();
        let source = ConfigSource::load(Some(&path), InputScope::Pure).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(outside.path().join("tola.toml"), &path).unwrap();

        for error in [
            ConfigSource::load(Some(&path), InputScope::Pure).unwrap_err(),
            ConfigSource::parse(&path, text, InputScope::Pure).unwrap_err(),
            source.read_changed().unwrap_err(),
            source.changed_text(text).unwrap_err(),
        ] {
            assert!(anyhow::Error::new(error).chain().any(|cause| matches!(
                cause.downcast_ref::<ConfigError>(),
                Some(ConfigError::Refused {
                    reason: ConfigSourceRefusal::OutsideSite,
                    ..
                })
            )));
        }

        assert!(ConfigSource::load(Some(&path), InputScope::Online).is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn pure_refuses_internal_state() {
        let directory = tempfile::tempdir().unwrap();
        let site = directory.path().join("site");
        let internal = site.join(tola_input::INTERNAL_DIR);
        std::fs::create_dir_all(&internal).unwrap();
        std::fs::write(internal.join("tola.toml"), "[site]\ntitle = \"Cached\"\n").unwrap();
        // The supplied path's parent is the site root, so the `.tola` exclusion applies to the
        // physical target rather than to a root derived from the file itself.
        let alias = site.join("tola.toml");
        std::os::unix::fs::symlink(internal.join("tola.toml"), &alias).unwrap();

        let error = ConfigSource::load(Some(&alias), InputScope::Pure).unwrap_err();
        assert!(anyhow::Error::new(error).chain().any(|cause| matches!(
            cause.downcast_ref::<ConfigError>(),
            Some(ConfigError::Refused {
                reason: ConfigSourceRefusal::GeneratedState,
                ..
            })
        )));
    }

    #[test]
    fn composed_decode_refuses_unknown_keys() {
        #[derive(Debug, Default, serde::Deserialize)]
        #[serde(default)]
        struct EditorConfig {
            mode: String,
        }

        #[derive(Debug, Default, serde::Deserialize)]
        #[serde(default)]
        struct HostConfig {
            #[serde(rename = "host-editor")]
            editor: EditorConfig,
        }

        let directory = tempfile::tempdir().unwrap();
        let accepted = ConfigSource::parse(
            &directory.path().join("tola.toml"),
            "[host-editor]\nmode = \"custom\"\n",
            InputScope::Online,
        )
        .unwrap();
        let (_, host) = accepted.decode_with_host::<HostConfig>().unwrap();
        assert_eq!(host.schema().editor.mode, "custom");

        for (text, key) in [
            ("[unknown]\nvalue = true\n", "unknown"),
            ("[site]\ntitel = \"Misspelled\"\n", "site.titel"),
            (
                "[host-editor]\nmode = \"custom\"\n\"spe?ed\" = 2\n",
                "host-editor.spe?ed",
            ),
            // A root key this schema no longer declares, which earlier sites still hold.
            ("version = \"0.8.0\"\n", "version"),
        ] {
            let source = ConfigSource::parse(
                &directory.path().join("tola.toml"),
                text,
                InputScope::Online,
            )
            .unwrap();
            let error = source.decode_with_host::<HostConfig>().unwrap_err();
            let Some(tola_diagnostic::DiagnosticCause::UnknownConfigurationFields { fields }) =
                &error.diagnostics()[0].cause
            else {
                panic!("missing unknown field diagnostic for {text}");
            };
            assert_eq!(fields[0].name, key);
        }
    }
}
