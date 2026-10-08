//! Compile-scoped world and access evidence.

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use typst::Library;
use typst::World;
use typst::diag::{FileError, FileResult, SourceDiagnostic};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, Source};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;

use crate::diagnostic::package_imports::PackageImporters;
use crate::diagnostic::{CompileError, Diagnostics, NativeDiagnostic};
use crate::world::TypstWorld;
use crate::world::file::{
    DiskReadPath, FileRead, FileSnapshot, Loaded, ReadAttempt, ReadLocator, ReadOrigin,
};
use crate::world::package::PackageCheck;

/// File inputs observed during one compilation or scan.
///
/// Records runtime-resolved paths, not a directory inventory.
/// [`Self::disk_reads`] retains failed physical reads so callers can recover
/// when missing inputs are created.
/// Cached reuse retains prior read evidence, including physical paths and package checks.
#[derive(Debug, Clone, Default)]
pub struct AccessedDeps {
    /// Successful reads consumed by the operation.
    pub reads: Vec<FileRead>,
    /// Retained physical read paths, including failed reads.
    pub disk_reads: Vec<DiskReadPath>,
    /// Package-directory checks that determined package selection.
    pub package_checks: Vec<PackageCheck>,
}

impl AccessedDeps {
    /// Whether the operation recorded no successful reads, physical read paths, or
    /// package-selection checks.
    pub fn is_empty(&self) -> bool {
        self.reads.is_empty() && self.disk_reads.is_empty() && self.package_checks.is_empty()
    }

    /// Number of successful file reads.
    pub fn read_count(&self) -> usize {
        self.reads.len()
    }

    /// Number of retained physical read paths.
    pub fn disk_read_count(&self) -> usize {
        self.disk_reads.len()
    }

    /// Number of package-directory checks used for package selection.
    pub fn package_check_count(&self) -> usize {
        self.package_checks.len()
    }

    /// Iterate over successful reads.
    pub fn reads(&self) -> &[FileRead] {
        &self.reads
    }

    /// Retained physical read paths.
    pub fn disk_reads(&self) -> &[DiskReadPath] {
        &self.disk_reads
    }

    /// Iterate over package-selection checks.
    pub fn package_checks(&self) -> &[PackageCheck] {
        &self.package_checks
    }
}

/// Generate the read-evidence accessors of a failed operation.
///
/// The type owns one boxed `details` value with `error` and `accessed` fields.
/// `stopped` names the operation in the generated documentation.
macro_rules! failure_evidence {
    ($failure:ident, $stopped:literal) => {
        impl $failure {
            #[doc = concat!("Files and packages observed before ", $stopped, " stopped.")]
            pub fn accessed(&self) -> &$crate::session::AccessedDeps {
                &self.details.accessed
            }

            #[doc = concat!("Successful file reads observed before ", $stopped, " stopped.")]
            pub fn file_reads(&self) -> &[$crate::world::file::FileRead] {
                self.details.accessed.reads()
            }

            #[doc = concat!("Physical read paths retained before ", $stopped, " stopped.")]
            pub fn disk_reads(&self) -> &[$crate::world::file::DiskReadPath] {
                self.details.accessed.disk_reads()
            }

            #[doc = concat!("Package-directory checks observed before ", $stopped, " stopped.")]
            pub fn package_checks(&self) -> &[$crate::world::package::PackageCheck] {
                self.details.accessed.package_checks()
            }

            #[doc = concat!("Structured diagnostics when ", $stopped, " failed.")]
            pub fn diagnostics(&self) -> Option<&$crate::diagnostic::Diagnostics> {
                self.details.error.diagnostics()
            }

            /// Borrow the underlying structured error.
            pub fn error(&self) -> &$crate::diagnostic::CompileError {
                &self.details.error
            }

            /// Consume the failure into observed inputs and the underlying error.
            pub fn into_parts(
                self,
            ) -> (
                $crate::session::AccessedDeps,
                $crate::diagnostic::CompileError,
            ) {
                let details = *self.details;
                (details.accessed, details.error)
            }
        }

        impl std::fmt::Display for $failure {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.details.error.fmt(formatter)
            }
        }

        impl std::error::Error for $failure {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.details.error)
            }
        }
    };
}

