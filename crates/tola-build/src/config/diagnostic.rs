//! Structured diagnostics for configuration loading and warnings.

use std::borrow::Cow;
use std::ops::Range;
use std::path::Path;

use crate::config::source::{ConfigPositions, SourceExcerpt, source_excerpt};
use crate::config::{ConfigDiagnostic, ConfigDiagnosticTag, ConfigError};
use crate::diagnostic::{Diagnostic, DiagnosticCause, DiagnosticError, Severity, UnknownField};

/// The site root of a configuration file is the directory that contains it, so its
/// site-relative spelling is the file name.
fn configuration_root(path: &Path) -> &Path {
    path.parent().unwrap_or(path)
}

/// The site-root-relative spelling of a configuration file path.
fn config_display_path(path: &Path) -> String {
    crate::filesystem::display_path(path, configuration_root(path))
}

/// Help for an unknown configuration key, which is either a misspelling or a key a newer Tola
/// release wrote.
pub(crate) const UNKNOWN_FIELD_HELP: &str = "Correct the spelling, or remove the field; if a newer Tola wrote the file, update the executable to read it";

/// A configuration file the parser stopped at.
///
/// The message is the parser's own reason and the location is the span it stopped at, so the
/// author reads what is wrong with their TOML rather than that it is wrong somewhere.
pub(crate) fn syntax_error(
    path: &Path,
    input: &str,
    reason: &str,
    span: Option<Range<usize>>,
) -> Diagnostic {
    document_error(path, input, reason.to_owned(), span)
}

/// A value in a valid configuration file that the configuration schema rejects.
pub(crate) fn schema_error(
    path: &Path,
    document: &toml_edit::Document<std::sync::Arc<str>>,
    message: &str,
    span: Option<Range<usize>>,
) -> Diagnostic {
    let SchemaMessage { message, help } = schema_message(message);
    let key = span
        .as_ref()
        .and_then(|span| crate::config::source::key_at_span(document, span));
    let message = match key {
        Some(key)
            if message
                .strip_prefix('`')
                .and_then(|message| message.strip_prefix(key.as_str()))
                .is_some_and(|suffix| suffix.starts_with('`')) =>
        {
            message.into_owned()
        }
        Some(key) => format!("`{key}`: {message}"),
        None => message.into_owned(),
    };
    let mut diagnostic = document_error(path, document.raw(), message, span);
    if let Some(help) = help {
        diagnostic = diagnostic.with_help(help);
    }
    diagnostic
}

fn document_error(
    path: &Path,
    input: &str,
    message: String,
    span: Option<Range<usize>>,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::at_path(
        crate::codes::config::TOML,
        Severity::Error,
        config_display_path(path),
        message,
    );
    if let Some(span) = span
        && let Some(excerpt) = source_excerpt(input, span)
    {
        diagnostic = diagnostic.with_source_excerpt(
            excerpt.line,
            excerpt.column,
            excerpt.text,
            Some(excerpt.highlight),
        );
    }
    diagnostic
}

/// One TOML deserialization failure rewritten into configuration language.
///
/// Only the unknown-field rewrite has help: the key may come from a newer Tola release
/// rather than from a mistake in the file.
struct SchemaMessage<'a> {
    message: Cow<'a, str>,
    help: Option<&'static str>,
}

impl<'a> SchemaMessage<'a> {
    fn new(message: Cow<'a, str>) -> Self {
        Self {
            message,
            help: None,
        }
    }

    fn with_help(mut self, help: &'static str) -> Self {
        self.help = Some(help);
        self
    }
}

