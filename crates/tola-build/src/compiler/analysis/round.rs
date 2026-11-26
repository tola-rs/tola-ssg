use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use tola_typst::{Diagnostics, FileRead, PackageCheck};
use typst::World;

use crate::compiler::SiteHost;
use crate::config::SiteConfig;
use crate::content::{ContentSource, SourceFileInput, SourceSet, SourcesInput};

use super::settlement::{RoundResult, reads_tola_capability};
use super::{ANALYSIS_INPUT_IDENTITY, SourceAnalysisCache};

const SOURCE_ENTRY_OVERLAY_DIVISOR: usize = 8;
const MIN_SOURCE_ENTRY_OVERLAY: usize = 8;
const MAX_SOURCE_ENTRY_OVERLAY: usize = 128;

pub(super) struct AnalyzedSource {
    pub(super) id: crate::content::ContentId,
    pub(super) layout: crate::content::ContentSourceLayout,
    pub(super) source: PathBuf,
    pub(super) evidence: Vec<FileRead>,
    pub(super) package_checks: Vec<PackageCheck>,
    pub(super) diagnostics: Diagnostics,
    pub(super) package_bindings: crate::package::SiteBindings,
    pub(super) sources_input: Option<SourcesInput>,
    pub(super) source_reads: Vec<(typst::syntax::RootedPath, Option<Arc<SourceFileInput>>)>,
    pub(super) declaration: Option<crate::content::SourceMetadataDeclaration>,
    /// Where this source calls the retired `<tola-meta>` label.
    pub(super) legacy_label: Option<tola_diagnostic::Location>,
}

impl AnalyzedSource {
    pub(super) fn belongs_to(&self, src: &ContentSource) -> bool {
        self.source == src.source() && self.id == *src.id() && self.layout == src.source_layout()
    }

    pub(super) fn inputs_match(
        &self,
        bindings: &crate::package::SiteBindings,
        srcs: &SourcesInput,
    ) -> bool {
        self.package_bindings.matches(bindings)
            && self
                .sources_input
                .as_ref()
                .is_none_or(|cached| cached == srcs)
            && self
                .source_reads
                .iter()
                .all(|(file, cached)| srcs.get(file) == cached.as_ref())
    }
}

type AnalyzedSources = BTreeMap<PathBuf, Arc<AnalyzedSource>>;

/// Deterministic source-entry table with a shared base and bounded overlay.
#[derive(Clone, Default)]
pub(super) struct SourceEntryIndex {
    base: Arc<AnalyzedSources>,
    overlay: Arc<AnalyzedSources>,
    pub(super) ordered_paths: Arc<[PathBuf]>,
}

impl SourceEntryIndex {
    pub(super) fn from_ordered(entries: Vec<Arc<AnalyzedSource>>, prev: &Self) -> Self {
        let same_paths = prev.ordered_paths.len() == entries.len()
            && prev
                .ordered_paths
                .iter()
                .zip(&entries)
                .all(|(path, entry)| path == &entry.source);
        if !same_paths {
            let paths: Vec<_> = entries.iter().map(|entry| entry.source.clone()).collect();
            let base = entries
                .into_iter()
                .map(|entry| (entry.source.clone(), entry))
                .collect();
            return Self {
                base: Arc::new(base),
                overlay: Arc::default(),
                ordered_paths: paths.into(),
            };
        }

        let mut overlay = prev.overlay.as_ref().clone();
        for entry in entries {
            let path = entry.source.as_path();
            if prev
                .get(path)
                .is_some_and(|cached| Arc::ptr_eq(cached, &entry))
            {
                continue;
            }
            if prev
                .base
                .get(path)
                .is_some_and(|base| Arc::ptr_eq(base, &entry))
            {
                overlay.remove(path);
            } else {
                overlay.insert(entry.source.clone(), entry);
            }
        }

        let limit = (prev.ordered_paths.len() / SOURCE_ENTRY_OVERLAY_DIVISOR)
            .clamp(MIN_SOURCE_ENTRY_OVERLAY, MAX_SOURCE_ENTRY_OVERLAY);
        if overlay.len() > limit {
            let base = prev
                .ordered_paths
                .iter()
                .map(|path| {
                    (
                        path.clone(),
                        overlay
                            .get(path)
                            .or_else(|| prev.base.get(path))
                            .expect("source entry exists in base or overlay")
                            .clone(),
                    )
                })
                .collect();
            Self {
                base: Arc::new(base),
                overlay: Arc::default(),
                ordered_paths: Arc::clone(&prev.ordered_paths),
            }
        } else {
            Self {
                base: Arc::clone(&prev.base),
                overlay: Arc::new(overlay),
                ordered_paths: Arc::clone(&prev.ordered_paths),
            }
        }
    }

