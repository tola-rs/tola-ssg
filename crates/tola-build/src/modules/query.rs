//! Read-only queries over Typst model entries.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::{Map, Number, Value as Json};
use typst::World;
use typst::foundations::Value;

use crate::BuildResources;
use crate::cancellation::BuildCancellation;
use crate::check::{SourceDiagnosticSession, SourceRevision};
use crate::config::ResolvedSiteConfig;
use crate::filesystem::display_path;
use crate::modules::{Query, QueryFormat, ResolvedModule};
use tola_diagnostic::{Diagnostic, Severity};

/// A declared generator input; this query did not run or verify its producer.
#[derive(Debug, Serialize)]
pub struct GeneratedInput {
    pub producer: String,
    pub path: String,
}

/// The input policy of a model query, distinct from a published site revision.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub mode: &'static str,
    pub phase: &'static str,
    pub hooks: &'static str,
    pub generation_status: &'static str,
    pub generated: Vec<GeneratedInput>,
    /// Read-time evidence from model evaluation and entity-source validation only; not
    /// complete coverage of everything this query reads.
    pub read_coverage: &'static str,
    pub reads: Vec<InputRead>,
    /// Actual attempted disk paths, including unsuccessful reads without a digest.
    pub disk_read_attempts: Vec<PathBuf>,
}

