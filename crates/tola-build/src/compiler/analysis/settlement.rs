use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tola_input::normalize_path;
use tola_typst::{Diagnostics, FileRead, PackageCheck};

use crate::content::{SourceRecords, SourceSet, SourcesInput};

use super::round::{AnalysisRound, AnalyzedSource, SourceEntryIndex};

pub(super) type SourceReads = (PathBuf, Vec<FileRead>, Vec<PackageCheck>);
pub(super) type SettledRound = (Diagnostics, Vec<SourceReads>);

pub(super) enum RoundResult {
    Complete {
        source: PathBuf,
        entry: Arc<AnalyzedSource>,
        accessed: tola_typst::AccessedDeps,
        reused_reader: Option<crate::compiler::TypstDependencyReader>,
    },
    Failed {
        source: PathBuf,
        accessed: tola_typst::AccessedDeps,
        error: Box<tola_typst::CompileError>,
        declared: Option<crate::content::SourceMetadataDeclaration>,
    },
    Cancelled {
        accessed: tola_typst::AccessedDeps,
    },
}

#[derive(Debug)]
struct SourceAnalysisErrors {
    errors: Vec<(PathBuf, tola_typst::CompileError)>,
}

impl std::fmt::Display for SourceAnalysisErrors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "source analysis failed for {} {}",
            self.errors.len(),
            if self.errors.len() == 1 {
                "file"
            } else {
                "files"
            }
        )
    }
}

impl std::error::Error for SourceAnalysisErrors {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.errors
            .first()
            .map(|(_, err)| err as &(dyn std::error::Error + 'static))
    }
}

impl SourceAnalysisErrors {
    fn into_error(self, root: &Path, completed: &Diagnostics) -> anyhow::Error {
        let root = normalize_path(root);
        let mut diags = Vec::new();
        let mut retain = |diag| {
            if !diags.contains(&diag) {
                diags.push(diag);
            }
        };
        for diag in completed {
            retain(crate::compiler::diagnostic::convert_diagnostic(
                diag.clone(),
                &root,
                tola_diagnostic::codes::typst::COMPILE,
            ));
        }
        for (path, err) in &self.errors {
            let rel = crate::compiler::diagnostic::site_relative(path, &root);
            if let Some(items) = err.diagnostics() {
                for item in items {
                    let mut diag = item.clone();
                    if diag.location.path.is_none() {
                        diag.location.path = rel.clone();
                    }
                    retain(crate::compiler::diagnostic::convert_diagnostic(
                        diag,
                        &root,
                        tola_diagnostic::codes::typst::COMPILE,
                    ));
                }
            } else if let Some(items) =
                crate::compiler::diagnostic::compile_failure_diagnostics(err, &root)
            {
                items.into_iter().for_each(&mut retain);
            } else {
                let diag = tola_diagnostic::Diagnostic::new(
                    tola_diagnostic::codes::typst::SOURCE_ANALYSIS,
                    tola_diagnostic::Severity::Error,
                    "Tola could not read this source's metadata",
                )
                .with_help(
                    "Rerun with `--log-file tola.log`, then report this at \
                     https://github.com/tola-rs/tola-ssg/issues",
                );
                retain(match rel {
                    Some(rel) => diag.with_path(rel),
                    None => diag,
                });
            }
        }
        anyhow::Error::new(tola_diagnostic::DiagnosticError::attach(
            anyhow::Error::new(self),
            diags,
        ))
    }
}

#[derive(Debug, thiserror::Error)]
enum MetadataSettlementError {
    #[error(
        "these sources keep changing each other's `<tola-meta>` values, so Tola could not settle the site metadata: {sources}"
    )]
    CyclingSources { sources: String },
    #[error(
        "Tola could not settle the site metadata after {rounds} reads; these sources change their `<tola-meta>` values every time: {sources}"
    )]
    Unsettled { rounds: usize, sources: String },
}

#[derive(Default)]
pub(super) struct Settlement {
    pub(super) source_set: SourceSet,
    pub(super) entries: SourceEntryIndex,
    tried: HashMap<u128, Vec<SourcesInput>>,
    pub(super) rounds: usize,
    pub(super) round_limit: usize,
}

impl Settlement {
    pub(super) fn new(source_set: SourceSet, round_limit: usize) -> Self {
        Self {
            source_set,
            round_limit,
            rounds: 1,
            ..Self::default()
        }
    }