pub(crate) use failure_evidence;

pub(crate) struct CompileSession<'a> {
    world: &'a TypstWorld,
    /// The file this session compiles: the world's own main, unless a caller chose another file.
    main: FileId,
    snapshot: Arc<FileSnapshot>,
    files: SessionFiles,
}

impl<'a> CompileSession<'a> {
    pub(crate) fn start(world: &'a TypstWorld) -> Self {
        Self::start_at(world, world.main())
    }

    /// Compile `main` of `world`, which is how an editor answers about a file the site's own
    /// program never reached: the same imports, packages, and fonts compile that file alone.
    pub(crate) fn start_at(world: &'a TypstWorld, main: FileId) -> Self {
        Self {
            world,
            main,
            snapshot: world.file_snapshot(),
            files: SessionFiles::default(),
        }
    }

    /// Consume the session after all compilation and diagnostic reads are complete.
    pub(crate) fn finish(self) -> AccessedDeps {
        self.files.finish()
    }

    /// Resolve warnings before freezing evidence: source locations can read files.
    ///
    /// The importers come back with the diagnostics so a caller that retains the compilation
    /// resolves the diagnostics its own producers report later.
    pub(crate) fn finish_with_diagnostics(
        self,
        warnings: impl IntoIterator<Item = SourceDiagnostic>,
    ) -> (AccessedDeps, Diagnostics, PackageImporters) {
        let importers = self.package_importers();
        let native = warnings.into_iter().map(NativeDiagnostic::from).collect();
        let diagnostics = self.resolve(native, &importers);
        (self.finish(), diagnostics, importers)
    }

    pub(crate) fn compilation_error(
        &self,
        diagnostics: impl IntoIterator<Item = SourceDiagnostic>,
    ) -> CompileError {
        let raw = diagnostics
            .into_iter()
            .map(NativeDiagnostic::from)
            .collect::<Vec<_>>();
        CompileError::from_resolved(self.resolve(raw, &self.package_importers()))
    }

    /// The site files that import each package this session read.
    fn package_importers(&self) -> PackageImporters {
        let slots = self.files.slots.read();
        let packages = slots
            .values()
            .filter_map(|slot| match slot.id.root() {
                typst::syntax::VirtualRoot::Package(package) => Some(package.to_string()),
                typst::syntax::VirtualRoot::Project => None,
            })
            .collect::<BTreeSet<_>>();
        let sources = slots
            .values()
            .filter_map(|slot| {
                let attempt = slot.source.get()?;
                let loaded = attempt.result.as_ref().ok()?;
                let path = site_path(loaded.read.evidence().locator())?;
                Some((path, loaded.value.clone()))
            })
            .collect::<Vec<_>>();
        drop(slots);
        PackageImporters::collect(sources, &packages)
    }

    /// Resolve raw diagnostics against this session's world.
    ///
    /// A diagnostic Typst locates inside a package names the site files that import that
    /// package, because Typst attaches no call site for it. A diagnostic about a package this
    /// compilation could not install names every directory checked.
    fn resolve(&self, raw: Vec<NativeDiagnostic>, importers: &PackageImporters) -> Diagnostics {
        let mut diagnostics =
            Diagnostics::resolve_owned(self, raw, crate::SourceContextLimit::default());
        diagnostics.attach_imported_by(|package| importers.importing(package).to_vec());
        let failures = self.package_failures();
        if !failures.is_empty() {
            diagnostics.attach_package_failures(|diagnostic| {
                let package = self.imported_package(diagnostic.source())?;
                failures
                    .iter()
                    .find(|failure| failure.package == package)
                    .cloned()
            });
        }
        diagnostics
    }

