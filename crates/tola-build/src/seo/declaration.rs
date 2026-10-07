//! Typed interpretation of final Bundle write declarations.

use typst::foundations::{Dict, Selector, Value};
use typst::introspection::Location;
use typst::syntax::{Span, VirtualPath};

use super::RenderError;
use crate::cancellation::BuildCancellation;
use crate::config::ResolvedSiteConfig;
use tola_address::OutputPath;

const FRAGMENT_ENCODE_SET: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b'%')
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'`');

#[derive(Debug)]
pub(crate) struct SeoDeclarationError {
    label: &'static str,
    span: Span,
    output: Option<String>,
    field: String,
    /// Every parser returns this error by value; boxing keeps content violations from widening
    /// those result values.
    violation: Box<DeclarationViolation>,
    /// The diagnostic's hint: the next action the author takes for this violation.
    help: Option<String>,
}

impl std::fmt::Display for SeoDeclarationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid <{}> declaration", self.label)?;
        if let Some(output) = &self.output {
            write!(formatter, " for `{output}`")?;
        }
        if !self.field.is_empty() {
            write!(formatter, " at `{}`", self.field)?;
        }
        write!(formatter, ": {}", self.violation)
    }
}

impl std::error::Error for SeoDeclarationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.violation.as_ref())
    }
}

impl SeoDeclarationError {
    pub(crate) fn source_diagnostic(&self) -> tola_typst::NativeDiagnostic {
        let diagnostic = typst::diag::SourceDiagnostic::error(self.span, self.to_string());
        match &self.help {
            Some(help) => diagnostic.with_hint(help.as_str()),
            None => diagnostic,
        }
        .into()
    }

    /// Name the concrete next action this violation asks of the author.
    pub(super) fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

#[derive(Debug, thiserror::Error)]
enum DeclarationViolation {
    #[error("missing required field")]
    Missing,
    #[error("expected {expected}, found {actual}")]
    Type {
        expected: &'static str,
        actual: String,
    },
    #[error("unknown field")]
    Unknown { allowed: String },
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Content(#[source] super::feed::FeedContentViolation),
}

impl DeclarationViolation {
    /// The next action for a violation the protocol can name without the failing site.
    fn help(&self, field: &str) -> Option<String> {
        match self {
            // An absent field's action also names the entry that holds it, which only the call
            // site knows.
            Self::Missing => None,
            Self::Type { expected, .. } if field.is_empty() => {
                Some(format!("Give the declaration a {expected} value"))
            }
            Self::Type { expected, .. } => Some(format!("Give `{field}` a {expected} value")),
            Self::Unknown { allowed } => Some(format!("Use one of: {allowed}")),
            Self::Invalid(_) => None,
            Self::Content(violation) => Some(violation.help()),
        }
    }
}

pub(super) struct SeoDeclaration {
    label: &'static str,
    span: Span,
    pub(super) fields: Dict,
}

impl SeoDeclaration {
    pub(super) fn parse(
        label: &'static str,
        declaration: tola_typst::MetadataDeclaration,
    ) -> Result<Self, SeoDeclarationError> {
        let span = declaration.span();
        let value = declaration.into_value();
        let Value::Dict(fields) = value else {
            let violation = DeclarationViolation::Type {
                expected: "dictionary",
                actual: value.ty().short_name().into(),
            };
            let help = violation.help("");
            return Err(SeoDeclarationError {
                label,
                span,
                output: None,
                field: String::new(),
                violation: Box::new(violation),
                help,
            });
        };
        Ok(Self {
            label,
            span,
            fields,
        })
    }

    pub(super) fn invalid(&self, field: &str, message: impl Into<String>) -> SeoDeclarationError {
        self.error(field, DeclarationViolation::Invalid(message.into()))
    }
    pub(super) fn content_violation(
        &self,
        field: &str,
        violation: super::feed::FeedContentViolation,
    ) -> SeoDeclarationError {
        self.error(field, DeclarationViolation::Content(violation))
    }