    pub(super) fn get(&self, src: &Path) -> Option<&Arc<AnalyzedSource>> {
        self.overlay.get(src).or_else(|| self.base.get(src))
    }

    pub(super) fn entries(&self) -> impl Iterator<Item = &Arc<AnalyzedSource>> {
        self.ordered_paths
            .iter()
            .map(|path| self.get(path).expect("ordered source entry exists"))
    }

    pub(super) fn reused_from(&self, prev: Option<&Self>) -> Arc<[PathBuf]> {
        let Some(prev) = prev else {
            return Arc::default();
        };
        self.entries()
            .filter(|entry| {
                prev.get(&entry.source)
                    .is_some_and(|cached| Arc::ptr_eq(cached, entry))
            })
            .map(|entry| entry.source.clone())
            .collect::<Vec<_>>()
            .into()
    }
}

pub(super) struct AnalysisRound<'a> {
    pub(super) cfg: &'a SiteConfig,
    pub(super) host: &'a SiteHost,
    pub(super) reuse: Option<&'a SourceAnalysisCache>,
    pub(super) candidate_files: &'a Arc<tola_typst::FileView>,
    pub(super) bindings: &'a crate::package::SiteBindings,
    pub(super) cancellation: &'a tola_typst::BundleCancellation,
}

impl AnalysisRound<'_> {
    pub(super) fn run(
        &self,
        srcs: &SourceSet,
        prev_round: &SourceEntryIndex,
        library: &tola_packages::library::SiteLibrary,
    ) -> Vec<anyhow::Result<RoundResult>> {
        let input = srcs.inputs();
        srcs.sources()
            .par_iter()
            .map(|src| self.analyze(src, prev_round, input, library))
            .collect()
    }

    fn analyze(
        &self,
        src: &ContentSource,
        prev_round: &SourceEntryIndex,
        input: &SourcesInput,
        library: &tola_packages::library::SiteLibrary,
    ) -> anyhow::Result<RoundResult> {
        self.cancellation.ensure_active()?;
        match self.reused(src, input, prev_round) {
            Some(outcome) => Ok(outcome),
            None => self.evaluate(src, src.source().to_path_buf(), input, library),
        }
    }

    fn reused(
        &self,
        src: &ContentSource,
        input: &SourcesInput,
        prev_round: &SourceEntryIndex,
    ) -> Option<RoundResult> {
        let src_path = src.source().to_path_buf();
        if let Some(entry) = prev_round
            .get(src.source())
            .filter(|entry| entry.belongs_to(src) && entry.inputs_match(self.bindings, input))
        {
            return Some(RoundResult::Complete {
                source: src_path,
                accessed: tola_typst::AccessedDeps {
                    reads: entry.evidence.clone(),
                    package_checks: entry.package_checks.clone(),
                    ..tola_typst::AccessedDeps::default()
                },
                reused_reader: None,
                entry: Arc::clone(entry),
            });
        }
        let cache = self
            .reuse
            .filter(|cache| cache.may_reuse_entry(src.source()))?;
        let entry = cache.entry_for_source(src)?;
        if !entry.inputs_match(self.bindings, input) {
            return None;
        }
        let accessed = entry_candidate_accessed(entry, self.candidate_files, self.cancellation)?;
        Some(RoundResult::Complete {
            source: src_path.clone(),
            accessed,
            reused_reader: Some(crate::compiler::TypstDependencyReader::ContentSource(
                src_path,
            )),
            entry: Arc::clone(entry),
        })
    }

    fn evaluate(
        &self,
        src: &ContentSource,
        src_path: PathBuf,
        input: &SourcesInput,
        library: &tola_packages::library::SiteLibrary,
    ) -> anyhow::Result<RoundResult> {
        let world = self
            .host
            .world(
                self.cfg.root(),
                src.source(),
                library,
                Arc::clone(self.candidate_files),
                self.cancellation,
            )
            .map_err(|err| anyhow::Error::new(tola_typst::CompileError::from(err)))?;
        let header = tola_packages::CaptureHeader::new(world.main(), ANALYSIS_INPUT_IDENTITY);
        let (captured, analyzed) =
            tola_typst::scan_world_observed(&world, [header.value()]).into_parts();
        match analyzed {
            Ok(scan) => {
                let accessed = scan.accessed().clone();
                if self.cancellation.is_cancelled() {
                    return Ok(RoundResult::Cancelled { accessed });
                }
                let reads = accessed.reads.clone();
                let uses_sources =
                    reads_tola_capability(&reads, tola_packages::TolaPackage::Source);
                let source_reads = reads
                    .iter()
                    .filter_map(|read| {
                        let file = crate::package::source_query_file(read.evidence().locator())?;
                        Some((file.clone(), input.get(&file).cloned()))
                    })
                    .collect();
                let declaration =
                    match tola_packages::decode_capture(&captured, &header, scan.source()) {
                        Ok(capture) => capture.declaration,
                        Err(err) => {
                            return Ok(RoundResult::Failed {
                                source: src_path,
                                accessed,
                                error: Box::new(unreadable_declaration_error(&world, &scan, err)),
                                declared: None,
                            });
                        }
                    };
                let declaration = match declaration {
                    tola_packages::Declaration::Absent => None,
                    tola_packages::Declaration::One(declared) => Some(declaration_record(declared)),
                    tola_packages::Declaration::Duplicate(declared) => {
                        return Ok(RoundResult::Failed {
                            source: src_path,
                            accessed,
                            error: Box::new(duplicate_declaration_error(&world, &scan, &declared)),
                            declared: None,
                        });
                    }
                };
                let legacy_label = declaration
                    .is_none()
                    .then(|| label_declaration(&scan))
                    .flatten()
                    .and_then(|span| crate::compiler::diagnostic::source_location(&world, span));
                if self.cancellation.is_cancelled() {
                    return Ok(RoundResult::Cancelled { accessed });
                }
                Ok(RoundResult::Complete {
                    source: src_path.clone(),
                    accessed,
                    reused_reader: None,
                    entry: Arc::new(AnalyzedSource {
                        id: src.id().clone(),
                        layout: src.source_layout(),
                        source: src_path,
                        evidence: reads,
                        package_checks: scan.package_checks().to_vec(),
                        diagnostics: scan.diagnostics().clone(),
                        package_bindings: self.bindings.clone(),
                        sources_input: uses_sources.then(|| input.clone()),
                        source_reads,
                        declaration,
                        legacy_label,
                    }),
                })
            }
            Err(failure) => {
                let (accessed, err) = failure.into_parts();
                if self.cancellation.is_cancelled() || err.is_cancelled() {
                    return Ok(RoundResult::Cancelled { accessed });
                }
                // A source that fails after declaring still contributes its declaration.
                let declared = declared_before_failure(&captured, &header, &world);
                Ok(RoundResult::Failed {
                    source: src_path,
                    accessed,
                    error: Box::new(err),
                    declared,
                })
            }
        }
    }
}