    /// Every package this compilation could not provide, with the directories checked.
    ///
    /// Reads are recorded once per file, so a failed package read appears once even
    /// when several diagnostics report it.
    fn package_failures(&self) -> Vec<crate::diagnostic::ResolvedPackageFailure> {
        let mut failures = Vec::new();
        for slot in self.files.slots.read().values() {
            let typst::syntax::VirtualRoot::Package(package) = slot.id.root() else {
                continue;
            };
            let failed = unresolved_package(slot.source.get())
                .or_else(|| unresolved_package(slot.bytes.get()));
            let Some((checks, reason)) = failed else {
                continue;
            };
            let searched = checks
                .iter()
                .map(|check| crate::diagnostic::ResolvedPackageSearch {
                    root: check.root().to_path_buf(),
                    candidate: check.candidate().to_path_buf(),
                })
                .collect::<Vec<_>>();
            failures.push(crate::diagnostic::ResolvedPackageFailure {
                package: package.to_string(),
                reason,
                searched,
            });
        }
        failures.sort_by(|left, right| left.package.cmp(&right.package));
        failures.dedup();
        failures
    }

    /// The package this diagnostic imports, when it imports one.
    fn imported_package(&self, diagnostic: &SourceDiagnostic) -> Option<String> {
        let id = diagnostic.span.id()?;
        let slot = self.files.slots.read().get(&id).cloned()?;
        let attempt = slot.source.get()?;
        let loaded = attempt.result.as_ref().ok()?;
        crate::diagnostic::package_imports::imported_package(&loaded.value, diagnostic.span)
    }

    fn read_source(&self, id: FileId) -> FileResult<Source> {
        self.files.slot(id).source(self.world, &self.snapshot)
    }

    fn read_file(&self, id: FileId) -> FileResult<Bytes> {
        self.files.slot(id).file(self.world, &self.snapshot)
    }
}

/// What a read that never reached an installed package observed.
fn unresolved_package<T>(
    attempt: Option<&ReadAttempt<T>>,
) -> Option<(
    &[PackageCheck],
    crate::diagnostic::ResolvedPackageFailureReason,
)> {
    let attempt = attempt?;
    let reason = unresolved_package_reason(attempt.result.as_ref().err()?)?;
    Some((&attempt.package_checks, reason))
}

/// Why a package could not be provided, when a read never reached one.
///
/// A package that is present but missing a file inside it fails differently, and a
/// version Typst itself names fails differently again, so neither answers here.
fn unresolved_package_reason(
    error: &FileError,
) -> Option<crate::diagnostic::ResolvedPackageFailureReason> {
    use crate::diagnostic::ResolvedPackageFailureReason;
    use typst::diag::PackageError;

    match error {
        FileError::Package(PackageError::NotFound(_)) => {
            Some(ResolvedPackageFailureReason::NotInstalled)
        }
        FileError::Package(
            PackageError::NetworkFailed(_)
            | PackageError::MalformedArchive(_)
            | PackageError::Other(_),
        ) => Some(ResolvedPackageFailureReason::Unavailable),
        _ => None,
    }
}

/// The compilation-relative path of a site file, when a read locator names one.
fn site_path(locator: &ReadLocator) -> Option<String> {
    match locator {
        ReadLocator::Root(path) | ReadLocator::ProvidedRoot(path) => {
            Some(path.to_string_lossy().into_owned())
        }
        _ => None,
    }
}

fn read_origin_sort_key(origin: &ReadOrigin) -> (u8, Option<std::path::PathBuf>) {
    match origin {
        ReadOrigin::Disk(path) => (0, Some(path.as_path().to_path_buf())),
        ReadOrigin::Provider => (1, None),
        ReadOrigin::NonPersistent => (2, None),
    }
}

impl World for CompileSession<'_> {
    fn library(&self) -> &LazyHash<Library> {
        self.world.library()
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.world.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        self.read_source(id)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.read_file(id)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.world.font(index)
    }

    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        self.world.today(offset)
    }
}

struct SessionFile {
    id: FileId,
    source: OnceLock<ReadAttempt<Loaded<Source>>>,
    bytes: OnceLock<ReadAttempt<Loaded<Bytes>>>,
}

impl SessionFile {
    fn source(&self, world: &TypstWorld, files: &FileSnapshot) -> FileResult<Source> {
        self.source
            .get_or_init(|| world.source_from_read(files, self.id, files.read(self.id)))
            .result
            .as_ref()
            .map(|loaded| loaded.value.clone())
            .map_err(Clone::clone)
    }

