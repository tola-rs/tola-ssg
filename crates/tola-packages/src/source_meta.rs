//! The native source declaration and the protocol that carries it to the build.
//!
//! A source declares its metadata by calling `tola-meta(...)` while it is evaluated. The call
//! returns a labelled metadata marker for the document the author writes, and appends one
//! declaration event to the tracked sink of the scan it runs in, which a host reads whether or
//! not the source program ran to completion. A capture header, which the host writes before
//! evaluation, opens that channel, and [`decode_capture`] reads the header and the events back.
//!
//! Each event retains the marker's source span separately from the source it declares for.

use std::ops::Range;

use tola_typst::CapturedValues;
use typst::World;
use typst::diag::SourceResult;
use typst::ecow::EcoString;
use typst::engine::Engine;
use typst::foundations::{Content, Dict, Label, NativeElement, Str, Value, func};
use typst::introspection::MetadataElem;
use typst::syntax::{FileId, RootedPath, Source, Span, SpanKind};
use typst::utils::PicoStr;

/// The protocol every declaration event carries.
const DECLARATION_PROTOCOL: &str = "tola.source-declaration/2";

/// The protocol every capture header carries.
const CAPTURE_PROTOCOL: &str = "tola.source-capture/1";

/// The label a declaration marker carries, which is what `query(<tola-meta>)` reads it by.
const DECLARATION_LABEL: &str = "tola-meta";

// The names the wire spells its fields with: one authority for what the native writes, what a
// host writes, and what the decoder reads.
const PROTOCOL: &str = "protocol";
const OWNER: &str = "owner";
const VALUE: &str = "value";
const INPUT_IDENTITY: &str = "input_identity";

/// Declare the metadata of the source that calls this; only eager evaluation declares.
#[func]
pub(super) fn tola_meta(
    engine: &mut Engine,
    span: Span,
    /// The declared metadata: the source's own view of itself.
    payload: Dict,
) -> SourceResult<Content> {
    let marker = declaration_marker(payload, span);
    let owner = owner_of(engine.world.main());
    tola_typst::capture_source_value(engine, span, declaration_event(&owner, marker.clone()));
    Ok(marker)
}

/// The header a host writes into a scan's sink before it evaluates one source.
///
/// The header keeps the first record of the channel. Its equality is what [`decode_capture`]
/// checks, so a capture that lost the header — to a full channel, or to another producer — is
/// refused rather than read as a declaration-free source.
#[derive(Debug, Clone)]
pub struct CaptureHeader {
    owner: EcoString,
    input_identity: EcoString,
}

impl CaptureHeader {
    /// The header `source` is captured under; `input_identity` returns unchanged.
    pub fn new(source: FileId, input_identity: &str) -> Self {
        Self {
            owner: owner_of(source),
            input_identity: input_identity.into(),
        }
    }

    /// The record a host writes into the sink before evaluation.
    pub fn value(&self) -> Value {
        let mut header = Dict::new();
        header.insert(Str::from(PROTOCOL), Value::Str(CAPTURE_PROTOCOL.into()));
        header.insert(Str::from(OWNER), Value::Str(self.owner.clone().into()));
        header.insert(
            Str::from(INPUT_IDENTITY),
            Value::Str(self.input_identity.clone().into()),
        );
        Value::Dict(header)
    }
}

/// What one source's capture says: the input it was taken under, and the declaration it made.
#[derive(Debug)]
pub struct SourceCapture {
    /// The identity of the input the scan was taken under.
    pub input_identity: EcoString,
    /// The declaration the source made for the round this capture belongs to.
    pub declaration: Declaration,
}

/// The declaration one source made in one round.
#[derive(Debug)]
pub enum Declaration {
    /// The source declared nothing.
    Absent,
    /// The source declared once.
    One(DeclaredMeta),
    /// The source declared more than once, which no round resolves: the caller reports every
    /// position instead of choosing between them.
    Duplicate(Vec<DeclaredMeta>),
}

