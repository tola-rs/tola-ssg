//! In-memory compilation and export of Typst bundles.
//!
//! Typst fans out inside both on rayon's pool — compilation per child document, export per
//! file — so this module adds no second layer and no pool of its own. Reusing the previous
//! export's entries is the incremental lever: matching storage is shared, absent bytes are
//! re-exported.

use crate::diagnostic::package_imports::PackageImporters;
use crate::diagnostic::{CompileError, Diagnostics};
use crate::introspection::{MetadataDeclaration, label_selector, metadata_declaration};
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
    let compiled = typst::compile::<Bundle>(&session);
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
        Ok(bundle) => bundle,
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

    /// Actual disk paths used for read attempts during compilation.
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
    use typst::foundations::{Dict, Value};
    use typst_bundle::BundleOptions;
    pub(super) fn shares_backing(left: &BundleBytes, right: &BundleBytes) -> bool {
        let left = left.as_slice();
        let right = right.as_slice();
        left.len() == right.len()
            && (left.is_empty() || std::ptr::eq(left.as_ptr(), right.as_ptr()))
    }

    pub(super) fn entry_from<'a>(entries: &'a [BundleEntry], path: &str) -> &'a BundleEntry {
        entries
            .iter()
            .find(|entry| entry.path().get_with_slash() == path)
            .unwrap_or_else(|| panic!("missing bundle entry {path}"))
    }

    pub(super) fn entry<'a>(export: &'a BundleExport, path: &str) -> &'a BundleEntry {
        export
            .entries()
            .iter()
            .find(|entry| entry.path.get_with_slash() == path)
            .unwrap()
    }

    pub(super) fn compile(source: &str) -> BundleExport {
        export(&TempDir::new().unwrap(), "main.typ", source)
    }

    pub(super) fn compile_without_export(source: &str) -> BundleCompilation {
        let dir = TempDir::new().unwrap();
        let world = world_for(&dir, "main.typ", source);
        compile_bundle_world(&world, &BundleCancellation::default()).unwrap()
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

    fn world_with_inputs(dir: &TempDir, entry: &str, source: &str, inputs: Dict) -> TypstWorld {
        fontless_world(world_builder(dir, entry, source).with_inputs_dict(inputs))
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

    fn export(dir: &TempDir, entry: &str, source: &str) -> BundleExport {
        export_world(&world_for(dir, entry, source))
    }

    fn export_with_inputs(dir: &TempDir, entry: &str, source: &str, inputs: Dict) -> BundleExport {
        export_world(&world_with_inputs(dir, entry, source, inputs))
    }
    #[test]
    fn each_document_keeps_its_metadata() {
        let result = compile(
            r#"
            #document("a.html", title: [Native Alpha])[
              #metadata((title: [Alpha])) <page-meta>
            ]
            #document("b.html")[#metadata((title: [Beta])) <page-meta>]
            "#,
        );
        let documents = result.documents().collect::<Vec<_>>();
        assert_eq!(documents.len(), 2);
        assert_eq!(documents[0].path().get_with_slash(), "/a.html");
        assert_eq!(documents[0].kind(), BundleDocumentKind::Html);
        let Value::Dict(meta) = documents[0].metadata_first("page-meta").unwrap() else {
            panic!("expected metadata dictionary");
        };
        assert!(matches!(meta.get("title"), Ok(Value::Content(_))));
        assert_eq!(documents[0].info().title.as_deref(), Some("Native Alpha"));
        let declarations = documents[0].metadata_declarations("page-meta");
        assert_eq!(declarations.len(), 1);
        assert!(!declarations[0].span().is_detached());
        assert!(declarations[0].location().is_some());
        assert!(matches!(declarations[0].value(), Value::Dict(_)));
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
        use std::fs;
        use std::sync::Arc;
        #[cfg(feature = "parallel")]
        use std::sync::Barrier;
        use std::sync::atomic::AtomicBool;
        #[cfg(feature = "parallel")]
        use std::sync::atomic::{AtomicUsize, Ordering};

        use super::{
            entry, export, export_with_inputs, export_world, world_for, world_with_files,
            world_with_inputs,
        };
        use crate::{
            AccessedDeps, BundleCancellation, BundleExport, FileProvider, FileResolver, FileTarget,
            ReadLocator, compile_bundle_world,
        };
        use comemo::Tracked;
        use tempfile::TempDir;
        use typst::diag::{At, SourceResult, bail};
        use typst::engine::Engine;
        use typst::foundations::{Context, Dict, Func, IntoValue, NativeFunc, Str, func};
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

        fn html(result: &BundleExport, path: &str) -> String {
            String::from_utf8(entry(result, path).bytes.as_slice().to_vec()).unwrap()
        }

        #[test]
        fn root_declarations_converge() {
            let dir = TempDir::new().unwrap();
            let result = export(
                &dir,
                "site.typ",
                r#"
        #context {
          let seen = query(<child-feedback>).len()
          let path = if seen == 0 { "initial.html" } else { "settled.html" }
          document(path)[#metadata(none) <child-feedback>settled]
        }
        "#,
            );

            assert_eq!(result.entries().len(), 1);
            assert_eq!(result.entries()[0].path.get_with_slash(), "/settled.html");
        }

        #[test]
        fn child_queries_observe_the_whole_bundle() {
            let dir = TempDir::new().unwrap();
            let result = export(
                &dir,
                "site.typ",
                r#"
        #document("a.html")[#metadata(none) <shared-label>A]
        #document("b.html")[#context [count: #query(<shared-label>).len()]]
        "#,
            );

            assert!(html(&result, "/b.html").contains("count: 1"));
        }

        #[test]
        fn cross_document_labels_have_anchors() {
            let dir = TempDir::new().unwrap();
            let result = export(
                &dir,
                "site.typ",
                r#"
        #document("a.html")[= Target <cross-target>]
        #document("b.html")[#link(<cross-target>)[open target]]
        "#,
            );

            let source = html(&result, "/a.html");
            let link = html(&result, "/b.html");
            assert!(source.contains("id=\""), "{source}");
            assert!(link.contains("href=\"a.html#"), "{link}");
        }

        #[test]
        fn body_metadata_sets_the_document_path() {
            let dir = TempDir::new().unwrap();
            fs::write(
                dir.path().join("document.typ"),
                "#metadata(\"from-body.html\") <path-meta>\n= Document\n",
            )
            .unwrap();
            let result = export(
                &dir,
                "site.typ",
                r#"
        #let emit(fallback) = context {
          let body = include "document.typ"
          let meta = query(<path-meta>).first(default: none)
          let path = if meta == none { fallback } else { meta.value }
          document(path, body)
        }
        #emit("fallback.html")
        "#,
            );

            assert_eq!(
                result
                    .entries()
                    .iter()
                    .map(|entry| entry.path.get_with_slash())
                    .collect::<Vec<_>>(),
                ["/from-body.html"]
            );
            assert!(html(&result, "/from-body.html").contains("Document"));
        }

        #[test]
        fn included_content_realizes_per_document() {
            let dir = TempDir::new().unwrap();
            fs::write(
                dir.path().join("document.typ"),
                r#"
        #context html.span(class: "current-page")[#state("current-page").get()]
        #metadata("document") <source-meta>
        "#,
            )
            .unwrap();
            let result = export(
                &dir,
                "site.typ",
                r#"
        #let body = include "document.typ"
        #let emit(path, page) = document(path, [
          #state("current-page", page).update(_ => page)
          #body
        ])
        #emit("first.html", "first")
        #emit("second.html", "second")
        "#,
            );

            let first = html(&result, "/first.html");
            let second = html(&result, "/second.html");

            assert!(
                first.contains(r#"class="current-page">first</span>"#),
                "{first}"
            );
            assert!(
                second.contains(r#"class="current-page">second</span>"#),
                "{second}"
            );
            assert!(
                !first.contains(r#"class="current-page">second</span>"#),
                "{first}"
            );
            assert!(
                !second.contains(r#"class="current-page">first</span>"#),
                "{second}"
            );

            assert_eq!(result.documents().count(), 2);
            for document in result.documents() {
                assert_eq!(
                    document.metadata_first("source-meta").unwrap(),
                    "document".into_value()
                );
            }
        }

        #[test]
        fn export_keeps_native_source_order() {
            let dir = TempDir::new().unwrap();
            let world = world_for(
                &dir,
                "site.typ",
                "#document(\"a.html\", [A])\n#document(\"b.html\", [B])\n",
            );
            let result = export_world(&world);

            assert_eq!(
                result
                    .entries()
                    .iter()
                    .map(|entry| entry.path.get_with_slash())
                    .collect::<Vec<_>>(),
                ["/a.html", "/b.html"]
            );
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

        #[test]
        fn whole_run_reads_survive_recompilation() {
            let dir = TempDir::new().unwrap();
            fs::write(dir.path().join("first.txt"), "first").unwrap();
            fs::write(dir.path().join("second.txt"), "second").unwrap();
            let world = world_for(
                &dir,
                "site.typ",
                r#"
        #document("first.html")[#context read("first.txt")]
        #document("second.html")[#context read("second.txt")]
        "#,
            );
            let compile = || compile_bundle_world(&world, &BundleCancellation::default()).unwrap();

            let first = compile();
            let second = compile();
            assert_eq!(first.accessed().reads, second.accessed().reads);
            assert_eq!(first.documents().count(), 2);
            assert_eq!(second.documents().count(), 2);
        }

        #[test]
        fn document_introspection_stays_separate() {
            let dir = TempDir::new().unwrap();
            let result = export(
                &dir,
                "site.typ",
                r#"
        #document("a/index.html", [
          = Alpha <alpha>
          #link("/only-a/")[A]
        ])
        #document("b/index.html", [
          = Beta <beta>
          #link("/only-b/")[B]
        ])
        "#,
            );
            let mut documents = result.documents().collect::<Vec<_>>();
            documents.sort_by(|left, right| {
                left.path()
                    .get_with_slash()
                    .cmp(right.path().get_with_slash())
            });

            assert_eq!(
                documents[0]
                    .links()
                    .into_iter()
                    .map(|link| link.dest)
                    .collect::<Vec<_>>(),
                ["/only-a/"]
            );
            assert_eq!(documents[0].headings()[0].text, "Alpha");
            assert_eq!(
                documents[0].headings()[0].supplement.as_deref(),
                Some("Section")
            );

            assert_eq!(
                documents[1]
                    .links()
                    .into_iter()
                    .map(|link| link.dest)
                    .collect::<Vec<_>>(),
                ["/only-b/"]
            );
            assert_eq!(documents[1].headings()[0].text, "Beta");
        }

        #[test]
        fn inputs_resolve_values_by_document_path() {
            let dir = TempDir::new().unwrap();
            fs::write(
                dir.path().join("contextual-value.typ"),
                r#"
        #let document-path = sys.inputs.at("test-document-path")
        #let values = sys.inputs.at("test-document-values")
        #let contextual-value() = values.at(document-path())
        "#,
            )
            .unwrap();
            fs::write(
                dir.path().join("document.typ"),
                r#"
        #import "contextual-value.typ": contextual-value
        #context {
          html.span(class: "contextual-id")[#contextual-value().id]
        }
        "#,
            )
            .unwrap();

            let value = |id: &str| {
                let mut value = Dict::new();
                value.insert("id".into(), id.into_value());
                value.into_value()
            };
            let mut contexts = Dict::new();
            contexts.insert("/a/index.html".into(), value("a"));
            contexts.insert("/b/index.html".into(), value("b"));
            contexts.insert("/tags/rust/index.html".into(), value("tag:rust"));
            let mut inputs = Dict::new();
            inputs.insert(
                "test-document-path".into(),
                Func::from(current_document_path::data()).into_value(),
            );
            inputs.insert("test-document-values".into(), contexts.into_value());

            let result = export_with_inputs(
                &dir,
                "site.typ",
                r#"
        #import "contextual-value.typ": contextual-value
        #let body = include "document.typ"
        #document("a/index.html", body)
        #document("b/index.html", body)
        #document("tags/rust/index.html", [
          #context {
            html.span(class: "contextual-id")[#contextual-value().id]
          }
        ])
        "#,
                inputs,
            );

            for (path, id) in [
                ("/a/index.html", "a"),
                ("/b/index.html", "b"),
                ("/tags/rust/index.html", "tag:rust"),
            ] {
                let document = html(&result, path);
                assert!(
                    document.contains(&format!(r#"class="contextual-id">{id}</span>"#)),
                    "{document}"
                );
            }
        }

        #[test]
        fn document_path_requires_document_context() {
            let dir = TempDir::new().unwrap();
            let mut inputs = Dict::new();
            inputs.insert(
                "test-document-path".into(),
                Func::from(current_document_path::data()).into_value(),
            );

            let world = world_with_inputs(
                &dir,
                "site.typ",
                "#let document-path = sys.inputs.at(\"test-document-path\")\n#context document-path()\n",
                inputs,
            );
            let error = compile_bundle_world(&world, &BundleCancellation::default()).unwrap_err();
            let diagnostics = error.diagnostics().unwrap();

            assert!(diagnostics.errors().any(|diagnostic| {
                diagnostic
                    .message
                    .contains("test current requires a Bundle document context")
            }));
        }

        #[test]
        fn failed_compilation_keeps_diagnostics() {
            let dir = TempDir::new().unwrap();
            let world = world_for(&dir, "site.typ", "#unknown-name\n");

            let error = compile_bundle_world(&world, &BundleCancellation::default()).unwrap_err();
            let diagnostics = error.diagnostics().unwrap();

            assert!(diagnostics.errors().any(|diagnostic| {
                diagnostic
                    .message
                    .contains("unknown variable: unknown-name")
            }));
        }

        #[test]
        fn failed_compilation_retains_reads() {
            let dir = TempDir::new().unwrap();
            let world = world_for(
                &dir,
                "site.typ",
                "#document(\"index.html\")[#context read(\"missing.txt\")]\n",
            );

            let failure = compile_bundle_world(&world, &BundleCancellation::default()).unwrap_err();

            assert!(
                failure
                    .accessed()
                    .disk_reads
                    .iter()
                    .any(|path| path.as_path().ends_with("missing.txt"))
            );
            assert!(failure.diagnostics().unwrap().has_errors());
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
        fn non_convergence_warnings_match() {
            let (_directory, world) = world(
                r#"
        #document("index.html")[
          #show strong: none
          #context {
            let count = query(heading).len()
            count * [= Generated]
          }
        ]
        "#,
            );
            let official = typst::compile::<typst_bundle::Bundle>(&world);
            official.output.unwrap();
            let adapter = compile_bundle_world(&world, &BundleCancellation::default()).unwrap();

            assert_eq!(
                adapter
                    .diagnostics()
                    .raw()
                    .iter()
                    .map(|diagnostic| diagnostic.source())
                    .collect::<Vec<_>>(),
                official.warnings.iter().collect::<Vec<_>>(),
            );
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
        fn final_inventory_resolves_cross_document_anchor() {
            let (_directory, world) = world(
                r#"
        #document("a.html")[= Target <cross-target>]
        #document("b.html")[#link(<cross-target>)[open target]]
        "#,
            );
            let compilation = compile_bundle_world(&world, &BundleCancellation::default()).unwrap();
            let target = compilation
                .document(&VirtualPath::new("a.html").unwrap())
                .unwrap()
                .html_inventory(&BundleCancellation::default())
                .unwrap()
                .unwrap();
            let source = compilation
                .document(&VirtualPath::new("b.html").unwrap())
                .unwrap()
                .html_inventory(&BundleCancellation::default())
                .unwrap()
                .unwrap();
            let reference = source
                .references()
                .iter()
                .find(|reference| reference.tag() == "a" && reference.attribute() == "href")
                .expect("cross-document anchor has a final href");
            let fragment = reference
                .destination()
                .split_once('#')
                .map(|(_, fragment)| fragment)
                .expect("cross-document href has an anchor");

            assert!(reference.destination().starts_with("a.html#"));
            assert_eq!(
                reference.reference_use(),
                crate::html::HtmlReferenceUse::Navigation
            );
            assert!(target.fragments().iter().any(|id| id.value() == fragment));
        }

        #[test]
        fn final_inventory_records_frame_links() {
            let (_directory, world) = world(
                r#"#document("index.html")[#html.frame(link("/target/")[inside frame])]
#document("target/index.html")[Target]"#,
            );
            let compilation = compile_bundle_world(&world, &BundleCancellation::default()).unwrap();
            let inventory = compilation
                .document(&VirtualPath::new("index.html").unwrap())
                .unwrap()
                .html_inventory(&BundleCancellation::default())
                .unwrap()
                .unwrap();

            assert!(inventory.references().iter().any(|reference| {
                reference.tag() == "svg:a"
                    && reference.attribute() == "href"
                    && reference.destination() == "/target/"
            }));
        }

        #[test]
        fn heading_fragment_uses_the_final_anchor() {
            let (_directory, world) = world(
                r#"
        #document("index.html")[
          #link(<heading-target>)[Jump]
          = Heading <heading-target>
        ]
        "#,
            );
            let compilation = compile_bundle_world(&world, &BundleCancellation::default()).unwrap();
            let document = compilation
                .document(&VirtualPath::new("index.html").unwrap())
                .unwrap();
            let heading = document.headings().into_iter().next().unwrap();
            let fragment = heading.fragment.expect("linked heading has a final anchor");
            let inventory = document
                .html_inventory(&BundleCancellation::default())
                .unwrap()
                .unwrap();

            assert!(
                inventory
                    .fragments()
                    .iter()
                    .any(|target| target.value() == fragment)
            );
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
    mod content_handoff {
        use std::fs;

        #[cfg(feature = "scan")]
        use super::world_with_inputs;
        use super::{export_world, world_for};
        #[cfg(feature = "scan")]
        use crate::{BundleCancellation, ReadLocator, TypstWorld, compile_bundle_world};
        use tempfile::TempDir;
        #[cfg(feature = "scan")]
        use typst::foundations::{Dict, IntoValue, Value};

        #[cfg(feature = "scan")]
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
        #[cfg(feature = "scan")]
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
        fn include_renders_contextual_values() {
            let dir = TempDir::new().unwrap();
            fs::write(
                dir.path().join("document.typ"),
                r#"
        #let visits = counter("content-handoff-control")
        #visits.update(4)
        #context html.span(class: "context-value")[#visits.display()]
        "#,
            )
            .unwrap();
            let world = world_for(
                &dir,
                "site.typ",
                "#document(\"index.html\")[#include \"document.typ\"]\n",
            );
            let bundle = export_world(&world);
            let html = String::from_utf8(bundle.entries()[0].bytes.as_slice().to_vec()).unwrap();

            assert!(html.contains("context-value"));
            assert!(html.contains(">4</span>"), "{html}");
        }

        #[test]
        #[cfg(feature = "scan")]
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