/// The native identity of bytes read by a query, before display formatting.
#[derive(Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
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
#[derive(Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
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
    /// Add one stage's reads and the disk attempts behind them.
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
    pub format: crate::modules::QueryFormat,
    pub module_manifest: Option<String>,
    pub input: Input,
    pub success: bool,
    pub result_digest: Option<String>,
    pub data: Option<Json>,
    pub diagnostics: Vec<Diagnostic>,
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
    config: Arc<ResolvedSiteConfig>,
    module: &ResolvedModule,
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
    config: Arc<ResolvedSiteConfig>,
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
    config: Arc<ResolvedSiteConfig>,
    module: &ResolvedModule,
    declaration: &Query,
    name: &str,
    resources: BuildResources,
    overlays: Vec<(PathBuf, Arc<str>)>,
    cancellation: &BuildCancellation,
) -> Result<super::entities::EntityQueryResult> {
    ensure!(
        declaration.format == QueryFormat::Entities,
        "query `{name}` does not return entities"
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

enum ModelQuery<'a> {
    Module {
        module: &'a ResolvedModule,
        declaration: &'a Query,
        name: &'a str,
    },
    File {
        entry: &'a Path,
        label: &'a str,
    },
}

fn evaluate(
    config: Arc<ResolvedSiteConfig>,
    query: ModelQuery<'_>,
    resources: BuildResources,
    overlays: Vec<(PathBuf, Arc<str>)>,
    cancellation: &BuildCancellation,
    materialize: bool,
) -> Result<super::entities::EntityQueryResult> {
    cancellation.ensure_active()?;
    let configured = super::prepared::configure(&config, resources.input_mode(), cancellation)?;
    let (name, label, format, module_manifest, query_config) = match query {
        ModelQuery::Module {
            module,
            declaration,
            name,
        } => {
            let local_name = name
                .strip_prefix(&format!("{}.", module.name))
                .with_context(|| {
                    format!(
                        "`{name}` must start with `{}`, the module the query belongs to",
                        module.name
                    )
                })?;
            let selected_module = &configured
                .module_inputs
                .configured
                .get(&module.name)
                .with_context(|| {
                    format!(
                        "Tola could not find the configuration of module `{}`",
                        module.name
                    )
                })?
                .module;
            let selected_query = selected_module.queries.get(local_name).with_context(|| {
                format!(
                    "module `{}` no longer declares the query `{local_name}`; run the query again",
                    module.name
                )
            })?;
            ensure!(
                selected_module == module && selected_query == declaration,
                "the declaration of query `{name}` changed while Tola was resolving the module; run the query again"
            );
            let entry = selected_module
                .root
                .join(&selected_query.entry)
                .canonicalize()
                .with_context(|| format!("Tola could not read the `entry` of query `{name}`"))?;
            ensure!(
                entry.starts_with(&selected_module.root),
                "the `entry` of query `{name}` points outside its module"
            );
            resources.source_boundary(&config).check(&entry)?;
            (
                name.to_owned(),
                selected_query.label.clone(),
                selected_query.format,
                Some(display_path(&selected_module.manifest, config.get_root())),
                super::prepared::select_query(&configured, selected_module, selected_query)?,
            )
        }
        ModelQuery::File { entry, label } => {
            let entry = if entry.is_absolute() {
                entry.to_path_buf()
            } else {
                config.get_root().join(entry)
            };
            let entry = entry.canonicalize().context("read model query file")?;
            ensure!(
                entry.starts_with(config.get_root()) && entry.is_file(),
                "query file must be inside the site"
            );
            resources.source_boundary(&config).check(&entry)?;
            (
                format!("{}#{label}", display_path(&entry, config.get_root())),
                label.to_owned(),
                QueryFormat::Json,
                None,
                super::prepared::select_query_file(&configured, &entry)?,
            )
        }
    };
    let query_config = Arc::new(query_config);
    ensure!(
        query_config
            .module_inputs
            .manifests_are_fresh(cancellation)?,
        "the module changed while Tola was preparing the query `{name}`; run the query again"
    );
    let mut generated = config
        .build()
        .hooks
        .before_build
        .iter()
        .filter(|hook| hook.enable)
        .flat_map(|hook| {
            hook.generates.iter().map(|path| GeneratedInput {
                producer: hook.name.clone(),
                path: display_path(path, config.get_root()),
            })
        })
        .collect::<Vec<_>>();
    for module in configured.module_inputs.configured.values() {
        generated.extend(super::hooks::declared_generated_inputs(&module.module).map(
            |(producer, path)| GeneratedInput {
                producer,
                path: display_path(&path, config.get_root()),
            },
        ));
    }
    let mut input = Input {
        mode: if overlays.is_empty() {
            "disk"
        } else {
            "overlay"
        },
        phase: "model",
        hooks: "skipped",
        generation_status: if generated.is_empty() {
            "not-declared"
        } else {
            "unverified"
        },
        generated,
        read_coverage: "model-and-entity-sources",
        reads: Vec::new(),
        disk_read_attempts: Vec::new(),
    };
    let mut session =
        SourceDiagnosticSession::with_prepared_modules(Arc::clone(&query_config), resources);
    let revision = Arc::new(session.inspect(overlays, cancellation)?);
    input.record(revision.file_reads(), revision.disk_reads().iter().cloned());
    // Every early stop hands back the same result with no entities; only success fills them.
    let no_entities =
        |result: QueryResult, revision: Arc<SourceRevision>| super::entities::EntityQueryResult {
            result,
            entities: Vec::new(),
            revision: Some(revision),
            source_reads: Vec::new(),
            source_accessed: Default::default(),
        };
    ensure!(
        query_config
            .module_inputs
            .manifests_are_fresh(cancellation)?,
        "the module changed while Tola was evaluating the query `{name}`; run the query again"
    );
    let mut result = QueryResult {
        format_version: 1,
        query: name.clone(),
        format,
        module_manifest,
        input,
        success: false,
        result_digest: None,
        data: None,
        diagnostics: revision.diagnostics().to_vec(),
    };
    if result
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error)
    {
        return Ok(no_entities(result, revision));
    }
    let Some(bundle) = revision.checked().and_then(|checked| checked.bundle()) else {
        result.failure("The model entry did not compile".into());
        return Ok(no_entities(result, revision));
    };
    let declarations = bundle.metadata_declarations(&label);
    if declarations.len() != 1 {
        result.failure(format!(
            "query {name} requires one <{}> metadata declaration, found {}",
            label,
            declarations.len(),
        ));
        return Ok(no_entities(result, revision));
    }
    let mut entities = Vec::new();
    let mut source_reads = Vec::new();
    let mut source_accessed = tola_typst::AccessedDeps::default();
    let query_value = match format {
        QueryFormat::Json => json_value(declarations[0].value(), "data"),
        QueryFormat::Entities => {
            super::entities::parse(declarations[0].value()).and_then(|parsed| {
                let world = revision.checked().expect("a bundle has a world").world();
                (source_reads, source_accessed) =
                    read_entity_sources(&parsed, world, cancellation)?;
                let entities_json = if materialize {
                    super::entities::json(&parsed)?
                } else {
                    Json::Null
                };
                entities = parsed;
                Ok(entities_json)
            })
        }
    };
    result.input.record(
        &source_accessed.reads,
        source_accessed
            .disk_reads
            .iter()
            .map(|read| read.as_path().to_path_buf()),
    );
    match query_value {
        Ok(query_json) => {
            if materialize {
                let bytes = serde_json::to_vec(&query_json)?;
                result.result_digest = Some(format!("blake3:{}", blake3::hash(&bytes).to_hex()));
                result.data = Some(query_json);
            }
            result.success = true;
        }
        Err(error) => result.failure(format!("query {name}: {error}")),
    }
    cancellation.ensure_active()?;
    Ok(super::entities::EntityQueryResult {
        result,
        entities,
        revision: Some(revision),
        source_reads,
        source_accessed,
    })
}