/// Top-level call of the retired `<tola-meta>` label in `scan`.
fn label_declaration(scan: &tola_typst::ScanResult) -> Option<typst::syntax::Span> {
    use typst::syntax::ast::{self, AstNode};

    let markup = scan
        .source()
        .root()
        .cast::<ast::Markup>()
        .expect("an evaluated Typst source has a markup root");
    let calls: HashSet<_> = markup
        .exprs()
        .filter_map(|mut expr| {
            while let ast::Expr::Parenthesized(group) = expr {
                expr = group.expr();
            }
            match expr {
                ast::Expr::FuncCall(call) => Some(call.span()),
                _ => None,
            }
        })
        .collect();

    scan.metadata_declarations(crate::metadata::SOURCE_METADATA_LABEL)
        .into_iter()
        .map(|decl| decl.span())
        .find(|span| calls.contains(span))
}

fn declaration_record(
    declared: tola_packages::DeclaredMeta,
) -> crate::content::SourceMetadataDeclaration {
    crate::content::SourceMetadataDeclaration {
        metadata: crate::metadata::SourceMetadata::from_dict(declared.metadata),
        file: declared.file,
        range: declared.range,
    }
}

fn declared_before_failure(
    captured: &tola_typst::CapturedValues,
    header: &tola_packages::CaptureHeader,
    world: &tola_typst::TypstWorld,
) -> Option<crate::content::SourceMetadataDeclaration> {
    let src = captured.source(world.main())?;
    let Ok(capture) = tola_packages::decode_capture(captured, header, src) else {
        return None;
    };
    match capture.declaration {
        tola_packages::Declaration::One(declared) => Some(declaration_record(declared)),
        _ => None,
    }
}