    pub(super) fn reference_warning(
        &self,
        field: &str,
        message: impl std::fmt::Display,
    ) -> tola_typst::NativeDiagnostic {
        typst::diag::SourceDiagnostic::warning(
            self.span,
            format!("<{}> at `{field}`: {message}", self.label),
        )
        .into()
    }

    fn error(&self, field: &str, violation: DeclarationViolation) -> SeoDeclarationError {
        let help = violation.help(field);
        SeoDeclarationError {
            label: self.label,
            span: self.span,
            output: match self.fields.get("output") {
                Ok(Value::Str(output)) => Some(output.to_string()),
                _ => None,
            },
            field: field.into(),
            violation: Box::new(violation),
            help,
        }
    }

    pub(super) fn expected(
        &self,
        field: &str,
        expected: &'static str,
        value: &Value,
    ) -> SeoDeclarationError {
        self.error(
            field,
            DeclarationViolation::Type {
                expected,
                actual: value.ty().short_name().into(),
            },
        )
    }

    pub(super) fn required<'a>(
        &self,
        fields: &'a Dict,
        key: &str,
        parent: &str,
    ) -> Result<&'a Value, SeoDeclarationError> {
        fields.get(key).map_err(|_| self.missing(key, parent))
    }

    /// A required field is absent; the hint names the field and the entry that holds it.
    fn missing(&self, key: &str, parent: &str) -> SeoDeclarationError {
        self.error(&field_path(parent, key), DeclarationViolation::Missing)
            .with_help(format!("Add `{key}` to {}", holding_entry(parent)))
    }

    pub(super) fn required_array<'a>(
        &self,
        fields: &'a Dict,
        key: &str,
        parent: &str,
    ) -> Result<&'a typst::foundations::Array, SeoDeclarationError> {
        match self.required(fields, key, parent)? {
            Value::Array(values) => Ok(values),
            value => Err(self.expected(&field_path(parent, key), "array", value)),
        }
    }

    pub(super) fn check_fields(
        &self,
        fields: &Dict,
        allowed: &[&str],
        parent: &str,
    ) -> Result<(), SeoDeclarationError> {
        for (name, _) in fields {
            if !allowed.contains(&name.as_str()) {
                let allowed = allowed
                    .iter()
                    .map(|field| format!("`{field}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(self.error(
                    &field_path(parent, name.as_str()),
                    DeclarationViolation::Unknown { allowed },
                ));
            }
        }
        Ok(())
    }

    pub(super) fn text(&self, value: &Value, field: &str) -> Result<String, SeoDeclarationError> {
        match value {
            Value::Str(text) => Ok(text.to_string()),
            Value::Content(content) => Ok(content.plain_text().to_string()),
            _ => Err(self.expected(field, "string or content", value)),
        }
    }

    pub(super) fn string(&self, value: &Value, field: &str) -> Result<String, SeoDeclarationError> {
        let Value::Str(text) = value else {
            return Err(self.expected(field, "string", value));
        };
        Ok(text.to_string())
    }

    /// Missing and `none` omit a string field; `auto` is not an omission.
    pub(super) fn optional_string(
        &self,
        fields: &Dict,
        key: &str,
        parent: &str,
    ) -> Result<Option<String>, SeoDeclarationError> {
        match fields.get(key).ok() {
            None | Some(Value::None) => Ok(None),
            Some(value) => self.string(value, &field_path(parent, key)).map(Some),
        }
    }

    pub(super) fn output(&self) -> Result<OutputPath, SeoDeclarationError> {
        let raw = self.string(self.required(&self.fields, "output", "")?, "/output")?;
        let output =
            OutputPath::parse(&raw).map_err(|error| self.invalid("/output", error.to_string()))?;
        if output.is_reserved_for_non_system_output() {
            return Err(self
                .invalid("/output", "output path is reserved for Tola")
                .with_help("Choose a path outside `_tola`"));
        }
        Ok(output)
    }

    pub(super) fn require_origin(
        &self,
        config: &ResolvedSiteConfig,
    ) -> Result<(), SeoDeclarationError> {
        if config.site.origin.is_none() {
            return Err(self
                .invalid(
                    "/output",
                    "`site.origin` is required to generate absolute URLs",
                )
                .with_help("Set `site.origin` to the site's public origin"));
        }
        Ok(())
    }
}