/// Read each entity's declared source in this query's world and keep that evidence.
///
/// An entity names a source the model compiled, so the bytes read here must be the ones that
/// compile saw; a mismatch means the source changed while the query ran.
fn read_entity_sources(
    entities: &[super::entities::Entity],
    world: &tola_typst::TypstWorld,
    cancellation: &BuildCancellation,
) -> Result<(Vec<typst::syntax::Source>, tola_typst::AccessedDeps)> {
    let mut reads = Vec::new();
    let mut accessed = tola_typst::AccessedDeps::default();
    for entity in entities {
        cancellation.ensure_active()?;
        let Some(source) = entity.source else {
            continue;
        };
        let (bytes, mut read_accessed) = world.read_file_with_evidence(source);
        accessed.reads.append(&mut read_accessed.reads);
        accessed.disk_reads.append(&mut read_accessed.disk_reads);
        accessed
            .package_checks
            .append(&mut read_accessed.package_checks);
        let id = &entity.id;
        let unreadable = || format!("Tola could not read the source named by entity `{id}`");
        let bytes = bytes.map_err(|error| anyhow::Error::new(error).context(unreadable()))?;
        let read = world
            .source(source)
            .map_err(|error| anyhow::Error::new(error).context(unreadable()))?;
        ensure!(
            bytes.as_slice() == read.text().as_bytes(),
            "the source named by entity `{id}` changed while the query ran"
        );
        reads.push(read);
    }
    Ok((reads, accessed))
}

impl QueryResult {
    fn failure(&mut self, message: String) {
        self.diagnostics.push(Diagnostic::new(
            tola_diagnostic::codes::query::RESULT,
            Severity::Error,
            message,
        ));
    }
}

