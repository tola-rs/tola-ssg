//! Typed native diagnostics transported through Typst errors and its retained warning sink.

use std::num::{NonZeroU16, NonZeroU64};
use std::sync::{Arc, LazyLock};
use typst::diag::{SourceDiagnostic, Tracepoint};
use typst::ecow::{EcoString, EcoVec};
use typst::syntax::package::PackageSpec;
use typst::syntax::{DiagSpan, FileId, RootedPath, Span, Spanned, VirtualPath, VirtualRoot};

/// The virtual package holding one file per diagnostic Tola produces. A diagnostic points its span
/// at such a file, whose path spells the location, projection, and producer identity it really has.
static DIAGNOSTIC_PACKAGE: LazyLock<PackageSpec> = LazyLock::new(|| {
    "@tola/diagnostic:0.0.0"
        .parse()
        .expect("fixed diagnostic package")
});
const PRODUCER_DIRECTORY: &str = ".tola-diagnostic/producer/";

/// The producer identity and evidence a diagnostic retains independently of its prose.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DiagnosticOrigin {
    /// An upstream Typst diagnostic with no producer identity.
    #[default]
    Typst,
    /// Opaque evidence whose meaning belongs to the producer.
    Producer(ProducerDiagnosticOrigin),
}

/// A producer's named, ordered evidence, shared by native and resolved diagnostics.
///
/// Names and field order belong to the producer's serialization contract. The adapter transports
/// these values without interpreting their names or contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducerDiagnosticOrigin {
    name: EcoString,
    fields: Arc<Vec<(EcoString, EcoString)>>,
}

impl ProducerDiagnosticOrigin {
    /// The producer's type identity, independent of diagnostic prose.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Evidence in the order the producer declared.
    pub fn fields(&self) -> &[(EcoString, EcoString)] {
        &self.fields
    }
}

struct SerializedFields<'a>(&'a [(EcoString, EcoString)]);

impl serde::Serialize for SerializedFields<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut fields = serializer.serialize_map(Some(self.0.len()))?;
        for (name, value) in self.0 {
            fields.serialize_entry(name, value)?;
        }
        fields.end()
    }
}

impl serde::Serialize for DiagnosticOrigin {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            Self::Typst => serializer.serialize_unit_variant("DiagnosticOrigin", 0, "Typst"),
            Self::Producer(origin) => {
                let mut tagged = serializer.serialize_map(Some(1))?;
                tagged.serialize_entry(&origin.name, &SerializedFields(&origin.fields))?;
                tagged.end()
            }
        }
    }
}

struct DeserializedFields(Vec<(EcoString, EcoString)>);

impl<'de> serde::Deserialize<'de> for DeserializedFields {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldsVisitor;
        impl<'de> serde::de::Visitor<'de> for FieldsVisitor {
            type Value = DeserializedFields;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("ordered producer evidence")
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut fields = Vec::new();
                while let Some(field) = map.next_entry()? {
                    fields.push(field);
                }
                Ok(DeserializedFields(fields))
            }
        }
        deserializer.deserialize_map(FieldsVisitor)
    }
}

impl<'de> serde::Deserialize<'de> for DiagnosticOrigin {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OriginVisitor;
        impl<'de> serde::de::Visitor<'de> for OriginVisitor {
            type Value = DiagnosticOrigin;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("Typst or a named producer origin")
            }

            fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<Self::Value, E> {
                if name == "Typst" {
                    Ok(DiagnosticOrigin::Typst)
                } else {
                    Err(E::custom("a producer origin requires its ordered evidence"))
                }
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let Some((name, fields)) = map.next_entry::<EcoString, DeserializedFields>()?
                else {
                    return Err(serde::de::Error::custom(
                        "a producer origin requires its name",
                    ));
                };
                if map.next_key::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::custom(
                        "a diagnostic has one producer origin",
                    ));
                }
                Ok(DiagnosticOrigin::Producer(ProducerDiagnosticOrigin {
                    name,
                    fields: Arc::new(fields.0),
                }))
            }
        }
        deserializer.deserialize_any(OriginVisitor)
    }
}