    fn file(&self, world: &TypstWorld, files: &FileSnapshot) -> FileResult<Bytes> {
        self.bytes
            .get_or_init(|| world.file_from_read(files, self.id, files.read(self.id)))
            .result
            .as_ref()
            .map(|loaded| loaded.value.clone())
            .map_err(Clone::clone)
    }
}

struct SessionFiles {
    slots: RwLock<FxHashMap<FileId, Arc<SessionFile>>>,
}

impl Default for SessionFiles {
    fn default() -> Self {
        Self {
            slots: RwLock::new(FxHashMap::default()),
        }
    }
}

impl SessionFiles {
    fn slot(&self, id: FileId) -> Arc<SessionFile> {
        if let Some(slot) = self.slots.read().get(&id) {
            return Arc::clone(slot);
        }

        Arc::clone(self.slots.write().entry(id).or_insert_with(|| {
            Arc::new(SessionFile {
                id,
                source: OnceLock::new(),
                bytes: OnceLock::new(),
            })
        }))
    }

    /// Freeze the union of every initialized slot's observed inputs.
    fn finish(self) -> AccessedDeps {
        let mut deps = AccessedDeps::default();
        for slot in self.slots.into_inner().into_values() {
            extend_observed_inputs(&mut deps, slot.source.get());
            extend_observed_inputs(&mut deps, slot.bytes.get());
        }
        deps.reads.sort_by_cached_key(|read| {
            (
                read.evidence().locator().sort_key(),
                read.evidence().digest().to_hex(),
                read_origin_sort_key(read.origin()),
            )
        });
        deps.reads.dedup();
        deps.disk_reads.sort();
        deps.disk_reads.dedup();
        deps.package_checks.sort_by_cached_key(|check| {
            (
                check.package().to_string(),
                check.tier() as u8,
                check.candidate().to_path_buf(),
                check.availability() as u8,
                check.was_selected(),
            )
        });
        deps.package_checks.dedup();
        deps
    }
}