/// Rewrite one TOML deserialization failure into configuration language.
fn schema_message(message: &str) -> SchemaMessage<'_> {
    if let Some(rest) = message
        .strip_prefix("invalid type: ")
        .or_else(|| message.strip_prefix("invalid value: "))
        && let Some((found, expected)) = rest.split_once(", expected ")
    {
        return SchemaMessage::new(Cow::Owned(format!(
            "expected {}; found {}",
            accepted_form(expected),
            written_value(found)
        )));
    }
    if let Some(rest) = message.strip_prefix("unknown variant `")
        && let Some((variant, accepted)) = rest.split_once("`, expected ")
    {
        return SchemaMessage::new(Cow::Owned(format!(
            "`{variant}` is not a supported value; use {accepted}"
        )));
    }
    if let Some(rest) = message.strip_prefix("unknown field `")
        && let Some((field, accepted)) = rest.split_once("`, expected ")
    {
        return SchemaMessage::new(Cow::Owned(format!(
            "unknown field `{field}`; use {accepted}"
        )))
        .with_help(UNKNOWN_FIELD_HELP);
    }
    if let Some(field) = message.strip_prefix("missing field `")
        && let Some(field) = field.strip_suffix('`')
    {
        return SchemaMessage::new(Cow::Owned(format!(
            "the required field `{field}` is missing"
        )));
    }
    if let Some(rest) = message.strip_prefix("unexpected keys in table: ")
        && let Some((unknown, allowed)) = rest.split_once(", available keys: ")
    {
        return SchemaMessage::new(Cow::Owned(format!(
            "unknown field: {unknown}; allowed fields are {allowed}"
        )));
    }
    if let Some(found) = message.strip_prefix("expected table, found ") {
        return SchemaMessage::new(Cow::Owned(format!(
            "expected a table; found {}",
            written_value(found)
        )));
    }
    if message.starts_with("data did not match any variant of untagged enum ") {
        return SchemaMessage::new(Cow::Borrowed(
            "this value does not match any supported form",
        ));
    }
    if message == "expected table with exactly 1 entry, found empty table" {
        return SchemaMessage::new(Cow::Borrowed("this table must contain exactly one entry"));
    }
    if let Some(rest) = message.strip_prefix("expected table key `")
        && let Some((key, found)) = rest.split_once("`, but was `")
    {
        return SchemaMessage::new(Cow::Owned(format!(
            "expected the table key `{key}`; found `{}`",
            found.trim_end_matches('`')
        )));
    }
    SchemaMessage::new(Cow::Borrowed(message))
}

/// The TOML form one serde expectation asks for.
fn accepted_form(expected: &str) -> Cow<'_, str> {
    match expected {
        "u8" => Cow::Owned(format!("an integer from {} to {}", u8::MIN, u8::MAX)),
        "u16" => Cow::Owned(format!("an integer from {} to {}", u16::MIN, u16::MAX)),
        "u32" => Cow::Owned(format!("an integer from {} to {}", u32::MIN, u32::MAX)),
        "u64" => Cow::Owned(format!("an integer from {} to {}", u64::MIN, u64::MAX)),
        "u128" => Cow::Owned(format!("an integer from {} to {}", u128::MIN, u128::MAX)),
        "usize" => Cow::Owned(format!("an integer from {} to {}", usize::MIN, usize::MAX)),
        "i8" => Cow::Owned(format!("an integer from {} to {}", i8::MIN, i8::MAX)),
        "i16" => Cow::Owned(format!("an integer from {} to {}", i16::MIN, i16::MAX)),
        "i32" => Cow::Owned(format!("an integer from {} to {}", i32::MIN, i32::MAX)),
        "i64" => Cow::Owned(format!("an integer from {} to {}", i64::MIN, i64::MAX)),
        "i128" => Cow::Owned(format!("an integer from {} to {}", i128::MIN, i128::MAX)),
        "isize" => Cow::Owned(format!("an integer from {} to {}", isize::MIN, isize::MAX)),
        "f32" | "f64" => Cow::Borrowed("a number"),
        "bool" | "a boolean" => Cow::Borrowed("true or false"),
        "str" | "string" | "a string" | "char" | "a character" => Cow::Borrowed("a string"),
        "path string" => Cow::Borrowed("a path"),
        "a sequence" => Cow::Borrowed("an array"),
        "a map" => Cow::Borrowed("a table"),
        // An enum expectation lists the accepted values as quoted keys.
        expected if expected.contains('`') => Cow::Borrowed(expected),
        expected if expected.starts_with("struct ") || expected.starts_with("tuple struct ") => {
            Cow::Borrowed("a table")
        }
        expected if expected.starts_with("a tuple") => Cow::Borrowed("an array"),
        _ => Cow::Borrowed("a supported value"),
    }
}

