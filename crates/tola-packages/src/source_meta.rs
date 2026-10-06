//! The native source declaration and the protocol that carries it to the build.
//!
//! A source declares its metadata by calling `tola-meta(...)` while it is evaluated. The call
//! returns a labelled metadata marker for the document the author writes, and appends one
//! declaration event to the tracked sink of the scan it runs in, which a host reads whether or
//! not the source program ran to completion. A capture header, which the host writes before
//! evaluation, opens that channel, and [`decode_capture`] reads the header and the events back.
//!
//! The channel carries `(value, styles)` and no span, so an event carries the byte range of its
//! own call site inside its value.

use std::fmt;
use std::ops::Range;

use tola_typst::CapturedValues;
use typst::World;
use typst::diag::{At, SourceResult, bail};
use typst::ecow::EcoString;
use typst::engine::Engine;
use typst::foundations::{Content, Dict, Label, NativeElement, Str, Value, func};
use typst::introspection::MetadataElem;
use typst::syntax::{FileId, Source, Span, SpanKind};
use typst::utils::PicoStr;

/// The protocol every declaration event carries.
pub const DECLARATION_PROTOCOL: &str = "tola.source-declaration/1";

/// The protocol every capture header carries.
pub const CAPTURE_PROTOCOL: &str = "tola.source-capture/1";

/// The label a declaration marker carries, which is what `query(<tola-meta>)` reads it by.
pub const DECLARATION_LABEL: &str = "tola-meta";

// The names the wire spells its fields with: one authority for what the native writes, what a
// host writes, and what the decoder reads.
const PROTOCOL: &str = "protocol";
const OWNER: &str = "owner";
const SPAN: &str = "span";
const VALUE: &str = "value";
const MODE: &str = "mode";
const INPUT_IDENTITY: &str = "input_identity";
const START: &str = "start";
const END: &str = "end";

/// The mode one capture is taken in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    /// Record the declarations of the source being scanned.
    Collect,
    /// Compare a declaration against a closed source instead of recording it.
    ///
    /// Named by the protocol for the render phase; [`decode_capture`] implements collection only.
    RenderClosed,
    /// Inspect a source without recording or accepting anything.
    ///
    /// Named by the protocol for editor inspection; [`decode_capture`] implements collection
    /// only.
    Inspect,
}

impl CaptureMode {
    /// The name this mode carries on the wire.
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Collect => "collect",
            Self::RenderClosed => "render-closed",
            Self::Inspect => "inspect",
        }
    }
}

/// Declare the metadata of the source that calls this.
///
/// The call is the declaration: the dictionary, at the byte range of this call site, is recorded
/// in the tracked sink of the scan this runs in, so a build reads it whether or not the rest of
/// the source program succeeds. Only a call written in the file being scanned declares; a helper
/// in another file names its own file, and the scan of that file is what reads it.
///
/// The returned marker is an invisible metadata element carrying the same dictionary at this call
/// site, labelled `tola-meta` so `query(<tola-meta>)` still finds it. Placing the marker any
/// number of times declares nothing further.
#[func]
pub(super) fn tola_meta(
    engine: &mut Engine,
    span: Span,
    /// The declared metadata: the source's own view of itself.
    payload: Dict,
) -> SourceResult<Content> {
    let main = engine.world.main();
    if span.id() == Some(main) {
        let source = engine.world.source(main).at(span)?;
        let Some(range) = call_range(&source, span) else {
            bail!(
                span,
                "this `tola-meta` call cannot be located in the source being scanned"
            );
        };
        let owner = owner_of(main);
        engine
            .sink
            .value(declaration_event(&owner, range, payload.clone()), None);
    }
    Ok(declaration_marker(payload, span))
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
    mode: CaptureMode,
}

impl CaptureHeader {
    /// The header `source` is captured under.
    ///
    /// `input_identity` is the host's name for the input this scan is taken under; the decoder
    /// carries it back unchanged.
    pub fn new(source: FileId, input_identity: &str, mode: CaptureMode) -> Self {
        Self {
            owner: owner_of(source),
            input_identity: input_identity.into(),
            mode,
        }
    }