pub(super) fn field_path(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}

/// RFC 6901 pointer to one element of the array at `parent`.
pub(super) fn index_path(parent: &str, index: usize) -> String {
    format!("{parent}/{index}")
}

/// The phrase a hint uses to name what holds a missing field.
///
/// The pointer's own segments name the holder, so `/entries/0` reads as `` `entries` entry 1 `` and
/// `/entries/0/content` reads as `` `content` in `entries` entry 1 ``.
fn holding_entry(pointer: &str) -> String {
    let segments = pointer
        .split('/')
        .skip(1)
        .map(|segment| segment.replace("~1", "/").replace("~0", "~"))
        .collect::<Vec<_>>();
    let mut holders = Vec::new();
    let mut rest = segments.as_slice();
    while let Some((segment, following)) = rest.split_first() {
        match following
            .first()
            .and_then(|index| index.parse::<usize>().ok())
        {
            Some(index) => {
                holders.push(format!("`{segment}` entry {}", index + 1));
                rest = &following[1..];
            }
            None => {
                holders.push(format!("`{segment}`"));
                rest = following;
            }
        }
    }
    if holders.is_empty() {
        return "the declaration".to_owned();
    }
    holders.reverse();
    holders.join(" in ")
}

pub(super) struct ResolvedDocumentTarget {
    pub(super) output: OutputPath,
    pub(super) url: url::Url,
    pub(super) title: Option<String>,
}