/// One declaration: the dictionary a source declared and the call site that declared it.
#[derive(Debug)]
pub struct DeclaredMeta {
    /// The declared metadata.
    pub metadata: Dict,
    /// The file containing the declaration, which can be a shared helper.
    pub file: RootedPath,
    /// The byte range of the declaration within `file`.
    pub range: Range<usize>,
}

/// Read one source's declaration out of the values a scan captured.
///
/// The header and every event must name `source`. Declaration locations use captured source
/// snapshots, so a helper's span cannot be resolved against its caller or a later file version.
///
/// A channel that reached [`typst::engine::Sink::MAX_VALUES`] records is always refused: the sink
/// drops the writes beyond its capacity without a trace, so a full channel cannot say whether it
/// holds every record or only the first ones.
pub fn decode_capture(
    captured: &CapturedValues,
    header: &CaptureHeader,
    source: &Source,
) -> Result<SourceCapture, CaptureError> {
    if captured.is_saturated() {
        return Err(CaptureError::Saturated);
    }

    let owner = owner_of(source.id());
    let mut records = captured.records().iter();
    let Some((first, styles)) = records.next() else {
        return Err(CaptureError::MissingHeader);
    };
    if styles.is_some() || first != &header.value() {
        return Err(CaptureError::MissingHeader);
    }
    if header.owner != owner {
        return Err(CaptureError::ForeignOwner {
            expected: owner.clone(),
            found: header.owner.clone(),
        });
    }

    let mut declared = Vec::new();
    for (record, styles) in records {
        if styles.is_some() {
            return Err(CaptureError::Styled);
        }
        let Value::Dict(record) = record else {
            return Err(CaptureError::NotADictionary);
        };
        match string_field(record, PROTOCOL)? {
            DECLARATION_PROTOCOL => {}
            CAPTURE_PROTOCOL => return Err(CaptureError::MisplacedHeader),
            found => {
                return Err(CaptureError::UnknownProtocol {
                    found: found.into(),
                });
            }
        }
        let named = string_field(record, OWNER)?;
        if named != owner.as_str() {
            return Err(CaptureError::ForeignOwner {
                expected: owner,
                found: named.into(),
            });
        }
        let Ok(Value::Content(marker)) = record.get(VALUE) else {
            return Err(CaptureError::Malformed { field: VALUE });
        };
        let Some(element) = marker.to_packed::<MetadataElem>() else {
            return Err(CaptureError::Malformed { field: VALUE });
        };
        let Value::Dict(metadata) = &element.value else {
            return Err(CaptureError::Malformed { field: VALUE });
        };
        let span = marker.span();
        let file = span.id().ok_or(CaptureError::UnlocatedDeclaration)?;
        let source = captured
            .source(file)
            .ok_or(CaptureError::UnlocatedDeclaration)?;
        let range = call_range(source, span)
            .filter(|range| range.start <= range.end && range.end <= source.text().len())
            .ok_or(CaptureError::UnlocatedDeclaration)?;
        declared.push(DeclaredMeta {
            metadata: metadata.clone(),
            file: file.get().clone(),
            range,
        });
    }

    let declaration = match declared.len() {
        0 => Declaration::Absent,
        1 => Declaration::One(declared.remove(0)),
        _ => Declaration::Duplicate(declared),
    };
    Ok(SourceCapture {
        input_identity: header.input_identity.clone(),
        declaration,
    })
}