/// Add every input one initialized slot attempt observed to the frozen evidence.
fn extend_observed_inputs<T>(deps: &mut AccessedDeps, attempt: Option<&ReadAttempt<T>>) {
    let Some(attempt) = attempt else {
        return;
    };
    deps.reads.extend(attempt.reads.iter().cloned());
    deps.disk_reads.extend(attempt.disk_reads.iter().cloned());
    deps.package_checks
        .extend(attempt.package_checks.iter().cloned());
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tempfile::TempDir;
    use typst::World;

    use super::{CompileSession, unresolved_package_reason};
    use crate::world::TypstWorld;
    use crate::world::file::{FileProvider, FileResolver, FileTarget, SharedFileCache, file_id};

    struct ChangingFiles {
        reads: AtomicUsize,
    }

    impl FileProvider for ChangingFiles {
        fn target(&self, id: typst::syntax::FileId) -> Option<FileTarget> {
            (id == file_id("/changing.typ")).then(|| {
                let version = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
                FileTarget::Bytes(Arc::from(format!("= Version {version}").into_bytes()))
            })
        }
    }

    /// The `@preview/cetz` file one session test reads, without a package directory.
    struct PreviewFiles;

    impl FileProvider for PreviewFiles {
        fn target(&self, id: typst::syntax::FileId) -> Option<FileTarget> {
            let typst::syntax::VirtualRoot::Package(package) = id.root() else {
                return None;
            };
            let spec: crate::world::package::PackageSpec = "@preview/cetz:0.3.4".parse().unwrap();
            (*package == spec)
                .then(|| FileTarget::Bytes(Arc::from(&b"#let canvas(body) = body"[..])))
        }
    }

    fn world_with(
        dir: &TempDir,
        main: &str,
        source: &str,
        configure: impl FnOnce(crate::world::WorldBuilder) -> crate::world::WorldBuilder,
    ) -> TypstWorld {
        let path = dir.path().join(main);
        fs::write(&path, source).unwrap();
        configure(
            TypstWorld::builder(&path, dir.path())
                .with_local_cache()
                .no_fonts(),
        )
        .build(&crate::BundleCancellation::default())
        .expect("valid test world")
    }

    fn world_for(dir: &TempDir, main: &str, source: &str) -> TypstWorld {
        world_with(dir, main, source, |builder| builder)
    }

    #[test]
    fn importers_name_the_package() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("templates")).unwrap();
        fs::write(
            dir.path().join("templates/page.typ"),
            "#import \"@preview/cetz:0.3.4\": canvas\n",
        )
        .unwrap();
        fs::write(dir.path().join("content.typ"), "no imports here\n").unwrap();
        let files = FileResolver::new().with_provider(PreviewFiles);
        let world = world_with(
            &dir,
            "main.typ",
            "#import \"templates/page.typ\": page\n#page()",
            |builder| builder.with_files(Arc::new(files)),
        );

        let session = CompileSession::start(&world);
        for path in ["main.typ", "templates/page.typ", "content.typ"] {
            session.source(file_id(path)).unwrap();
        }
        let package = typst::syntax::FileId::new(typst::syntax::RootedPath::new(
            typst::syntax::VirtualRoot::Package("@preview/cetz:0.3.4".parse().unwrap()),
            typst::syntax::VirtualPath::new("lib.typ").unwrap(),
        ));
        session.source(package).unwrap();
        let importers = session.package_importers();

        assert_eq!(
            importers.importing("@preview/cetz:0.3.4"),
            ["templates/page.typ"]
        );
        assert!(importers.importing("@preview/absent:1.0.0").is_empty());
    }

    #[test]
    fn repeated_reads_report_once() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("templated.typ"), "= Templated").unwrap();
        let world = world_for(&dir, "main.typ", "= Main");
        let id = file_id("templated.typ");

        let once = CompileSession::start(&world);
        once.source(id).unwrap();
        let once = once.finish();
        assert_eq!(once.read_count(), 1);

        let repeated = CompileSession::start(&world);
        for _ in 0..3 {
            assert!(repeated.source(id).unwrap().text().contains("Templated"));
        }
        let repeated = repeated.finish();

        assert_eq!(repeated.reads, once.reads);
        assert_eq!(repeated.read_count(), 1);
    }

    #[cfg(feature = "parallel")]
    #[test]
    fn collects_reads_from_rayon_workers() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("first.typ"), "= First").unwrap();
        fs::write(dir.path().join("second.typ"), "= Second").unwrap();
        let world = world_for(&dir, "main.typ", "= Main");
        let expected = [
            world.root().join("first.typ"),
            world.root().join("second.typ"),
        ];
        let session = CompileSession::start(&world);

        let (first, second) = rayon::join(
            || session.source(file_id("first.typ")),
            || session.source(file_id("second.typ")),
        );
        assert!(first.is_ok());
        assert!(second.is_ok());

        let accessed = session.finish();
        assert_eq!(
            accessed
                .disk_reads
                .iter()
                .map(|path| path.as_path())
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(std::path::PathBuf::as_path)
                .collect::<Vec<_>>()
        );
        assert_eq!(accessed.reads.len(), 2);
    }

    #[test]
    fn each_session_observes_one_file_version() {
        let dir = TempDir::new().unwrap();
        let main = dir.path().join("main.typ");
        let cache = Arc::new(SharedFileCache::new());
        let first_world = world_with(&dir, "main.typ", "= Version 1", |builder| {
            builder.with_shared_cache(Arc::clone(&cache))
        });
        let second_world = world_with(&dir, "main.typ", "= Version 1", |builder| {
            builder.with_shared_cache(cache)
        });
        let first = CompileSession::start(&first_world);

        assert!(
            first
                .source(first.main())
                .unwrap()
                .text()
                .contains("Version 1")
        );
        fs::write(&main, "= Version 2").unwrap();

        let second = CompileSession::start(&second_world);
        assert!(
            second
                .source(second.main())
                .unwrap()
                .text()
                .contains("Version 2")
        );
        assert!(
            first
                .source(first.main())
                .unwrap()
                .text()
                .contains("Version 1")
        );

        let first = first.finish();
        let second = second.finish();
        assert_eq!(first.reads.len(), 1);
        assert_eq!(second.reads.len(), 1);
        assert_ne!(
            first.reads[0].evidence().digest(),
            second.reads[0].evidence().digest()
        );
    }

    #[test]
    fn source_and_file_share_bytes() {
        let dir = TempDir::new().unwrap();
        let files = FileResolver::new().with_provider(ChangingFiles {
            reads: AtomicUsize::new(0),
        });
        let world = world_with(&dir, "main.typ", "= Main", |builder| {
            builder.with_files(Arc::new(files))
        });
        let session = CompileSession::start(&world);
        let id = file_id("changing.typ");

        let source = session.source(id).unwrap();
        assert!(source.text().contains("Version 1"));

        let bytes = session.file(id).unwrap();
        assert_eq!(bytes.as_slice(), source.text().as_bytes());
    }

    #[test]
    fn failed_source_keeps_original_bytes() {
        let dir = TempDir::new().unwrap();
        let cache = Arc::new(SharedFileCache::new());
        let world = world_with(&dir, "main.typ", "", |builder| {
            builder.with_shared_cache(cache)
        });
        let path = dir.path().join("broken.typ");
        fs::write(&path, [0xff]).unwrap();
        let id = file_id("broken.typ");
        let session = CompileSession::start(&world);
        assert_eq!(
            session.source(id).unwrap_err(),
            typst::diag::FileError::InvalidUtf8
        );

        fs::write(&path, "fixed").unwrap();
        assert_eq!(session.file(id).unwrap().as_slice(), &[0xff]);
        let accessed = session.finish();
        assert_eq!(
            accessed.reads[0].evidence().digest(),
            crate::ContentDigest::of(&[0xff])
        );
        assert_eq!(
            CompileSession::start(&world).source(id).unwrap().text(),
            "fixed"
        );
    }

    #[test]
    fn missing_source_stays_missing() {
        let dir = TempDir::new().unwrap();
        let world = world_with(&dir, "main.typ", "", |builder| {
            builder.with_shared_cache(Arc::new(SharedFileCache::new()))
        });
        let id = file_id("missing.typ");
        let session = CompileSession::start(&world);
        assert!(session.source(id).is_err());
        fs::write(dir.path().join("missing.typ"), "created").unwrap();
        assert!(session.file(id).is_err());
        assert!(session.finish().reads.is_empty());
        assert_eq!(
            CompileSession::start(&world).source(id).unwrap().text(),
            "created"
        );
    }

    #[test]
    fn package_errors_map_to_failure_reasons() {
        use crate::diagnostic::ResolvedPackageFailureReason;
        use typst::diag::{FileError, PackageError};

        let spec: crate::world::package::PackageSpec = "@preview/cetz:0.3.4".parse().unwrap();
        for (label, error, expected) in [
            (
                "not installed",
                FileError::Package(PackageError::NotFound(spec.clone())),
                Some(ResolvedPackageFailureReason::NotInstalled),
            ),
            (
                "download failed",
                FileError::Package(PackageError::NetworkFailed(None)),
                Some(ResolvedPackageFailureReason::Unavailable),
            ),
            (
                "archive malformed",
                FileError::Package(PackageError::MalformedArchive(None)),
                Some(ResolvedPackageFailureReason::Unavailable),
            ),
            (
                "could not be stored",
                FileError::Package(PackageError::Other(None)),
                Some(ResolvedPackageFailureReason::Unavailable),
            ),
            (
                "version named by Typst",
                FileError::Package(PackageError::VersionNotFound(
                    spec,
                    crate::world::package::PackageVersion {
                        major: 0,
                        minor: 3,
                        patch: 4,
                    },
                )),
                None,
            ),
            (
                "a file inside the package",
                FileError::NotFound(std::path::PathBuf::from("lib.typ")),
                None,
            ),
        ] {
            assert_eq!(unresolved_package_reason(&error), expected, "{label}");
        }
    }
}
