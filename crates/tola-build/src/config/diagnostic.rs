//! Structured diagnostics for configuration loading and warnings.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;

use crate::config::{ConfigDiagnostic, ConfigDiagnosticTag, ConfigDiagnostics, ConfigError};
use tola_diagnostic::toml::{TomlMessage, UNKNOWN_FIELD_HELP, field_replacement};
use tola_diagnostic::{
    Diagnostic, DiagnosticCause, DiagnosticError, Location, RelatedInformation, Severity,
    SourceLine, SourcePositions, UnknownField, WrittenPosition, display_path,
};

/// The site root of a configuration file is the directory that contains it, so its
/// site-relative spelling is the file name.
fn configuration_root(path: &Path) -> &Path {
    path.parent().unwrap_or(path)
}

/// The site-root-relative spelling of a configuration file path.
fn config_display_path(path: &Path, root: Option<&Path>) -> String {
    display_path(path, root.unwrap_or_else(|| configuration_root(path)))
}

/// A TOML parser error in any file contributing to the site configuration.
pub(super) fn syntax_error_at_root(
    root: &Path,
    path: &Path,
    input: &str,
    reason: &str,
    span: Option<Range<usize>>,
) -> Diagnostic {
    document_error(display_path(path, root), input, reason, span).with_file(path.to_owned())
}

fn named_schema_message(key: Option<&str>, message: Cow<'_, str>) -> String {
    match key {
        Some(key)
            if message
                .strip_prefix('`')
                .and_then(|message| message.strip_prefix(key))
                .is_some_and(|suffix| suffix.starts_with('`')) =>
        {
            message.into_owned()
        }
        Some(key) => format!("`{key}`: {message}"),
        None => message.into_owned(),
    }
}

fn document_error(
    path: String,
    input: &str,
    reason: &str,
    span: Option<Range<usize>>,
) -> Diagnostic {
    let TomlMessage { message, help } = TomlMessage::from_error(reason);
    let mut diagnostic = Diagnostic::at_path(
        tola_diagnostic::codes::config::TOML,
        Severity::Error,
        path,
        message,
    );
    if let Some(help) = help {
        diagnostic = diagnostic.with_help(help);
    }
    if let Some(span) = span {
        diagnostic = diagnostic.with_source_span(input, span);
    }
    diagnostic
}

/// A rejected value or missing field in the effective configuration, located in a source file.
pub(super) fn value_error(
    path: &Path,
    positions: &SourcePositions,
    pointer: &str,
    field: &str,
    reason: &str,
) -> Diagnostic {
    let TomlMessage { message, help } = TomlMessage::from_error(reason);
    let message = named_schema_message((!field.is_empty()).then_some(field), message);
    let root = positions.root();
    let diagnostic = Diagnostic::at_path(
        tola_diagnostic::codes::config::TOML,
        Severity::Error,
        config_display_path(path, root),
        message,
    );
    let written = positions
        .at_pointer(pointer)
        .map(WrittenPosition::value)
        .or_else(|| {
            let mut pointer = pointer;
            while let Some((parent, _)) = pointer.rsplit_once('/') {
                if let Some(written) = positions.at_pointer(parent) {
                    return Some(written);
                }
                pointer = parent;
            }
            None
        });
    let mut diagnostic = attach_position(diagnostic, written, root);
    if let Some(help) = help {
        diagnostic = diagnostic.with_help(help);
    }
    diagnostic
}

pub(super) fn report_source_error(
    error: ConfigError,
    path: Option<&Path>,
    positions: Option<&SourcePositions>,
) -> DiagnosticError {
    let display =
        path.map(|path| config_display_path(path, positions.and_then(SourcePositions::root)));
    let diagnostics = error_diagnostics(&error, display.as_deref(), positions);
    DiagnosticError::attach(
        error.into(),
        at_config_path(diagnostics, display.as_deref()),
    )
}

pub(crate) fn attach_load(
    error: anyhow::Error,
    path: Option<&Path>,
    positions: Option<&SourcePositions>,
) -> anyhow::Error {
    attach(error, tola_diagnostic::codes::config::LOAD, path, positions)
}

/// The failure a configuration step reports the diagnostics it collected with.
pub(crate) fn config_error(diagnostics: ConfigDiagnostics) -> anyhow::Error {
    ConfigError::Diagnostics(diagnostics).into()
}

