//! Eager module evaluation and extraction before contextual realization or layout.
//!
//! # Example
//!
//! ```ignore
//! use tola_typst::prelude::*;
//!
//! let cancellation = BundleCancellation::default();
//! let world = TypstWorld::builder(source, root)
//!     .with_local_cache()
//!     .with_inputs([("draft", true)])
//!     .no_fonts()
//!     .build(&cancellation)?;
//! let result = scan_world(&world)?;
//! let links = result.extract(LinkExtractor::new());
//!
//! // Multiple extractions in one pass
//! let (links, headings) = result.extract((
//!     LinkExtractor::new(),
//!     HeadingExtractor::new(),
//! ));
//! ```

use typst::World;
use typst::comemo::Track;
use typst::diag::SourceDiagnostic;
use typst::engine::{Route, Sink, Traced};
use typst::foundations::{Content, Label, Module, Styles, Value};
use typst::introspection::MetadataElem;
use typst::syntax::{Source, Span};

pub use crate::extract::{
    Extractor, Heading, HeadingExtractor, Link, LinkExtractor, LinkSource, extract,
};
use crate::session::{AccessedDeps, CompileSession, failure_evidence};

use crate::diagnostic::{CompileError, Diagnostics};
use crate::introspection::{
    MetadataCardinalityError, MetadataDeclaration, label_selector, metadata_declaration,
    unique_metadata_value,
};
use crate::world::TypstWorld;
use crate::world::file::{DiskReadPath, FileRead};

/// One eager module evaluation with its extracted content and read evidence.
#[derive(Debug)]
pub struct ScanResult {
    /// The exact parsed main source consumed by this evaluation.
    source: Source,
    /// The complete eager evaluation, including exported definitions.
    module: Module,
    /// Shared Content derived once from the immutable module. Typst's Module
    /// exposes its Content through a consuming accessor only.
    content: Content,
    /// Files and packages accessed during scanning.
    accessed: AccessedDeps,
    /// Scan diagnostics (warnings only).
    diagnostics: Diagnostics,
}

/// A failed Eval-only scan together with every input observed before failure.
#[derive(Debug)]
pub struct ScanFailure {
    details: Box<ScanFailureDetails>,
}

#[derive(Debug)]
struct ScanFailureDetails {
    error: CompileError,
    accessed: AccessedDeps,
}

impl ScanFailure {
    fn new(error: CompileError, accessed: AccessedDeps) -> Self {
        Self {
            details: Box::new(ScanFailureDetails { error, accessed }),
        }
    }
}

failure_evidence!(ScanFailure, "evaluation");

/// Every value one eager scan's sink received, in write order.
///
/// `typst_eval::eval` gives a native function its tracked `Sink`, and `Sink::value` appends
/// `(value, styles)` there. The channel has no span, so a producer that needs a position
/// writes it into the value. The sink keeps its first [`Sink::MAX_VALUES`] records and drops every
/// later write, so a capture that reaches the cap cannot say how many writes were lost.
#[derive(Debug, Clone, Default)]
pub struct CapturedValues {
    records: Vec<(Value, Option<Styles>)>,
}

impl CapturedValues {
    /// The records in write order.
    pub fn records(&self) -> &[(Value, Option<Styles>)] {
        &self.records
    }

    /// Whether the sink reached its capacity, discarding every write that followed.
    ///
    /// A saturated channel reads the same whether it was filled exactly or truncated, so a
    /// consumer cannot tell an absent record from a lost one. The sink never keeps more than
    /// [`Sink::MAX_VALUES`] records, so reaching the cap is the state a caller sees.
    pub fn is_saturated(&self) -> bool {
        self.records.len() >= Sink::MAX_VALUES
    }
}

/// Whether a traced observation can be read as every value its span produced.
///
/// [`typst::trace`] returns the values the sink kept for one span. The sink stops at
/// [`Sink::MAX_VALUES`] without recording that it did, so a span that reached the cap may have
/// lost values and a span that observed none proves nothing. Both states read as incomplete: a
/// caller that needs the whole observation has to treat them alike.
///
/// The cap is named only here and in [`CapturedValues::is_saturated`], so a reader that needs the
/// whole observation asks this rather than comparing lengths itself.
pub fn observation_is_complete(values: &[(Value, Option<Styles>)]) -> bool {
    !values.is_empty() && values.len() < Sink::MAX_VALUES
}

/// One eager scan together with the values its sink received.
///
/// A capture is not read evidence, and a source that fails after declaring still reports that
/// declaration, so it comes back beside either outcome rather than inside one:
/// [`ObservedScan::into_parts`] is the only way to reach an outcome without it.
#[derive(Debug)]
pub struct ObservedScan {
    captured: CapturedValues,
    result: Result<ScanResult, ScanFailure>,
}

