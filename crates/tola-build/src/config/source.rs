//! A configuration snapshot, its include applications, and the authored origins of effective values.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::de::{DeserializeOwned, IntoDeserializer};
use serde::{Deserialize, Serialize};
use tola_config::{FragmentContext, FragmentIssueKind, ValidateFragment, ValueStep};
use tola_diagnostic::{
    Diagnostic, DiagnosticError, RelatedInformation, SourcePositions, WrittenPosition,
};
use tola_input::InputScope;

use super::{ConfigPresence, ConfigSourceRefusal};
use crate::cancellation::{BuildCancellation, BuildCancelled};

mod load;
mod merge;
pub use load::{ConfigApplication, ConfigFile, ConfigFileState, ConfigInclude};

/// The original declaration that contributed an effective value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigOrigin {
    pub source_id: usize,
    pub key_path: Vec<ValueStep>,
}

/// Current contributions and declarations they replaced.
#[derive(Debug, Clone, Default)]
pub struct ConfigFieldOrigins {
    pub origins: Vec<ConfigOrigin>,
    pub overridden: Vec<ConfigOrigin>,
}

/// Why a configuration could not be selected, read, or decoded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigSourceError {
    #[error("{diagnostic}")]
    NotFound {
        start: PathBuf,
        #[source]
        diagnostic: DiagnosticError,
    },
    #[error("{0}")]
    Path(#[source] DiagnosticError),
    #[error("{0}")]
    Read(#[source] DiagnosticError),
    #[error("{0}")]
    Syntax(#[source] DiagnosticError),
    #[error("{0}")]
    Schema(#[source] DiagnosticError),
    #[error("{0}")]
    SourcePath(#[from] typst::diag::FileError),
    #[error("{0}")]
    Resolution(#[source] anyhow::Error),
    #[error(transparent)]
    Cancelled(#[from] BuildCancelled),
    /// Failed loads retain dependencies so the host can observe a repair.
    #[error("{source}")]
    Inputs {
        #[source]
        source: Box<ConfigSourceError>,
        paths: Vec<PathBuf>,
    },
}

impl ConfigSourceError {
    pub fn diagnostics(&self) -> &[Diagnostic] {
        match self {
            Self::NotFound { diagnostic, .. }
            | Self::Path(diagnostic)
            | Self::Read(diagnostic)
            | Self::Syntax(diagnostic)
            | Self::Schema(diagnostic) => diagnostic.diagnostics(),
            Self::Inputs { source, .. } => source.diagnostics(),
            Self::Resolution(error) => tola_diagnostic::attached(error).unwrap_or_default(),
            Self::SourcePath(_) | Self::Cancelled(_) => &[],
        }
    }

    /// Include sources discovered before this failure, including missing targets.
    pub fn paths(&self) -> &[PathBuf] {
        match self {
            Self::Inputs { paths, .. } => paths,
            _ => &[],
        }
    }

    pub(super) fn with_paths(self, mut paths: Vec<PathBuf>) -> Self {
        paths.extend_from_slice(self.paths());
        paths.sort();
        paths.dedup();
        match self {
            Self::Inputs { source, .. } => Self::Inputs { source, paths },
            source => Self::Inputs {
                source: Box::new(source),
                paths,
            },
        }
    }
}

/// One immutable configuration graph bound to the primary file's site root.
///
/// Raw values are combined before defaults or schema construction. Source documents
/// remain separate so diagnostics and source freshness never use synthetic TOML bytes.
#[derive(Debug, Clone)]
pub struct ConfigSource {
    path: PathBuf,
    root: PathBuf,
    scope: InputScope,
    graph: Arc<load::Loaded>,
    presence: Arc<ConfigPresence>,
    core_presence: Arc<ConfigPresence>,
    overlays: Arc<[(PathBuf, Arc<str>)]>,
    defaults: bool,
}

impl ConfigSource {
    pub fn parse(path: &Path, source: &str, scope: InputScope) -> Result<Self, ConfigSourceError> {
        Self::parse_with_sources(path, source, scope, &[], None)
    }

    /// Parse an authoritative primary text with the other editor or initializer overlays.
    pub fn parse_with_sources(
        path: &Path,
        source: &str,
        scope: InputScope,
        overlays: &[(PathBuf, Arc<str>)],
        cancellation: Option<&BuildCancellation>,
    ) -> Result<Self, ConfigSourceError> {
        let (path, root) = super::loading::resolve_config_location(path)?;
        let primary = tola_input::lexical_path_identity(&path);
        let mut texts = overlays
            .iter()
            .filter(|(candidate, _)| {
                std::path::absolute(candidate).ok().is_none_or(|candidate| {
                    tola_input::lexical_path_identity(&candidate) != primary
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        texts.push((path.clone(), Arc::from(source)));
        Self::load_at_root(path, root, scope, &texts, cancellation)
    }

    pub fn load(explicit: Option<&Path>, scope: InputScope) -> Result<Self, ConfigSourceError> {
        Self::load_with_sources(explicit, scope, &[], None)
    }

    /// Read every referenced document through one overlay and cancellation context.
    pub fn load_with_sources(
        explicit: Option<&Path>,
        scope: InputScope,
        overlays: &[(PathBuf, Arc<str>)],
        cancellation: Option<&BuildCancellation>,
    ) -> Result<Self, ConfigSourceError> {
        if let Some(cancellation) = cancellation {
            cancellation.ensure_active()?;
        }
        let (path, root) = super::loading::select_config_location(explicit)?;
        Self::load_at_root(path, root, scope, overlays, cancellation)
    }

    fn load_at_root(
        path: PathBuf,
        root: PathBuf,
        scope: InputScope,
        overlays: &[(PathBuf, Arc<str>)],
        cancellation: Option<&BuildCancellation>,
    ) -> Result<Self, ConfigSourceError> {
        let graph = load::load(path.clone(), root.clone(), scope, overlays, cancellation)?;
        let presence = Arc::new(ConfigPresence::from_value(&graph.value));
        let core = toml::Value::Table(
            graph
                .value
                .as_table()
                .expect("TOML root")
                .iter()
                .filter(|(key, _)| is_core_key(key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        Ok(Self {
            path,
            root,
            scope,
            graph: Arc::new(graph),
            presence,
            core_presence: Arc::new(ConfigPresence::from_value(&core)),
            overlays: overlays.to_vec().into(),
            defaults: false,
        })
    }

    /// A workspace without a primary file uses virtual empty input, not a failed disk read.
    pub fn defaults(root: &Path, scope: InputScope) -> Result<Self, ConfigSourceError> {
        let root = tola_input::normalize_path(root);
        let path = root.join(super::loading::CONFIG_FILE_NAME);
        let mut source =
            Self::load_at_root(path.clone(), root, scope, &[(path, Arc::from(""))], None)?;
        source.defaults = true;
        Ok(source)
    }

    pub fn decode<T: DeserializeOwned + ValidateFragment>(
        &self,
    ) -> Result<ParsedConfig<T>, ConfigSourceError> {
        self.decode_reporting_unknown_fields(|_| true)
    }

    pub fn formatter(&self) -> Result<super::section::FormatterConfig, ConfigSourceError> {
        use super::section::FormatterConfig;
        #[derive(Deserialize, tola_config::Fragment)]
        struct FormatterSection {
            #[serde(default)]
            formatter: FormatterConfig,
        }
        self.decode_reporting_unknown_fields::<FormatterSection>(|root| {
            root == FormatterConfig::TEMPLATE_SECTION
        })
        .map(|parsed| parsed.schema.formatter)
    }

    pub fn decode_with_host<T: DeserializeOwned + ValidateFragment>(
        &self,
    ) -> Result<(ParsedConfig<super::SiteConfigSchema>, ParsedConfig<T>), ConfigSourceError> {
        Ok((
            self.decode_site()?,
            self.decode_reporting_unknown_fields(|key| !is_core_key(key))?,
        ))
    }

    pub(super) fn decode_site(
        &self,
    ) -> Result<ParsedConfig<super::SiteConfigSchema>, ConfigSourceError> {
        self.decode_reporting_unknown_fields(is_core_key)
    }

    fn validate_fragments<T: ValidateFragment>(
        &self,
        report_unknown: &dyn Fn(&str) -> bool,
    ) -> Result<(), ConfigSourceError> {
        let mut diagnostics = Vec::new();
        for (file_index, file) in self.graph.files.iter().enumerate() {
            let mut issues = Vec::new();
            T::validate_fragment(
                file.value(),
                &mut FragmentContext::new(&mut issues, Some(report_unknown)),
            );
            let mut unknown = Vec::new();
            for issue in issues {
                let key = field_name(&issue.path);
                let pointer = pointer(&issue.path);
                let reason = match issue.kind {
                    FragmentIssueKind::UnknownKey => {
                        unknown.push((key, pointer));
                        continue;
                    }
                    FragmentIssueKind::WrongType { expected, actual } => {
                        format!("expected {expected}; found {actual}")
                    }
                    FragmentIssueKind::InvalidValue { message } => message,
                };
                let mut diagnostic = super::diagnostic::value_error(
                    &self.path,
                    file.positions(),
                    &pointer,
                    &key,
                    &reason,
                );
                if let Some(application) = self
                    .graph
                    .application_files
                    .iter()
                    .position(|source| *source == Some(file_index))
                {
                    self.include_chain(&mut diagnostic, application);
                }
                diagnostics.push(diagnostic);
            }
            diagnostics.extend(super::diagnostic::unknown_fields(
                &self.path,
                Some(file.positions()),
                &unknown,
            ));
            if !diagnostics.is_empty() {
                break;
            }
        }
        if diagnostics.is_empty() {
            return Ok(());
        }
        let error = DiagnosticError::new(diagnostics[0].message.clone(), diagnostics);
        Err(ConfigSourceError::Schema(error).with_paths(self.graph.paths.clone()))
    }

    fn decode_reporting_unknown_fields<T: DeserializeOwned + ValidateFragment>(
        &self,
        report_unknown: impl Fn(&str) -> bool,
    ) -> Result<ParsedConfig<T>, ConfigSourceError> {
        self.validate_fragments::<T>(&report_unknown)?;
        // Keep TOML types when entering Serde. toml::Value's own deserializer
        // exposes a date as a string, which would change the accepted schema.
        let value = self
            .graph
            .value
            .serialize(toml_edit::ser::ValueSerializer::new())
            .map_err(|error| self.schema_failure(&[], error.to_string(), error.into()))?;
        let mut track = serde_path_to_error::Track::new();
        let mut ignored = Vec::new();
        let schema = serde_ignored::deserialize(
            serde_path_to_error::Deserializer::new(value.into_deserializer(), &mut track),
            |path| {
                let mut steps = Vec::new();
                ignored_steps(&path, &mut steps);
                if steps.first().is_none_or(|step| match step {
                    ValueStep::Key(key) => report_unknown(key),
                    ValueStep::Index(_) => true,
                }) {
                    ignored.push((field_name(&steps), pointer(&steps)));
                }
            },
        )
        .map_err(|error: toml_edit::de::Error| {
            let steps = track
                .path()
                .iter()
                .filter_map(|step| match step {
                    serde_path_to_error::Segment::Map { key } => Some(ValueStep::Key(key.clone())),
                    serde_path_to_error::Segment::Seq { index } => Some(ValueStep::Index(*index)),
                    serde_path_to_error::Segment::Enum { variant } => {
                        Some(ValueStep::Key(variant.clone()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            self.schema_failure(&steps, error.message().to_owned(), error.into())
        })?;
        if !ignored.is_empty() {
            return Err(ConfigSourceError::Schema(DiagnosticError::new(
                "configuration contains unknown fields",
                super::diagnostic::unknown_fields(&self.path, Some(self.positions()), &ignored),
            ))
            .with_paths(self.graph.paths.clone()));
        }
        Ok(ParsedConfig {
            schema,
            source: self.clone(),
        })
    }

    fn schema_failure(
        &self,
        steps: &[ValueStep],
        reason: String,
        error: anyhow::Error,
    ) -> ConfigSourceError {
        let mut diagnostic = super::diagnostic::value_error(
            &self.path,
            self.positions(),
            &pointer(steps),
            &field_name(steps),
            &reason,
        );
        let mut field = steps;
        let origins = loop {
            if let Some(origins) = self.graph.origins.get(field) {
                break Some(origins);
            }
            if field.is_empty() {
                break None;
            }
            field = &field[..field.len() - 1];
        };
        if let Some(origins) = origins {
            let mut locations = Vec::new();
            for origin in &origins.origins {
                if let Some(position) = self.origin_positions(origin) {
                    let file = &self.graph.applications[origin.source_id].path;
                    let mut location = tola_diagnostic::Location {
                        file: Some(file.to_owned()),
                        path: tola_diagnostic::display_path(
                            file,
                            self.path.parent().unwrap_or(&self.root),
                        ),
                        line: None,
                        column: None,
                        range: None,
                        source_lines: Vec::new(),
                    };
                    if let Some(excerpt) = position.excerpt() {
                        location.line = Some(excerpt.line);
                        location.column = Some(excerpt.column);
                    }
                    location.range = Some(position.range());
                    if diagnostic.location.as_ref() != Some(&location)
                        && !locations.contains(&location)
                    {
                        locations.push(location);
                    }
                }
            }
            diagnostic
                .related
                .extend(locations.into_iter().map(|location| RelatedInformation {
                    message: "Contributing configuration declaration".into(),
                    location: Some(location),
                    source: None,
                }));
            if let Some(origin) = origins.origins.first() {
                self.include_chain(&mut diagnostic, origin.source_id);
            }
        }
        ConfigSourceError::Schema(DiagnosticError::attach(error, vec![diagnostic]))
            .with_paths(self.graph.paths.clone())
    }

    fn include_chain(&self, diagnostic: &mut Diagnostic, mut id: usize) {
        while let Some(include) = self
            .graph
            .applications
            .get(id)
            .and_then(|app| app.included_by.as_ref())
        {
            let parent = &self.graph.applications[include.source_id];
            let mut location = tola_diagnostic::Location {
                file: Some(parent.path.clone()),
                path: tola_diagnostic::display_path(
                    &parent.path,
                    self.path.parent().unwrap_or(&self.root),
                ),
                line: None,
                column: None,
                range: None,
                source_lines: Vec::new(),
            };
            location.line = Some(include.line);
            location.column = Some(include.column);
            location.range = include.range;
            diagnostic.related.push(RelatedInformation {
                message: "Included from here".into(),
                location: Some(location),
                source: None,
            });
            id = include.source_id;
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn source_hash(&self) -> blake3::Hash {
        self.graph.hash
    }
    pub fn sources(&self) -> &[ConfigFile] {
        &self.graph.files
    }
    pub fn applications(&self) -> &[ConfigApplication] {
        &self.graph.applications
    }
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.graph.paths.iter().map(PathBuf::as_path)
    }
    pub fn origins(&self) -> &BTreeMap<Vec<ValueStep>, ConfigFieldOrigins> {
        &self.graph.origins
    }
    pub fn origin_positions(&self, origin: &ConfigOrigin) -> Option<WrittenPosition<'_>> {
        let index = self
            .graph
            .application_files
            .get(origin.source_id)
            .copied()
            .flatten()?;
        self.graph.files[index]
            .positions()
            .at_pointer(&pointer(&origin.key_path))
    }
    pub fn presence(&self) -> &ConfigPresence {
        &self.presence
    }
    pub(super) fn core_presence(&self) -> &ConfigPresence {
        &self.core_presence
    }
    pub(super) fn positions(&self) -> &Arc<SourcePositions> {
        &self.graph.positions
    }
    pub(super) fn attach_load(&self, error: anyhow::Error) -> anyhow::Error {
        ConfigSourceError::Resolution(super::diagnostic::attach_load(
            error,
            Some(&self.path),
            Some(self.positions()),
        ))
        .with_paths(self.graph.paths.clone())
        .into()
    }

    /// Re-read all include inputs; changing only a fragment still makes a candidate.
    pub fn read_changed(&self) -> Result<Option<Self>, ConfigSourceError> {
        self.changed_sources(&[], None)
    }

    pub fn changed_sources(
        &self,
        overlays: &[(PathBuf, Arc<str>)],
        cancellation: Option<&BuildCancellation>,
    ) -> Result<Option<Self>, ConfigSourceError> {
        let next = Self::load_at_root(
            self.path.clone(),
            self.root.clone(),
            self.scope,
            overlays,
            cancellation,
        )?;
        Ok((next.source_hash() != self.source_hash()).then_some(next))
    }

    pub fn changed_text(&self, source: &str) -> Result<Option<Self>, ConfigSourceError> {
        let overlays = [(self.path.clone(), Arc::from(source))];
        self.changed_sources(&overlays, None)
    }

    /// Revalidate the same disk/overlay view that selected this configuration.
    pub fn is_current(
        &self,
        cancellation: Option<&BuildCancellation>,
    ) -> Result<bool, ConfigSourceError> {
        if let Some(cancellation) = cancellation {
            cancellation.ensure_active()?;
        }
        if self.defaults {
            return match std::fs::symlink_metadata(&self.path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
                Ok(_) => Ok(false),
                Err(error) => Err(ConfigSourceError::Read(DiagnosticError::new(
                    "Tola could not check the configuration source",
                    vec![Diagnostic::at_path(
                        tola_diagnostic::codes::config::IO,
                        tola_diagnostic::Severity::Error,
                        tola_diagnostic::display_path(&self.path, &self.root),
                        error.to_string(),
                    )],
                ))
                .with_paths(self.graph.paths.clone())),
            };
        }
        Self::load_at_root(
            self.path.clone(),
            self.root.clone(),
            self.scope,
            &self.overlays,
            cancellation,
        )
        .map(|current| current.source_hash() == self.source_hash())
    }
}

pub(super) fn check_source_path(
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
            super::ConfigError::Refused {
                path: path.to_owned(),
                reason,
            },
            Some(path),
            positions,
        ),
    ))
}

fn is_core_key(field: &str) -> bool {
    super::SiteConfigSchema::SECTIONS.contains(&field)
}

pub(super) fn pointer(path: &[ValueStep]) -> String {
    let mut pointer = String::new();
    for step in path {
        match step {
            ValueStep::Key(key) => tola_diagnostic::push_pointer(&mut pointer, key),
            ValueStep::Index(index) => {
                tola_diagnostic::push_pointer(&mut pointer, &index.to_string())
            }
        }
    }
    pointer
}

fn field_name(path: &[ValueStep]) -> String {
    let mut result = String::new();
    for step in path {
        if !result.is_empty() {
            result.push('.');
        }
        match step {
            ValueStep::Key(key) => result.push_str(&toml_edit::Key::new(key.as_str()).to_string()),
            ValueStep::Index(index) => result.push_str(&index.to_string()),
        }
    }
    result
}

fn ignored_steps(path: &serde_ignored::Path<'_>, steps: &mut Vec<ValueStep>) {
    use serde_ignored::Path;
    match path {
        Path::Root => {}
        Path::Map { parent, key } => {
            ignored_steps(parent, steps);
            steps.push(ValueStep::Key(key.clone()));
        }
        Path::Seq { parent, index } => {
            ignored_steps(parent, steps);
            steps.push(ValueStep::Index(*index));
        }
        Path::Some { parent }
        | Path::NewtypeStruct { parent }
        | Path::NewtypeVariant { parent } => ignored_steps(parent, steps),
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
mod tests;