fn attach(
    error: anyhow::Error,
    fallback_code: tola_diagnostic::DiagnosticCode,
    path: Option<&Path>,
    positions: Option<&SourcePositions>,
) -> anyhow::Error {
    if tola_diagnostic::attached(&error).is_some() {
        return error;
    }
    let display =
        path.map(|path| config_display_path(path, positions.and_then(SourcePositions::root)));
    let config = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ConfigError>());
    let diagnostics = match config {
        Some(config) => error_diagnostics(config, display.as_deref(), positions),
        None => vec![tola_diagnostic::fallback(fallback_code, &error)],
    };
    let mut diagnostics = at_config_path(diagnostics, display.as_deref());
    if diagnostics.is_empty() {
        let diagnostic = tola_diagnostic::fallback(fallback_code, &error);
        diagnostics.push(match &display {
            Some(display) => diagnostic.with_path(&**display),
            None => diagnostic,
        });
    }
    anyhow::Error::new(DiagnosticError::attach(error, diagnostics))
}

fn at_config_path(diagnostics: Vec<Diagnostic>, display: Option<&str>) -> Vec<Diagnostic> {
    diagnostics
        .into_iter()
        .map(
            |diagnostic| match (diagnostic.location.is_none(), display) {
                (true, Some(display)) => diagnostic.with_path(display),
                _ => diagnostic,
            },
        )
        .collect()
}

/// Convert retained configuration warnings into the shared diagnostic protocol.
pub fn warning_diagnostics(config: &crate::config::ResolvedSiteConfig) -> Vec<Diagnostic> {
    let root = Some(config.get_root());
    let display = display_path(config.config_path(), config.get_root());
    let positions = config.positions();
    let (experimental, other): (Vec<_>, Vec<_>) = config
        .warnings()
        .iter()
        .partition(|warning| warning.tag() == Some(ConfigDiagnosticTag::Experimental));
    let mut diagnostics = Vec::new();
    if !experimental.is_empty() {
        let mut diagnostic = Diagnostic::at_path(
            tola_diagnostic::codes::config::EXPERIMENTAL,
            Severity::Warning,
            &display,
            "experimental configuration is enabled",
        );
        for warning in &experimental {
            diagnostic = diagnostic.with_note(warning.field.as_str());
        }
        diagnostic = attach_position(
            diagnostic,
            positions.and_then(|positions| written_position(positions, experimental[0])),
            root,
        );
        for warning in &experimental[1..] {
            let Some(written) =
                positions.and_then(|positions| written_position(positions, warning))
            else {
                continue;
            };
            let Some(path) = written_display(written, root) else {
                continue;
            };
            if diagnostic
                .location
                .as_ref()
                .is_some_and(|location| location.path == path)
            {
                continue;
            }
            diagnostic.related.push(RelatedInformation {
                message: warning.field.as_str().to_owned(),
                location: Some(written_location(path, written)),
                source: None,
            });
        }
        diagnostic = diagnostic.with_help("Remove these fields");
        diagnostics.push(diagnostic);
    }
    diagnostics.extend(other.into_iter().map(|warning| {
        let mut diagnostic = Diagnostic::at_path(
            tola_diagnostic::codes::config::WARNING,
            Severity::Warning,
            &display,
            &warning.message,
        )
        .with_note(warning.field.as_str());
        diagnostic = attach_position(
            diagnostic,
            positions.and_then(|positions| written_position(positions, warning)),
            root,
        );
        if let Some(help) = &warning.help {
            diagnostic = diagnostic.with_help(help);
        }
        diagnostic
    }));
    diagnostics
}

/// One configuration error, located at the configuration file when its path is known.
fn configuration_error(display: Option<&str>, message: &str) -> Diagnostic {
    match display {
        Some(display) => Diagnostic::at_path(
            tola_diagnostic::codes::config::INVALID,
            Severity::Error,
            display,
            message,
        ),
        None => Diagnostic::new(
            tola_diagnostic::codes::config::INVALID,
            Severity::Error,
            message,
        ),
    }
}

