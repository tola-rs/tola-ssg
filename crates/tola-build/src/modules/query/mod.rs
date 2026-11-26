//! Read-only queries over Typst model entries.

mod prepare;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use serde::Serialize;
use serde_json::Value as Json;
use tola_diagnostic::{Diagnostic, Severity, display_path};
use tola_module::declaration::Query;
use tola_module::entities::Entity;
use tola_module::module_set::LoadedModule;

use self::prepare::{ModelQuery, prepare};
use crate::BuildResources;
use crate::cancellation::BuildCancellation;
use crate::check::{SourceDiagnosticSession, SourceRevision};
use crate::config::SiteConfig;
use crate::hooks::generated_input_producer;

/// A declared generator input; this query did not run or verify its producer.
#[derive(Debug, Clone, Serialize)]
pub struct GeneratedInput {
    pub producer: String,
    pub path: String,
}

/// The input policy of a model query, distinct from a published site revision.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub mode: &'static str,
    pub phase: &'static str,
    pub hooks: &'static str,
    pub generation_status: &'static str,
    pub generated: Vec<GeneratedInput>,
    /// Read-time evidence from model evaluation and entity-source validation only.
    pub read_coverage: &'static str,
    pub reads: Vec<InputRead>,
    /// Actual attempted disk paths, including unsuccessful reads without a digest.
    pub disk_read_attempts: Vec<PathBuf>,
}

/// The native identity of bytes read by a query, before display formatting.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum InputSource {
    File {
        root: String,
        path: PathBuf,
        provided: bool,
    },
    NonPersistent {
        name: String,
    },
}

/// Exact bytes consumed by a successful read; no later filesystem rehash is used.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct InputRead {
    #[serde(flatten)]
    pub source: InputSource,
    pub digest: String,
    pub origin: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_path: Option<PathBuf>,
}

impl From<&tola_typst::FileRead> for InputRead {
    fn from(read: &tola_typst::FileRead) -> Self {
        use tola_typst::{ReadLocator, ReadOrigin};

        let file = |root: String, path: &Path, provided| InputSource::File {
            root,
            path: path.to_path_buf(),
            provided,
        };
        let source = match read.evidence().locator() {
            ReadLocator::Root(path) => file("project".into(), path, false),
            ReadLocator::Package { package, path } => file(package.to_string(), path, false),
            ReadLocator::ProvidedRoot(path) => file("project".into(), path, true),
            ReadLocator::ProvidedPackage { package, path } => file(package.to_string(), path, true),
            ReadLocator::NonPersistent(name) => InputSource::NonPersistent { name: name.clone() },
        };
        let (origin, disk_path) = match read.origin() {
            ReadOrigin::Disk(path) => ("disk", Some(path.as_path().to_path_buf())),
            ReadOrigin::Provider => ("provider", None),
            ReadOrigin::NonPersistent => ("non-persistent", None),
        };
        Self {
            source,
            digest: format!("blake3:{}", read.evidence().digest().to_hex()),
            origin,
            disk_path,
        }
    }
}

impl Input {
    fn record<'a>(
        &mut self,
        reads: impl IntoIterator<Item = &'a tola_typst::FileRead>,
        attempts: impl IntoIterator<Item = PathBuf>,
    ) {
        self.reads.extend(reads.into_iter().map(InputRead::from));
        self.reads.sort_unstable();
        self.reads.dedup();
        self.disk_read_attempts.extend(attempts);
        self.disk_read_attempts.sort_unstable();
        self.disk_read_attempts.dedup();
    }
}

/// Versioned model output. `result_digest` identifies data, not an edit revision.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    pub format_version: u32,
    pub query: String,
    pub module_manifest: Option<String>,
    pub input: Input,
    pub success: bool,
    pub result_digest: Option<String>,
    pub data: Option<Json>,
    pub diagnostics: Vec<Diagnostic>,
}

/// A query result and the exact compilation whose world supplied its values and source identities.
pub struct QueryEvaluation {
    pub result: QueryResult,
    pub entities: Vec<Entity>,
    pub revision: Option<Arc<SourceRevision>>,
    pub source_reads: Vec<typst::syntax::Source>,
    pub source_accessed: tola_typst::AccessedDeps,
}

/// Why one module query could not evaluate.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    /// The query stopped because its build was cancelled, which is not a site problem.
    #[error("build cancelled")]
    Cancelled(#[from] crate::cancellation::BuildCancelled),
    #[error(transparent)]
    Evaluation(anyhow::Error),
}

impl From<anyhow::Error> for QueryError {
    fn from(error: anyhow::Error) -> Self {
        if crate::cancellation::is_cancelled(&error) {
            Self::Cancelled(crate::cancellation::BuildCancelled)
        } else {
            Self::Evaluation(error)
        }
    }
}

