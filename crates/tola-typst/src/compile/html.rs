//! HTML compilation of one prepared world.
//!
//! # Example
//!
//! ```ignore
//! use std::sync::Arc;
//! use tola_typst::prelude::*;
//!
//! let cancellation = BundleCancellation::default();
//! let world = TypstWorld::builder(entry, root)
//!     .with_local_cache()
//!     .with_fonts(Arc::new(FontStore::new()))
//!     .build(&cancellation)?;
//! let html = compile_world(&world)?.html()?;
//! ```

use crate::diagnostic::{CompileError, Diagnostics};
use crate::html::HtmlDocument;
use crate::world::TypstWorld;
use crate::world::file::{DiskReadPath, FileRead};

use typst::syntax::FileId;

use crate::session::{AccessedDeps, CompileSession, failure_evidence};

/// Result of a successful compilation.
#[derive(Debug)]
pub struct CompileResult {
    document: HtmlDocument,
    accessed: AccessedDeps,
    diagnostics: Diagnostics,
}

/// A failed single-document compilation together with every input observed
/// before the failure.
#[derive(Debug)]
pub struct CompileFailure {
    details: Box<CompileFailureDetails>,
}

#[derive(Debug)]
struct CompileFailureDetails {
    error: CompileError,
    accessed: AccessedDeps,
}

impl CompileFailure {
    fn new(error: CompileError, accessed: AccessedDeps) -> Self {
        Self {
            details: Box::new(CompileFailureDetails { error, accessed }),
        }
    }

    /// Whether the compilation stopped because it was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.details.error.is_cancelled()
    }
}

failure_evidence!(CompileFailure, "compilation");

impl CompileResult {
    /// Get the compiled HTML document.
    pub fn document(&self) -> &HtmlDocument {
        &self.document
    }

    /// Convert the document to HTML bytes.
    pub fn html(&self) -> Result<Vec<u8>, CompileError> {
        self.html_with_options(&typst_html::HtmlOptions::default())
    }

    /// Convert the document to HTML bytes with explicit exporter options.
    ///
    /// Repeated exports can use different options without mutation or recompilation.
    pub fn html_with_options(
        &self,
        options: &typst_html::HtmlOptions,
    ) -> Result<Vec<u8>, CompileError> {
        typst_html::html(self.document.as_inner(), options)
            .map(|s| s.into_bytes())
            .map_err(|diagnostics| {
                let native = diagnostics
                    .into_iter()
                    .map(crate::diagnostic::NativeDiagnostic::from)
                    .collect::<Vec<_>>();
                CompileError::html_export(crate::diagnostic::export_error_message(&native))
            })
    }

    /// Get files and packages accessed during compilation.
    pub fn accessed(&self) -> &AccessedDeps {
        &self.accessed
    }

    /// Get successful file reads consumed during compilation.
    pub fn file_reads(&self) -> &[FileRead] {
        &self.accessed.reads
    }

    /// Package-directory checks observed during compilation.
    pub fn package_checks(&self) -> &[crate::world::package::PackageCheck] {
        &self.accessed.package_checks
    }

    /// Get actual disk paths used for read attempts during compilation.
    pub fn disk_reads(&self) -> &[DiskReadPath] {
        &self.accessed.disk_reads
    }

    /// Get compilation diagnostics (warnings).
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }

    /// Take ownership of the document.
    pub fn into_document(self) -> HtmlDocument {
        self.document
    }

    /// Destructure the result into document, dependency evidence, and diagnostics.
    pub fn into_parts(self) -> (HtmlDocument, AccessedDeps, Diagnostics) {
        (self.document, self.accessed, self.diagnostics)
    }
}

/// Compile an explicitly constructed [`TypstWorld`].
///
/// Use [`compile_world_with_evidence`] when a caller also needs the reads
/// observed before a failed compilation.
pub fn compile_world(world: &TypstWorld) -> Result<CompileResult, CompileError> {
    compile_world_with_evidence(world).map_err(|failure| failure.into_parts().1)
}

/// Compile a world and retain dependency evidence when compilation fails.
pub fn compile_world_with_evidence(world: &TypstWorld) -> Result<CompileResult, CompileFailure> {
    with_session(world, CompileSession::start(world))
}

/// Compile one of the world's files as the document it is read as.
///
/// An editor answers about a source the site's root program never reached, and that source compiles
/// on its own against the same imports, packages, and fonts.
pub fn compile_source_with_evidence(
    world: &TypstWorld,
    main: FileId,
) -> Result<CompileResult, CompileFailure> {
    with_session(world, CompileSession::start_at(world, main))
}