impl ObservedScan {
    fn new(captured: CapturedValues, result: Result<ScanResult, ScanFailure>) -> Self {
        Self { captured, result }
    }

    /// The values the sink received, whichever way the evaluation went.
    pub fn captured(&self) -> &CapturedValues {
        &self.captured
    }

    /// The evaluation's own outcome.
    pub fn result(&self) -> Result<&ScanResult, &ScanFailure> {
        self.result.as_ref()
    }

    /// Consume the observation into the capture and the outcome.
    pub fn into_parts(self) -> (CapturedValues, Result<ScanResult, ScanFailure>) {
        (self.captured, self.result)
    }
}

impl ScanResult {
    /// The parsed main source whose spans occur in this evaluation.
    pub fn source(&self) -> &Source {
        &self.source
    }

    /// The complete eagerly evaluated module, including its exported scope.
    ///
    /// Contextual content and show rules still require document compilation.
    pub fn module(&self) -> &Module {
        &self.module
    }

    /// Run a custom extractor or tuple of extractors over eager content.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Multiple extractors (tuple)
    /// let (links, headings) = result.extract((
    ///     LinkExtractor::new(),
    ///     HeadingExtractor::new(),
    /// ));
    /// ```
    #[inline]
    pub fn extract<E: Extractor>(&self, extractor: E) -> E::Output {
        extract(&self.content, extractor)
    }

    /// Extract eager links without realizing contextual or shown content.
    #[inline]
    pub fn links(&self) -> Vec<Link> {
        self.extract(LinkExtractor::new())
    }

    /// Extract eager headings without realizing contextual or shown content.
    #[inline]
    pub fn headings(&self) -> Vec<Heading> {
        self.extract(HeadingExtractor::new())
    }

    /// Extract the first metadata value by label in native eager order.
    #[inline]
    pub fn metadata_first(&self, label: &str) -> Option<Value> {
        MetadataFirstExtractor::new(label).and_then(|e| self.extract(e))
    }

    /// One eager metadata value, distinguishing absence from duplicate declarations.
    pub fn metadata_unique(&self, label: &str) -> Result<Option<Value>, MetadataCardinalityError> {
        unique_metadata_value(label, self.metadata_all(label))
    }

    /// Extract all metadata values by label in document order.
    #[inline]
    pub fn metadata_all(&self, label: &str) -> Vec<Value> {
        MetadataValuesExtractor::new(label)
            .map(|extractor| self.extract(extractor))
            .unwrap_or_default()
    }

    /// Extract eager metadata values together with their native source spans.
    ///
    /// Spans identify construction sites, which may be helper definitions rather
    /// than callers. Eager elements have no compiled locations. Content inside
    /// another metadata value is not a declaration.
    pub fn metadata_declarations(&self, label: &str) -> Vec<MetadataDeclaration> {
        let Some(label) = label_selector(label) else {
            return Vec::new();
        };
        let mut declarations = Vec::new();
        crate::extract::visit_content(&self.content, &mut |element| {
            if element.label() == Some(label)
                && let Some(declaration) = metadata_declaration(element)
            {
                declarations.push(declaration);
            }
        });
        declarations
    }

    /// Get the raw eager module content tree.
    pub fn content(&self) -> &Content {
        &self.content
    }

    /// Get files and packages accessed during scanning.
    pub fn accessed(&self) -> &AccessedDeps {
        &self.accessed
    }

    /// Get successful file reads consumed during scanning.
    pub fn file_reads(&self) -> &[FileRead] {
        &self.accessed.reads
    }

    /// Package-directory checks observed during scanning.
    pub fn package_checks(&self) -> &[crate::world::package::PackageCheck] {
        &self.accessed.package_checks
    }

    /// Physical read paths retained by the scan.
    pub fn disk_reads(&self) -> &[DiskReadPath] {
        &self.accessed.disk_reads
    }

    /// Get scan diagnostics.
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }
}

/// Extracts the first metadata value by label.
#[derive(Debug)]
pub struct MetadataFirstExtractor {
    label: Label,
    value: Option<Value>,
}

impl MetadataFirstExtractor {
    /// Create a new metadata extractor for the given label.
    pub fn new(label: &str) -> Option<Self> {
        Some(Self {
            label: label_selector(label)?,
            value: None,
        })
    }
}

impl Extractor for MetadataFirstExtractor {
    type Output = Option<Value>;

    fn visit(&mut self, elem: &Content) {
        if self.value.is_some() {
            return;
        }

        if let Some(meta) = elem.to_packed::<MetadataElem>()
            && meta.label() == Some(self.label)
        {
            self.value = Some(meta.value.clone());
        }
    }

    fn finish(self) -> Self::Output {
        self.value
    }
}