/// How one diagnostic reaches the author's code through Tola's own packages.
///
/// A Tola package wraps a call the author wrote and reports from inside that wrapper, so the
/// diagnostic's own span names Tola's file and its frames name Tola's helpers. `packages` are the
/// packages that wrapper consists of: the primary location is the producer's own span when it
/// already lies outside them — the author's file, or a package the author installed — and the
/// first caller outside them otherwise. Frames of these packages are omitted from the resolved
/// trace either way; raw diagnostic spans and traces remain unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosticProjection(EcoVec<PackageSpec>);

impl DiagnosticProjection {
    /// Project through the packages Tola wrapped the author's call in.
    pub fn through(packages: impl IntoIterator<Item = PackageSpec>) -> Self {
        Self(packages.into_iter().collect())
    }

    /// Whether `span` belongs to a file of one of these packages.
    fn covers(&self, span: DiagSpan) -> bool {
        span.id().is_some_and(
            |id| matches!(id.root(), VirtualRoot::Package(package) if self.0.contains(package)),
        )
    }

    /// Whether `span` addresses a file outside these packages: the author's own, or an installed one.
    fn outside(&self, span: DiagSpan) -> bool {
        span.id().is_some_and(|id| match id.root() {
            VirtualRoot::Project => true,
            VirtualRoot::Package(package) => !self.0.contains(package),
        })
    }

    /// The comma-joined package specifications one diagnostic file path spells.
    fn encoded(&self) -> String {
        self.0
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The projection one diagnostic file path spells, when every entry names a package.
    fn decoded(encoded: &str) -> Option<Self> {
        encoded
            .split(',')
            .filter(|package| !package.is_empty())
            .map(str::parse)
            .collect::<Result<EcoVec<PackageSpec>, _>>()
            .ok()
            .map(Self)
    }
}

/// One original native diagnostic with its producer identity and projection.
///
/// Incoming upstream diagnostics convert once, before resolution or filtering. Tola's own produced
/// diagnostics are unwrapped here; raw spans and traces retain their original evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeDiagnostic {
    source: SourceDiagnostic,
    origin: DiagnosticOrigin,
    projection: DiagnosticProjection,
}

impl NativeDiagnostic {
    /// The original upstream diagnostic, without Tola's producer identity.
    pub fn source(&self) -> &SourceDiagnostic {
        &self.source
    }

    /// The producer identity retained by this native value and its resolved projection.
    pub fn origin(&self) -> &DiagnosticOrigin {
        &self.origin
    }

    /// The span the author's code occupies, as the projection selects it.
    pub(super) fn primary_span(&self) -> DiagSpan {
        if self.projection.outside(self.source.span) {
            return self.source.span;
        }
        self.source
            .trace
            .iter()
            .find(|trace| self.projection.outside(trace.span.into()))
            .map_or(self.source.span, |caller| caller.span.into())
    }

    /// The frames the resolved diagnostic keeps, in the order the stack recorded them.
    ///
    /// The primary call frame and a projection's packages are answered elsewhere, so they are not
    /// repeated; every other frame stays, because none can be told to carry the failing value.
    pub(super) fn retained_traces(&self) -> impl Iterator<Item = &Spanned<Tracepoint>> {
        let primary = self.primary_span();
        let relocated = primary != self.source.span;
        let projection = &self.projection;
        self.source.trace.iter().filter(move |trace| {
            !(relocated && DiagSpan::from(trace.span) == primary)
                && !projection.covers(trace.span.into())
        })
    }
}

impl From<SourceDiagnostic> for NativeDiagnostic {
    fn from(mut source: SourceDiagnostic) -> Self {
        let mut projection = DiagnosticProjection::default();
        let origin = source.span.id().and_then(|id| {
            let VirtualRoot::Package(package) = id.root() else {
                return None;
            };
            if package != &*DIAGNOSTIC_PACKAGE {
                return None;
            }
            let spelled = id
                .vpath()
                .get_without_slash()
                .strip_prefix(PRODUCER_DIRECTORY)?;
            let mut fields = spelled.split('/');
            let located = decoded_location(fields.next()?)?;
            projection = DiagnosticProjection::decoded(&decode(fields.next()?)?)?;
            let name = decode(fields.next()?)?;
            let mut evidence = Vec::new();
            while let Some(key) = fields.next() {
                evidence.push((decode(key)?, decode(fields.next()?)?));
            }
            source.span = located;
            // A path without a producer name spells the projection alone, so the diagnostic keeps
            // the upstream origin it already had.
            (!name.is_empty()).then(|| {
                DiagnosticOrigin::Producer(ProducerDiagnosticOrigin {
                    name,
                    fields: Arc::new(evidence),
                })
            })
        });
        assert!(
            !source.span.id().is_some_and(|id| {
                matches!(id.root(), VirtualRoot::Package(package) if package == &*DIAGNOSTIC_PACKAGE)
            }),
            "an owned diagnostic must decode to its original source span"
        );
        Self {
            source,
            origin: origin.unwrap_or_default(),
            projection,
        }
    }
}