/// Evaluate one model with the site's root, content discovery and package bindings.
/// No hooks, output export or publication runs. Overlays use absolute source paths.
pub fn run(
    config: Arc<SiteConfig>,
    module: &LoadedModule,
    declaration: &Query,
    name: &str,
    resources: BuildResources,
    overlays: Vec<(PathBuf, Arc<str>)>,
    cancellation: &BuildCancellation,
) -> std::result::Result<QueryResult, QueryError> {
    Ok(evaluate(
        config,
        ModelQuery::Module {
            module,
            declaration,
            name,
        },
        resources,
        overlays,
        cancellation,
        true,
    )?
    .result)
}

/// Evaluate a project-owned Typst file and read one labelled metadata value.
pub fn run_file(
    config: Arc<SiteConfig>,
    entry: &Path,
    label: &str,
    resources: BuildResources,
    overlays: Vec<(PathBuf, Arc<str>)>,
    cancellation: &BuildCancellation,
) -> std::result::Result<QueryResult, QueryError> {
    Ok(evaluate(
        config,
        ModelQuery::File { entry, label },
        resources,
        overlays,
        cancellation,
        true,
    )?
    .result)
}

/// Evaluate an entity query and retain its model world and native source identities.
/// The typed result leaves the JSON data and digest unmaterialized; use `run` for CLI output.
pub fn run_entities(
    config: Arc<SiteConfig>,
    module: &LoadedModule,
    declaration: &Query,
    name: &str,
    resources: BuildResources,
    overlays: Vec<(PathBuf, Arc<str>)>,
    cancellation: &BuildCancellation,
) -> Result<QueryEvaluation> {
    ensure!(
        declaration.has_entities(),
        "query `{name}` declares no references or definitions"
    );
    evaluate(
        config,
        ModelQuery::Module {
            module,
            declaration,
            name,
        },
        resources,
        overlays,
        cancellation,
        false,
    )
}

/// A retained query entry with compiler resources and dependency-based source analysis.
pub struct QuerySession {
    site: Arc<super::LoadedSite>,
    session: SourceDiagnosticSession,
    generated: Vec<GeneratedInput>,
}

/// One query entry compiled from an immutable set of disk and unsaved inputs.
pub struct QueryCompilation {
    site: Arc<super::LoadedSite>,
    revision: Arc<SourceRevision>,
    input: Input,
}

pub(super) struct SelectedQuery {
    name: String,
    label: String,
    entities: bool,
    module_manifest: Option<String>,
}

fn evaluate(
    cfg: Arc<SiteConfig>,
    query: ModelQuery<'_>,
    resources: BuildResources,
    overlays: Vec<(PathBuf, Arc<str>)>,
    cancel: &BuildCancellation,
    materialize: bool,
) -> Result<QueryEvaluation> {
    let (mut session, selected) = prepare(cfg, query, resources, cancel)?;
    session
        .inspect(overlays, cancel)?
        .read(selected, materialize, cancel)
}

impl QuerySession {
    /// Prepare a module query entry for repeated editor revisions.
    pub fn new(
        config: Arc<SiteConfig>,
        module: &LoadedModule,
        declaration: &Query,
        name: &str,
        resources: BuildResources,
        cancellation: &BuildCancellation,
    ) -> Result<Self> {
        let (session, _) = prepare(
            config,
            ModelQuery::Module {
                module,
                declaration,
                name,
            },
            resources,
            cancellation,
        )?;
        Ok(session)
    }

    /// Inspect new source inputs while retaining reusable compiler analysis for this entry.
    pub fn inspect(
        &mut self,
        overlays: Vec<(PathBuf, Arc<str>)>,
        cancellation: &BuildCancellation,
    ) -> Result<QueryCompilation> {
        cancellation.ensure_active()?;
        ensure!(
            self.site
                .prepared()
                .manifests_are_fresh(&cancellation.bundle_cancellation())?,
            "a module changed before Tola could evaluate its query; run the query again"
        );
        let mut input = Input {
            mode: if overlays.is_empty() {
                "disk"
            } else {
                "overlay"
            },
            phase: "model",
            hooks: "skipped",
            generation_status: if self.generated.is_empty() {
                "not-declared"
            } else {
                "unverified"
            },
            generated: self.generated.clone(),
            read_coverage: "model-and-entity-sources",
            reads: Vec::new(),
            disk_read_attempts: Vec::new(),
        };
        let revision = Arc::new(self.session.inspect(overlays, cancellation)?);
        input.record(revision.file_reads(), revision.disk_reads().iter().cloned());
        let compiled = QueryCompilation {
            site: Arc::clone(&self.site),
            revision,
            input,
        };
        compiled.ensure_current(cancellation)?;
        Ok(compiled)
    }
}

