//! In-memory compilation and export of Typst bundles.
//!
//! Typst fans out inside both on rayon's pool — compilation per child document, export per
//! file — so this module adds no second layer and no pool of its own. Reusing the previous
//! export's entries is the incremental lever: matching storage is shared, absent bytes are
//! re-exported.

use crate::diagnostic::package_imports::PackageImporters;
use crate::diagnostic::{CompileError, Diagnostics};
use crate::introspection::{MetadataDeclaration, labeled_declarations};
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
use typst_bundle::Bundle;

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
    accessed: AccessedDeps,
    error: Box<CompileError>,
}

impl BundleCompileFailure {
    fn new(accessed: AccessedDeps, error: CompileError) -> Self {
        Self {
            accessed,
            error: Box::new(error),
        }
    }

    pub(crate) fn without_reads(error: CompileError) -> Self {
        Self::new(AccessedDeps::default(), error)
    }

    /// Whether the caller cancelled this compilation.
    pub fn is_cancelled(&self) -> bool {
        self.error.is_cancelled()
    }
}

impl std::fmt::Display for BundleCompileFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for BundleCompileFailure {
    /// The boxed cause is the structured compiler failure, so the chain reaches it.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.error)
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
    let session = CompileSession::start(world).with_cancellation(cancellation);
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
        labeled_declarations(self.introspector(), label)
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
        self.bundle
            .files
            .iter()
            .filter_map(move |(path, file)| CompiledBundleDocument::of(path, file, self))
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
        CompiledBundleDocument::of(path, file, self)
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
    use crate::world::file::FileStore;
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
    fn bundle_reuses_pkg_evidence() {
        let dir = TempDir::new().unwrap();
        let packages = dir.path().join("packages");
        let pkg = packages.join("local/demo/1.0.0");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(
            pkg.join("typst.toml"),
            "[package]\nname = \"demo\"\nversion = \"1.0.0\"\nentrypoint = \"lib.typ\"\n",
        )
        .unwrap();
        fs::write(pkg.join("lib.typ"), "#let text = \"payload\"").unwrap();
        let locations = crate::PackageLocations::from_absolute_roots(Some(packages), None).unwrap();
        let files = Arc::new(FileStore::from_package_locations(
            locations,
            crate::PackageFetchMode::LocalOnly,
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

    fn world_builder(dir: &TempDir, entry: &str, src: &str) -> crate::world::WorldBuilder {
        let path = dir.path().join(entry);
        fs::write(&path, src).unwrap();
        TypstWorld::builder(&path, dir.path())
    }

    fn fontless_world(builder: crate::world::WorldBuilder) -> TypstWorld {
        builder
            .with_local_cache()
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .expect("valid test world")
    }

    pub(super) fn world_for(dir: &TempDir, entry: &str, src: &str) -> TypstWorld {
        fontless_world(world_builder(dir, entry, src))
    }

    fn world_with_files(
        dir: &TempDir,
        entry: &str,
        src: &str,
        files: Arc<FileStore>,
    ) -> TypstWorld {
        fontless_world(world_builder(dir, entry, src).with_files(files))
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
    fn bundle_metadata_owners() {
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
            .map(|decl| {
                assert!(!decl.span().is_detached());
                let Value::Dict(value) = decl.value() else {
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
            .map(|decl| {
                compilation
                    .introspector()
                    .path(decl.location().unwrap())
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
        let doc_decls = compilation
            .document(&path)
            .unwrap()
            .metadata_declarations("publication");
        assert_eq!(doc_decls.len(), 2);
        assert_eq!(doc_decls[0].location(), declarations[1].location());
        assert_eq!(doc_decls[1].location(), declarations[2].location());
    }
    mod bundle_context {
        use parking_lot::Mutex;
        use std::sync::Arc;
        #[cfg(feature = "parallel")]
        use std::sync::Barrier;
        #[cfg(feature = "parallel")]
        use std::sync::atomic::{AtomicUsize, Ordering};

        use super::{export_world, world_for, world_with_files};
        use crate::{
            AccessedDeps, BundleCancellation, FileProvider, FileStore, FileTarget, ReadLocator,
            compile_bundle_world,
        };
        use tempfile::TempDir;

        #[test]
        fn bundle_root_converges() {
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
        fn bundle_compile_cancelled() {
            let dir = TempDir::new().unwrap();
            let world = world_for(
                &dir,
                "site.typ",
                "#document(\"a.html\", [A])\n#document(\"b.html\", [B])\n",
            );
            let cancel = BundleCancellation::new();
            cancel.clone().cancel();
            let err = compile_bundle_world(&world, &cancel).unwrap_err();

            assert!(err.is_cancelled());
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
            fn target(&self, fid: typst::syntax::FileId) -> Option<FileTarget> {
                let path = fid.vpath().get_with_slash().to_owned();
                if matches!(path.as_str(), "/a.txt" | "/b.txt" | "/shared.txt") {
                    self.reads.lock().push(path.clone());
                    return Some(FileTarget::Bytes(Arc::from(path.into_bytes())));
                }
                None
            }
        }

        #[test]
        fn bundle_shares_reads() {
            let dir = TempDir::new().unwrap();
            let reads = Arc::new(Mutex::new(Vec::new()));
            let files = Arc::new(FileStore::new().with_provider(RecordingFiles {
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
                .map(|doc| doc.path().get_with_slash().to_owned())
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
            fn target(&self, fid: typst::syntax::FileId) -> Option<FileTarget> {
                let path = fid.vpath().get_with_slash();
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
        fn bundle_parallel_shares_reads() {
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
                Arc::new(FileStore::new().with_provider(ParallelFiles {
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

        use crate::{BundleCancellation, BundleEntryKind, TypstWorld, compile_bundle_world};
        use tempfile::TempDir;
        use typst::syntax::{VirtualPath, VirtualRoot};

        fn world(src: &str) -> (TempDir, TypstWorld) {
            let dir = TempDir::new().unwrap();
            let world = super::world_for(&dir, "site.typ", src);
            (dir, world)
        }

        #[test]
        fn bundle_body_source_ids() {
            let dir = TempDir::new().unwrap();
            fs::write(dir.path().join("document.typ"), "Document").unwrap();
            let world = super::world_for(
                &dir,
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
                    .map(|fid| {
                        assert_eq!(fid.get().root(), &VirtualRoot::Project);
                        fid.vpath().get_with_slash().to_owned()
                    })
                    .collect::<Vec<_>>()
            };
            let direct = source_paths("direct.html");
            let wrapped = source_paths("wrapped.html");

            assert_eq!(direct, vec!["/document.typ"]);
            assert_eq!(wrapped, direct);
        }

        #[test]
        fn bundle_outputs_order() {
            let (_dir, world) = world(
                r#"
        #asset("data.txt", "payload")
        #document("b.html")[B]
        #document("a.svg")[#box(width: 2pt, height: 2pt, fill: red)]
        "#,
            );
            let cancel = BundleCancellation::default();
            let compilation = compile_bundle_world(&world, &cancel).unwrap();
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

            let doc_path = VirtualPath::new("b.html").unwrap();
            let asset_path = VirtualPath::new("data.txt").unwrap();
            assert!(compilation.document(&doc_path).is_some());
            assert!(compilation.document(&asset_path).is_none());
        }
    }
    #[cfg(feature = "scan")]
    mod content_handoff {
        use std::fs;

        use super::{export_world, fontless_world, world_builder, world_for};
        use crate::{BundleCancellation, ReadLocator, TypstWorld, compile_bundle_world};
        use tempfile::TempDir;
        use typst::foundations::{Dict, IntoValue, Value};

        fn world_with_inputs(dir: &TempDir, entry: &str, src: &str, inputs: Dict) -> TypstWorld {
            fontless_world(world_builder(dir, entry, src).with_inputs_dict(inputs))
        }

        fn handoff_world(dir: &TempDir, doc: &str, site: &str) -> (TypstWorld, crate::ScanResult) {
            let scan = crate::scan_world(&world_for(dir, "document.typ", doc)).unwrap();
            let mut inputs = Dict::new();
            inputs.insert("body".into(), scan.content().clone().into_value());
            (world_with_inputs(dir, "site.typ", site, inputs), scan)
        }

        #[test]
        fn bundle_handoff_content() {
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

            let doc = bundle.documents().next().unwrap();
            let Value::Dict(source_meta) = doc.metadata_first("source-meta").unwrap() else {
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
        fn bundle_handoff_deferred_diag() {
            let dir = TempDir::new().unwrap();
            let (world, _scan) = handoff_world(
                &dir,
                "#context { panic(\"deferred document failure\") }\n",
                "#document(\"index.html\")[#sys.inputs.body]\n",
            );
            let err = compile_bundle_world(&world, &BundleCancellation::default()).unwrap_err();
            let diags = err.diagnostics().unwrap();
            let diag = diags
                .errors()
                .find(|diag| diag.message.contains("deferred document failure"))
                .expect("deferred document diagnostic");

            assert!(
                diag.location
                    .source_lines
                    .iter()
                    .any(|line| line.text.contains("deferred document failure")),
                "{diag:?}"
            );
            assert!(format!("{diags}").contains("document.typ"));
        }
    }
}