fn with_session(
    world: &TypstWorld,
    session: CompileSession<'_>,
) -> Result<CompileResult, CompileFailure> {
    let typst::diag::Warned { output, warnings } = typst::compile(&session);
    if let Some(error) = world.font_failure() {
        return Err(CompileFailure::new(error.into(), session.finish()));
    }

    let document = match output {
        Ok(document) => document,
        Err(errors) => {
            let error = session.compilation_error(errors.into_iter().chain(warnings));
            return Err(CompileFailure::new(error, session.finish()));
        }
    };

    let document = HtmlDocument::new(document);

    let (accessed, diagnostics, _) = session.finish_with_diagnostics(warnings);

    Ok(CompileResult {
        document,
        accessed,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::SourceSnapshot;
    use crate::world::file::{ContentDigest, FileMap, FileResolver, ReadLocator, file_id};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use tempfile::TempDir;
    use typst::diag::{SourceDiagnostic, SourceResult, Tracepoint};
    use typst::engine::Engine;
    use typst::foundations::{Dict, Func, IntoValue, NativeFunc, Str, Value, func};

    /// A fontless world with a task-local cache for one test source.
    fn world(root: &Path, file: &Path) -> crate::world::WorldBuilder {
        TypstWorld::builder(file, root)
            .with_local_cache()
            .no_fonts()
    }

    #[func]
    fn spanned_diagnostic(engine: &mut Engine, fail: bool) -> SourceResult<Str> {
        // Construct spans without consulting the World: only diagnostic
        // resolution may attempt to read these files, including the missing trace source.
        let span = |path: &str| {
            typst::syntax::Source::new(file_id(path), "diagnostic only".into())
                .root()
                .span()
        };
        let diagnostic = if fail {
            SourceDiagnostic::error(span("primary.typ"), "diagnostic evidence")
        } else {
            SourceDiagnostic::warning(span("primary.typ"), "diagnostic evidence")
        }
        .with_spanned_hint("related source", span("hint.typ"))
        .with_tracepoint(Tracepoint::Call(None), span("missing-trace.typ"));
        if fail {
            Err(vec![diagnostic].into())
        } else {
            engine.sink.warn(diagnostic);
            Ok(Str::new())
        }
    }

    #[test]
    fn missing_package_names_every_directory() {
        let dir = TempDir::new().unwrap();
        let main = dir.path().join("main.typ");
        fs::write(&main, "#import \"@preview/absent:1.0.0\": value\n#value").unwrap();
        let declared_packages = dir.path().join("packages");
        fs::create_dir_all(&declared_packages).unwrap();
        let locations = crate::world::package::PackageLocations::default()
            .with_declared_root(declared_packages.clone())
            .unwrap();
        let files = FileResolver::from_package_locations(
            locations,
            crate::world::package::PackageFetchPolicy::LocalOnly,
        );
        let world = world(dir.path(), &main)
            .with_files(Arc::new(files))
            .build(&crate::BundleCancellation::default())
            .unwrap();

        let failure = compile_world_with_evidence(&world).unwrap_err();
        let diagnostics = failure.error().diagnostics().unwrap();
        let failure = diagnostics
            .iter()
            .find_map(|diagnostic| diagnostic.package_failure.as_ref())
            .expect("the import reports a package that could not be provided");

        assert_eq!(failure.package, "@preview/absent:1.0.0");
        assert_eq!(
            failure.reason,
            crate::diagnostic::ResolvedPackageFailureReason::NotInstalled
        );
        assert_eq!(failure.searched.len(), 1);
        assert_eq!(failure.searched[0].root, declared_packages);
        assert_eq!(
            failure.searched[0].candidate,
            declared_packages.join("preview/absent/1.0.0")
        );
    }

    #[test]
    fn diagnostic_reads_are_retained() {
        for bundle in [false, true] {
            for fail in [false, true] {
                let dir = TempDir::new().unwrap();
                let main = dir.path().join("main.typ");
                fs::write(&main, format!("#let _ = (sys.inputs.diagnostic)({fail})")).unwrap();
                for name in ["primary.typ", "hint.typ"] {
                    fs::write(dir.path().join(name), "diagnostic only").unwrap();
                }
                let mut inputs = Dict::new();
                inputs.insert(
                    "diagnostic".into(),
                    Func::from(spanned_diagnostic::data()).into_value(),
                );
                let world = world(dir.path(), &main)
                    .with_inputs_dict(inputs)
                    .build(&crate::BundleCancellation::default())
                    .unwrap();
                let (accessed, diagnostics) = if bundle {
                    match crate::compile_bundle_world(&world, &crate::BundleCancellation::new()) {
                        Ok(result) => {
                            assert!(!fail);
                            (result.accessed().clone(), result.diagnostics().clone())
                        }
                        Err(failure) => {
                            assert!(fail, "unexpected Bundle failure: {:?}", failure.error());
                            (
                                failure.accessed().clone(),
                                failure.error().diagnostics().unwrap().clone(),
                            )
                        }
                    }
                } else {
                    match compile_world_with_evidence(&world) {
                        Ok(result) => {
                            assert!(!fail);
                            (result.accessed().clone(), result.diagnostics().clone())
                        }
                        Err(failure) => {
                            assert!(fail, "unexpected HTML failure: {:?}", failure.error());
                            (
                                failure.accessed().clone(),
                                failure.error().diagnostics().unwrap().clone(),
                            )
                        }
                    }
                };
                for name in ["primary.typ", "hint.typ"] {
                    assert!(
                        accessed.reads.iter().any(|read| matches!(
                            read.evidence().locator(),
                            crate::ReadLocator::Root(path) if path == Path::new(name)
                        )),
                        "missing evidence for {name}"
                    );
                }
                assert!(
                    accessed
                        .disk_reads
                        .iter()
                        .any(|path| { path.as_path() == world.root().join("missing-trace.typ") })
                );
                let diagnostic = diagnostics
                    .iter()
                    .find(|diagnostic| diagnostic.message == "diagnostic evidence")
                    .unwrap();
                assert_eq!(diagnostic.location.path.as_deref(), Some("primary.typ"));
                assert_eq!(
                    diagnostic.hints[0].location.path.as_deref(),
                    Some("hint.typ")
                );
                assert_eq!(
                    diagnostic.traces[0].location.location_failure,
                    Some(crate::diagnostic::LocationFailure::SourceUnavailable)
                );
            }
        }
    }

    #[test]
    fn runtime_reads_keep_exact_paths() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.typ");
        fs::create_dir_all(dir.path().join("templates")).unwrap();

        // `read`, `import`, and `include` resolve runtime-built paths through
        // distinct mechanisms; each must leave exact on-disk evidence.
        for (source, asset_body, expected_path, expected_text) in [
            (
                r#"#let name = "document"
#read("templates/" + name + ".txt")"#,
                "runtime payload",
                "templates/document.txt",
                "runtime payload",
            ),
            (
                r#"#let name = "document"
#import "templates/" + name + ".typ" as template
#template.value"#,
                "#let value = [runtime module]",
                "templates/document.typ",
                "runtime module",
            ),
            (
                r#"#let name = "document"
#include "templates/" + name + ".typ""#,
                "[runtime include]",
                "templates/document.typ",
                "runtime include",
            ),
        ] {
            let asset = dir.path().join(expected_path);
            fs::write(&asset, asset_body).unwrap();
            fs::write(&file, source).unwrap();

            let built = world(dir.path(), &file)
                .build(&crate::BundleCancellation::default())
                .unwrap();
            let result = compile_world(&built).unwrap();
            let html = String::from_utf8_lossy(&result.html().unwrap()).into_owned();
            let asset = fs::canonicalize(&asset).unwrap();

            assert!(html.contains(expected_text), "{html}");
            assert!(
                result
                    .disk_reads()
                    .iter()
                    .any(|path| path.as_path() == asset),
                "dynamic read of {expected_path} was not retained: {:?}",
                result.disk_reads()
            );
        }
    }

    #[test]
    fn compiler_uses_its_own_resolver() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.typ");
        fs::write(&file, r#"#read("/provided.txt")"#).unwrap();

        let compile = |files: FileResolver| -> String {
            let built = world(dir.path(), &file)
                .with_files(Arc::new(files))
                .build(&crate::BundleCancellation::default())
                .unwrap();
            let html = compile_world(&built).unwrap().html().unwrap();
            String::from_utf8_lossy(&html).into_owned()
        };

        let mut first = FileMap::new();
        first.insert(file_id("/provided.txt"), b"first".to_vec());
        let first = compile(FileResolver::new().with_provider(first));

        let mut second = FileMap::new();
        second.insert(file_id("/provided.txt"), b"second".to_vec());
        let second = compile(FileResolver::new().with_provider(second));

        assert!(first.contains("first"), "{first}");
        assert!(second.contains("second"), "{second}");
    }

    #[test]
    fn metadata_queries_keep_native_values() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.typ");
        fs::write(
            &file,
            r#"#metadata((id: 1, width: 12pt, summary: [Hello])) <post-meta>
#metadata((id: 2, role: "second")) <post-meta>
= Content"#,
        )
        .unwrap();

        let built = world(dir.path(), &file)
            .build(&crate::BundleCancellation::default())
            .unwrap();
        let result = compile_world(&built).unwrap();
        let document = result.document();

        let first = document
            .metadata_first("post-meta")
            .unwrap()
            .cast::<Dict>()
            .unwrap();
        assert_eq!(first.get("id").unwrap().clone().cast::<i64>().unwrap(), 1);
        assert!(matches!(
            first.get(&Str::from("width")),
            Ok(Value::Length(_))
        ));
        assert!(matches!(
            first.get(&Str::from("summary")),
            Ok(Value::Content(_))
        ));

        let all = document.metadata_all("post-meta");
        assert_eq!(all.len(), 2);
        assert_eq!(
            all[1].clone().cast::<Dict>().unwrap().get("id"),
            Ok(&Value::Int(2))
        );
    }

    #[test]
    fn failed_read_retains_missing_path() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.typ");
        fs::create_dir_all(dir.path().join("templates")).unwrap();
        fs::write(
            &file,
            r#"#let name = "missing"
#read("templates/" + name + ".txt")"#,
        )
        .unwrap();

        let built = world(dir.path(), &file)
            .build(&crate::BundleCancellation::default())
            .unwrap();
        let failure = compile_world_with_evidence(&built)
            .expect_err("missing runtime read should fail compilation");

        assert!(
            failure
                .disk_reads()
                .iter()
                .any(|path| path.as_path().ends_with(Path::new("templates/missing.txt"))),
            "missing runtime read was not retained: {:?}",
            failure.disk_reads()
        );
    }

    #[test]
    fn html_export_observes_options() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.typ");
        fs::write(&file, "= Hello").unwrap();

        let built = world(dir.path(), &file)
            .build(&crate::BundleCancellation::default())
            .unwrap();
        let result = compile_world(&built).unwrap();
        let compact = result.html().unwrap();
        let pretty = result
            .html_with_options(&typst_html::HtmlOptions { pretty: true })
            .unwrap();

        assert_ne!(compact, pretty);
        assert!(String::from_utf8_lossy(&pretty).contains("\n"));
    }

    #[test]
    fn world_inputs_reach_the_document() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.typ");
        fs::write(
            &file,
            r#"#let title = sys.inputs.at("title", default: "Default")
= #title"#,
        )
        .unwrap();

        let built = world(dir.path(), &file)
            .with_inputs([("title", "Custom Title")])
            .build(&crate::BundleCancellation::default())
            .unwrap();
        let html = compile_world(&built).unwrap().html().unwrap();
        assert!(String::from_utf8_lossy(&html).contains("Custom Title"));
    }

    #[test]
    fn snapshot_world_keeps_frozen_bytes() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("snapshot.typ");
        let original = b"= Snapshot A";
        fs::write(&file, original).unwrap();

        let files = Arc::new(FileResolver::new());
        let loaded = SourceSnapshot::build_with_files(
            &[PathBuf::from(&file)],
            dir.path(),
            Arc::clone(&files),
        )
        .unwrap();
        let (snapshot, _) = loaded.into_snapshot_and_accessed();
        let built = TypstWorld::builder(&file, dir.path())
            .with_files(files)
            .with_snapshot(Arc::new(snapshot))
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .unwrap();
        fs::write(&file, "= Snapshot B").unwrap();

        let result = compile_world(&built).unwrap();

        let html = String::from_utf8_lossy(&result.html().unwrap()).into_owned();
        assert!(html.contains("Snapshot A"), "{html}");
        assert!(!html.contains("Snapshot B"), "{html}");
        let expected = ContentDigest::of(original);
        assert!(result.file_reads().iter().any(|read| {
            matches!(read.evidence().locator(), ReadLocator::Root(path) if path == Path::new("snapshot.typ"))
                && read.evidence().digest() == expected
        }));
    }
}