    /// The record a host writes into the sink before evaluation.
    pub fn value(&self) -> Value {
        let mut header = Dict::new();
        header.insert(Str::from(PROTOCOL), Value::Str(CAPTURE_PROTOCOL.into()));
        header.insert(Str::from(MODE), Value::Str(self.mode.wire().into()));
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
    /// The byte range of the declaring call site in the source.
    pub range: Range<usize>,
}

/// Read one source's declaration out of the values a scan captured.
///
/// `header` is the header the host wrote before evaluation and `source` is the file it evaluated:
/// the channel must still start with that header, its owner must be this source, and every
/// declaration must lie inside it. Any other shape refuses the whole capture, so a caller never
/// reads a protocol failure as "this source declared nothing".
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
    if header.mode != CaptureMode::Collect {
        return Err(CaptureError::UnsupportedMode {
            mode: header.mode.wire().into(),
        });
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
        let Ok(Value::Dict(span)) = record.get(SPAN) else {
            return Err(CaptureError::Malformed { field: SPAN });
        };
        let start = match span.get(START) {
            Ok(Value::Int(start)) => *start,
            _ => return Err(CaptureError::Malformed { field: START }),
        };
        let end = match span.get(END) {
            Ok(Value::Int(end)) => *end,
            _ => return Err(CaptureError::Malformed { field: END }),
        };
        if start < 0 || end < start || (end as u64) > source.text().len() as u64 {
            return Err(CaptureError::SpanOutside { start, end });
        }
        let Ok(Value::Dict(metadata)) = record.get(VALUE) else {
            return Err(CaptureError::Malformed { field: VALUE });
        };
        declared.push(DeclaredMeta {
            metadata: metadata.clone(),
            range: start as usize..end as usize,
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
#[derive(Debug)]
pub enum CaptureError {
    /// The capture does not start with the header the host wrote: the channel received nothing,
    /// or a write took the first record.
    MissingHeader,
    /// A record's protocol is not the one its place in the capture allows, including another
    /// version of a protocol this decoder reads.
    UnknownProtocol {
        /// The protocol the record named.
        found: EcoString,
    },
    /// A capture header follows the records it must precede.
    MisplacedHeader,
    /// A record is not a dictionary, so it carries no protocol fields.
    NotADictionary,
    /// A record misses a field the protocol gives it, or carries another type there.
    Malformed {
        /// The field that is missing or has another type.
        field: &'static str,
    },
    /// A record carried styles, which no protocol record does.
    Styled,
    /// The capture names a source other than the one being decoded.
    ForeignOwner {
        /// The source the caller is decoding for.
        expected: EcoString,
        /// The source the record named.
        found: EcoString,
    },
    /// The capture was taken in a mode this decoder does not implement.
    UnsupportedMode {
        /// The mode the capture was taken in.
        mode: EcoString,
    },
    /// A declared position is not a byte range inside the source being decoded.
    SpanOutside {
        /// The first byte the record declared.
        start: i64,
        /// The byte after the last one the record declared.
        end: i64,
    },
    /// The channel held as many records as it can, so writes may have been dropped and no
    /// declaration can be read from what remains.
    Saturated,
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHeader => write!(
                formatter,
                "the scan captured nothing in front of its `{CAPTURE_PROTOCOL}` header"
            ),
            Self::UnknownProtocol { found } => write!(
                formatter,
                "the scan captured a `{found}` record where the protocol allows none"
            ),
            Self::MisplacedHeader => {
                write!(
                    formatter,
                    "a capture header follows the records it must precede"
                )
            }
            Self::NotADictionary => write!(formatter, "a captured value is not a protocol record"),
            Self::Malformed { field } => {
                write!(formatter, "a protocol record misses its `{field}`")
            }
            Self::Styled => write!(formatter, "a protocol record carried styles"),
            Self::ForeignOwner { expected, found } => write!(
                formatter,
                "the capture belongs to `{found}`, not `{expected}`"
            ),
            Self::UnsupportedMode { mode } => write!(
                formatter,
                "the capture was taken in mode `{mode}`, which cannot collect declarations"
            ),
            Self::SpanOutside { start, end } => write!(
                formatter,
                "a declaration at bytes {start}..{end} lies outside the source it declares in"
            ),
            Self::Saturated => write!(
                formatter,
                "the capture channel is full, so a declaration may have been dropped"
            ),
        }
    }
}

impl std::error::Error for CaptureError {}

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
fn declaration_event(owner: &str, range: Range<usize>, payload: Dict) -> Value {
    let mut span = Dict::new();
    span.insert(Str::from(START), Value::Int(range.start as i64));
    span.insert(Str::from(END), Value::Int(range.end as i64));

    let mut event = Dict::new();
    event.insert(Str::from(PROTOCOL), Value::Str(DECLARATION_PROTOCOL.into()));
    event.insert(Str::from(OWNER), Value::Str(owner.into()));
    event.insert(Str::from(SPAN), Value::Dict(span));
    event.insert(Str::from(VALUE), Value::Dict(payload));
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
    use typst::foundations::{Array, Func, NativeFunc, Styles};
    use typst::syntax::VirtualRoot;
    use typst::utils::LazyHash;
    use typst::{Library, LibraryExt};

    use super::*;
    use crate::TolaPackage;
    use crate::builtin::TOLA_NAMESPACE;
    use crate::library::{HostInputs, SiteLibrary};

    /// `tola-meta` imported from the package an author imports it from.
    const IMPORT: &str = "#import \"@tola/source:0.0.0\": tola-meta\n";

    /// The header one host writes before it evaluates `main.typ`.
    fn header() -> CaptureHeader {
        CaptureHeader::new(file_id("main.typ"), "input-1", CaptureMode::Collect)
    }

    /// The observed scan of one site whose main is `main.typ`, header included.
    fn scan(files: &[(&str, &str)], records: Vec<Value>) -> ObservedScan {
        let mut written = vec![header().value()];
        written.extend(records);
        scan_world_observed(&site(files), written)
    }

    /// The declaration of the source one scan evaluated.
    fn decode(scan: &ObservedScan) -> Result<SourceCapture, CaptureError> {
        decode_with(scan, &header())
    }

    /// The declaration one scan made, read against the header its host wrote.
    fn decode_with(
        scan: &ObservedScan,
        header: &CaptureHeader,
    ) -> Result<SourceCapture, CaptureError> {
        let source = scan.result().expect("the source evaluates").source();
        decode_capture(scan.captured(), header, source)
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
            &[TOLA_NAMESPACE]
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
    fn refused_call_writes_no_event() {
        for call in ["#tola-meta(1)", "#tola-meta((:), extra: 1)"] {
            let text = IMPORT.to_owned() + call;
            let world = site(&[("main.typ", &text)]);
            let scan = scan_world_observed(&world, [header().value()]);
            assert!(scan.result().is_err(), "the wrapper refuses `{call}`");
            let source = world.source(world.main()).expect("the main source reads");
            let capture = decode_capture(scan.captured(), &header(), &source).unwrap();
            assert!(
                matches!(capture.declaration, Declaration::Absent),
                "`{call}` wrote no event"
            );
        }
    }

    #[test]
    fn imported_call_records_nothing() {
        let scan = scan(
            &[
                (
                    "main.typ",
                    "#import \"helper.typ\": declare\n#declare((origin: \"helper\"))\n",
                ),
                (
                    "helper.typ",
                    &(IMPORT.to_owned() + "#let declare(value) = tola-meta(value)\n"),
                ),
            ],
            Vec::new(),
        );
        let result = scan.result().expect("the helper runs");
        let declarations = result.metadata_declarations(DECLARATION_LABEL);
        assert_eq!(declarations.len(), 1, "the helper's call ran");
        assert_eq!(declarations[0].span().id(), Some(file_id("helper.typ")));
        assert!(matches!(
            decode(&scan).unwrap().declaration,
            Declaration::Absent
        ));
    }

    #[test]
    fn own_call_records_declaration() {
        let text = IMPORT.to_owned() + "#tola-meta((title: \"Post\"))";
        let scan = scan(&[("main.typ", &text)], Vec::new());
        let capture = decode(&scan).unwrap();
        let declared = match capture.declaration {
            Declaration::One(declared) => declared,
            other => panic!("expected one declaration, decoded {other:?}"),
        };
        assert_eq!(
            declared.metadata.get("title"),
            Ok(&Value::Str("Post".into()))
        );
        let source = scan.result().unwrap().source();
        assert_eq!(
            &source.text()[declared.range],
            "tola-meta((title: \"Post\"))"
        );
    }

    #[test]
    fn reused_marker_declares_once() {
        let text =
            IMPORT.to_owned() + "#let marker = tola-meta((title: \"Post\"))\n#marker\n#marker\n";
        let scan = scan(&[("main.typ", &text)], Vec::new());
        assert!(matches!(
            decode(&scan).unwrap().declaration,
            Declaration::One(_)
        ));
    }

    #[test]
    fn declaration_marker_keeps_label_and_span() {
        let text = IMPORT.to_owned() + "#tola-meta((title: \"Post\"))";
        let scan = scan(&[("main.typ", &text)], Vec::new());
        let result = scan.result().expect("the source evaluates");
        let declarations = result.metadata_declarations(DECLARATION_LABEL);
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].value(), &Value::Dict(payload("Post")));

        let source = result.source();
        let range = source.find(declarations[0].span()).unwrap().range();
        assert_eq!(&source.text()[range], "tola-meta((title: \"Post\"))");
    }

    #[test]
    fn repeated_declaration_is_reported() {
        let text =
            IMPORT.to_owned() + "#tola-meta((title: \"First\"))\n#tola-meta((title: \"Second\"))\n";
        let scan = scan(&[("main.typ", &text)], Vec::new());
        let declared = match decode(&scan).unwrap().declaration {
            Declaration::Duplicate(declared) => declared,
            other => panic!("expected both declarations, decoded {other:?}"),
        };
        assert_eq!(declared.len(), 2);
        assert_eq!(
            declared[0].metadata.get("title"),
            Ok(&Value::Str("First".into()))
        );
        assert_eq!(
            declared[1].metadata.get("title"),
            Ok(&Value::Str("Second".into()))
        );
    }

    #[test]
    fn capture_without_host_header_is_refused() {
        let text = IMPORT.to_owned() + "#tola-meta((title: \"Post\"))";

        let absent = scan_world_observed(&site(&[("main.typ", &text)]), []);
        assert!(matches!(decode(&absent), Err(CaptureError::MissingHeader)));

        let mut older = header().value();
        let Value::Dict(record) = &mut older else {
            unreachable!()
        };
        record.insert(
            Str::from(PROTOCOL),
            Value::Str("tola.source-capture/0".into()),
        );
        let stale = scan_world_observed(&site(&[("main.typ", &text)]), [older]);
        assert!(matches!(decode(&stale), Err(CaptureError::MissingHeader)));
    }

    #[test]
    fn foreign_owner_is_refused() {
        let text = IMPORT.to_owned() + "#tola-meta((title: \"Post\"))";
        let foreign = declaration_event("/other.typ", 0..1, payload("Post"));
        let scan = scan(&[("main.typ", "= Post\n")], vec![foreign]);
        assert!(matches!(
            decode(&scan),
            Err(CaptureError::ForeignOwner { .. })
        ));

        let other = CaptureHeader::new(file_id("other.typ"), "input-1", CaptureMode::Collect);
        let scan = scan_world_observed(&site(&[("main.typ", &text)]), [other.value()]);
        assert!(matches!(
            decode_with(&scan, &other),
            Err(CaptureError::ForeignOwner { .. })
        ));
    }

    #[test]
    fn saturated_capture_is_refused() {
        let mut text = IMPORT.to_owned();
        for index in 0..10 {
            text.push_str(&format!("#tola-meta((index: {index}))\n"));
        }
        let scan = scan(&[("main.typ", &text)], Vec::new());
        assert!(
            scan.captured().is_saturated(),
            "eleven writes fill the channel"
        );
        assert!(matches!(decode(&scan), Err(CaptureError::Saturated)));
    }

    #[test]
    fn malformed_capture_is_refused() {
        let with_field = |record: Value, field: &'static str, value: Value| {
            let Value::Dict(mut record) = record else {
                panic!("a declaration event is a dictionary");
            };
            record.insert(Str::from(field), value);
            Value::Dict(record)
        };
        let cases: [(&str, Value); 5] = [
            ("a value that is not a record", Value::Int(7)),
            ("a record without a protocol", Value::Dict(Dict::new())),
            (
                "a record of another protocol version",
                with_field(
                    declaration_event("/main.typ", 0..1, payload("Post")),
                    PROTOCOL,
                    Value::Str("tola.source-declaration/0".into()),
                ),
            ),
            (
                "a declaration whose value is not a dictionary",
                with_field(
                    declaration_event("/main.typ", 0..1, payload("Post")),
                    VALUE,
                    Value::Int(1),
                ),
            ),
            (
                "a declaration outside the source",
                declaration_event("/main.typ", 0..4096, payload("Post")),
            ),
        ];
        for (case, record) in cases {
            let scan = scan(&[("main.typ", "= Post\n")], vec![record]);
            assert!(decode(&scan).is_err(), "refused {case}");
        }
    }

    #[test]
    fn stray_header_is_refused() {
        let scan = scan(&[("main.typ", "= Post\n")], vec![header().value()]);
        assert!(matches!(decode(&scan), Err(CaptureError::MisplacedHeader)));
    }

    #[test]
    fn styled_record_is_refused() {
        let mut library = Library::builder().build();
        library
            .global
            .scope_mut()
            .define("styled", Func::from(styled::data()));
        let mut sources = FileMap::new();
        sources.insert(file_id("main.typ"), &b"#styled((:))\n"[..]);
        let world = virtual_site(sources, Arc::new(LazyHash::new(library)));
        let scan = scan_world_observed(&world, [header().value()]);
        assert!(matches!(decode(&scan), Err(CaptureError::Styled)));
    }
}