/// Extracts all metadata values by label.
#[derive(Debug)]
pub struct MetadataValuesExtractor {
    label: Label,
    values: Vec<Value>,
}

impl MetadataValuesExtractor {
    /// Create a metadata extractor for the given label.
    pub fn new(label: &str) -> Option<Self> {
        Some(Self {
            label: label_selector(label)?,
            values: Vec::new(),
        })
    }
}

impl Extractor for MetadataValuesExtractor {
    type Output = Vec<Value>;

    fn visit(&mut self, elem: &Content) {
        if let Some(meta) = elem.to_packed::<MetadataElem>()
            && meta.label() == Some(self.label)
        {
            self.values.push(meta.value.clone());
        }
    }

    fn finish(self) -> Self::Output {
        self.values
    }
}

/// Evaluate an explicitly constructed world without realization or layout.
pub fn scan_world(world: &TypstWorld) -> Result<ScanResult, CompileError> {
    scan_world_with_evidence(world).map_err(|failure| failure.into_parts().1)
}

/// Evaluate an explicitly constructed world and retain dependency evidence on failure.
///
/// The same evaluation as [`scan_world_observed`], for a caller with no protocol of its own: it
/// keeps the outcome and discards the captured values.
pub fn scan_world_with_evidence(world: &TypstWorld) -> Result<ScanResult, ScanFailure> {
    scan_world_observed(world, []).into_parts().1
}

/// Evaluate an explicitly constructed world and keep every value its sink received.
///
/// `header` is written into the sink before evaluation, in order, so a host can give its protocol
/// records of its own ahead of anything the evaluated source writes. The capture comes back on the
/// failed path too: a source that stops after writing still reports what it wrote. A source that
/// cannot be read at all is never evaluated, so its capture is empty.
pub fn scan_world_observed(
    world: &TypstWorld,
    header: impl IntoIterator<Item = Value>,
) -> ObservedScan {
    let session = CompileSession::start(world);
    let source = match session.source(session.main()) {
        Ok(source) => source,
        Err(error) => {
            let diagnostic =
                SourceDiagnostic::error(Span::detached(), "could not read this source")
                    .with_hint(unreadable_file_help(&error));
            let error = session.compilation_error([diagnostic]);
            return ObservedScan::new(
                CapturedValues::default(),
                Err(ScanFailure::new(error, session.finish())),
            );
        }
    };

    let mut sink = Sink::new();
    for value in header {
        sink.value(value, None);
    }

    let traced = Traced::default();
    let route = Route::default();
    let result = typst_eval::eval(
        (&session as &dyn World).track(),
        session.library(),
        traced.track(),
        sink.track_mut(),
        route.track(),
        &source,
    );

    // `values(self)` and `warnings(self)` both consume the sink, so the capture is read first,
    // from a copy; `delayed(&mut self)` only borrows it.
    let captured = CapturedValues {
        records: sink.clone().values().into_iter().collect(),
    };
    let delayed = sink.delayed();
    let warnings = sink.warnings().to_vec();
    if let Some(error) = world.font_failure() {
        return ObservedScan::new(
            captured,
            Err(ScanFailure::new(error.into(), session.finish())),
        );
    }

    let module = match result {
        Ok(module) => module,
        Err(errors) => {
            let error = session.compilation_error(errors.into_iter().chain(warnings));
            return ObservedScan::new(captured, Err(ScanFailure::new(error, session.finish())));
        }
    };

    if !delayed.is_empty() {
        let error = session.compilation_error(delayed.into_iter().chain(warnings));
        return ObservedScan::new(captured, Err(ScanFailure::new(error, session.finish())));
    }

    let (accessed, diagnostics, _) = session.finish_with_diagnostics(warnings);
    ObservedScan::new(
        captured,
        Ok(ScanResult {
            source,
            content: module.clone().content(),
            module,
            accessed,
            diagnostics,
        }),
    )
}

