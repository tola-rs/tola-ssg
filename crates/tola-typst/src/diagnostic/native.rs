//! Typed native diagnostics transported by Typst's retained warning sink.

use std::num::NonZeroU64;
use std::sync::{Arc, LazyLock};
use typst::diag::SourceDiagnostic;
use typst::ecow::EcoString;
use typst::syntax::package::PackageSpec;
use typst::syntax::{DiagSpan, FileId, RootedPath, Span, VirtualPath, VirtualRoot};

static CARRIER_PACKAGE: LazyLock<PackageSpec> = LazyLock::new(|| {
    "@tola/diagnostic:0.0.0"
        .parse()
        .expect("fixed diagnostic package")
});
const CARRIER_DIRECTORY: &str = ".tola-diagnostic/producer/";

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

/// One original native diagnostic with its producer identity.
///
/// Incoming upstream diagnostics convert once, before resolution or filtering. Tola's own
/// warning carriers are unwrapped here; every exposed span remains the original native span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeDiagnostic {
    source: SourceDiagnostic,
    origin: DiagnosticOrigin,
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
}

impl From<SourceDiagnostic> for NativeDiagnostic {
    fn from(mut source: SourceDiagnostic) -> Self {
        let origin = source.span.id().and_then(|id| {
            let VirtualRoot::Package(package) = id.root() else {
                return None;
            };
            if package != &*CARRIER_PACKAGE {
                return None;
            }
            let packet = id
                .vpath()
                .get_without_slash()
                .strip_prefix(CARRIER_DIRECTORY)?;
            let mut fields = packet.split('/');
            let original = fields
                .next()?
                .parse::<u64>()
                .ok()
                .and_then(NonZeroU64::new)?;
            let name = decode(fields.next()?)?;
            let mut evidence = Vec::new();
            while let Some(key) = fields.next() {
                evidence.push((decode(key)?, decode(fields.next()?)?));
            }
            source.span = Span::from_raw(original).into();
            Some(DiagnosticOrigin::Producer(ProducerDiagnosticOrigin {
                name,
                fields: Arc::new(evidence),
            }))
        });
        assert!(
            !source.span.id().is_some_and(|id| {
                matches!(id.root(), VirtualRoot::Package(package) if package == &*CARRIER_PACKAGE)
            }),
            "an owned diagnostic carrier must decode to its original source span"
        );
        Self {
            source,
            origin: origin.unwrap_or_default(),
        }
    }
}