impl QueryCompilation {
    fn failure<'a>(
        &self,
        res: &mut QueryResult,
        msg: String,
        missing: impl IntoIterator<Item = &'a Path>,
    ) {
        let root = self.site.root();
        let hooks = self.site.hooks();
        let mut diag = Diagnostic::new(tola_diagnostic::codes::query::RESULT, Severity::Error, msg);
        for (path, producer) in missing
            .into_iter()
            .filter_map(|p| {
                generated_input_producer(hooks, root, p).map(|prod| (display_path(p, root), prod))
            })
            .collect::<std::collections::BTreeSet<_>>()
        {
            diag = diag.with_note(format!(
                "`{path}` is declared by the before-build hook `{producer}`"
            ));
        }
        if !diag.notes.is_empty() {
            diag = diag.with_help(
                "Run `tola check` to execute the before-build hooks, then retry the query",
            );
        }
        res.diagnostics.push(diag);
    }

    /// Read a declared entity query from this entry's compiled metadata.
    pub fn read_entities(
        &self,
        module: &LoadedModule,
        declaration: &Query,
        name: &str,
        cancellation: &BuildCancellation,
    ) -> Result<QueryEvaluation> {
        self.ensure_current(cancellation)?;
        let local = name
            .strip_prefix(&format!("{}.", module.name))
            .with_context(|| format!("query `{name}` must belong to module `{}`", module.name))?;
        ensure!(
            self.site.prepared().modules.get(&module.name) == Some(module)
                && module.queries.get(local) == Some(declaration),
            "the declaration of query `{name}` changed; run the query again"
        );
        ensure!(
            declaration.has_entities(),
            "query `{name}` declares no references or definitions"
        );
        let entry = tola_module::bundle::package_path(&module.name, &declaration.entry)?.intern();
        ensure!(
            self.site.entry_file() == Some(entry),
            "query `{name}` uses a different entry file"
        );
        self.read(
            SelectedQuery {
                name: name.to_owned(),
                label: declaration.label.clone(),
                entities: true,
                module_manifest: Some(display_path(&module.manifest, self.site.root())),
            },
            false,
            cancellation,
        )
    }

    fn ensure_current(&self, cancel: &BuildCancellation) -> Result<()> {
        cancel.ensure_active()?;
        ensure!(
            self.site
                .prepared()
                .manifests_are_fresh(&cancel.bundle_cancellation())?,
            "a module changed while Tola was evaluating its query; run the query again"
        );
        Ok(())
    }

    fn read(
        &self,
        selected: SelectedQuery,
        materialize: bool,
        cancel: &BuildCancellation,
    ) -> Result<QueryEvaluation> {
        self.ensure_current(cancel)?;
        let SelectedQuery {
            name,
            label,
            entities,
            module_manifest,
        } = selected;
        let revision = Arc::clone(&self.revision);
        let no_entities = |result: QueryResult, revision: Arc<SourceRevision>| QueryEvaluation {
            result,
            entities: Vec::new(),
            revision: Some(revision),
            source_reads: Vec::new(),
            source_accessed: Default::default(),
        };
        let mut res = QueryResult {
            format_version: 2,
            query: name.clone(),
            module_manifest,
            input: self.input.clone(),
            success: false,
            result_digest: None,
            data: None,
            diagnostics: revision.diagnostics().to_vec(),
        };
        let checked = revision.checked().filter(|_| {
            !res.diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error)
        });
        let Some((world, bundle)) = checked.and_then(|c| c.bundle().map(|b| (c.world(), b))) else {
            self.failure(
                &mut res,
                "The model entry did not compile".into(),
                revision.missing_disk_reads().iter().map(PathBuf::as_path),
            );
            return Ok(no_entities(res, revision));
        };
        let outcome = tola_module::query::run_query(tola_module::query::QueryRequest {
            name: &name,
            label: &label,
            entities,
            materialize,
            world,
            bundle,
            cancellation: &cancel.bundle_cancellation(),
        })?;
        res.input.record(
            &outcome.accessed.reads,
            outcome
                .accessed
                .disk_reads
                .iter()
                .map(|r| r.as_path().to_path_buf()),
        );
        if let Some(failure) = outcome.failure {
            self.failure(
                &mut res,
                failure,
                outcome
                    .accessed
                    .disk_reads
                    .iter()
                    .filter(|r| r.is_missing())
                    .map(|r| r.as_path()),
            );
            return Ok(QueryEvaluation {
                source_accessed: outcome.accessed,
                ..no_entities(res, revision)
            });
        }
        if materialize {
            let data = outcome
                .data
                .as_ref()
                .expect("a materialized query has a value");
            let bytes = serde_json::to_vec(data)?;
            res.result_digest = Some(format!("blake3:{}", blake3::hash(&bytes).to_hex()));
        }
        res.data = outcome.data;
        res.success = true;
        self.ensure_current(cancel)?;
        Ok(QueryEvaluation {
            result: res,
            entities: outcome.entities,
            revision: Some(revision),
            source_reads: outcome.source_reads,
            source_accessed: outcome.accessed,
        })
    }
}