fn error_diagnostics(
    error: &ConfigError,
    display: Option<&str>,
    positions: Option<&SourcePositions>,
) -> Vec<Diagnostic> {
    let root = positions.and_then(SourcePositions::root);
    match error {
        ConfigError::Diagnostics(diagnostics) => diagnostics
            .errors()
            .iter()
            .map(|config_diagnostic| validation_diagnostic(config_diagnostic, display, positions))
            .collect(),
        ConfigError::Toml {
            path,
            input,
            source,
        } => vec![syntax_error_at_root(
            root.unwrap_or_else(|| configuration_root(path)),
            path,
            input,
            source.message(),
            source.span(),
        )],
        ConfigError::Io(path, source) => {
            let message = match source.kind() {
                std::io::ErrorKind::NotFound => "the configuration file does not exist",
                std::io::ErrorKind::PermissionDenied => "the configuration file is not readable",
                _ => "Tola could not read the configuration file",
            };
            vec![Diagnostic::at_path(
                tola_diagnostic::codes::config::IO,
                Severity::Error,
                config_display_path(path, root),
                message,
            )]
        }
        ConfigError::Refused { path, reason } => {
            let (message, help) = match reason {
                crate::config::ConfigSourceRefusal::GeneratedState => (
                    "the configuration file is Tola's own generated state",
                    "Move it outside `.tola`, or point `--config` at the site's own `tola.toml`",
                ),
                crate::config::ConfigSourceRefusal::OutsideSite => (
                    "the configuration file resolves outside the site",
                    "Replace it with a file inside the site, or with a link that stays inside",
                ),
            };
            vec![
                Diagnostic::at_path(
                    tola_diagnostic::codes::config::IO,
                    Severity::Error,
                    config_display_path(path, root),
                    message,
                )
                .with_help(help),
            ]
        }
        ConfigError::UnknownFields { fields } => unknown_field_diagnostics(
            display,
            root,
            fields.iter().map(|field| {
                (
                    field.as_str(),
                    positions.and_then(|positions| positions.at(field)),
                )
            }),
        ),
        ConfigError::PathNotAFile { .. } => {
            vec![Diagnostic::new(
                tola_diagnostic::codes::config::INVALID,
                Severity::Error,
                error.to_string(),
            )]
        }
    }
}

/// Unknown fields with their display name and exact pointer, grouped by original file.
pub(super) fn unknown_fields(
    path: &Path,
    positions: Option<&SourcePositions>,
    fields: &[(String, String)],
) -> Vec<Diagnostic> {
    let root = positions.and_then(SourcePositions::root);
    let display = config_display_path(path, root);
    unknown_field_diagnostics(
        Some(&display),
        root,
        fields.iter().map(|(field, pointer)| {
            (
                field.as_str(),
                positions.and_then(|positions| positions.at_pointer(pointer)),
            )
        }),
    )
}