pub(super) fn json_value(value: &Value, at: &str) -> Result<Json> {
    Ok(match value {
        Value::None => Json::Null,
        Value::Bool(value) => Json::Bool(*value),
        Value::Int(value) => Json::Number((*value).into()),
        Value::Float(value) => Json::Number(
            Number::from_f64(*value)
                .with_context(|| format!("{at} must contain a finite number"))?,
        ),
        Value::Str(value) => Json::String(value.to_string()),
        Value::Array(values) => Json::Array(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| json_value(value, &format!("{at}[{index}]")))
                .collect::<Result<_>>()?,
        ),
        Value::Dict(values) => {
            let mut object = Map::new();
            for (key, value) in values {
                object.insert(key.to_string(), json_value(value, &format!("{at}.{key}"))?);
            }
            Json::Object(object)
        }
        other => bail!(
            "{at} contains {}; project it to a JSON value explicitly",
            other.ty()
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SiteConfigSchema;
    use crate::{InputScope, modules};
    use std::collections::BTreeMap;
    use std::fs;

    fn resolved_modules(
        site: &std::path::Path,
        declarations: &BTreeMap<String, modules::ModuleDeclaration>,
        mode: crate::inputs::InputMode,
    ) -> std::result::Result<
        BTreeMap<String, modules::ResolvedModule>,
        modules::ModuleResolutionError,
    > {
        let roots =
            crate::inputs::module_roots(site, mode, None, declarations, &Default::default())?;
        modules::resolve_modules_with_roots(site, declarations, &roots, mode)
    }

    struct Site {
        directory: tempfile::TempDir,
        config: Arc<ResolvedSiteConfig>,
        module: ResolvedModule,
    }

    impl Site {
        fn new(model: &str) -> Self {
            Self::with_namespace("forest", model)
        }

        fn with_namespace(namespace: &str, model: &str) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            fs::create_dir_all(root.join("content")).unwrap();
            fs::create_dir_all(root.join("theme")).unwrap();
            fs::write(root.join("site.typ"), "#unknown_layout_function()").unwrap();
            fs::write(root.join("theme/model.typ"), model).unwrap();
            fs::write(
                root.join("theme/tola-module.toml"),
                "[queries.nodes]\nentry='model.typ'\nlabel='nodes'\n\n",
            )
            .unwrap();
            let declarations = BTreeMap::from([(
                namespace.into(),
                modules::ModuleDeclaration {
                    source: modules::ModuleSource::Local {
                        path: "theme".into(),
                    },
                    ..Default::default()
                },
            )]);
            let config = Arc::new(
                SiteConfigSchema {
                    modules: declarations.clone(),
                    ..Default::default()
                }
                .resolve(
                    &root.join("tola.toml"),
                    tola_typst::PackageLocations::default(),
                    &crate::config::loading::BuildOverrides::default(),
                )
                .unwrap(),
            );
            let module = resolved_modules(
                root,
                &declarations,
                crate::inputs::InputMode::new(InputScope::Offline, false),
            )
            .unwrap()
            .remove(namespace)
            .unwrap();
            Self {
                directory,
                config,
                module,
            }
        }

        fn run(&self, overlays: Vec<(PathBuf, Arc<str>)>) -> QueryResult {
            run(
                Arc::clone(&self.config),
                &self.module,
                &self.module.queries["nodes"],
                &format!("{}.nodes", self.module.name),
                BuildResources::new().with_input_scope(InputScope::Offline),
                overlays,
                &BuildCancellation::default(),
            )
            .unwrap()
        }
    }

    #[test]
    fn source_reads_keep_their_bytes() {
        let site = Site::with_namespace(
            "catalog",
            "#import \"@tola/source:0.0.0\": all-sources\n#metadata(all-sources().map(s => (path:s.path, title:s.meta.title))) <nodes>",
        );
        let source = site.directory.path().join("content/glossary.typ");
        let original =
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Glossary\"))";
        fs::write(&source, original).unwrap();
        let result = site.run(Vec::new());
        assert!(result.success, "{:?}", result.diagnostics);
        assert_eq!(result.input.read_coverage, "model-and-entity-sources");
        assert_eq!(result.input.mode, "disk");
        let read = result
            .input
            .reads
            .iter()
            .find(|read| {
                matches!(&read.source, InputSource::File { root, path, provided: false }
                if root == "project" && path == Path::new("content/glossary.typ"))
            })
            .expect("source-analysis reads must not disappear behind all-sources metadata");
        fs::write(&source, "changed after query").unwrap();
        let serialized = serde_json::to_value(read).unwrap();
        assert_eq!(
            serialized["digest"],
            format!("blake3:{}", blake3::hash(original.as_bytes()).to_hex())
        );
        assert_eq!(serialized["origin"], "disk");
        assert!(
            serialized["diskPath"]
                .as_str()
                .unwrap()
                .ends_with("content/glossary.typ")
        );
        assert_eq!(serialized["path"], "content/glossary.typ");
    }

    #[test]
    fn failed_query_keeps_reads() {
        let model = "#metadata(json(\"missing.json\"))<nodes>";
        let site = Site::with_namespace("catalog", model);
        let result = site.run(Vec::new());
        assert!(!result.success);
        assert!(
            result
                .input
                .disk_read_attempts
                .iter()
                .any(|path| path.ends_with("theme/missing.json"))
        );
        let entry = result
            .input
            .reads
            .iter()
            .find(|read| {
                matches!(&read.source, InputSource::File { root, path, .. }
                if root.starts_with("@tola-modules/catalog:") && path == Path::new("model.typ"))
            })
            .expect("a failed query retains its native package entry's successful read");
        assert_eq!(entry.origin, "disk");
        assert!(
            entry
                .disk_path
                .as_ref()
                .unwrap()
                .ends_with("theme/model.typ")
        );
        assert_eq!(
            entry.digest,
            format!("blake3:{}", blake3::hash(model.as_bytes()).to_hex())
        );
        assert!(!result.input.reads.iter().any(|read| {
            matches!(&read.source, InputSource::File { path, .. } if path.ends_with("missing.json"))
        }));
    }

    #[test]
    fn read_evidence_keeps_conflicts_distinct() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let path = root.join("glossary.typ");
        let observe = |text: &str| {
            fs::write(&path, text).unwrap();
            let world = tola_typst::TypstWorld::builder(&path, root)
                .with_local_cache()
                .no_fonts()
                .build(&tola_typst::BundleCancellation::default())
                .unwrap();
            let id = tola_typst::file_id_from_path(&path, root).unwrap();
            let (bytes, accessed) = world.read_file_with_evidence(id);
            assert_eq!(bytes.unwrap().as_slice(), text.as_bytes());
            accessed.reads
        };
        let first = observe("first version");
        let second = observe("second version");
        let old = InputRead::from(&first[0]);
        assert_eq!(
            old.digest,
            format!("blake3:{}", blake3::hash(b"first version").to_hex())
        );
        let mut result = Site::with_namespace("catalog", "#metadata(())<nodes>").run(Vec::new());
        result.input.reads.clear();
        result.input.record(
            first.iter().chain(&first).chain(&second),
            std::iter::empty(),
        );
        assert_eq!(
            result.input.reads.len(),
            2,
            "conflicting byte observations cannot be collapsed to one revision"
        );
        assert_eq!(result.input.reads[0].source, result.input.reads[1].source);
        assert_ne!(result.input.reads[0].digest, result.input.reads[1].digest);
    }

    #[test]
    fn entity_native_source_unsaved_model() {
        let mut site = Site::new("#metadata(())<nodes>");
        let root = site.directory.path();
        fs::write(root.join("content/a.typ"), "saved").unwrap();
        fs::write(
            root.join("theme/tola-module.toml"),
            "[queries.nodes]\nentry='model.typ'\nlabel='nodes'\nformat='entities'\n",
        )
        .unwrap();
        site.module = resolved_modules(
            root,
            site.config.modules(),
            crate::inputs::InputMode::new(InputScope::Offline, false),
        )
        .unwrap()
        .remove("forest")
        .unwrap();
        let model = "#import \"@tola/site:0.0.0\": project-path\n#metadata(((id: \"a\", label: \"Unsaved\", source: project-path(\"content/a.typ\")),))<nodes>";
        let result = run_entities(
            Arc::clone(&site.config),
            &site.module,
            &site.module.queries["nodes"],
            "forest.nodes",
            BuildResources::new().with_input_scope(InputScope::Offline),
            vec![(root.join("theme/model.typ"), Arc::from(model))],
            &BuildCancellation::default(),
        )
        .unwrap();
        assert!(result.result.success, "{:?}", result.result.diagnostics);
        assert_eq!(result.entities[0].label, "Unsaved");
        assert!(result.revision.as_ref().unwrap().checked().is_some());
        assert_eq!(
            result.entities[0]
                .source
                .unwrap()
                .vpath()
                .get_without_slash(),
            "content/a.typ"
        );
        assert!(result.result.data.is_none());
        assert!(!result.source_accessed.is_empty());
        assert!(!root.join("public").exists());
    }

    #[test]
    fn cancelled_query_does_not_compile() {
        let site = Site::new("#metadata(())<nodes>");
        let canceller = crate::cancellation::BuildCanceller::new();
        canceller.cancel();
        let error = run(
            Arc::clone(&site.config),
            &site.module,
            &site.module.queries["nodes"],
            "forest.nodes",
            BuildResources::new().with_input_scope(InputScope::Offline),
            Vec::new(),
            &canceller.token(),
        )
        .expect_err("cancelled query");
        assert!(crate::cancellation::is_cancelled(&anyhow::Error::new(
            error
        )));
    }

    #[test]
    fn changed_query_rejected_until_resolved() {
        let mut site = Site::new("#metadata((revision:\"v1\"))<nodes>");
        let root = site.directory.path();
        let manifest = root.join("theme/tola-module.toml");
        let first = fs::read_to_string(&manifest).unwrap();
        fs::write(&manifest, &first).unwrap();
        let resolve = || {
            resolved_modules(
                root,
                site.config.modules(),
                crate::inputs::InputMode::new(InputScope::Offline, false),
            )
            .unwrap()
            .remove("forest")
            .unwrap()
        };
        site.module = resolve();
        let initial = site.run(Vec::new());
        assert_eq!(initial.data, Some(serde_json::json!({"revision":"v1"})));

        fs::write(
            root.join("theme/second.typ"),
            "#metadata((revision:\"v2\"))<updated-nodes>",
        )
        .unwrap();
        fs::write(
            &manifest,
            first
                .replace("entry='model.typ'", "entry='second.typ'")
                .replace("label='nodes'", "label='updated-nodes'\n"),
        )
        .unwrap();
        let Err(failure) = run(
            Arc::clone(&site.config),
            &site.module,
            &site.module.queries["nodes"],
            "forest.nodes",
            BuildResources::new().with_input_scope(InputScope::Offline),
            Vec::new(),
            &BuildCancellation::default(),
        ) else {
            panic!("a stale descriptor cannot label newly evaluated data");
        };
        let message = failure.to_string();
        assert!(
            message.contains("forest.nodes") && message.contains("changed"),
            "{message}"
        );

        site.module = resolve();
        let updated = site.run(Vec::new());
        assert!(updated.success, "{:?}", updated.diagnostics);
        assert_eq!(updated.data, Some(serde_json::json!({"revision":"v2"})));
    }

    #[test]
    fn ordinary_module_bindings_between_inspections() {
        let site = Site::new("#metadata(())<nodes>");
        let root = site.directory.path();
        fs::write(root.join("site.typ"),
            "#import \"@tola/module:0.0.0\": get-module\n#metadata(get-module(\"forest\").version)<current-version>").unwrap();
        let manifest = root.join("theme/tola-module.toml");
        let source = fs::read_to_string(&manifest).unwrap();
        let versioned = |version: &str| format!("name='forester'\nversion='{version}'\n{source}");
        let mut session = SourceDiagnosticSession::with_resources(
            Arc::clone(&site.config),
            BuildResources::new().with_input_scope(InputScope::Offline),
        );
        for version in ["1.0.0", "2.0.0"] {
            fs::write(&manifest, versioned(version)).unwrap();
            let revision = session
                .inspect(Vec::new(), &BuildCancellation::default())
                .unwrap();
            let bundle = revision
                .checked()
                .and_then(|checked| checked.bundle())
                .unwrap_or_else(|| panic!("{:?}", revision.diagnostics()));
            let declarations = bundle.metadata_declarations("current-version");
            assert_eq!(declarations.len(), 1);
            let Value::Str(current) = declarations[0].value() else {
                panic!("the module version must remain a string");
            };
            assert_eq!(current.as_str(), version);
        }
    }
}