pub(super) fn resolve_target(
    declaration: &SeoDeclaration,
    value: &Value,
    field: &str,
    output: &OutputPath,
    config: &ResolvedSiteConfig,
    compilation: &tola_typst::BundleCompilation,
    cancellation: &BuildCancellation,
) -> Result<ResolvedDocumentTarget, RenderError> {
    cancellation.ensure_active()?;
    let introspector = compilation.introspector();
    let (path, fragment, location) = match value {
        Value::Str(path) => (
            parse_target_path(declaration, path.as_str(), field)?,
            None,
            None,
        ),
        Value::Dict(fields) => {
            declaration.check_fields(fields, &["output", "fragment"], field)?;
            let path_field = field_path(field, "output");
            let raw =
                declaration.string(declaration.required(fields, "output", field)?, &path_field)?;
            let fragment = declaration.optional_string(fields, "fragment", field)?;
            (
                parse_target_path(declaration, &raw, &path_field)?,
                fragment,
                None,
            )
        }
        Value::Label(label) => {
            let content = introspector
                .query_unique(&Selector::Label(*label))
                .map_err(|_| declaration.invalid(field, "no element has this target label"))?;
            let location = content.location().ok_or_else(|| {
                declaration
                    .invalid(field, "this label has no location in a document")
                    .with_help("Place the label inside a `#document(...)` body")
            })?;
            let path = introspector.path(location).ok_or_else(|| {
                declaration
                    .invalid(field, "this label does not belong to a document")
                    .with_help("Place the label inside a `#document(...)` body")
            })?;
            (path.clone(), None, Some(location))
        }
        Value::Dyn(dynamic) if dynamic.is::<Location>() => {
            let location = value
                .clone()
                .cast::<Location>()
                .expect("location type checked");
            let path = introspector.path(location).ok_or_else(|| {
                declaration
                    .invalid(field, "this location does not belong to a document")
                    .with_help("Place the target inside a `#document(...)` body")
            })?;
            (path.clone(), None, Some(location))
        }
        _ => {
            return Err(declaration
                .expected(
                    field,
                    "document output path, label, location, or (output, fragment) dictionary",
                    value,
                )
                .into());
        }
    };
    let document = compilation.document(&path).ok_or_else(|| {
        declaration
            .invalid(
                field,
                format!("no document produces `{}`", path.get_without_slash()),
            )
            .with_help("Name a document by its logical output path, such as `notes/index.html`")
    })?;
    let target_output = OutputPath::parse(path.get_without_slash())
        .map_err(|_| declaration.invalid(field, "the target output path is not valid"))?;
    let route = tola_address::route_for_output(&target_output);
    let mut target_url = url::Url::parse(&config.canonical_url(&route))
        .map_err(|_| declaration.invalid(field, "the target URL is not valid"))?;
    if let Some(fragment) = fragment {
        if let Some(html) = document.html_inventory(&cancellation.bundle_cancellation())? {
            let mut found = false;
            for anchor in html.fragments() {
                cancellation.ensure_active()?;
                if anchor.value() == fragment {
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(declaration
                    .invalid(
                        field,
                        format!(
                            "target document `{target_output}` has no exported anchor `{fragment}`"
                        ),
                    )
                    .with_help(
                        "Name an anchor the document exports, or drop `fragment` to target the whole document",
                    )
                    .into());
            }
        }
        let fragment =
            percent_encoding::utf8_percent_encode(&fragment, FRAGMENT_ENCODE_SET).to_string();
        target_url.set_fragment(Some(&fragment));
    }
    if let Some(location) = location {
        let base_path = VirtualPath::new(output.as_str()).expect("validated output path");
        let resolved = typst::model::LateLinkResolver::new(Some(&base_path), introspector)
            .resolve(location)
            .ok_or_else(|| {
                declaration
                    .invalid(field, "target element has no exported anchor")
                    .with_help("Target the document's output path, or an element the site links to")
            })?
            .into_relative_uri()
            .map_err(|_| {
                declaration.invalid(field, "the target link cannot be built from this location")
            })?;
        let output_url = tola_address::asset_url_from_output(output);
        let resolved_url = url::Url::parse(&config.canonical_url(&output_url))
            .and_then(|base| base.join(&resolved))
            .map_err(|_| {
                declaration.invalid(field, "the target link cannot be built from this location")
            })?;
        target_url.set_fragment(resolved_url.fragment());
    }
    cancellation.ensure_active()?;
    Ok(ResolvedDocumentTarget {
        output: target_output,
        url: target_url,
        title: document.info().title.as_ref().map(ToString::to_string),
    })
}

fn parse_target_path(
    declaration: &SeoDeclaration,
    raw: &str,
    field: &str,
) -> Result<VirtualPath, SeoDeclarationError> {
    let path =
        OutputPath::parse(raw).map_err(|error| declaration.invalid(field, error.to_string()))?;
    VirtualPath::new(path.as_str()).map_err(|_| {
        declaration.invalid(field, "the target output path is not a valid Bundle path")
    })
}

pub(super) fn published_date(
    declaration: &SeoDeclaration,
    value: &Value,
    field: &str,
) -> Result<atom_syndication::FixedDateTime, SeoDeclarationError> {
    let text = match value {
        Value::Datetime(datetime) => crate::seo::date::to_rfc3339(datetime).ok_or_else(|| {
            declaration
                .invalid(
                    field,
                    "datetime must include a complete date (year, month, and day)",
                )
                .with_help("Give the time in UTC with hour, minute, and second")
        })?,
        Value::Str(text) => text.to_string(),
        _ => {
            return Err(declaration.expected(
                field,
                "datetime or RFC 3339 string with timezone",
                value,
            ));
        }
    };
    text.parse().map_err(|_| {
        declaration.invalid(
            field,
            "expected a valid RFC 3339 timestamp with timezone, such as 2026-09-01T12:00:00Z",
        )
    })
}
