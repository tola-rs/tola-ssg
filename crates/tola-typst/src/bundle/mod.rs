//! In-memory compilation and export of Typst bundles.
//!
//! Typst fans out inside both on rayon's pool — compilation per child document, export per
//! file — so this module adds no second layer and no pool of its own. Reusing the previous
//! export's entries is the incremental lever: matching storage is shared, absent bytes are
//! re-exported.

use crate::diagnostic::package_imports::PackageImporters;
use crate::diagnostic::{CompileError, Diagnostics};
use crate::introspection::{MetadataDeclaration, label_selector, metadata_declaration};
use crate::reference_occurrence::ObservedOutput;
use crate::session::{AccessedDeps, CompileSession, failure_evidence};
use crate::world::TypstWorld;
use crate::world::file::{DiskReadPath, FileRead};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use typst::foundations::NativeElement;
use typst::foundations::Output as _;
use typst::foundations::{Content, Selector};
use typst::introspection::Introspector;
use typst::model::Document as _;
use typst::model::DocumentElem;
use typst_bundle::{Bundle, BundleFile};

mod cancellation;
mod derived;
mod entries;
mod export;

pub use cancellation::BundleCancellation;
pub use derived::CompiledBundleDocument;
pub use entries::{BundleBytes, BundleDocumentKind, BundleEntries, BundleEntry, BundleEntryKind};
pub use export::BundleExport;

pub(super) use entries::{bundle_file_kind, document_kind};

/// One compiled native Bundle before output export.
#[derive(Debug)]
pub struct BundleCompilation {
    bundle: Bundle,
    original_bodies: OnceLock<HashMap<typst::syntax::VirtualPath, Content>>,
    html_indexes: parking_lot::Mutex<HashMap<typst::syntax::VirtualPath, HtmlDocumentIndexes>>,
    accessed: AccessedDeps,
    diagnostics: Diagnostics,
    importers: Arc<PackageImporters>,
}

/// Derived views share the lifetime and mutation boundary of their native DOM.
#[derive(Debug, Default)]
struct HtmlDocumentIndexes {
    inventory: Option<Arc<crate::html::HtmlDocumentInventory>>,
    fragments: Option<Arc<crate::html::HtmlFragmentIndex>>,
}

/// A failed Bundle compilation with every read observed before failure.
#[derive(Debug)]
pub struct BundleCompileFailure {
    details: Box<BundleCompileFailureDetails>,
}

#[derive(Debug)]
struct BundleCompileFailureDetails {
    accessed: AccessedDeps,
    error: CompileError,
}

impl BundleCompileFailure {
    fn new(accessed: AccessedDeps, error: CompileError) -> Self {
        Self {
            details: Box::new(BundleCompileFailureDetails { accessed, error }),
        }
    }

    pub(crate) fn without_reads(error: CompileError) -> Self {
        Self::new(AccessedDeps::default(), error)
    }

    /// Whether the caller cancelled this compilation.
    pub fn is_cancelled(&self) -> bool {
        self.details.error.is_cancelled()
    }
}

failure_evidence!(BundleCompileFailure, "compilation");

/// Compile a native Bundle without exporting its outputs.
pub fn compile_bundle_world(
    world: &TypstWorld,
    cancellation: &BundleCancellation,
) -> Result<BundleCompilation, BundleCompileFailure> {
    if cancellation.is_cancelled() {
        return Err(BundleCompileFailure::without_reads(
            CompileError::cancelled(),
        ));
    }
    let session = CompileSession::start(world);
    let compiled = typst::compile::<ObservedOutput<Bundle>>(&session);
    let warnings = compiled.warnings;

    if cancellation.is_cancelled() {
        return Err(BundleCompileFailure::new(
            session.finish(),
            CompileError::cancelled(),
        ));
    }

    if let Some(error) = world.font_failure() {
        return Err(BundleCompileFailure::new(session.finish(), error.into()));
    }
    let bundle = match compiled.output {
        Ok(observed) => observed.into_inner(),
        Err(errors) => {
            let error = session.compilation_error(errors.into_iter().chain(warnings));
            let accessed = session.finish();
            return Err(BundleCompileFailure::new(
                accessed,
                cancellation.ensure_active().err().unwrap_or(error),
            ));
        }
    };
    let (accessed, diagnostics, importers) = session.finish_with_diagnostics(warnings);
    if let Err(error) = cancellation.ensure_active() {
        return Err(BundleCompileFailure::new(accessed, error));
    }
    Ok(BundleCompilation {
        bundle,
        original_bodies: OnceLock::new(),
        html_indexes: parking_lot::Mutex::new(HashMap::new()),
        accessed,
        diagnostics,
        importers: Arc::new(importers),
    })
}

