//! Source metadata declarations via `tola-meta(...)` and the scan capture protocol.
//!
//! `tola-meta` emits a labelled `<tola-meta>` metadata element and appends a declaration event to
//! the tracked scan sink. [`decode_capture`] validates the leading [`CaptureHeader`] and decodes
//! events against captured source snapshots.

use std::ops::Range;

use tola_typst::CapturedValues;
use typst::World;
use typst::diag::SourceResult;
use typst::ecow::EcoString;
use typst::engine::Engine;
use typst::foundations::{Content, Dict, Label, NativeElement, Value, func};
use typst::introspection::MetadataElem;
use typst::syntax::{FileId, RootedPath, Source, Span, SpanKind};
use typst::utils::PicoStr;

const DECLARATION_PROTOCOL: &str = "tola.source-declaration/2";
const CAPTURE_PROTOCOL: &str = "tola.source-capture/1";
const DECLARATION_LABEL: &str = "tola-meta";

const PROTOCOL: &str = "protocol";
const OWNER: &str = "owner";
const VALUE: &str = "value";
const INPUT_IDENTITY: &str = "input_identity";

/// Declares metadata for the calling source during eager evaluation.
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

/// Header written into a scan's sink before evaluating a source.
#[derive(Debug, Clone)]
pub struct CaptureHeader {
    owner: EcoString,
    input_identity: EcoString,
}

impl CaptureHeader {
    pub fn new(source: FileId, input_identity: &str) -> Self {
        Self {
            owner: owner_of(source),
            input_identity: input_identity.into(),
        }
    }

    pub fn value(&self) -> Value {
        Value::Dict(crate::dict([
            (PROTOCOL, Value::Str(CAPTURE_PROTOCOL.into())),
            (OWNER, Value::Str(self.owner.clone().into())),
            (
                INPUT_IDENTITY,
                Value::Str(self.input_identity.clone().into()),
            ),
        ]))
    }
}

/// Decoded scan capture: the input identity and declared metadata state.
#[derive(Debug)]
pub struct SourceCapture {
    pub input_identity: EcoString,
    pub declaration: Declaration,
}

#[derive(Debug)]
pub enum Declaration {
    Absent,
    One(DeclaredMeta),
    Duplicate(Vec<DeclaredMeta>),
}

#[derive(Debug)]
pub struct DeclaredMeta {
    pub metadata: Dict,
    pub file: RootedPath,
    pub range: Range<usize>,
}