    fn settle(
        &mut self,
        outcomes: Vec<anyhow::Result<RoundResult>>,
        inputs: &mut crate::compiler::BuildInputs,
        deps: Option<&crate::compiler::CompilationDependencies>,
        round_records: &SourceRecords,
        root: &Path,
    ) -> anyhow::Result<Option<SettledRound>> {
        for outcome in outcomes.iter().filter_map(|outcome| outcome.as_ref().ok()) {
            let accessed = match outcome {
                RoundResult::Complete { accessed, .. }
                | RoundResult::Failed { accessed, .. }
                | RoundResult::Cancelled { accessed } => accessed,
            };
            inputs.record_accessed(accessed);
            let RoundResult::Complete {
                reused_reader: Some(reader),
                ..
            } = outcome
            else {
                continue;
            };
            deps.expect("a reused reader belongs to a retained analysis")
                .record_reader_reads(inputs, reader);
        }

        let mut diags = Diagnostics::new();
        let mut dep_reads = Vec::new();
        let mut errors = Vec::new();
        let mut declarations = BTreeMap::new();
        let mut has_source_dep_errors = false;
        let mut entries = Vec::with_capacity(self.source_set.sources().len());
        for outcome in outcomes {
            match outcome? {
                RoundResult::Complete {
                    source,
                    entry,
                    accessed,
                    ..
                } => {
                    let mut reads = accessed.reads;
                    let mut seen = HashSet::new();
                    reads.retain(|read| seen.insert(read.clone()));
                    dep_reads.push((source, reads, entry.package_checks.clone()));
                    diags.extend(&entry.diagnostics);
                    declarations.insert(entry.source.clone(), entry.declaration.clone());
                    entries.push(entry);
                }
                RoundResult::Failed {
                    source,
                    accessed,
                    error,
                    declared,
                } => {
                    has_source_dep_errors |=
                        reads_tola_capability(&accessed.reads, tola_packages::TolaPackage::Source)
                            || accessed.reads.iter().any(|read| {
                                crate::package::source_query_file(read.evidence().locator())
                                    .is_some()
                            });
                    if declared.is_some() {
                        declarations.insert(source.clone(), declared);
                    }
                    errors.push((source, *error));
                }
                RoundResult::Cancelled { .. } => {
                    return Err(anyhow::Error::new(tola_typst::CompileError::cancelled()));
                }
            }
        }

        let index = SourceEntryIndex::from_ordered(entries, &self.entries);
        let source_set = self.source_set.with_metadata(&declarations);
        let converged = source_set.inputs() == self.source_set.inputs();

        // Another source's metadata can make this source valid; retry while it changes.
        if !errors.is_empty() && (converged || !has_source_dep_errors) {
            return Err(SourceAnalysisErrors { errors }.into_error(root, &diags));
        }
        if converged {
            (self.source_set, self.entries) = (source_set, index);
            return Ok(Some((diags, dep_reads)));
        }
        let settled_records = source_set.inputs().to_source_records();
        let tried = self.tried.get(&settled_records.semantic_digest());
        if tried.is_some_and(|tried| tried.contains(source_set.inputs())) {
            return Err(MetadataSettlementError::CyclingSources {
                sources: changed_metadata_listing(&self.source_set, &source_set, root),
            }
            .into());
        }
        if self.rounds >= self.round_limit {
            return Err(MetadataSettlementError::Unsettled {
                rounds: self.round_limit,
                sources: changed_metadata_listing(&self.source_set, &source_set, root),
            }
            .into());
        }
        self.tried
            .entry(round_records.semantic_digest())
            .or_default()
            .push(self.source_set.inputs().clone());
        (self.source_set, self.entries) = (source_set, index);
        self.rounds += 1;
        Ok(None)
    }
}

pub(super) fn settle_metadata_rounds(
    round: &AnalysisRound<'_>,
    settlement: &mut Settlement,
    deps: Option<&crate::compiler::CompilationDependencies>,
    inputs: &mut crate::compiler::BuildInputs,
) -> anyhow::Result<SettledRound> {
    loop {
        let records = settlement.source_set.inputs().to_source_records();
        let library = round.bindings.library(records.clone());
        let outcomes = round.run(&settlement.source_set, &settlement.entries, &library);
        let Some((diags, dep_reads)) =
            settlement.settle(outcomes, inputs, deps, &records, round.cfg.root())?
        else {
            continue;
        };
        crate::compiler::diagnostic::ensure_native_reports_succeed(round.cfg.root(), &diags)?;
        return Ok((diags, dep_reads));
    }
}

fn changed_metadata_listing(prev: &SourceSet, next: &SourceSet, root: &Path) -> String {
    prev.sources()
        .iter()
        .zip(next.sources())
        .filter(|(prev, next)| prev.metadata() != next.metadata())
        .map(|(_, src)| tola_diagnostic::display_path(src.source(), root))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn reads_tola_capability(
    evidence: &[FileRead],
    expected: tola_packages::TolaPackage,
) -> bool {
    evidence
        .iter()
        .any(|read| expected.owns_capability_read(read.evidence().locator()))
}