/// The TOML value an author wrote, named in TOML terms.
fn written_value(found: &str) -> Cow<'_, str> {
    match found {
        "array" | "sequence" => return Cow::Borrowed("an array"),
        "map" | "table" | "inline table" => return Cow::Borrowed("a table"),
        "datetime" => return Cow::Borrowed("a date"),
        "unit value" => return Cow::Borrowed("an empty value"),
        _ => {}
    }
    let (kind, value) = found
        .strip_prefix("floating point ")
        .map(|value| ("floating point", value))
        .or_else(|| found.split_once(' '))
        .unwrap_or(("", found));
    let named = match kind {
        "string" => "the string",
        "character" => "the character",
        "boolean" => "the boolean",
        "integer" | "floating point" => "the number",
        _ => return Cow::Borrowed("a value"),
    };
    Cow::Owned(format!("{named} `{}`", value.trim_matches('`')))
}

pub(crate) fn attach_source(error: anyhow::Error, diagnostic: Diagnostic) -> anyhow::Error {
    anyhow::Error::new(DiagnosticError::attach(error, vec![diagnostic]))
}

pub(crate) fn attach_load(
    error: anyhow::Error,
    path: Option<&Path>,
    positions: Option<&ConfigPositions>,
) -> anyhow::Error {
    attach(error, crate::codes::config::LOAD, path, positions)
}

pub(crate) fn attach_reload(
    error: anyhow::Error,
    path: &Path,
    positions: Option<&ConfigPositions>,
) -> anyhow::Error {
    attach(error, crate::codes::config::RELOAD, Some(path), positions)
}

fn attach(
    error: anyhow::Error,
    fallback_code: crate::diagnostic::DiagnosticCode,
    path: Option<&Path>,
    positions: Option<&ConfigPositions>,
) -> anyhow::Error {
    if crate::diagnostic::attached(&error).is_some() {
        return error;
    }
    let display = path.map(config_display_path);
    let config = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ConfigError>());
    let mut diagnostics = match config {
        Some(config) => error_diagnostics(config, display.as_deref(), positions),
        None => vec![crate::diagnostic::fallback(fallback_code, &error)],
    }
    .into_iter()
    .map(
        |diagnostic| match (diagnostic.location.is_none(), &display) {
            (true, Some(display)) => diagnostic.with_path(&**display),
            _ => diagnostic,
        },
    )
    .collect::<Vec<_>>();
    if diagnostics.is_empty() {
        let diagnostic = crate::diagnostic::fallback(fallback_code, &error);
        diagnostics.push(match &display {
            Some(display) => diagnostic.with_path(&**display),
            None => diagnostic,
        });
    }
    anyhow::Error::new(DiagnosticError::attach(error, diagnostics))
}

/// Convert retained configuration warnings into the shared diagnostic protocol.
pub fn warning_diagnostics(config: &crate::config::ResolvedSiteConfig) -> Vec<Diagnostic> {
    let display = crate::filesystem::display_path(config.config_path(), config.get_root());
    let positions = config.positions();
    let (experimental, other): (Vec<_>, Vec<_>) = config
        .warnings()
        .iter()
        .partition(|warning| warning.tag() == Some(ConfigDiagnosticTag::Experimental));
    let mut diagnostics = Vec::new();
    if !experimental.is_empty() {
        let mut diagnostic = Diagnostic::at_path(
            crate::codes::config::EXPERIMENTAL,
            Severity::Warning,
            &display,
            "experimental configuration is enabled",
        );
        for warning in &experimental {
            diagnostic = diagnostic.with_note(warning.field.as_str());
        }
        if let Some(excerpt) =
            positions.and_then(|positions| positions.excerpt(experimental[0].field.as_str()))
        {
            diagnostic = diagnostic.with_source_excerpt(
                excerpt.line,
                excerpt.column,
                excerpt.text,
                Some(excerpt.highlight),
            );
        }
        diagnostic = diagnostic.with_help("Remove these fields");
        diagnostics.push(diagnostic);
    }
    diagnostics.extend(other.into_iter().map(|warning| {
        let mut diagnostic = Diagnostic::at_path(
            crate::codes::config::WARNING,
            Severity::Warning,
            &display,
            &warning.message,
        )
        .with_note(warning.field.as_str());
        if let Some(excerpt) = positions.and_then(|positions| written_excerpt(positions, warning)) {
            diagnostic = diagnostic.with_source_excerpt(
                excerpt.line,
                excerpt.column,
                excerpt.text,
                Some(excerpt.highlight),
            );
        }
        if let Some(help) = &warning.help {
            diagnostic = diagnostic.with_help(help);
        }
        diagnostic
    }));
    diagnostics
}