/// Why one capture cannot be read as a source's declaration.
///
/// Every variant refuses the whole capture: the caller has no declaration to read, not an absent
/// one.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// The capture does not start with the header the host wrote: the channel received nothing,
    /// or a write took the first record.
    #[error(
        "the scan captured nothing in front of its `{}` header",
        CAPTURE_PROTOCOL
    )]
    MissingHeader,
    /// A record's protocol is not the one its place in the capture allows, including another
    /// version of a protocol this decoder reads.
    #[error("the scan captured a `{found}` record where the protocol allows none")]
    UnknownProtocol {
        /// The protocol the record named.
        found: EcoString,
    },
    /// A capture header follows the records it must precede.
    #[error("a capture header follows the records it must precede")]
    MisplacedHeader,
    /// A record is not a dictionary, so it carries no protocol fields.
    #[error("a captured value is not a protocol record")]
    NotADictionary,
    /// A record misses a field the protocol gives it, or carries another type there.
    #[error("a protocol record misses its `{field}`")]
    Malformed {
        /// The field that is missing or has another type.
        field: &'static str,
    },
    /// A record carried styles, which no protocol record does.
    #[error("a protocol record carried styles")]
    Styled,
    /// The capture names a source other than the one being decoded.
    #[error("the capture belongs to `{found}`, not `{expected}`")]
    ForeignOwner {
        /// The source the caller is decoding for.
        expected: EcoString,
        /// The source the record named.
        found: EcoString,
    },
    /// A declaration's span cannot be located in its own source.
    #[error("a declaration has no readable source location")]
    UnlocatedDeclaration,
    /// The channel held as many records as it can, so writes may have been dropped and no
    /// declaration can be read from what remains.
    #[error("the capture channel is full, so a declaration may have been dropped")]
    Saturated,
}

/// One string field every protocol record carries.
fn string_field<'a>(record: &'a Dict, field: &'static str) -> Result<&'a str, CaptureError> {
    match record.get(field) {
        Ok(Value::Str(value)) => Ok(value.as_str()),
        _ => Err(CaptureError::Malformed { field }),
    }
}

/// The owner one source file is named by on the wire: its site-root path with a leading slash.
fn owner_of(source: FileId) -> EcoString {
    source.vpath().get_with_slash().into()
}

/// The byte range this call site's span names in `source`.
fn call_range(source: &Source, span: Span) -> Option<Range<usize>> {
    match span.get() {
        SpanKind::Number { num, .. } => source.range(num, None),
        SpanKind::Range { range, .. } => Some(range),
        SpanKind::Detached => None,
    }
}

/// The marker a declaration call returns: the declared dictionary as a labelled metadata element
/// at the call site.
fn declaration_marker(payload: Dict, span: Span) -> Content {
    MetadataElem::new(Value::Dict(payload))
        .pack()
        .labelled(declaration_label())
        .spanned(span)
}

/// The declaration label as a value. The constant is never empty, so it always forms a label.
fn declaration_label() -> Label {
    Label::new(PicoStr::intern(DECLARATION_LABEL)).expect("the declaration label is not empty")
}