fn declaration_failure(
    world: &tola_typst::TypstWorld,
    scan: &tola_typst::ScanResult,
    diag: tola_typst::diagnostic::NativeDiagnostic,
) -> tola_typst::CompileError {
    let mut diags = Diagnostics::resolve(world, &[diag]);
    diags.extend(scan.diagnostics());
    tola_typst::CompileError::Compilation { diagnostics: diags }
}

fn unreadable_declaration_error(
    world: &tola_typst::TypstWorld,
    scan: &tola_typst::ScanResult,
    err: tola_packages::CaptureError,
) -> tola_typst::CompileError {
    let (msg, hint) = match &err {
        tola_packages::CaptureError::Saturated => (
            "this source writes more metadata declarations than Tola can read",
            "Keep one `#tola-meta((...))` call; a source declares once",
        ),
        _ => (
            "Tola could not read this source's metadata declarations",
            "Rebuild the site; report this if it repeats",
        ),
    };
    let diag = tola_typst::diagnostic::producer_error_at(
        scan.source().id(),
        0..0,
        tola_packages::DECLARATION_PROTOCOL_ORIGIN,
        &[],
        msg,
        tola_typst::diagnostic::DiagnosticProjection::default(),
    )
    .with_hint(hint)
    .with_hint(err.to_string());
    declaration_failure(world, scan, diag.into())
}

fn duplicate_declaration_error(
    world: &tola_typst::TypstWorld,
    scan: &tola_typst::ScanResult,
    declared: &[tola_packages::DeclaredMeta],
) -> tola_typst::CompileError {
    let decl = declared
        .get(1)
        .expect("a duplicate declaration holds a second position");
    let diag = typst::diag::SourceDiagnostic::error(
        typst::syntax::DiagSpan::from_range(
            typst::syntax::FileId::new(decl.file.clone()),
            decl.range.clone(),
        ),
        format!("this source declares its metadata {} times", declared.len()),
    )
    .with_hint(format!(
        "Keep one metadata declaration for `{}`",
        scan.source().id().vpath().get_without_slash()
    ));
    declaration_failure(world, scan, diag.into())
}

fn entry_candidate_accessed(
    entry: &AnalyzedSource,
    candidate: &tola_typst::FileView,
    cancellation: &tola_typst::BundleCancellation,
) -> Option<tola_typst::AccessedDeps> {
    let mut accessed = tola_typst::AccessedDeps::default();
    for read in &entry.evidence {
        let id = file_id_for_locator(read.evidence().locator())?;
        let observed = candidate.matching_accessed(id, read, cancellation)?;
        accessed.reads.extend(observed.reads);
        accessed.disk_reads.extend(observed.disk_reads);
        accessed.package_checks.extend(observed.package_checks);
    }
    tola_typst::sort_package_checks(&mut accessed.package_checks);
    accessed.package_checks.dedup();
    (accessed.package_checks == entry.package_checks).then_some(accessed)
}

fn file_id_for_locator(locator: &tola_typst::ReadLocator) -> Option<typst::syntax::FileId> {
    use typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};

    match locator {
        tola_typst::ReadLocator::Root(path) | tola_typst::ReadLocator::ProvidedRoot(path) => {
            Some(tola_typst::file_id(path))
        }
        tola_typst::ReadLocator::Package { package, path }
        | tola_typst::ReadLocator::ProvidedPackage { package, path } => {
            let path = VirtualPath::new(path.to_str()?).ok()?;
            Some(FileId::new(RootedPath::new(
                VirtualRoot::Package(package.clone()),
                path,
            )))
        }
        tola_typst::ReadLocator::NonPersistent(_) => None,
    }
}
