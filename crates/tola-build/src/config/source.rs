//! One parsed configuration source shared by schema decoding and resolution.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::ConfigSourceRefusal;
use super::{ConfigError, ConfigPresence};
use crate::resources::InputScope;

/// Where every key of one parsed configuration document is written, by dotted path.
///
/// A diagnostic that names a configuration field asks for that key's excerpt, so the author reads
/// the line they must change instead of only the field's name. Paths use the spelling field paths
/// use, including an array element's decimal index.
#[derive(Debug, Clone)]
pub(crate) struct ConfigPositions {
    source: Arc<str>,
    keys: std::collections::BTreeMap<String, std::ops::Range<usize>>,
}

impl Default for ConfigPositions {
    fn default() -> Self {
        Self {
            source: Arc::from(""),
            keys: std::collections::BTreeMap::new(),
        }
    }
}

impl ConfigPositions {
    /// Record every key of one parsed document against the exact source that produced it.
    pub(crate) fn of(document: &toml_edit::Document<Arc<str>>, source: Arc<str>) -> Self {
        let mut keys = std::collections::BTreeMap::new();
        collect_table_keys(document.as_table(), "", &mut keys);
        Self { source, keys }
    }

    /// The written key a dotted configuration path names, with its line and highlight.
    pub(crate) fn excerpt(&self, path: &str) -> Option<SourceExcerpt> {
        source_excerpt(&self.source, self.keys.get(path)?.clone())
    }

    /// The exact span the document writes one dotted configuration path in.
    pub(crate) fn written(&self, path: &str) -> Option<crate::diagnostic::SourceRange> {
        source_range(&self.source, self.keys.get(path)?.clone())
    }
}

fn collect_table_keys(
    table: &toml_edit::Table,
    prefix: &str,
    keys: &mut std::collections::BTreeMap<String, std::ops::Range<usize>>,
) {
    for (name, _) in table.iter() {
        let Some((key, item)) = table.get_key_value(name) else {
            continue;
        };
        let path = joined_path(prefix, name);
        // A table header writes its key without a value line, so the table's own span is where
        // that key is written.
        let span = key.span().or_else(|| match item {
            toml_edit::Item::Table(table) => table.span(),
            _ => None,
        });
        if let Some(span) = span {
            record_key_span(keys, &path, span);
        }
        collect_item_keys(item, &path, keys);
    }
}

fn collect_item_keys(
    item: &toml_edit::Item,
    path: &str,
    keys: &mut std::collections::BTreeMap<String, std::ops::Range<usize>>,
) {
    match item {
        toml_edit::Item::Table(table) => collect_table_keys(table, path, keys),
        toml_edit::Item::ArrayOfTables(elements) => {
            for (index, table) in elements.iter().enumerate() {
                let element = joined_path(path, &index.to_string());
                // An element has no key of its own, so its header line is where it starts.
                if let Some(span) = table.span() {
                    record_key_span(keys, &element, span);
                }
                collect_table_keys(table, &element, keys);
            }
        }
        toml_edit::Item::Value(toml_edit::Value::InlineTable(inline)) => {
            for (name, _) in inline.iter() {
                let Some((key, item)) = inline.get_key_value(name) else {
                    continue;
                };
                let path = joined_path(path, name);
                if let Some(span) = key.span() {
                    record_key_span(keys, &path, span);
                }
                collect_item_keys(item, &path, keys);
            }
        }
        toml_edit::Item::Value(_) | toml_edit::Item::None => {}
    }
}

/// Keep the first key a path names, so a later duplicate cannot move an author elsewhere.
fn record_key_span(
    keys: &mut std::collections::BTreeMap<String, std::ops::Range<usize>>,
    path: &str,
    span: std::ops::Range<usize>,
) {
    keys.entry(path.to_owned()).or_insert(span);
}

fn joined_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}.{name}")
    }
}

pub(super) fn key_at_span(
    document: &toml_edit::Document<Arc<str>>,
    span: &std::ops::Range<usize>,
) -> Option<String> {
    table_key_at_span(document.as_table(), "", span)
}

fn table_key_at_span(
    table: &toml_edit::Table,
    prefix: &str,
    span: &std::ops::Range<usize>,
) -> Option<String> {
    table
        .iter()
        .find_map(|(name, item)| item_key_at_span(item, &joined_path(prefix, name), span))
}