/// One declaration event, as the wire encodes it.
fn declaration_event(owner: &str, marker: Content) -> Value {
    let mut event = Dict::new();
    event.insert(Str::from(PROTOCOL), Value::Str(DECLARATION_PROTOCOL.into()));
    event.insert(Str::from(OWNER), Value::Str(owner.into()));
    event.insert(Str::from(VALUE), Value::Content(marker));
    Value::Dict(event)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use tola_typst::{
        FileMap, FileProvider, FileResolver, FileTarget, ObservedScan, TypstWorld, file_id,
        scan_world_observed,
    };
    use typst::Library;
    use typst::engine::Sink;
    use typst::foundations::{Array, Styles};
    use typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};
    use typst::utils::LazyHash;

    use super::*;
    use crate::TolaPackage;
    use crate::builtin::TOLA_NAMESPACE;
    use crate::library::{HostInputs, SiteLibrary};

    /// `tola-meta` imported from the package an author imports it from.
    const IMPORT: &str = "#import \"@tola/source:0.0.0\": tola-meta\n";

    /// The header one host writes before it evaluates `main.typ`.
    fn header() -> CaptureHeader {
        CaptureHeader::new(file_id("main.typ"), "input-1")
    }

    /// The observed scan of one site whose main is `main.typ`, header included.
    fn scan(files: &[(&str, &str)], records: Vec<Value>) -> (TypstWorld, ObservedScan) {
        let mut written = vec![header().value()];
        written.extend(records);
        let world = site(files);
        let scanned = scan_world_observed(&world, written);
        (world, scanned)
    }

    /// The declaration of the source one scan evaluated.
    fn decode(world: &TypstWorld, scan: &ObservedScan) -> Result<SourceCapture, CaptureError> {
        let source = scan.captured().source(world.main()).unwrap();
        decode_capture(scan.captured(), &header(), source)
    }

    /// One virtual site: `files` are its sources, and the builtin packages serve themselves.
    ///
    /// The world's main is `main.typ`; no path below its root exists on disk.
    fn site(files: &[(&str, &str)]) -> TypstWorld {
        let mut sources = FileMap::new();
        for (path, text) in files {
            sources.insert(file_id(path), text.as_bytes().to_vec());
        }
        virtual_site(sources, SiteLibrary::new(host_inputs()).shared())
    }

    /// The world one in-memory site is scanned in, with the library a caller binds.
    fn virtual_site(sources: FileMap, library: Arc<LazyHash<Library>>) -> TypstWorld {
        let root = std::env::temp_dir().join("tola-source-meta");
        let main = root.join("main.typ");
        TypstWorld::builder(&main, &root)
            .with_files(Arc::new(
                FileResolver::new().with_provider(SiteFiles(sources)),
            ))
            .with_local_cache()
            .no_fonts()
            .with_shared_library(library)
            .build(&tola_typst::BundleCancellation::default())
            .unwrap()
    }

    /// The host bindings of one build: every site-specific value is empty.
    fn host_inputs() -> HostInputs {
        HostInputs {
            site: Dict::new(),
            modules: Dict::new(),
            asset_urls: Dict::new(),
            asset_origins: Dict::new(),
            source_records: Array::new(),
            source_records_by_file: Dict::new(),
            source_origins: Dict::new(),
        }
    }

    /// Virtual files for one test site: the site's own sources, then the builtin packages.
    struct SiteFiles(FileMap);

    impl FileProvider for SiteFiles {
        fn target(&self, id: FileId) -> Option<FileTarget> {
            if let Some(target) = self.0.target(id) {
                return Some(target);
            }
            let VirtualRoot::Package(spec) = id.root() else {
                return None;
            };
            let path = Path::new(id.vpath().get_with_slash().trim_start_matches('/'));
            TolaPackage::from_spec(spec)
                .and_then(|package| package.file(path))
                .map(|bytes| FileTarget::Bytes(Arc::from(bytes.into_owned().into_bytes())))
        }

        fn owned_namespaces(&self) -> &'static [&'static str] {
            &[TOLA_NAMESPACE, "tola-modules"]
        }
    }

    /// The declared dictionary holding `title`.
    fn payload(title: &str) -> Dict {
        let mut payload = Dict::new();
        payload.insert("title".into(), Value::Str(title.into()));
        payload
    }

    /// A producer that writes a record the protocol does not allow: a styles chain.
    #[func]
    fn styled(engine: &mut Engine, value: Value) -> SourceResult<Value> {
        engine.sink.value(value, Some(Styles::new()));
        Ok(Value::None)
    }
    #[test]
    fn natives_use_calling_alias() {
        let package: tola_typst::PackageSpec = "@tola-modules/notes:0.0.0".parse().unwrap();
        let package_file = |path: &str| {
            FileId::new(RootedPath::new(
                VirtualRoot::Package(package.clone()),
                VirtualPath::new(path).unwrap(),
            ))
        };
        let mut files = FileMap::new();
        files.insert(
            file_id("main.typ"),
            br#"
#import "@tola/module:0.0.0": get-module
#import "@tola-modules/notes:0.0.0": chosen
#assert.eq(get-module("notes").name, "notes")
#assert.eq(chosen.name, "notes")
"#
            .to_vec(),
        );
        files.insert(
            package_file("/typst.toml"),
            b"[package]\nname=\"notes\"\nversion=\"0.0.0\"\nentrypoint=\"lib.typ\"\n".to_vec(),
        );
        files.insert(
            package_file("/lib.typ"),
            br#"
#import "@tola/module:0.0.0": get-module
#let chosen = get-module()
"#
            .to_vec(),
        );
        let mut record = Dict::new();
        record.insert("name".into(), Value::Str("notes".into()));
        let mut inputs = host_inputs();
        inputs.modules.insert("notes".into(), Value::Dict(record));
        let world = virtual_site(files, SiteLibrary::new(inputs).shared());
        let scan = scan_world_observed(&world, []);
        assert!(scan.result().is_ok(), "{:?}", scan.result().err());
    }

    #[test]
    fn lookup_rejects_unknown_alias() {
        for (expression, expected) in [
            (
                "get-module()",
                "must be called from a configured module package",
            ),
            ("get-module(auto)", "found auto"),
            (
                "get-module(\"missing\")",
                "module `missing` is not configured",
            ),
        ] {
            let text = format!("#import \"@tola/module:0.0.0\": get-module\n#{expression}");
            let (_, result) = scan(&[("main.typ", &text)], Vec::new());
            assert!(
                result.result().is_err(),
                "{expression} unexpectedly succeeded"
            );
            let diagnostics = format!("{:?}", result.result().err());
            assert!(diagnostics.contains(expected), "{diagnostics}");
        }
    }

    #[test]
    fn uuid_natives_match_rfc_vector() {
        let text = r#"
#import "@tola/uuid:0.0.0": namespace, normalize, v5
#assert.eq(normalize("550E8400E29B41D4A716446655440000"), "550e8400-e29b-41d4-a716-446655440000")
#assert.eq(normalize("{550E8400-E29B-41D4-A716-446655440000}"), "550e8400-e29b-41d4-a716-446655440000")
#assert.eq(normalize("urn:uuid:550e8400-e29b-41d4-a716-446655440000"), "550e8400-e29b-41d4-a716-446655440000")
#assert.eq(v5(namespace.dns, "www.widgets.com"), "21f7f8de-8051-5b89-8680-0195ef798b6a")
#assert.eq(v5(namespace.dns, "中文 é"), v5(namespace.dns, bytes("中文 é")))
#assert(v5(namespace.dns, "é") != v5(namespace.dns, "é"))
#assert.eq(namespace.url, "6ba7b811-9dad-11d1-80b4-00c04fd430c8")
#assert.eq(namespace.oid, "6ba7b812-9dad-11d1-80b4-00c04fd430c8")
#assert.eq(namespace.x500, "6ba7b814-9dad-11d1-80b4-00c04fd430c8")
"#;
        let (_, result) = scan(&[("main.typ", text)], Vec::new());
        assert!(result.result().is_ok(), "{:?}", result.result().err());
    }

    #[test]
    fn uuid_natives_reject_invalid_values() {
        for expression in [
            "normalize(\"invalid\")",
            "normalize(\" 550e8400-e29b-41d4-a716-446655440000\")",
            "v5(\"invalid\", \"x\")",
            "v5(namespace.dns, 1)",
        ] {
            let text =
                format!("#import \"@tola/uuid:0.0.0\": namespace, normalize, v5\n#{expression}");
            let (_, result) = scan(&[("main.typ", &text)], Vec::new());
            assert!(
                result.result().is_err(),
                "{expression} unexpectedly succeeded"
            );
        }
    }

    #[test]
    fn own_call_records_one_labelled_declaration() {
        let text = IMPORT.to_owned() + "#tola-meta((title: \"Post\"))";
        let (world, scan) = scan(&[("main.typ", &text)], Vec::new());
        let capture = decode(&world, &scan).unwrap();
        let Declaration::One(declared) = capture.declaration else {
            panic!("the source declares once");
        };
        assert_eq!(
            declared.metadata.get("title"),
            Ok(&Value::Str("Post".into()))
        );
        let result = scan.result().expect("the source evaluates");
        assert_eq!(
            &result.source().text()[declared.range],
            "tola-meta((title: \"Post\"))"
        );

        let declarations = result.metadata_declarations(DECLARATION_LABEL);
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].value(), &Value::Dict(payload("Post")));
        let range = result
            .source()
            .find(declarations[0].span())
            .unwrap()
            .range();
        assert_eq!(
            &result.source().text()[range],
            "tola-meta((title: \"Post\"))"
        );
    }

    #[test]
    fn wrapper_retains_declaration_source() {
        let helper = IMPORT.to_owned() + "#let note(..fields) = tola-meta(fields.named())";
        let (world, scan) = scan(
            &[
                (
                    "main.typ",
                    "#import \"theme.typ\": note\n#note(title: \"Post\")",
                ),
                ("theme.typ", &helper),
            ],
            Vec::new(),
        );
        let Declaration::One(declared) = decode(&world, &scan).unwrap().declaration else {
            panic!("the wrapper declares once");
        };
        assert_eq!(
            declared.metadata.get("title"),
            Ok(&Value::Str("Post".into()))
        );
        assert_eq!(declared.file, file_id("theme.typ").get().clone());
        assert_eq!(&helper[declared.range], "tola-meta(fields.named())");
    }

    #[test]
    fn reused_marker_declares_once() {
        let text =
            IMPORT.to_owned() + "#let marker = tola-meta((title: \"Post\"))\n#marker\n#marker\n";
        let (world, scan) = scan(&[("main.typ", &text)], Vec::new());
        assert!(matches!(
            decode(&world, &scan).unwrap().declaration,
            Declaration::One(_)
        ));
    }

    #[test]
    fn foreign_owner_is_refused() {
        let text = IMPORT.to_owned() + "#tola-meta((title: \"Post\"))";
        let foreign = declaration_event(
            "/other.typ",
            declaration_marker(payload("Post"), Span::detached()),
        );
        let (world, scan) = scan(&[("main.typ", "= Post\n")], vec![foreign]);
        assert!(matches!(
            decode(&world, &scan),
            Err(CaptureError::ForeignOwner { .. })
        ));

        let other = CaptureHeader::new(file_id("other.typ"), "input-1");
        let world = site(&[("main.typ", &text)]);
        let scan = scan_world_observed(&world, [other.value()]);
        assert!(matches!(
            decode_capture(scan.captured(), &other, scan.result().unwrap().source()),
            Err(CaptureError::ForeignOwner { .. })
        ));
    }
    /// `decode_capture` reads the header as the first record and every later one as an event, so a
    /// channel that kept its last records instead would read an event as the header.
    #[test]
    fn full_channel_keeps_earliest_records() {
        let mut text = IMPORT.to_owned();
        for index in 0..Sink::MAX_VALUES + 2 {
            text.push_str(&format!("#tola-meta((index: {index}))\n"));
        }
        let (_world, scan) = scan(&[("main.typ", &text)], Vec::new());
        let records = scan.captured().records();
        assert_eq!(records.len(), Sink::MAX_VALUES, "the sink stops at its cap");

        let (first, styles) = &records[0];
        assert_eq!(styles, &None, "the header carries no styles");
        assert_eq!(
            first,
            &header().value(),
            "the header keeps the first record"
        );

        let declared = records[1..]
            .iter()
            .map(|(value, styles)| {
                assert_eq!(styles, &None, "a declaration carries no styles");
                let Value::Dict(event) = value else {
                    panic!("a declaration event is a dictionary");
                };
                let Ok(Value::Content(marker)) = event.get(VALUE) else {
                    panic!("a declaration event holds its marker");
                };
                let metadata = &marker.to_packed::<MetadataElem>().unwrap().value;
                let Value::Dict(metadata) = metadata else {
                    panic!("a declaration marker holds its dictionary");
                };
                let Ok(Value::Int(index)) = metadata.get("index") else {
                    panic!("a declaration event holds the index it was written with");
                };
                *index
            })
            .collect::<Vec<_>>();
        assert_eq!(
            declared,
            (0..Sink::MAX_VALUES - 1)
                .map(|index| index as i64)
                .collect::<Vec<_>>(),
            "the records the channel keeps are the ones written first, in order"
        );
    }
}