fn error_diagnostics(
    error: &ConfigError,
    display: Option<&str>,
    positions: Option<&ConfigPositions>,
) -> Vec<Diagnostic> {
    match error {
        ConfigError::Diagnostics(diagnostics) => diagnostics
            .errors()
            .iter()
            .map(|item| validation_diagnostic(item, display, positions))
            .collect(),
        ConfigError::Toml {
            path,
            input,
            source,
        } => vec![syntax_error(path, input, source.message(), source.span())],
        ConfigError::Io(path, source) => {
            let message = match source.kind() {
                std::io::ErrorKind::NotFound => "the configuration file does not exist",
                std::io::ErrorKind::PermissionDenied => "the configuration file is not readable",
                _ => "Tola could not read the configuration file",
            };
            vec![Diagnostic::at_path(
                crate::codes::config::IO,
                Severity::Error,
                &*config_display_path(path),
                message,
            )]
        }
        ConfigError::Refused { path, reason } => {
            let message = match reason {
                crate::config::ConfigSourceRefusal::GeneratedState => {
                    "the configuration file is Tola's own generated state"
                }
                crate::config::ConfigSourceRefusal::OutsideSite => {
                    "the configuration file resolves outside the site"
                }
            };
            let help = match reason {
                crate::config::ConfigSourceRefusal::GeneratedState => {
                    "Move it outside `.tola`, or point `--config` at the site's own `tola.toml`"
                }
                crate::config::ConfigSourceRefusal::OutsideSite => {
                    "Replace it with a file inside the site, or with a link that stays inside"
                }
            };
            vec![
                Diagnostic::at_path(
                    crate::codes::config::IO,
                    Severity::Error,
                    &*config_display_path(path),
                    message,
                )
                .with_help(help),
            ]
        }
        ConfigError::UnknownFields { fields } => {
            let named = fields
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ");
            let message = format!("unknown configuration fields: {named}");
            let mut diagnostic = match display {
                Some(display) => Diagnostic::at_path(
                    crate::codes::config::INVALID,
                    Severity::Error,
                    display,
                    message,
                ),
                None => Diagnostic::new(crate::codes::config::INVALID, Severity::Error, message),
            };
            // The first key the document wrote is where the author corrects the spelling.
            if let Some(excerpt) = fields
                .first()
                .and_then(|field| positions.and_then(|positions| positions.excerpt(field)))
            {
                diagnostic = diagnostic.with_source_excerpt(
                    excerpt.line,
                    excerpt.column,
                    excerpt.text,
                    Some(excerpt.highlight),
                );
            }
            // Each key has both its line and the exact span that writes it, so a correction
            // needs no search of the document for its spelling.
            let written: Vec<UnknownField> = fields
                .iter()
                .map(|field| UnknownField {
                    name: field.clone(),
                    line: positions
                        .and_then(|positions| positions.excerpt(field))
                        .map(|excerpt| excerpt.line),
                    written: positions.and_then(|positions| positions.written(field)),
                })
                .collect();
            vec![
                diagnostic
                    .with_cause(DiagnosticCause::UnknownConfigurationFields { fields: written })
                    .with_help(UNKNOWN_FIELD_HELP),
            ]
        }
        ConfigError::Validation(message) => {
            vec![Diagnostic::new(
                crate::codes::config::INVALID,
                Severity::Error,
                message,
            )]
        }
    }
}