fn item_key_at_span(
    item: &toml_edit::Item,
    path: &str,
    span: &std::ops::Range<usize>,
) -> Option<String> {
    match item {
        toml_edit::Item::Table(table) => table_key_at_span(table, path, span).or_else(|| {
            table
                .span()
                .filter(|table| table.start <= span.start && span.end <= table.end)
                .map(|_| path.to_owned())
        }),
        toml_edit::Item::ArrayOfTables(tables) => {
            tables.iter().enumerate().find_map(|(index, table)| {
                table_key_at_span(table, &joined_path(path, &index.to_string()), span)
            })
        }
        toml_edit::Item::Value(value @ toml_edit::Value::InlineTable(table)) => table
            .iter()
            .find_map(|(name, _)| {
                let (_, value) = table.get_key_value(name)?;
                item_key_at_span(value, &joined_path(path, name), span)
            })
            .or_else(|| {
                value
                    .span()
                    .filter(|value| value.start <= span.start && span.end <= value.end)
                    .map(|_| path.to_owned())
            }),
        toml_edit::Item::Value(value) => value
            .span()
            .filter(|value| value.start <= span.start && span.end <= value.end)
            .map(|_| path.to_owned()),
        toml_edit::Item::None => None,
    }
}

/// The written line one span covers, with the highlight that points inside it.
pub(crate) struct SourceExcerpt {
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) text: String,
    pub(crate) highlight: (usize, usize),
}

/// The exact UTF-16 range one byte span covers, clamped to the text the document holds.
pub(super) fn source_range(
    content: &str,
    span: std::ops::Range<usize>,
) -> Option<crate::diagnostic::SourceRange> {
    Some(crate::diagnostic::SourceRange {
        start: source_position(content, span.start)?,
        end: source_position(content, span.end)?,
    })
}