/// A producer warning for Typst's tracked sink.
///
/// The caller emits this value with `engine.sink.warn`. Only warnings retained by Typst are
/// unwrapped on ingress, preserving memoized replay, deduplication, and final realization selection.
/// The process-local carrier retains the original source span identity and reads no source text.
pub fn producer_warning(
    span: Span,
    name: &str,
    fields: &[(&str, &str)],
    message: impl Into<EcoString>,
) -> SourceDiagnostic {
    use std::fmt::Write;
    let encoded_len = |text: &str| (2 * text.len()).max(1);
    let capacity = CARRIER_DIRECTORY.len()
        + 21
        + encoded_len(name)
        + fields
            .iter()
            .map(|(key, value)| 2 + encoded_len(key) + encoded_len(value))
            .sum::<usize>();
    let mut path = String::with_capacity(capacity);
    path.push_str(CARRIER_DIRECTORY);
    write!(path, "{}/", span.into_raw()).expect("writing a carrier into memory cannot fail");
    encode(name, &mut path);
    for (key, value) in fields {
        path.push('/');
        encode(key, &mut path);
        path.push('/');
        encode(value, &mut path);
    }
    let path = VirtualPath::new(path)
        .expect("diagnostic carriers contain only fixed segments and hexadecimal bytes");
    let id = FileId::new(RootedPath::new(
        VirtualRoot::Package(CARRIER_PACKAGE.clone()),
        path,
    ));
    SourceDiagnostic::warning(DiagSpan::from_range(id, 0..0), message)
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
    use std::sync::Arc;
    use typst::World;
    use typst::diag::SourceResult;
    use typst::engine::Engine;
    use typst::foundations::{Func, NativeFunc, Value, func};
    use typst::syntax::Span;
    use typst::utils::LazyHash;
    use typst::{Feature, Features, Library, LibraryExt};

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

    fn warning_world(source: &str) -> (tempfile::TempDir, crate::TypstWorld) {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("site.typ");
        std::fs::write(&main, source).unwrap();
        let mut library = Library::builder()
            .with_features(Features::from_iter([Feature::Html, Feature::Bundle]))
            .build();
        library
            .global
            .scope_mut()
            .define("warn-path", Func::from(warn_path::data()));
        let world = crate::TypstWorld::builder(&main, directory.path())
            .with_local_cache()
            .no_fonts()
            .with_shared_library(Arc::new(LazyHash::new(library)))
            .build(&crate::BundleCancellation::default())
            .unwrap();
        (directory, world)
    }

    #[test]
    fn memoized_warnings_keep_native_identity() {
        let (_directory, world) =
            warning_world("#for n in range(12) { warn-path(n) }\n#document(\"index.html\")[Body]");
        NATIVE_RUNS.with(|runs| runs.set(0));
        let cancellation = crate::BundleCancellation::default();
        let first = crate::compile_bundle_world(&world, &cancellation).unwrap();
        assert_eq!(NATIVE_RUNS.with(Cell::get), 12);
        let second = crate::compile_bundle_world(&world, &cancellation).unwrap();
        assert_eq!(NATIVE_RUNS.with(Cell::get), 12);
        let warnings = first
            .diagnostics()
            .filter(|diagnostic| matches!(diagnostic.origin, DiagnosticOrigin::Producer(_)));
        let expected = (0..12)
            .map(|number| {
                DiagnosticOrigin::Producer(ProducerDiagnosticOrigin {
                    name: "SampleWarning".into(),
                    fields: Arc::new(vec![("number".into(), number.to_string().into())]),
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            warnings
                .iter()
                .map(|diagnostic| diagnostic.origin.clone())
                .collect::<Vec<_>>(),
            expected,
        );
        assert_eq!(first.diagnostics().raw(), second.diagnostics().raw());
        assert!(
            warnings
                .raw()
                .iter()
                .all(|native| native.source().span.id() == Some(world.main()))
        );
        let resolved = crate::Diagnostics::resolve_with_options(
            &world,
            warnings.raw(),
            crate::SourceContextLimit {
                max_lines: 0,
                max_line_bytes: 0,
            },
        );
        assert_eq!(
            resolved
                .iter()
                .map(|diagnostic| diagnostic.origin.clone())
                .collect::<Vec<_>>(),
            expected,
        );
        assert_eq!(
            resolved
                .iter()
                .map(|diagnostic| diagnostic.location.range)
                .collect::<Vec<_>>(),
            warnings
                .iter()
                .map(|diagnostic| diagnostic.location.range)
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn nonfinal_realization_drops_warning() {
        let (_directory, world) = warning_world(
            "#document(\"index.html\")[#context { if query(heading).len() == 0 { warn-path(99) }; heading[Title] }]",
        );
        let compiled =
            crate::compile_bundle_world(&world, &crate::BundleCancellation::default()).unwrap();
        assert!(
            compiled
                .diagnostics()
                .iter()
                .all(|diagnostic| { diagnostic.origin == DiagnosticOrigin::Typst })
        );
    }

    #[test]
    fn failed_evaluation_retains_warning_origin() {
        let (_directory, world) =
            warning_world("#warn-path(3)\n#panic(\"different warning wording 3\")");
        let failed =
            crate::compile_bundle_world(&world, &crate::BundleCancellation::default()).unwrap_err();
        let diagnostics = failed.diagnostics().unwrap();
        let error = diagnostics.errors().next().unwrap();
        assert_eq!(error.origin, DiagnosticOrigin::Typst);
        assert_eq!(error.location.path.as_deref(), Some("site.typ"));
        let warning = diagnostics
            .warnings()
            .find(|diagnostic| matches!(diagnostic.origin, DiagnosticOrigin::Producer(_)))
            .unwrap();
        assert_eq!(warning.location.line, Some(1));
        assert_eq!(warning.location.path.as_deref(), Some("site.typ"));
    }
}
