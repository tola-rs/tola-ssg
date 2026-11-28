use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use tola_diagnostic::display_path;
use tola_module::bundle::EntrySelection;
use tola_module::declaration::Query;
use tola_module::module_set::LoadedModule;

use super::{GeneratedInput, QuerySession, SelectedQuery};
use crate::BuildResources;
use crate::cancellation::BuildCancellation;
use crate::check::SourceDiagnosticSession;
use crate::config::SiteConfig;
use crate::hooks::{generated_input_producer, generated_inputs};

pub(super) enum ModelQuery<'a> {
    Module {
        module: &'a LoadedModule,
        declaration: &'a Query,
        name: &'a str,
    },
    File {
        entry: &'a Path,
        label: &'a str,
    },
}

struct QueryEntryPath {
    path: PathBuf,
    generated: bool,
}

impl QueryEntryPath {
    fn can_compile(&self) -> bool {
        self.generated || self.path.is_file()
    }
}

fn query_entry_path(cfg: &SiteConfig, path: &Path) -> std::io::Result<QueryEntryPath> {
    match path.canonicalize() {
        Ok(path) => Ok(QueryEntryPath {
            path,
            generated: false,
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // A declared generated entry can reach the same read-only compiler as its imports.
            let path = tola_input::normalize_existing_prefix(path);
            if generated_input_producer(cfg.hooks(), cfg.root(), &path).is_some() {
                Ok(QueryEntryPath {
                    path,
                    generated: true,
                })
            } else {
                Err(err)
            }
        }
        Err(err) => Err(err),
    }
}

fn prepare_module_query(
    cfg: &SiteConfig,
    configured: &crate::modules::LoadedSite,
    resources: &BuildResources,
    module: &LoadedModule,
    decl: &Query,
    name: &str,
) -> Result<(SelectedQuery, EntrySelection)> {
    let local_name = name
        .strip_prefix(&format!("{}.", module.name))
        .with_context(|| {
            format!(
                "`{name}` must start with `{}`, the module the query belongs to",
                module.name
            )
        })?;
    let selected_mod = configured
        .prepared()
        .modules
        .get(module.name.as_str())
        .with_context(|| {
            format!(
                "Tola could not find the configuration of module `{}`",
                module.name
            )
        })?;
    let selected_query = selected_mod.queries.get(local_name).with_context(|| {
        format!(
            "module `{}` no longer declares the query `{local_name}`; run the query again",
            module.name
        )
    })?;
    ensure!(
        selected_mod == module && selected_query == decl,
        "the declaration of query `{name}` changed while Tola was resolving the module; run the query again"
    );
    let path = selected_mod.root.join(&selected_query.entry);
    let entry = query_entry_path(configured, &path)
        .with_context(|| format!("Tola could not read the `entry` of query `{name}`"))?;
    ensure!(
        entry.path.starts_with(&selected_mod.root),
        "the `entry` of query `{name}` points outside its module"
    );
    ensure!(
        entry.can_compile(),
        "the `entry` of query `{name}` is not a file"
    );
    resources.source_boundary(cfg).check(&entry.path)?;
    Ok((
        SelectedQuery {
            name: name.to_owned(),
            label: selected_query.label.clone(),
            entities: selected_query.has_entities(),
            module_manifest: Some(display_path(&selected_mod.manifest, cfg.root())),
        },
        tola_module::bundle::select_query(selected_mod, selected_query),
    ))
}

fn prepare_file_query(
    cfg: &SiteConfig,
    configured: &crate::modules::LoadedSite,
    resources: &BuildResources,
    entry: &Path,
    label: &str,
) -> Result<(SelectedQuery, EntrySelection)> {
    let entry = if entry.is_absolute() {
        entry.to_path_buf()
    } else {
        cfg.root().join(entry)
    };
    let root = cfg.root();
    let shown = display_path(&entry, root);
    let entry = query_entry_path(configured, &entry).map_err(|err| {
        let reason = if err.kind() == std::io::ErrorKind::NotFound {
            "it does not exist"
        } else {
            crate::filesystem::path_failure_reason(&err)
        };
        anyhow::Error::new(err).context(format!("could not read query file `{shown}`: {reason}"))
    })?;
    ensure!(
        entry.path.starts_with(root),
        "query file `{shown}` points outside the site"
    );
    ensure!(entry.can_compile(), "query path `{shown}` must name a file");
    resources.source_boundary(cfg).check(&entry.path)?;
    Ok((
        SelectedQuery {
            name: format!("{}#{label}", display_path(&entry.path, cfg.root())),
            label: label.to_owned(),
            entities: false,
            module_manifest: None,
        },
        tola_module::bundle::select_query_file(&entry.path),
    ))
}

pub(super) fn prepare(
    cfg: Arc<SiteConfig>,
    query: ModelQuery<'_>,
    resources: BuildResources,
    cancel: &BuildCancellation,
) -> Result<(QuerySession, SelectedQuery)> {
    cancel.ensure_active()?;
    let mut configured = crate::modules::site::prepare_site(&cfg, resources.input_mode(), cancel)?;
    crate::build::install_hooks(&mut configured, &resources)?;
    let (selected, query_cfg) = match query {
        ModelQuery::Module {
            module,
            declaration,
            name,
        } => prepare_module_query(&cfg, &configured, &resources, module, declaration, name)?,
        ModelQuery::File { entry, label } => {
            prepare_file_query(&cfg, &configured, &resources, entry, label)?
        }
    };
    configured.select_entry(query_cfg)?;
    let site = Arc::new(configured);
    ensure!(
        site.prepared()
            .manifests_are_fresh(&cancel.bundle_cancellation())?,
        "the module changed while Tola was preparing the query `{}`; run the query again",
        selected.name
    );
    let generated = generated_inputs(site.hooks())
        .filter(|(producer, _)| !producer.starts_with("@module/"))
        .map(|(producer, path)| GeneratedInput {
            producer: producer.to_owned(),
            path: display_path(path, cfg.root()),
        })
        .chain(site.prepared().modules.values().flat_map(|module| {
            tola_module::hooks::declared_generated_inputs(module).map(|(producer, path)| {
                GeneratedInput {
                    producer,
                    path: display_path(&path, cfg.root()),
                }
            })
        }))
        .collect();
    let session = SourceDiagnosticSession::with_prepared_modules(Arc::clone(&site), resources);
    Ok((
        QuerySession {
            site,
            session,
            generated,
        },
        selected,
    ))
}