/// One validation failure, pointed at the key it names when the source still holds it.
fn validation_diagnostic(
    item: &ConfigDiagnostic,
    display: Option<&str>,
    positions: Option<&ConfigPositions>,
) -> Diagnostic {
    let mut diagnostic = match display {
        Some(display) => Diagnostic::at_path(
            crate::codes::config::INVALID,
            Severity::Error,
            display,
            &item.message,
        ),
        None => Diagnostic::new(
            crate::codes::config::INVALID,
            Severity::Error,
            &item.message,
        ),
    }
    .with_note(item.field.as_str());
    if let Some(excerpt) = positions.and_then(|positions| written_excerpt(positions, item)) {
        diagnostic = diagnostic.with_source_excerpt(
            excerpt.line,
            excerpt.column,
            excerpt.text,
            Some(excerpt.highlight),
        );
    }
    if let Some(help) = &item.help {
        diagnostic = diagnostic.with_help(help);
    }
    diagnostic
}

/// The excerpt of the narrowest written key a configuration diagnostic names.
///
/// A diagnostic raised inside an array-table entry asks for the field within that entry first and
/// the entry's own key next, so a field its author left out still points at the entry to edit
/// rather than at the first entry of the array.
fn written_excerpt(positions: &ConfigPositions, item: &ConfigDiagnostic) -> Option<SourceExcerpt> {
    item.key_paths()
        .find_map(|key| positions.excerpt(key.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tola_config::FieldPath;

    #[test]
    fn warning_names_the_line_that_wrote_it() {
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
    fn excerpt_locates_one_written_line() {
        let content = "[build]\nentry = [\n";
        let start = content.find("entry = [").unwrap() + "entry = ".len();
        let excerpt = source_excerpt(content, start..start + 1).unwrap();
        assert_eq!(excerpt.line, 2);
        assert_eq!(excerpt.column, 9);
        assert_eq!(excerpt.text, "entry = [");
        assert_eq!(excerpt.highlight, (8, 9));
    }

    #[test]
    fn syntax_error_renders_saved_input() {
        let input = "[build]\nentry = [\n";
        let source = toml::from_str::<toml::Value>(input).unwrap_err();
        let reason = source.message().to_owned();
        let error = ConfigError::Toml {
            path: std::path::PathBuf::from("/missing/tola.toml"),
            input: input.into(),
            source,
        };

        let diagnostics = error_diagnostics(&error, Some("tola.toml"), None);

        assert_eq!(diagnostics[0].code, crate::codes::config::TOML);
        assert_eq!(diagnostics[0].message, reason);
        let location = diagnostics[0].location.as_ref().unwrap();
        assert_eq!(location.path, "tola.toml");
        assert_eq!(location.source_lines[0].text, "entry = [");
    }

    #[test]
    fn io_failures_name_the_configuration_file() {
        let error = ConfigError::Io(
            std::path::PathBuf::from("/site/tola.toml"),
            std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
        );

        let diagnostics = error_diagnostics(&error, Some("tola.toml"), None);

        assert_eq!(diagnostics[0].code, crate::codes::config::IO);
        assert_eq!(diagnostics[0].location.as_ref().unwrap().path, "tola.toml");
        assert_eq!(
            diagnostics[0].message,
            "the configuration file does not exist"
        );
        assert!(!diagnostics[0].message.contains("no such file"));
    }

    #[test]
    fn empty_diagnostics_fall_back_to_one_error() {
        let path = std::path::Path::new("/site/tola.toml");
        let error = anyhow::Error::new(ConfigError::Diagnostics(
            crate::config::ConfigDiagnostics::new(),
        ));

        let error = attach_load(error, Some(path), None);
        let diagnostics = crate::config::tests::attached_diagnostics(&error);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, crate::codes::config::LOAD);
        assert_eq!(diagnostics[0].location.as_ref().unwrap().path, "tola.toml");
    }

    #[test]
    fn missing_field_rewrite_offers_no_help() {
        let rewritten = schema_message("missing field `entry`");

        assert_eq!(rewritten.message, "the required field `entry` is missing");
        assert_eq!(rewritten.help, None);
    }
}