impl BundleCompilation {
    /// Read-only access to Typst's complete Bundle introspection.
    ///
    /// Queries include the Bundle root and all compiled child documents.
    /// Location, document, path, and anchor resolution use Typst's introspector.
    pub fn introspector(&self) -> &dyn Introspector {
        self.bundle.introspector()
    }

    /// Query all labeled metadata declarations across the complete Bundle.
    ///
    /// Native order, values, source spans, and locations are preserved,
    /// including declarations outside child documents. Invalid or absent
    /// labels produce an empty collection; label cardinality belongs to the
    /// caller's protocol.
    pub fn metadata_declarations(&self, label: &str) -> Vec<MetadataDeclaration> {
        let Some(label) = label_selector(label) else {
            return Vec::new();
        };
        self.introspector()
            .query(&Selector::Label(label))
            .iter()
            .filter_map(metadata_declaration)
            .collect()
    }

    /// Output paths and kinds in native Bundle source order.
    pub fn outputs(
        &self,
    ) -> impl Iterator<Item = (&typst::syntax::VirtualPath, BundleEntryKind)> + '_ {
        self.bundle
            .files
            .iter()
            .map(|(path, file)| (path, bundle_file_kind(file)))
    }

    /// Compiled documents in native output order without collecting provenance.
    pub fn documents(&self) -> impl Iterator<Item = CompiledBundleDocument<'_>> {
        self.bundle.files.iter().filter_map(move |(path, file)| {
            let BundleFile::Document(document) = file else {
                return None;
            };
            Some(CompiledBundleDocument {
                path,
                document,
                compilation: self,
            })
        })
    }

    fn original_bodies(&self) -> &HashMap<typst::syntax::VirtualPath, Content> {
        self.original_bodies.get_or_init(|| {
            self.bundle
                .introspector()
                .query(&Selector::Elem(DocumentElem::ELEM, None))
                .into_iter()
                .filter_map(|content| {
                    let document = content.to_packed::<DocumentElem>()?;
                    Some((document.path.as_ref().clone(), document.body.clone()))
                })
                .collect()
        })
    }

    /// One compiled document selected by its native Bundle path.
    pub fn document(
        &self,
        path: &typst::syntax::VirtualPath,
    ) -> Option<CompiledBundleDocument<'_>> {
        let (path, file) = self.bundle.files.get_key_value(path)?;
        let BundleFile::Document(document) = file else {
            return None;
        };
        Some(CompiledBundleDocument {
            path,
            document,
            compilation: self,
        })
    }

    /// Files and packages accessed during compilation.
    pub fn accessed(&self) -> &AccessedDeps {
        &self.accessed
    }

    /// Successful file reads consumed during compilation.
    pub fn file_reads(&self) -> &[FileRead] {
        &self.accessed.reads
    }

    /// Package-directory checks observed during compilation.
    pub fn package_checks(&self) -> &[crate::world::package::PackageCheck] {
        &self.accessed.package_checks
    }

    /// Sorted site files containing literal imports or includes of `package`.
    ///
    /// These are static navigation hints, not compiler-proven call sites.
    pub fn files_importing(&self, package: &str) -> &[String] {
        self.importers.importing(package)
    }

    /// Physical read paths retained by the compilation.
    pub fn disk_reads(&self) -> &[DiskReadPath] {
        &self.accessed.disk_reads
    }

    /// Compilation diagnostics (warnings).
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::file::FileResolver;
    use std::fs;
    use tempfile::TempDir;
    use typst::foundations::Value;
    use typst_bundle::BundleOptions;
    pub(super) fn entry_from<'a>(entries: &'a [BundleEntry], path: &str) -> &'a BundleEntry {
        entries
            .iter()
            .find(|entry| entry.path().get_with_slash() == path)
            .unwrap_or_else(|| panic!("missing bundle entry {path}"))
    }

    pub(super) fn compile(source: &str) -> BundleExport {
        export_world(&world_for(&TempDir::new().unwrap(), "main.typ", source))
    }

    pub(super) fn compile_without_export(source: &str) -> BundleCompilation {
        let dir = TempDir::new().unwrap();
        let world = world_for(&dir, "main.typ", source);
        compile_bundle_world(&world, &BundleCancellation::default()).unwrap()
    }

    #[test]
    fn repeated_compilation_keeps_package_evidence() {
        let dir = TempDir::new().unwrap();
        let packages = dir.path().join("packages");
        let package = packages.join("local/demo/1.0.0");
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("typst.toml"),
            "[package]\nname = \"demo\"\nversion = \"1.0.0\"\nentrypoint = \"lib.typ\"\n",
        )
        .unwrap();
        fs::write(package.join("lib.typ"), "#let text = \"payload\"").unwrap();
        let locations = crate::PackageLocations::from_absolute_roots(Some(packages), None).unwrap();
        let files = Arc::new(FileResolver::from_package_locations(
            locations,
            crate::PackageFetchPolicy::LocalOnly,
        ));
        let world = world_with_files(
            &dir,
            "site.typ",
            "#import \"@local/demo:1.0.0\": text\n#asset(\"text.txt\", text)",
            files,
        );
        let first = compile_bundle_world(&world, &BundleCancellation::default()).unwrap();
        let second = compile_bundle_world(&world, &BundleCancellation::default()).unwrap();
        assert!(!first.package_checks().is_empty());
        assert_eq!(second.package_checks(), first.package_checks());
        assert_eq!(second.file_reads(), first.file_reads());
        let first = first
            .export_default(&BundleCancellation::default(), None)
            .unwrap();
        let second = second
            .export_default(&BundleCancellation::default(), None)
            .unwrap();
        assert_eq!(first.entries()[0].bytes(), second.entries()[0].bytes());
    }

    fn world_builder(dir: &TempDir, entry: &str, source: &str) -> crate::world::WorldBuilder {
        let path = dir.path().join(entry);
        fs::write(&path, source).unwrap();
        TypstWorld::builder(&path, dir.path())
    }

    fn fontless_world(builder: crate::world::WorldBuilder) -> TypstWorld {
        builder
            .with_local_cache()
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .expect("valid test world")
    }

    pub(super) fn world_for(dir: &TempDir, entry: &str, source: &str) -> TypstWorld {
        fontless_world(world_builder(dir, entry, source))
    }

    fn world_with_files(
        dir: &TempDir,
        entry: &str,
        source: &str,
        files: Arc<FileResolver>,
    ) -> TypstWorld {
        fontless_world(world_builder(dir, entry, source).with_files(files))
    }

    fn export_world(world: &TypstWorld) -> BundleExport {
        compile_bundle_world(world, &BundleCancellation::default())
            .unwrap()
            .export(
                &BundleOptions::default(),
                &BundleCancellation::default(),
                None,
            )
            .unwrap()
    }

    #[test]
    fn metadata_declarations_keep_their_owner() {
        let compilation = compile_without_export(
            r#"
            #metadata((scope: "root", target: <second>)) <publication>
            #document("a.html")[
              #metadata((scope: "first")) <publication>
              #context [#metadata((scope: "contextual")) <publication>]
            ]
            #document("b.html")[#metadata((scope: "second")) <publication>] <second>
            "#,
        );
        let declarations = compilation.metadata_declarations("publication");
        assert_eq!(declarations.len(), 4);

        let scopes = declarations
            .iter()
            .map(|declaration| {
                assert!(!declaration.span().is_detached());
                let Value::Dict(value) = declaration.value() else {
                    panic!("expected metadata dictionary");
                };
                value.get("scope").unwrap().clone()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            scopes,
            ["root", "first", "contextual", "second"].map(|scope| Value::Str(scope.into()))
        );
        let paths = declarations
            .iter()
            .map(|declaration| {
                compilation
                    .introspector()
                    .path(declaration.location().unwrap())
                    .map(|path| path.get_with_slash())
            })
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            [None, Some("/a.html"), Some("/a.html"), Some("/b.html")]
        );
        assert!(
            compilation
                .introspector()
                .document(declarations[0].location().unwrap())
                .is_none()
        );

        let path = typst::syntax::VirtualPath::new("a.html").unwrap();
        let document_declarations = compilation
            .document(&path)
            .unwrap()
            .metadata_declarations("publication");
        assert_eq!(document_declarations.len(), 2);
        assert_eq!(
            document_declarations[0].location(),
            declarations[1].location()
        );
        assert_eq!(
            document_declarations[1].location(),
            declarations[2].location()
        );

        let Value::Dict(root) = declarations[0].value() else {
            panic!("expected root dictionary");
        };
        let Value::Label(target) = root.get("target").unwrap() else {
            panic!("expected native label target");
        };
        let target = compilation
            .introspector()
            .query_unique(&Selector::Label(*target))
            .unwrap();
        assert_eq!(
            compilation
                .introspector()
                .path(target.location().unwrap())
                .unwrap()
                .get_with_slash(),
            "/b.html"
        );
    }
    mod bundle_context {
        use parking_lot::Mutex;
        use std::sync::Arc;
        #[cfg(feature = "parallel")]
        use std::sync::Barrier;
        use std::sync::atomic::AtomicBool;
        #[cfg(feature = "parallel")]
        use std::sync::atomic::{AtomicUsize, Ordering};

        use super::{export_world, world_for, world_with_files};
        use crate::{
            AccessedDeps, BundleCancellation, FileProvider, FileResolver, FileTarget, ReadLocator,
            compile_bundle_world,
        };
        use comemo::Tracked;
        use tempfile::TempDir;
        use typst::diag::{At, SourceResult, bail};
        use typst::engine::Engine;
        use typst::foundations::{Context, Str, func};
        use typst::introspection::{PathIntrospection, here};
        use typst::syntax::Span;

        #[func(contextual)]
        fn current_document_path(
            engine: &mut Engine,
            context: Tracked<Context>,
            span: Span,
        ) -> SourceResult<Str> {
            let location = here(context).at(span)?;
            let Some(path) = engine.introspect(PathIntrospection(location, span)) else {
                bail!(span, "test current requires a Bundle document context");
            };
            Ok(path.get_with_slash().into())
        }

        #[test]
        fn root_declarations_converge() {
            let dir = TempDir::new().unwrap();
            let result = export_world(&world_for(
                &dir,
                "site.typ",
                r#"
        #context {
          let seen = query(<child-feedback>).len()
          let path = if seen == 0 { "initial.html" } else { "settled.html" }
          document(path)[#metadata(none) <child-feedback>settled]
        }
        "#,
            ));

            assert_eq!(result.entries().len(), 1);
            assert_eq!(result.entries()[0].path.get_with_slash(), "/settled.html");
        }

        #[test]
        fn cancelled_compile_reports_cancellation() {
            let dir = TempDir::new().unwrap();
            let world = world_for(
                &dir,
                "site.typ",
                "#document(\"a.html\", [A])\n#document(\"b.html\", [B])\n",
            );
            let cancelled = Arc::new(AtomicBool::new(true));

            let error = compile_bundle_world(
                &world,
                &BundleCancellation::default().with_cancellation(cancelled),
            )
            .unwrap_err();

            assert!(error.is_cancelled());
        }

        fn root_read_names(accessed: &AccessedDeps) -> std::collections::BTreeSet<String> {
            accessed
                .reads
                .iter()
                .filter_map(|read| match read.evidence().locator() {
                    ReadLocator::Root(path) | ReadLocator::ProvidedRoot(path) => {
                        path.file_name()?.to_str().map(str::to_owned)
                    }
                    _ => None,
                })
                .collect()
        }

        struct RecordingFiles {
            reads: Arc<Mutex<Vec<String>>>,
        }

        impl FileProvider for RecordingFiles {
            fn target(&self, id: typst::syntax::FileId) -> Option<FileTarget> {
                let path = id.vpath().get_with_slash().to_owned();
                if matches!(path.as_str(), "/a.txt" | "/b.txt" | "/shared.txt") {
                    self.reads.lock().push(path.clone());
                    return Some(FileTarget::Bytes(Arc::from(path.into_bytes())));
                }
                None
            }
        }

        #[test]
        fn official_compile_shares_whole_run_reads() {
            let dir = TempDir::new().unwrap();
            let reads = Arc::new(Mutex::new(Vec::new()));
            let files = Arc::new(FileResolver::new().with_provider(RecordingFiles {
                reads: Arc::clone(&reads),
            }));
            let world = world_with_files(
                &dir,
                "site.typ",
                r#"
        #document("a.html")[#context [#read("shared.txt") #read("a.txt")]]
        #document("b.html")[#context [#read("shared.txt") #read("b.txt")]]
        "#,
                files,
            );
            let compilation = compile_bundle_world(&world, &BundleCancellation::default()).unwrap();
            let output_paths = compilation
                .documents()
                .map(|document| document.path().get_with_slash().to_owned())
                .collect::<Vec<_>>();
            let compile_reads = root_read_names(compilation.accessed());
            let mut realization_reads = reads
                .lock()
                .iter()
                .filter(|path| path.as_str() != "/shared.txt")
                .cloned()
                .collect::<Vec<_>>();
            realization_reads.sort();

            assert_eq!(realization_reads, ["/a.txt", "/b.txt"]);
            assert_eq!(output_paths, ["/a.html", "/b.html"]);
            assert_eq!(
                compile_reads,
                ["a.txt", "b.txt", "shared.txt", "site.typ"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            );
        }

        #[cfg(feature = "parallel")]
        struct ParallelFiles {
            state: Arc<ParallelState>,
        }

        #[cfg(feature = "parallel")]
        struct ParallelState {
            background_barrier: Barrier,
            active_background: AtomicUsize,
            max_active_background: AtomicUsize,
        }

        #[cfg(feature = "parallel")]
        impl FileProvider for ParallelFiles {
            fn target(&self, id: typst::syntax::FileId) -> Option<FileTarget> {
                let path = id.vpath().get_with_slash();
                match path {
                    "/preferred.txt" => {}
                    "/first.txt" | "/second.txt" => {
                        let active =
                            self.state.active_background.fetch_add(1, Ordering::AcqRel) + 1;
                        self.state
                            .max_active_background
                            .fetch_max(active, Ordering::AcqRel);
                        self.state.background_barrier.wait();
                        self.state.active_background.fetch_sub(1, Ordering::AcqRel);
                    }
                    _ => return None,
                }
                Some(FileTarget::Bytes(Arc::from(path.as_bytes())))
            }
        }

        #[cfg(feature = "parallel")]
        #[test]
        fn parallel_children_share_whole_run_reads() {
            let dir = TempDir::new().unwrap();
            let state = Arc::new(ParallelState {
                background_barrier: Barrier::new(2),
                active_background: AtomicUsize::new(0),
                max_active_background: AtomicUsize::new(0),
            });
            let world = world_with_files(
                &dir,
                "site.typ",
                r#"
        #document("first.html")[#context read("first.txt")]
        #document("preferred.html")[#context read("preferred.txt")]
        #document("second.html")[#context read("second.txt")]
        "#,
                Arc::new(FileResolver::new().with_provider(ParallelFiles {
                    state: Arc::clone(&state),
                })),
            );
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .build()
                .unwrap();

            let compilation = pool
                .install(|| compile_bundle_world(&world, &BundleCancellation::default()))
                .unwrap();

            assert_eq!(state.max_active_background.load(Ordering::Acquire), 2);
            assert_eq!(compilation.documents().count(), 3);
            assert_eq!(
                root_read_names(compilation.accessed()),
                ["first.txt", "preferred.txt", "second.txt", "site.typ"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            );
        }
    }
    mod semantic_bundle {
        use std::fs;

        use crate::{
            BundleCancellation, BundleEntryKind, BundleOptions, TypstWorld, compile_bundle_world,
        };
        use tempfile::TempDir;
        use typst::diag::{Severity, SourceDiagnostic};
        use typst::ecow::EcoVec;
        use typst::foundations::Bytes;
        use typst::model::PagedFormat;
        use typst::syntax::{VirtualPath, VirtualRoot};
        use typst_bundle::{BundleDocument, BundleFile};

        fn world(source: &str) -> (TempDir, TypstWorld) {
            let directory = TempDir::new().unwrap();
            let world = super::world_for(&directory, "site.typ", source);
            (directory, world)
        }

        #[derive(Debug)]
        struct OfficialExport {
            warnings: EcoVec<SourceDiagnostic>,
            entries: Vec<(VirtualPath, BundleEntryKind, Bytes)>,
        }

        fn official_export(
            world: &TypstWorld,
            options: &BundleOptions,
        ) -> Result<OfficialExport, EcoVec<SourceDiagnostic>> {
            let compiled = typst::compile::<typst_bundle::Bundle>(world);
            let bundle = compiled.output?;
            let exported = typst_bundle::export(&bundle, options)?;
            let entries = bundle
                .files
                .iter()
                .map(|(path, file)| {
                    let kind = match file {
                        BundleFile::Asset(_) => BundleEntryKind::Asset,
                        BundleFile::Document(BundleDocument::Html(_)) => {
                            BundleEntryKind::HtmlDocument
                        }
                        BundleFile::Document(BundleDocument::Paged(_, extras)) => {
                            match extras.format {
                                PagedFormat::Pdf => BundleEntryKind::PdfDocument,
                                PagedFormat::Png => BundleEntryKind::PngDocument,
                                PagedFormat::Svg => BundleEntryKind::SvgDocument,
                            }
                        }
                    };
                    let bytes = exported
                        .get(path)
                        .expect("official Bundle export preserves every file")
                        .clone();
                    (path.clone(), kind, bytes)
                })
                .collect();
            Ok(OfficialExport {
                warnings: compiled.warnings,
                entries,
            })
        }

        #[test]
        fn adapter_export_matches_typst_bundle() {
            let options = BundleOptions::default();

            let (_empty_directory, empty_world) = world("");
            let empty = official_export(&empty_world, &options).unwrap();
            assert!(empty.entries.is_empty());

            let cases: [(&str, Vec<(String, BundleEntryKind)>); 2] = [
                (
                    r#"
        #asset("files/data.txt", "payload") <data>
        #document("index.html")[
          #box(width: 8pt, height: 8pt, fill: red) <home>
          #link(<figure>)[figure]
          #link(<data>)[data]
        ]
        #document("figure.svg")[
          #box(width: 12pt, height: 8pt, fill: blue) <figure>
          #link(<home>)[#box(width: 4pt, height: 4pt, fill: green)]
        ]
        "#,
                    vec![
                        ("/files/data.txt".into(), BundleEntryKind::Asset),
                        ("/index.html".into(), BundleEntryKind::HtmlDocument),
                        ("/figure.svg".into(), BundleEntryKind::SvgDocument),
                    ],
                ),
                (
                    r#"
        #asset("data.bin", bytes((1, 2, 3)))
        #document("index.html")[HTML]
        #document("paper.bin", format: "pdf")[#rect(width: 1pt, height: 1pt)]
        #document("preview.bin", format: "png")[#rect(width: 1pt, height: 1pt)]
        #document("vector.bin", format: "svg")[#rect(width: 1pt, height: 1pt)]
        "#,
                    vec![
                        ("/data.bin".into(), BundleEntryKind::Asset),
                        ("/index.html".into(), BundleEntryKind::HtmlDocument),
                        ("/paper.bin".into(), BundleEntryKind::PdfDocument),
                        ("/preview.bin".into(), BundleEntryKind::PngDocument),
                        ("/vector.bin".into(), BundleEntryKind::SvgDocument),
                    ],
                ),
            ];

            for (source, expected) in cases {
                let (_directory, world) = world(source);
                let official = official_export(&world, &options).unwrap();
                assert_eq!(
                    official
                        .entries
                        .iter()
                        .map(|(path, kind, _)| (path.get_with_slash().to_owned(), *kind))
                        .collect::<Vec<_>>(),
                    expected
                );
                let adapter = compile_bundle_world(&world, &BundleCancellation::default())
                    .unwrap()
                    .export(&options, &BundleCancellation::default(), None)
                    .unwrap();

                assert_eq!(
                    adapter
                        .diagnostics()
                        .raw()
                        .iter()
                        .map(|diagnostic| diagnostic.source())
                        .collect::<Vec<_>>(),
                    official.warnings.iter().collect::<Vec<_>>(),
                );
                assert_eq!(adapter.entries().len(), official.entries.len());
                for (adapter, (path, kind, bytes)) in
                    adapter.entries().iter().zip(&official.entries)
                {
                    assert_eq!(&adapter.path, path);
                    assert_eq!(adapter.kind, *kind);
                    assert_eq!(adapter.bytes.as_slice(), bytes.as_slice(), "{path:?}");
                }
            }
        }

        fn assert_errors_match(source: &str) {
            let (_directory, world) = world(source);
            let official = typst::compile::<typst_bundle::Bundle>(&world)
                .output
                .unwrap_err();
            let adapter = compile_bundle_world(&world, &BundleCancellation::default()).unwrap_err();
            let adapter = adapter
                .diagnostics()
                .unwrap()
                .raw()
                .iter()
                .filter(|diagnostic| diagnostic.source().severity == Severity::Error)
                .map(|diagnostic| diagnostic.source().clone())
                .collect::<Vec<_>>();
            assert_eq!(adapter, official.as_slice());
        }

        #[test]
        fn adapter_diagnostics_match_typst_bundle() {
            for source in [
                r#"
        #asset("same.html", "first")
        #document("same.html")[second]
        "#,
                "content outside a document",
                r#"
        #document("index.html")[#context panic("delayed equality")]
        "#,
                r#"
        #document("a.html")[#context panic("first child")]
        #document("b.html")[#context panic("second child")]
        "#,
            ] {
                assert_errors_match(source);
            }
        }

        #[test]
        fn body_source_ids_ignore_the_wrapper() {
            let directory = TempDir::new().unwrap();
            fs::write(directory.path().join("document.typ"), "Document").unwrap();
            let world = super::world_for(
                &directory,
                "site.typ",
                r#"#let thin(path, body) = document(path, body)
        #let body = include "document.typ"
        #document("direct.html", body)
        #thin("wrapped.html", body)
        "#,
            );
            let compilation = compile_bundle_world(&world, &BundleCancellation::default()).unwrap();

            let source_paths = |path: &str| {
                compilation
                    .document(&VirtualPath::new(path).unwrap())
                    .unwrap()
                    .source_ids()
                    .into_iter()
                    .map(|id| {
                        assert_eq!(id.get().root(), &VirtualRoot::Project);
                        id.vpath().get_with_slash().to_owned()
                    })
                    .collect::<Vec<_>>()
            };
            let direct = source_paths("direct.html");
            let wrapped = source_paths("wrapped.html");

            assert_eq!(direct, vec!["/document.typ"]);
            assert_eq!(wrapped, direct);
        }

        #[test]
        fn compiled_outputs_are_path_typed() {
            let (_directory, world) = world(
                r#"
        #asset("data.txt", "payload")
        #document("b.html")[B]
        #document("a.svg")[#box(width: 2pt, height: 2pt, fill: red)]
        "#,
            );
            let cancellation = BundleCancellation::default();
            let compilation = compile_bundle_world(&world, &cancellation).unwrap();
            let outputs = compilation
                .outputs()
                .map(|(path, kind)| (path.get_with_slash().to_owned(), kind))
                .collect::<Vec<_>>();
            assert_eq!(
                outputs,
                vec![
                    ("/data.txt".into(), BundleEntryKind::Asset),
                    ("/b.html".into(), BundleEntryKind::HtmlDocument),
                    ("/a.svg".into(), BundleEntryKind::SvgDocument),
                ]
            );

            let document_path = typst::syntax::VirtualPath::new("b.html").unwrap();
            let asset_path = typst::syntax::VirtualPath::new("data.txt").unwrap();
            assert!(compilation.document(&document_path).is_some());
            assert!(compilation.document(&asset_path).is_none());
            let all = compilation
                .export(&BundleOptions::default(), &cancellation, None)
                .unwrap();
            let whole = all
                .entries()
                .iter()
                .find(|entry| entry.path.get_with_slash() == "/b.html")
                .unwrap();
            assert_eq!(whole.kind, BundleEntryKind::HtmlDocument);
            assert!(!whole.bytes.as_slice().is_empty());
        }
    }
    #[cfg(feature = "scan")]
    mod content_handoff {
        use std::fs;

        use super::{export_world, fontless_world, world_builder, world_for};
        use crate::{BundleCancellation, ReadLocator, TypstWorld, compile_bundle_world};
        use tempfile::TempDir;
        use typst::foundations::{Dict, IntoValue, Value};

        fn world_with_inputs(dir: &TempDir, entry: &str, source: &str, inputs: Dict) -> TypstWorld {
            fontless_world(world_builder(dir, entry, source).with_inputs_dict(inputs))
        }

        fn handoff_world(
            dir: &TempDir,
            document: &str,
            site: &str,
        ) -> (TypstWorld, crate::ScanResult) {
            let scan = crate::scan_world(&world_for(dir, "document.typ", document)).unwrap();
            let mut inputs = Dict::new();
            inputs.insert("body".into(), scan.content().clone().into_value());
            (world_with_inputs(dir, "site.typ", site, inputs), scan)
        }

        #[test]
        fn scanned_content_survives_site_handoff() {
            let dir = TempDir::new().unwrap();
            fs::write(dir.path().join("payload.txt"), "Transferred body").unwrap();
            let (world, scan) = handoff_world(
                &dir,
                r#"
        #let body = read("payload.txt")
        #show heading: it => html.h2(class: "from-source")[#it.body]
        #let visits = counter("content-handoff")
        #visits.update(4)
        #metadata((owner: "document")) <source-meta>
        #context html.span(class: "context-value")[#visits.display()]
        = #body
        "#,
                r#"
        #let body = sys.inputs.body
        #document("index.html")[#body]
        "#,
            );
            let bundle = export_world(&world);

            let html = String::from_utf8(bundle.entries()[0].bytes.as_slice().to_vec()).unwrap();
            assert!(html.contains("Transferred body"));
            assert!(html.contains("from-source"));
            assert!(html.contains("context-value"));
            assert!(html.contains(">4</span>"), "{html}");

            let document = bundle.documents().next().unwrap();
            let Value::Dict(source_meta) = document.metadata_first("source-meta").unwrap() else {
                panic!("expected source metadata dictionary");
            };
            assert_eq!(source_meta.get("owner").unwrap(), &"document".into_value());
            assert!(scan.file_reads().iter().any(|read| {
                matches!(read.evidence().locator(), ReadLocator::Root(path) if path == std::path::Path::new("payload.txt"))
            }));
            assert!(!bundle.file_reads().iter().any(|read| {
                matches!(read.evidence().locator(), ReadLocator::Root(path) if path == std::path::Path::new("payload.txt"))
            }));
        }

        #[test]
        fn deferred_diagnostic_keeps_source_span() {
            let dir = TempDir::new().unwrap();
            let (world, _scan) = handoff_world(
                &dir,
                "#context { panic(\"deferred document failure\") }\n",
                "#document(\"index.html\")[#sys.inputs.body]\n",
            );
            let error = compile_bundle_world(&world, &BundleCancellation::default()).unwrap_err();
            let diagnostics = error.diagnostics().unwrap();
            let diagnostic = diagnostics
                .errors()
                .find(|diagnostic| diagnostic.message.contains("deferred document failure"))
                .expect("deferred document diagnostic");

            assert!(
                diagnostic
                    .location
                    .source_lines
                    .iter()
                    .any(|line| line.text.contains("deferred document failure")),
                "{diagnostic:?}"
            );
            assert!(format!("{diagnostics}").contains("document.typ"));
        }
    }
}