/// The zero-based UTF-16 position one byte offset names.
fn source_position(content: &str, offset: usize) -> Option<crate::diagnostic::SourcePosition> {
    let offset = offset.min(content.len());
    let offset = (0..=offset)
        .rev()
        .find(|candidate| content.is_char_boundary(*candidate))?;
    let line_start = content[..offset]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    Some(crate::diagnostic::SourcePosition {
        line: content[..line_start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count(),
        character: content[line_start..offset]
            .chars()
            .map(char::len_utf16)
            .sum(),
    })
}

pub(super) fn source_excerpt(content: &str, span: std::ops::Range<usize>) -> Option<SourceExcerpt> {
    let mut start = span.start.min(content.len());
    start = (0..=start)
        .rev()
        .find(|offset| content.is_char_boundary(*offset))?;
    if start == content.len() && content.ends_with('\n') {
        start -= 1;
    }
    let line_start = content[..start].rfind('\n').map_or(0, |offset| offset + 1);
    let line_end = content[start..]
        .find('\n')
        .map_or(content.len(), |offset| start + offset);
    let mut highlight = (start - line_start, span.end.min(line_end) - line_start);
    if highlight.1 <= highlight.0 {
        highlight.0 = highlight.0.saturating_sub(1);
        highlight.1 = highlight.0 + usize::from(line_end > line_start);
    }
    Some(SourceExcerpt {
        line: content[..line_start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1,
        column: content[line_start..start].chars().count() + 1,
        text: content[line_start..line_end].to_owned(),
        highlight,
    })
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
    positions: Arc<ConfigPositions>,
    presence: Arc<ConfigPresence>,
    core_presence: Arc<ConfigPresence>,
    hash: blake3::Hash,
}

impl ConfigSource {
    /// Parse supplied text. The path supplies diagnostic identity and its parent the root.
    ///
    /// `scope` decides which physical paths count as sources, so unsaved editor text for a file
    /// this scope refuses is rejected exactly as a disk read would be.
    pub fn parse(path: &Path, source: &str, scope: InputScope) -> Result<Self> {
        let (path, root) = super::loading::resolve_config_location(path)
            .map_err(|error| super::diagnostic::attach_load(error, None, None))?;
        Self::parse_at_root(path, root, source.into(), scope)
    }

    /// Read and parse one configuration document under `scope`.
    pub fn load(explicit: Option<&Path>, scope: InputScope) -> Result<Self> {
        let path = super::loading::resolve_config_path(explicit)
            .map_err(|error| super::diagnostic::attach_load(error, None, None))?;
        // The root is normalized: containment compares physical paths, so a site reached through a
        // symlinked alias must still recognize its own configuration file.
        let root = crate::filesystem::normalize_path(path.parent().unwrap_or(Path::new("")));
        if let Some(reason) = source_refusal(&root, &path, scope)? {
            return Err(super::diagnostic::attach_load(
                ConfigError::Refused { path, reason }.into(),
                None,
                None,
            ));
        }
        let source = std::fs::read_to_string(&path)
            .map_err(|error| ConfigError::Io(path.clone(), error))
            .map_err(|error| super::diagnostic::attach_load(error.into(), Some(&path), None))?;
        Self::parse(&path, &source, scope)
    }

    /// The configuration a workspace that holds no site configuration resolves through.
    ///
    /// The schema's defaults apply at the workspace's own root, and the document reports the path
    /// where a site configuration for that root would live. Nothing is read.
    pub fn defaults(root: &Path, scope: InputScope) -> Result<Self> {
        let root = crate::filesystem::normalize_path(root);
        Self::parse_at_root(
            root.join(super::loading::CONFIG_FILE_NAME),
            root,
            Arc::from(""),
            scope,
        )
    }

    /// Construct from bytes already in hand, after one boundary check.
    ///
    /// Every construction path passes here, so this is the single gate: [`Self::parse`] and
    /// [`Self::changed_text`] have no earlier check. [`Self::load`] and [`Self::read_changed`]
    /// check first only to avoid reading bytes from a path this gate would refuse.
    fn parse_at_root(
        path: PathBuf,
        root: PathBuf,
        source: Arc<str>,
        scope: InputScope,
    ) -> Result<Self> {
        if let Some(reason) = source_refusal(&root, &path, scope)? {
            return Err(super::diagnostic::attach_load(
                ConfigError::Refused { path, reason }.into(),
                None,
                None,
            ));
        }
        let document = toml_edit::Document::parse(Arc::clone(&source)).map_err(|error| {
            let diagnostic =
                super::diagnostic::syntax_error(&path, &source, error.message(), error.span());
            super::diagnostic::attach_source(error.into(), diagnostic)
        })?;
        let mut value: toml::Value = toml::Value::deserialize(toml_edit::de::Deserializer::from(
            document.clone(),
        ))
        .map_err(|error| {
            let diagnostic =
                super::diagnostic::schema_error(&path, &document, error.message(), error.span());
            super::diagnostic::attach_source(error.into(), diagnostic)
        })?;
        let positions = Arc::new(ConfigPositions::of(&document, Arc::clone(&source)));
        let presence = Arc::new(ConfigPresence::from_value(&value));
        // Select real top-level keys before presence flattens paths. A host key
        // containing dots must not impersonate a nested core configuration field.
        value
            .as_table_mut()
            .expect("a parsed configuration document has a table root")
            .retain(|field, _| super::SiteConfigSchema::SECTIONS.contains(&field));
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
    pub fn decode<T: DeserializeOwned>(&self) -> Result<ParsedConfig<T>> {
        self.decode_reporting_unknown_fields(|_| true)
    }

    /// Decode the core sections together with a host schema that declares only the sections
    /// the core schema does not own.
    ///
    /// A host composes the two without repeating the core section list. Both halves reject an
    /// unknown field, and they cannot share one pass: a flattened section hides its unknown
    /// keys from the field-path report, so the core sections are decoded once and the host's own
    /// sections are decoded again.
    pub fn decode_with_host<T: DeserializeOwned>(
        &self,
    ) -> Result<(ParsedConfig<super::SiteConfigSchema>, ParsedConfig<T>)> {
        let core = self.decode_site()?;
        let host = self.decode_reporting_unknown_fields(|path| {
            root_field(path).is_none_or(|field| !is_core_key(field))
        })?;
        Ok((core, host))
    }

    pub(super) fn decode_site(&self) -> Result<ParsedConfig<super::SiteConfigSchema>> {
        self.decode_reporting_unknown_fields(|path| root_field(path).is_some_and(is_core_key))
    }

    fn decode_reporting_unknown_fields<T: DeserializeOwned>(
        &self,
        report_unknown: impl Fn(&serde_ignored::Path<'_>) -> bool,
    ) -> Result<ParsedConfig<T>> {
        // The declaration is classified before the document's unknown fields are reported.
        let mut ignored = Vec::new();
        let schema = serde_ignored::deserialize(
            toml_edit::de::Deserializer::from(self.document.as_ref().clone()),
            |path: serde_ignored::Path| {
                if report_unknown(&path) {
                    ignored.push(config_field_name(&path.to_string()));
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
            super::diagnostic::attach_source(error.into(), diagnostic)
        })?;
        if !ignored.is_empty() {
            let error = ConfigError::UnknownFields { fields: ignored };
            return Err(super::diagnostic::attach_load(
                error.into(),
                Some(&self.path),
                Some(&self.positions),
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
    pub(super) fn positions(&self) -> &Arc<ConfigPositions> {
        &self.positions
    }

    /// Read the same source, parsing only changed bytes and retaining its original root and scope.
    ///
    /// The path is re-checked against the boundary because a configuration file can be replaced by
    /// a link after the initial load; a reload must refuse exactly what the first read refused.
    pub fn read_changed(&self) -> Result<Option<Self>> {
        if let Some(reason) = source_refusal(&self.root, &self.path, self.scope)? {
            return Err(super::diagnostic::attach_reload(
                ConfigError::Refused {
                    path: self.path.clone(),
                    reason,
                }
                .into(),
                &self.path,
                Some(&self.positions),
            ));
        }
        let source = std::fs::read_to_string(&self.path)
            .map_err(|error| ConfigError::Io(self.path.clone(), error))
            .map_err(|error| {
                super::diagnostic::attach_reload(error.into(), &self.path, Some(&self.positions))
            })?;
        self.changed_text(&source)
    }

    /// Parse changed text from the same source while retaining its bound root and scope.
    pub fn changed_text(&self, source: &str) -> Result<Option<Self>> {
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

/// The boundary's refusal for one configuration path, in this crate's vocabulary.
///
/// Configuration is read before any resolved configuration exists, so it shares the base boundary
/// every other site read uses and translates the reason at the one place that knows both types.
fn source_refusal(
    root: &Path,
    path: &Path,
    scope: InputScope,
) -> Result<Option<ConfigSourceRefusal>> {
    let boundary =
        crate::resources::base_source_boundary(&crate::filesystem::normalize_path(root), scope);
    Ok(boundary
        .refusal(path)
        .map_err(|error| anyhow::anyhow!("{error}"))?
        .map(|refusal| match refusal {
            tola_typst::SourceRefusal::GeneratedState => ConfigSourceRefusal::GeneratedState,
            tola_typst::SourceRefusal::OutsideSite => ConfigSourceRefusal::OutsideSite,
        }))
}

/// Whether `field` names a top-level key the core schema reads: one of its sections.
fn is_core_key(field: &str) -> bool {
    super::SiteConfigSchema::SECTIONS.contains(&field)
}

/// Name an ignored configuration path in TOML key vocabulary.
///
/// `serde_ignored` marks optional wrappers and newtypes with `?`; the key that
/// names the offending field drops it.
fn config_field_name(path: &str) -> String {
    path.chars().filter(|character| *character != '?').collect()
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
    use crate::config::tests::attached_diagnostics;
    #[cfg(unix)]
    use crate::config::tests::{diagnostic_code, diagnostic_message};

    #[test]
    fn positions_answer_diagnostic_paths() {
        let directory = tempfile::tempdir().unwrap();
        let text = "[site]\ntitle = \"Site\"\n\n[[site.seo.feeds]]\nformat = \"rss\"\n\n[[site.seo.feeds]]\nurl = \"/atom.xml\"\n\n[build.hooks]\nafter-publish = { command = \"echo\" }\n";
        let source = ConfigSource::parse(
            &directory.path().join("tola.toml"),
            text,
            InputScope::Online,
        )
        .unwrap();
        let positions = source.positions();
        let at = |path: &str| {
            positions
                .excerpt(path)
                .map(|excerpt| (excerpt.line, excerpt.column))
        };

        assert_eq!(at("site.title"), Some((2, 1)));
        assert_eq!(at("site.seo.feeds.0.format"), Some((5, 1)));
        assert_eq!(at("site.seo.feeds.1.url"), Some((8, 1)));
        assert_eq!(at("build.hooks.after-publish"), Some((11, 1)));
        assert_eq!(at("site.seo.feeds.0.command"), None);
    }

    #[test]
    fn host_sections_share_the_core_document() {
        let directory = tempfile::tempdir().unwrap();
        let text = "[site]\ntitle = \"Site\"\n[server]\nport = 9000\n[host-editor]\nmode = \"custom\"\n[\"site.extension\"]\nenabled = true\n";
        let source = ConfigSource::parse(
            &directory.path().join("tola.toml"),
            text,
            InputScope::Online,
        )
        .unwrap();
        let core = resolve_parsed_site_config(
            source.decode_site().unwrap(),
            tola_typst::PackageLocations::default(),
            &BuildOverrides::default(),
        )
        .unwrap();
        assert_eq!(core.config().site().title, "Site");
        assert_eq!(core.source_hash(), blake3::hash(text.as_bytes()));
        assert!(source.presence().contains("host-editor.mode"));
    }

    #[test]
    fn host_sections_do_not_hide_core_typos() {
        let directory = tempfile::tempdir().unwrap();
        let source = ConfigSource::parse(
            &directory.path().join("tola.toml"),
            "[build]\nminfy = true\n[host]\nvalue = 1\n",
            InputScope::Online,
        )
        .unwrap();
        let error = source.decode_site().unwrap_err();
        assert!(error.to_string().contains("build.minfy"));
    }

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
            crate::filesystem::normalize_existing_prefix(&alias)
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

    #[test]
    fn host_sections_decode_beside_core() {
        #[derive(Default, serde::Deserialize)]
        #[serde(default)]
        struct HostConfig {
            #[serde(rename = "host-editor")]
            editor: std::collections::BTreeMap<String, toml::Value>,
        }

        let directory = tempfile::tempdir().unwrap();
        let source = ConfigSource::parse(
            &directory.path().join("tola.toml"),
            "[site]\ntitle = \"Site\"\n\n[host-editor]\nmode = \"custom\"\n",
            InputScope::Online,
        )
        .unwrap();

        let (core, host) = source.decode_with_host::<HostConfig>().unwrap();

        assert_eq!(core.schema().site.title, "Site");
        assert_eq!(
            host.schema().editor["mode"],
            toml::Value::String("custom".to_owned())
        );
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
    fn numeric_schema_errors_keep_key_and_value() {
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
            let diagnostic = &attached_diagnostics(&error)[0];
            assert_eq!(diagnostic.code, crate::codes::config::TOML);
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
            let diagnostic = &attached_diagnostics(&error)[0];
            assert_eq!(diagnostic.code, crate::codes::config::TOML);
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
        std::fs::create_dir(&site).unwrap();
        std::fs::write(
            outside.path().join("tola.toml"),
            "[site]\ntitle = \"Outside\"\n",
        )
        .unwrap();
        let escaping = site.join("tola.toml");
        std::os::unix::fs::symlink(outside.path().join("tola.toml"), &escaping).unwrap();

        let refused = ConfigSource::load(Some(&escaping), InputScope::Pure).unwrap_err();
        assert_eq!(diagnostic_code(&refused), Some(crate::codes::config::IO));
        assert!(
            diagnostic_message(&refused).contains("resolves outside the site"),
            "{}",
            diagnostic_message(&refused)
        );

        // The same file loads under the permissive scope, so the refusal follows the scope.
        assert!(ConfigSource::load(Some(&escaping), InputScope::Online).is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn pure_refuses_configuration_resolving_into_internal_state() {
        let directory = tempfile::tempdir().unwrap();
        let site = directory.path().join("site");
        let internal = site.join(crate::filesystem::INTERNAL_DIR);
        std::fs::create_dir_all(&internal).unwrap();
        std::fs::write(internal.join("tola.toml"), "[site]\ntitle = \"Cached\"\n").unwrap();
        // The supplied path's parent is the site root, so the `.tola` exclusion applies to the
        // physical target rather than to a root derived from the file itself.
        let alias = site.join("tola.toml");
        std::os::unix::fs::symlink(internal.join("tola.toml"), &alias).unwrap();

        let refused = ConfigSource::load(Some(&alias), InputScope::Pure).unwrap_err();
        assert!(
            diagnostic_message(&refused)
                .contains("the configuration file is Tola's own generated state"),
            "{}",
            diagnostic_message(&refused)
        );
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

        for text in [
            "[unknown]\nvalue = true\n",
            "[site]\ntitel = \"Misspelled\"\n",
            "[host-editor]\nmode = \"custom\"\nspeed = 2\n",
            // A root key this schema no longer declares, which earlier sites still hold.
            "version = \"0.8.0\"\n",
        ] {
            let source = ConfigSource::parse(
                &directory.path().join("tola.toml"),
                text,
                InputScope::Online,
            )
            .unwrap();
            let error = source.decode_with_host::<HostConfig>().unwrap_err();
            assert!(
                error.to_string().contains("unknown configuration fields"),
                "{text}: {error:#}"
            );
        }
    }
}