/// A producer warning for Typst's tracked sink.
///
/// The caller emits this value with `engine.sink.warn`. Only warnings retained by Typst are
/// unwrapped on ingress, preserving memoized replay, deduplication, and final realization selection.
/// The diagnostic's span spells the original location, so ingress restores it without reading any
/// source text.
pub fn producer_warning(
    span: Span,
    name: &str,
    fields: &[(&str, &str)],
    message: impl Into<EcoString>,
) -> SourceDiagnostic {
    SourceDiagnostic::warning(
        diagnostic_file_span(
            Location::Span(span),
            &DiagnosticProjection::default(),
            Some(name),
            fields,
        ),
        message,
    )
}

/// A producer error retaining its original span and trace while selecting its resolved primary
/// location through `projection`. Return this value through Typst's ordinary error path.
pub fn producer_error(
    span: Span,
    name: &str,
    fields: &[(&str, &str)],
    message: impl Into<EcoString>,
    projection: DiagnosticProjection,
) -> SourceDiagnostic {
    SourceDiagnostic::error(
        diagnostic_file_span(Location::Span(span), &projection, Some(name), fields),
        message,
    )
}

/// A producer error at the byte `range` of `file`, for a producer that knows the position it
/// reports but holds no span into that file.
///
/// The identity and projection behave exactly as [`producer_error`]'s; only the location's form
/// differs.
pub fn producer_error_at(
    file: FileId,
    range: std::ops::Range<usize>,
    name: &str,
    fields: &[(&str, &str)],
    message: impl Into<EcoString>,
    projection: DiagnosticProjection,
) -> SourceDiagnostic {
    SourceDiagnostic::error(
        diagnostic_file_span(
            Location::Range { file, range },
            &projection,
            Some(name),
            fields,
        ),
        message,
    )
}

/// The span of one diagnostic reported at the byte `range` of `file` through `projection`, with no
/// producer identity: the diagnostic keeps the origin Typst gave it, while resolution still
/// restores that range and omits the wrapper's frames. Use it for a diagnostic Tola reports on
/// behalf of another producer, such as a site's own schema.
pub fn projected_span(
    file: FileId,
    range: std::ops::Range<usize>,
    projection: DiagnosticProjection,
) -> DiagSpan {
    diagnostic_file_span(Location::Range { file, range }, &projection, None, &[])
}

/// The location one diagnostic was reported at, as either form a diagnostic span takes.
#[derive(Debug, Clone)]
enum Location {
    /// A span into a source file, as a producer's own native call supplies it.
    Span(Span),
    /// A byte range of one file, for a position a reporter computes itself.
    Range {
        file: FileId,
        range: std::ops::Range<usize>,
    },
}

impl Location {
    /// The path segment that spells this location, as `decoded_location` reads it back.
    fn encoded(self) -> String {
        match self {
            Self::Span(span) => format!("s{}", span.into_raw()),
            Self::Range { file, range } => {
                format!("r{}-{}-{}", file.into_raw(), range.start, range.end)
            }
        }
    }
}

/// The diagnostic span one spelled path segment restores.
fn decoded_location(encoded: &str) -> Option<DiagSpan> {
    let (kind, rest) = encoded.split_at_checked(1)?;
    match kind {
        "s" => Some(Span::from_raw(NonZeroU64::new(rest.parse().ok()?)?).into()),
        "r" => {
            let (file, rest) = rest.split_once('-')?;
            let (start, end) = rest.split_once('-')?;
            if end.contains('-') {
                return None;
            }
            Some(DiagSpan::from_range(
                FileId::from_raw(NonZeroU16::new(file.parse().ok()?)?),
                start.parse().ok()?..end.parse().ok()?,
            ))
        }
        _ => None,
    }
}