/// The next action for one unreadable file, when Typst named a plain reason.
fn unreadable_file_help(error: &typst::diag::FileError) -> &'static str {
    use typst::diag::{FileError, PackageError};
    match error {
        FileError::NotFound(_) => "Create the file, or fix the path that reads it",
        FileError::AccessDenied => "Make the file readable",
        FileError::IsDirectory => "Replace the directory with a file, or point the path at a file",
        FileError::NotSource => "Give the file a Typst source extension",
        FileError::InvalidUtf8 => "Save the file as UTF-8",
        FileError::Realize(_) => "Use a path that is valid on this platform",
        FileError::Package(PackageError::NotFound(_)) => {
            "Install the package, or fix the version in the import"
        }
        FileError::Package(PackageError::VersionNotFound(_, _)) => {
            "Install that version of the package, or import the installed one"
        }
        FileError::Package(PackageError::NetworkFailed(_)) => {
            "Check the network connection, then run the build again"
        }
        FileError::Package(_) => "Install the package again",
        FileError::Other(_) => "Check the path, and that Tola may read the file",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::TempDir;
    use typst::diag::SourceResult;
    use typst::engine::Engine;
    use typst::foundations::{Func, NativeFunc, func};
    use typst::utils::LazyHash;
    use typst::{Feature, Features, Library, LibraryExt};

    fn fontless_world(root: &Path, file: &Path) -> TypstWorld {
        TypstWorld::builder(file, root)
            .with_local_cache()
            .no_fonts()
            .build(&crate::BundleCancellation::default())
            .unwrap()
    }

    /// Scan one source file in `root`.
    fn scan(root: &Path, file: &Path) -> ScanResult {
        scan_world(&fontless_world(root, file)).unwrap()
    }

    // How many times the recording native ran on this thread. A scan that leaves the count
    // unchanged was served from the memoized evaluation instead of run again.
    thread_local! {
        static RUNS: Cell<usize> = const { Cell::new(0) };
    }

    /// Append `payload` to the scan's sink: the shape one of a host's own natives writes records
    /// with.
    #[func]
    fn record(engine: &mut Engine, payload: Value) -> SourceResult<Value> {
        RUNS.with(|runs| runs.set(runs.get() + 1));
        engine.sink.value(payload, None);
        Ok(Value::None)
    }

    /// The runs of the recording native since the last read, on this thread.
    fn runs() -> usize {
        RUNS.with(|runs| runs.replace(0))
    }

    /// The library a recording world compiles with.
    fn recording_library() -> Arc<LazyHash<Library>> {
        let mut library = Library::builder()
            .with_features(Features::from_iter([Feature::Html, Feature::Bundle]))
            .build();
        library
            .global
            .scope_mut()
            .define("record", Func::from(record::data()));
        Arc::new(LazyHash::new(library))
    }

    /// The world one recording test scans `file` in.
    fn recording_world(root: &Path, file: &Path) -> TypstWorld {
        TypstWorld::builder(file, root)
            .with_local_cache()
            .no_fonts()
            .with_shared_library(recording_library())
            .build(&crate::BundleCancellation::default())
            .unwrap()
    }

    /// The captured values of one observation, in write order.
    fn captured(scan: &ObservedScan) -> Vec<Value> {
        scan.captured()
            .records()
            .iter()
            .map(|(value, _)| value.clone())
            .collect()
    }

    #[test]
    fn one_evaluation_supplies_all_reads() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("document.typ");
        fs::write(
            &path,
            "#let title = [Document]\n#let rank = 3\n#metadata(rank) <rank>\n= #title",
        )
        .unwrap();

        let scan = scan(directory.path(), &path);

        assert!(matches!(
            scan.module().field("title", ()).unwrap(),
            Value::Content(_)
        ));
        assert_eq!(scan.module().field("rank", ()).unwrap(), &Value::Int(3));
        assert_eq!(scan.metadata_first("rank"), Some(Value::Int(3)));
        assert_eq!(scan.headings()[0].text, "Document");
        assert_eq!(scan.module().clone().content(), *scan.content());
        assert_eq!(scan.file_reads().len(), 1);
        let declaration = scan.metadata_declarations("rank").pop().unwrap();
        assert_eq!(declaration.span().id(), Some(scan.source().id()));
        let range = scan.source().find(declaration.span()).unwrap().range();
        assert_eq!(&scan.source().text()[range], "metadata(rank)");
    }

    #[test]
    fn failed_scan_retains_the_missing_path() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.typ");
        let missing = std::fs::canonicalize(dir.path())
            .unwrap()
            .join("missing.txt");
        fs::write(&file, "#read(\"missing.txt\")").unwrap();
        let world = fontless_world(dir.path(), &file);

        let failure = scan_world_with_evidence(&world).unwrap_err();

        assert!(failure.diagnostics().is_some());
        assert!(
            failure
                .disk_reads()
                .iter()
                .any(|path| path.as_path() == missing),
            "missing disk path was not retained: {:?}",
            failure.disk_reads()
        );
    }

    #[test]
    fn repeated_scan_replays_capture() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("repeated.typ");
        fs::write(&path, "#record(\"from-cache\")\n").unwrap();
        let world = recording_world(directory.path(), &path);

        let first = scan_world_observed(&world, []);
        let first_runs = runs();
        let second = scan_world_observed(&world, []);
        let second_runs = runs();

        assert_eq!(first_runs, 1, "the first scan ran the native");
        assert_eq!(
            second_runs, 0,
            "the second scan reused the memoized evaluation"
        );
        assert_eq!(captured(&first), [Value::Str("from-cache".into())]);
        assert_eq!(
            captured(&second),
            captured(&first),
            "the reused evaluation wrote its records back into the sink"
        );
    }
}