fn unknown_field_diagnostics<'a>(
    display: Option<&str>,
    root: Option<&Path>,
    fields: impl Iterator<Item = (&'a str, Option<WrittenPosition<'a>>)>,
) -> Vec<Diagnostic> {
    let mut by_file = BTreeMap::<Option<String>, Vec<_>>::new();
    for (field, written) in fields {
        let path = written
            .and_then(|written| written_display(written, root))
            .or_else(|| display.map(str::to_owned));
        by_file.entry(path).or_default().push((field, written));
    }
    by_file
        .into_iter()
        .map(|(path, fields)| {
            let names = fields
                .iter()
                .map(|(name, _)| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ");
            let message = if fields.len() == 1 {
                format!("unknown configuration field {names}")
            } else {
                format!("unknown configuration fields {names}")
            };
            let mut diagnostic = attach_position(
                configuration_error(path.as_deref(), &message),
                fields[0].1,
                root,
            );
            for (name, _) in &fields {
                if let Some(key) =
                    field_replacement(name, super::field_paths().map(|field| field.as_str()))
                {
                    let replacement = name
                        .rsplit_once('.')
                        .map_or_else(|| key.to_owned(), |(table, _)| format!("{table}.{key}"));
                    diagnostic = diagnostic.with_help(format!(
                        "Check whether you meant `{replacement}` instead of `{name}`"
                    ));
                }
            }
            if diagnostic.help.is_empty() {
                diagnostic = diagnostic.with_help(UNKNOWN_FIELD_HELP);
            }
            let fields = fields
                .into_iter()
                .map(|(name, written)| {
                    let range = written.map(|written| written.range());
                    UnknownField {
                        name: name.to_owned(),
                        line: range.map(|range| range.start.line + 1),
                        written: range,
                    }
                })
                .collect();
            diagnostic.with_cause(DiagnosticCause::UnknownConfigurationFields { fields })
        })
        .collect()
}

/// One validation failure, pointed at the key it names when the source still holds it.
fn validation_diagnostic(
    config_diagnostic: &ConfigDiagnostic,
    display: Option<&str>,
    positions: Option<&SourcePositions>,
) -> Diagnostic {
    let mut diagnostic = attach_position(
        configuration_error(display, &config_diagnostic.message),
        positions.and_then(|positions| written_position(positions, config_diagnostic)),
        positions.and_then(SourcePositions::root),
    )
    .with_note(config_diagnostic.field.as_str());
    if let Some(help) = &config_diagnostic.help {
        diagnostic = diagnostic.with_help(help);
    }
    diagnostic
}

fn attach_position(
    mut diagnostic: Diagnostic,
    position: Option<WrittenPosition<'_>>,
    root: Option<&Path>,
) -> Diagnostic {
    if let Some(position) = position {
        let path = written_display(position, root)
            .or_else(|| diagnostic.location.take().map(|location| location.path));
        if let Some(path) = path {
            diagnostic.location = Some(written_location(path, position));
        }
    }
    diagnostic
}

fn written_display(position: WrittenPosition<'_>, root: Option<&Path>) -> Option<String> {
    Some(config_display_path(position.file()?, root))
}

fn written_location(path: String, position: WrittenPosition<'_>) -> Location {
    let excerpt = position.excerpt();
    Location {
        path,
        file: position.file().map(Path::to_owned),
        line: excerpt.as_ref().map(|excerpt| excerpt.line),
        column: excerpt.as_ref().map(|excerpt| excerpt.column),
        range: Some(position.range()),
        source_lines: excerpt
            .into_iter()
            .map(|excerpt| SourceLine::new(excerpt.line, excerpt.text, Some(excerpt.highlight)))
            .collect(),
    }
}

/// The narrowest written key a configuration diagnostic names.
///
/// A diagnostic raised inside an array-table entry asks for the field within that entry first and
/// the entry's own key next, so a field its author left out still points at the entry to edit
/// rather than at the first entry of the array.
fn written_position<'a>(
    positions: &'a SourcePositions,
    config_diagnostic: &ConfigDiagnostic,
) -> Option<WrittenPosition<'a>> {
    config_diagnostic
        .key_paths()
        .find_map(|key| positions.at(key.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tola_config::FieldPath;

    #[test]
    fn warning_points_at_the_writing_line() {
        let directory = tempfile::TempDir::new().unwrap();
        let mut config =
            crate::config::tests::load_test_config(directory.path(), "[site]\ntitle = \"Site\"\n");
        config.warnings.push(ConfigDiagnostic::warning(
            FieldPath::new("site.title"),
            "check this",
        ));

        let diagnostics = warning_diagnostics(&config);

        let location = diagnostics[0].location.as_ref().unwrap();
        assert_eq!(location.path, "tola.toml");
        assert_eq!((location.line, location.column), (Some(2), Some(1)));
    }

    #[test]
    fn syntax_error_points_at_supplied_text() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        let error = crate::config::ConfigSource::parse(
            &path,
            "[build]\nentry = @\n",
            tola_input::InputScope::Online,
        )
        .unwrap_err();

        let diagnostic = &error.diagnostics()[0];
        assert_eq!(diagnostic.code, tola_diagnostic::codes::config::TOML);
        let location = diagnostic.location.as_ref().unwrap();
        assert_eq!(location.path, "tola.toml");
        assert_eq!(location.line, Some(2));
    }

    #[test]
    fn unknown_field_corrections_stay_inside_their_file() {
        let root = Path::new("site");
        let entry = root.join("tola.toml");
        let mut positions = SourcePositions::in_root(root.to_owned());
        let fields = [
            ("enabled".to_owned(), "/enabled".to_owned()),
            ("limit".to_owned(), "/limit".to_owned()),
        ];
        for (file, text) in [
            (&entry, "enabled = true\n"),
            (&root.join("config/common.toml"), "\nlimit = 3\n"),
        ] {
            let text = Arc::<str>::from(text);
            let document = toml_edit::Document::parse(Arc::clone(&text)).unwrap();
            positions.copy_subtree(&SourcePositions::of(&document, text), "", "", file);
        }

        let diagnostics = unknown_fields(&entry, Some(&positions), &fields);
        assert_eq!(diagnostics.len(), 2);
        for (file, name, line) in [
            ("config/common.toml", "limit", 2),
            ("tola.toml", "enabled", 1),
        ] {
            let diagnostic = diagnostics
                .iter()
                .find(|diagnostic| diagnostic.location.as_ref().unwrap().path == file)
                .unwrap();
            assert_eq!(diagnostic.location.as_ref().unwrap().line, Some(line));
            let Some(DiagnosticCause::UnknownConfigurationFields { fields }) = &diagnostic.cause
            else {
                panic!("expected unknown field corrections for {file}");
            };
            assert_eq!(fields.len(), 1);
            assert_eq!(
                (fields[0].name.as_str(), fields[0].line),
                (name, Some(line))
            );
        }
    }
}