/// The span one Tola-produced diagnostic is reported at: a virtual file inside `DIAGNOSTIC_PACKAGE`
/// whose path spells the location it stands for, the projection, and the producer identity.
fn diagnostic_file_span(
    location: Location,
    projection: &DiagnosticProjection,
    name: Option<&str>,
    fields: &[(&str, &str)],
) -> DiagSpan {
    let location = location.encoded();
    let projection = projection.encoded();
    let name = name.unwrap_or_default();
    let encoded_len = |text: &str| (2 * text.len()).max(1);
    let capacity = PRODUCER_DIRECTORY.len()
        + location.len()
        + 2
        + encoded_len(&projection)
        + encoded_len(name)
        + fields
            .iter()
            .map(|(key, value)| 2 + encoded_len(key) + encoded_len(value))
            .sum::<usize>();
    let mut path = String::with_capacity(capacity);
    path.push_str(PRODUCER_DIRECTORY);
    path.push_str(&location);
    path.push('/');
    encode(&projection, &mut path);
    path.push('/');
    encode(name, &mut path);
    for (key, value) in fields {
        path.push('/');
        encode(key, &mut path);
        path.push('/');
        encode(value, &mut path);
    }
    let path = VirtualPath::new(path)
        .expect("diagnostic file paths contain only fixed segments and hexadecimal bytes");
    let id = FileId::new(RootedPath::new(
        VirtualRoot::Package(DIAGNOSTIC_PACKAGE.clone()),
        path,
    ));
    DiagSpan::from_range(id, 0..0)
}

fn encode(text: &str, encoded: &mut String) {
    if text.is_empty() {
        encoded.push('-');
        return;
    }
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for byte in text.bytes() {
        encoded.push(DIGITS[usize::from(byte >> 4)] as char);
        encoded.push(DIGITS[usize::from(byte & 15)] as char);
    }
}

fn decode(encoded: &str) -> Option<EcoString> {
    if encoded == "-" {
        return Some(EcoString::new());
    }
    if !encoded.len().is_multiple_of(2) {
        return None;
    }
    let bytes = encoded
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|digits| {
            let digit = |byte| match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                _ => None,
            };
            Some((digit(digits[0])? << 4) | digit(digits[1])?)
        })
        .collect::<Option<Vec<_>>>()?;
    std::str::from_utf8(&bytes).ok().map(EcoString::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    use typst::diag::SourceResult;
    use typst::engine::Engine;
    use typst::foundations::{Value, func};
    use typst::syntax::Span;

    thread_local! {
        static NATIVE_RUNS: Cell<usize> = const { Cell::new(0) };
    }

    #[func]
    fn warn_path(engine: &mut Engine, span: Span, number: i64) -> SourceResult<Value> {
        NATIVE_RUNS.with(|runs| runs.set(runs.get() + 1));
        let warning = producer_warning(
            span,
            "SampleWarning",
            &[("number", &number.to_string())],
            format!("different warning wording {number}"),
        );
        engine.sink.warn(warning);
        Ok(Value::None)
    }

    #[test]
    fn caller_location_preserves_native_evidence() {
        use typst::diag::Tracepoint;
        use typst::syntax::Spanned;
        let package_span = |package: &str| {
            let id = FileId::new(RootedPath::new(
                VirtualRoot::Package(package.parse().unwrap()),
                VirtualPath::new("lib.typ").unwrap(),
            ));
            typst::syntax::Source::new(id, "text".into()).root().span()
        };
        let original = package_span("@tola/schema:0.0.0");
        let wrapper = package_span("@tola/source:0.0.0");
        let helper = package_span("@preview/site-helper:1.0.0");
        let mut error = producer_error(
            original,
            "SchemaArgument",
            &[("actual", "none")],
            "schema argument is invalid",
            DiagnosticProjection::through(
                ["@tola/schema:0.0.0", "@tola/source:0.0.0"]
                    .into_iter()
                    .map(|package| package.parse().unwrap()),
            ),
        );
        for span in [wrapper, helper] {
            error
                .trace
                .push(Spanned::new(Tracepoint::Call(Some("parse".into())), span));
        }
        let native = NativeDiagnostic::from(error);
        assert_eq!(native.source().span, DiagSpan::from(original));
        assert_eq!(native.source().trace.len(), 2);
        assert_eq!(native.primary_span(), DiagSpan::from(helper));
        assert!(matches!(native.origin(), DiagnosticOrigin::Producer(_)));
    }
}
