//! Re-exports for glob imports.
//!
//! ```ignore
//! use tola_typst::prelude::*;
//! ```

pub use crate::bundle::{
    BundleBytes, BundleCancellation, BundleCompilation, BundleCompileFailure, BundleDocumentKind,
    BundleEntries, BundleEntry, BundleEntryKind, BundleExport, CompiledBundleDocument,
    compile_bundle_world,
};
pub use crate::compile::{
    CompileFailure, CompileResult, compile_source_with_evidence, compile_world,
    compile_world_with_evidence,
};
pub use crate::session::AccessedDeps;
pub use crate::world::SourceSnapshot;
pub use typst_bundle::BundleOptions;

#[cfg(feature = "scan")]
pub use crate::compile::scan::{
    CapturedValues, Extractor, Heading, HeadingExtractor, Link, LinkExtractor, LinkSource,
    MetadataFirstExtractor, MetadataValuesExtractor, ObservedScan, ScanFailure, ScanResult,
    extract, scan_world, scan_world_observed, scan_world_with_evidence,
};

pub use crate::diagnostic::{
    CompileError, Diagnostic, DiagnosticFilter, DiagnosticOrigin, DiagnosticSeverity,
    DiagnosticSummary, Diagnostics, FilterType, Hint, LocationFailure, NativeDiagnostic,
    PackageKind, ProducerDiagnosticOrigin, ResolvedPackageFailure, ResolvedPackageFailureReason,
    ResolvedPackageSearch, ResolvedSource, SourceContextLimit, SourceDiagnostic, SourceLine,
    SourceLocation, SourcePosition, SourceRange, SourceTruncation, Trace, TraceKind,
};
#[cfg(feature = "legacy-serialization")]
#[allow(deprecated)]
pub use crate::diagnostic::{
    ResolvedDiagnostic, ResolvedHint, ResolvedLocationFailure, ResolvedSeverity,
    ResolvedSourceLine, ResolvedSpan, ResolvedTrace, ResolvedTraceKind, ResolvedTruncation,
};

pub use crate::introspection::{MetadataCardinalityError, MetadataDeclaration};

pub use crate::world::file::{
    CandidateFileSnapshot, ContentDigest, DiskReadPath, EmptyFiles, FileMap, FileProvider,
    FileRead, FileResolver, FileTarget, ReadEvidence, ReadLocator, ReadOrigin, SharedFileCache,
    file_id, file_id_from_path, hash_length_prefixed, virtual_file_id,
};

pub use crate::world::font::{FontLoadError, FontOptions, FontStore};

pub use crate::world::library::{GLOBAL_LIBRARY, create_library_with_inputs};

pub use crate::world::{
    SourceBoundary, SourceRefusal, TypstWorld, WorldBuildError, WorldBuilder, normalize_path,
};

pub use crate::world::package;
pub use crate::world::package::{
    PackageAvailability, PackageCheck, PackageFetchPolicy, PackageLocation, PackageLocationSource,
    PackageLocations, PackagePreparationFailure, PackageSpec, PackageStore, PackageTier,
    PackageVersion, PreparedPackage, sort_package_checks,
};

pub use crate::html::{
    HtmlBaseHref, HtmlDocument, HtmlDocumentInventory, HtmlFragment, HtmlFragmentError,
    HtmlFragmentExport, HtmlFragmentKind, HtmlFragmentOptions, HtmlFragmentSelection,
    HtmlFrameStyleError, HtmlRawText, HtmlRawTextError, HtmlReference, HtmlReferenceUse,
};
pub use typst_html::HtmlOptions;