/// Decodes a source's declaration from captured scan sink values.
///
/// Saturated channels (`Sink::MAX_VALUES`) are refused because later writes may have been dropped.
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
            expected: owner,
            found: header.owner.clone(),
        });
    }

    let mut declared = Vec::new();
    for (rec, styles) in records {
        declared.push(decode_record(captured, &owner, rec, styles.is_some())?);
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

fn decode_record(
    captured: &CapturedValues,
    owner: &str,
    rec: &Value,
    styled: bool,
) -> Result<DeclaredMeta, CaptureError> {
    if styled {
        return Err(CaptureError::Styled);
    }
    let Value::Dict(rec) = rec else {
        return Err(CaptureError::NotADictionary);
    };
    match str_field(rec, PROTOCOL)? {
        DECLARATION_PROTOCOL => {}
        CAPTURE_PROTOCOL => return Err(CaptureError::MisplacedHeader),
        found => {
            return Err(CaptureError::UnknownProtocol {
                found: found.into(),
            });
        }
    }
    let named = str_field(rec, OWNER)?;
    if named != owner {
        return Err(CaptureError::ForeignOwner {
            expected: owner.into(),
            found: named.into(),
        });
    }
    let Ok(Value::Content(marker)) = rec.get(VALUE) else {
        return Err(CaptureError::Malformed { field: VALUE });
    };
    let Some(elem) = marker.to_packed::<MetadataElem>() else {
        return Err(CaptureError::Malformed { field: VALUE });
    };
    let Value::Dict(meta) = &elem.value else {
        return Err(CaptureError::Malformed { field: VALUE });
    };
    let span = marker.span();
    let file = span.id().ok_or(CaptureError::UnlocatedDeclaration)?;
    let src = captured
        .source(file)
        .ok_or(CaptureError::UnlocatedDeclaration)?;
    let range = call_range(src, span)
        .filter(|r| r.start <= r.end && r.end <= src.text().len())
        .ok_or(CaptureError::UnlocatedDeclaration)?;
    Ok(DeclaredMeta {
        metadata: meta.clone(),
        file: file.get().clone(),
        range,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error(
        "the scan captured nothing in front of its `{}` header",
        CAPTURE_PROTOCOL
    )]
    MissingHeader,
    #[error("the scan captured a `{found}` record where the protocol allows none")]
    UnknownProtocol { found: EcoString },
    #[error("a capture header follows the records it must precede")]
    MisplacedHeader,
    #[error("a captured value is not a protocol record")]
    NotADictionary,
    #[error("a protocol record misses its `{field}`")]
    Malformed { field: &'static str },
    #[error("a protocol record carried styles")]
    Styled,
    #[error("the capture belongs to `{found}`, not `{expected}`")]
    ForeignOwner {
        expected: EcoString,
        found: EcoString,
    },
    #[error("a declaration has no readable source location")]
    UnlocatedDeclaration,
    #[error("the capture channel is full, so a declaration may have been dropped")]
    Saturated,
}

fn str_field<'a>(rec: &'a Dict, field: &'static str) -> Result<&'a str, CaptureError> {
    crate::dict_str(rec, field).ok_or(CaptureError::Malformed { field })
}

fn owner_of(src: FileId) -> EcoString {
    src.vpath().get_with_slash().into()
}

fn call_range(src: &Source, span: Span) -> Option<Range<usize>> {
    match span.get() {
        SpanKind::Number { num, .. } => src.range(num, None),
        SpanKind::Range { range, .. } => Some(range),
        SpanKind::Detached => None,
    }
}

fn declaration_marker(payload: Dict, span: Span) -> Content {
    MetadataElem::new(Value::Dict(payload))
        .pack()
        .labelled(declaration_label())
        .spanned(span)
}

fn declaration_label() -> Label {
    Label::new(PicoStr::intern(DECLARATION_LABEL)).expect("declaration label is nonempty")
}

fn declaration_event(owner: &str, marker: Content) -> Value {
    Value::Dict(crate::dict([
        (PROTOCOL, Value::Str(DECLARATION_PROTOCOL.into())),
        (OWNER, Value::Str(owner.into())),
        (VALUE, Value::Content(marker)),
    ]))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tola_typst::{ObservedScan, TypstWorld, file_id, scan_world_observed};
    use typst::Library;
    use typst::engine::Sink;
    use typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};
    use typst::utils::LazyHash;

    use super::*;
    use crate::library::SiteLibrary;
    use crate::test_support::{empty_inputs, file_map, test_world};

    const IMPORT: &str = "#import \"@tola/source:0.0.0\": tola-meta\n";

    fn header() -> CaptureHeader {
        CaptureHeader::new(file_id("main.typ"), "input-1")
    }

    fn scan(files: &[(&str, &str)], records: Vec<Value>) -> (TypstWorld, ObservedScan) {
        let mut written = vec![header().value()];
        written.extend(records);
        let w = site(files);
        let scanned = scan_world_observed(&w, written);
        (w, scanned)
    }

    fn decode(world: &TypstWorld, scan: &ObservedScan) -> Result<SourceCapture, CaptureError> {
        let src = scan.captured().source(world.main()).unwrap();
        decode_capture(scan.captured(), &header(), src)
    }

    fn site(files: &[(&str, &str)]) -> TypstWorld {
        virtual_site(file_map(files), SiteLibrary::new(empty_inputs()).shared())
    }

    fn virtual_site(files: tola_typst::FileMap, library: Arc<LazyHash<Library>>) -> TypstWorld {
        test_world("tola-source-meta", files, library)
    }

    fn payload(title: &str) -> Dict {
        crate::dict([("title", Value::Str(title.into()))])
    }

    #[test]
    fn module_alias_lookup() {
        let pkg: tola_typst::PackageSpec = "@tola-modules/notes:0.0.0".parse().unwrap();
        let pkg_file = |path: &str| {
            FileId::new(RootedPath::new(
                VirtualRoot::Package(pkg.clone()),
                VirtualPath::new(path).unwrap(),
            ))
        };
        let mut files = file_map(&[(
            "main.typ",
            "\n#import \"@tola/module:0.0.0\": get-module\n#import \"@tola-modules/notes:0.0.0\": chosen\n#assert.eq(get-module(\"notes\").name, \"notes\")\n#assert.eq(chosen.name, \"notes\")\n",
        )]);
        files.insert(
            pkg_file("/typst.toml"),
            b"[package]\nname=\"notes\"\nversion=\"0.0.0\"\nentrypoint=\"lib.typ\"\n".to_vec(),
        );
        files.insert(
            pkg_file("/lib.typ"),
            b"\n#import \"@tola/module:0.0.0\": get-module\n#let chosen = get-module()\n".to_vec(),
        );
        let mut inputs = empty_inputs();
        inputs.modules.insert(
            "notes".into(),
            Value::Dict(crate::dict([("name", Value::Str("notes".into()))])),
        );
        let world = virtual_site(files, SiteLibrary::new(inputs).shared());
        let scan = scan_world_observed(&world, []);
        assert!(scan.result().is_ok(), "{:?}", scan.result().err());
    }

    #[test]
    fn module_lookup_errors() {
        for (expr, expected) in [
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
            let text = format!("#import \"@tola/module:0.0.0\": get-module\n#{expr}");
            let (_, res) = scan(&[("main.typ", &text)], Vec::new());
            assert!(res.result().is_err(), "{expr} unexpectedly succeeded");
            let diags = format!("{:?}", res.result().err());
            assert!(diags.contains(expected), "{diags}");
        }
    }

    #[test]
    fn uuid_rfc_vectors() {
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
        let (_, res) = scan(&[("main.typ", text)], Vec::new());
        assert!(res.result().is_ok(), "{:?}", res.result().err());
    }

    #[test]
    fn uuid_invalid_inputs() {
        for expr in [
            "normalize(\"invalid\")",
            "normalize(\" 550e8400-e29b-41d4-a716-446655440000\")",
            "v5(\"invalid\", \"x\")",
            "v5(namespace.dns, 1)",
        ] {
            let text = format!("#import \"@tola/uuid:0.0.0\": namespace, normalize, v5\n#{expr}");
            let (_, res) = scan(&[("main.typ", &text)], Vec::new());
            assert!(res.result().is_err(), "{expr} unexpectedly succeeded");
        }
    }

    #[test]
    fn meta_single_declaration() {
        let text = IMPORT.to_owned() + "#tola-meta((title: \"Post\"))";
        let (world, scan) = scan(&[("main.typ", &text)], Vec::new());
        let Declaration::One(declared) = decode(&world, &scan).unwrap().declaration else {
            panic!("source declares once");
        };
        assert_eq!(
            declared.metadata.get("title"),
            Ok(&Value::Str("Post".into()))
        );
        let res = scan.result().expect("source evaluates");
        assert_eq!(
            &res.source().text()[declared.range],
            "tola-meta((title: \"Post\"))"
        );

        let decls = res.metadata_declarations(DECLARATION_LABEL);
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].value(), &Value::Dict(payload("Post")));
        let range = res.source().find(decls[0].span()).unwrap().range();
        assert_eq!(&res.source().text()[range], "tola-meta((title: \"Post\"))");
    }

    #[test]
    fn meta_wrapper_span() {
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
            panic!("wrapper declares once");
        };
        assert_eq!(
            declared.metadata.get("title"),
            Ok(&Value::Str("Post".into()))
        );
        assert_eq!(declared.file, file_id("theme.typ").get().clone());
        assert_eq!(&helper[declared.range], "tola-meta(fields.named())");
    }

    #[test]
    fn meta_reused_marker() {
        let text =
            IMPORT.to_owned() + "#let marker = tola-meta((title: \"Post\"))\n#marker\n#marker\n";
        let (world, scan) = scan(&[("main.typ", &text)], Vec::new());
        assert!(matches!(
            decode(&world, &scan).unwrap().declaration,
            Declaration::One(_)
        ));
    }

    #[test]
    fn meta_rejects_foreign_owner() {
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

    #[test]
    fn meta_saturated_channel() {
        let mut text = IMPORT.to_owned();
        for idx in 0..Sink::MAX_VALUES + 2 {
            text.push_str(&format!("#tola-meta((index: {idx}))\n"));
        }
        let (_world, scan) = scan(&[("main.typ", &text)], Vec::new());
        let records = scan.captured().records();
        assert_eq!(records.len(), Sink::MAX_VALUES);

        let (first, styles) = &records[0];
        assert_eq!(styles, &None);
        assert_eq!(first, &header().value());

        let indices: Vec<_> = records[1..]
            .iter()
            .map(|(val, styles)| {
                assert_eq!(styles, &None);
                let Value::Dict(event) = val else {
                    panic!("declaration event is a dictionary");
                };
                let Ok(Value::Content(marker)) = event.get(VALUE) else {
                    panic!("declaration event holds marker");
                };
                let Value::Dict(meta) = &marker.to_packed::<MetadataElem>().unwrap().value else {
                    panic!("declaration marker holds dictionary");
                };
                let Ok(Value::Int(idx)) = meta.get("index") else {
                    panic!("declaration marker holds index");
                };
                *idx
            })
            .collect();
        assert_eq!(
            indices,
            (0..Sink::MAX_VALUES - 1)
                .map(|i| i as i64)
                .collect::<Vec<_>>()
        );
    }
}
