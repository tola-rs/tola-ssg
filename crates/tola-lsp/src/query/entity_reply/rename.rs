use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use lsp_types::{PrepareRenameResponse, TextEdit, Uri, WorkspaceEdit};
use tola_build::config::SiteConfig;
use tola_build::modules::query::QueryEvaluation;
use tola_module::entities::Entity;
use tola_module::module_set::LoadedModule;
use tola_typst::typst::World as _;
use tola_typst::typst::foundations::{Repr, Str};
use tola_typst::typst::syntax::{FileId, Source, VirtualRoot};

use crate::analysis::NameQueryError;
use crate::packages::PackageAccess;
use crate::position::utf16_range;
use crate::protocol::{SourceQuery, SourceReply};

use super::super::QuerySite;
use super::super::semantic::Semantic;
use super::{
    EntityOccurrences, EntityQuery, EntityScope, Occurrence, checked_compilation,
    definition_ranges, entity_bindings, module_package,
};

struct RenameProposal {
    changes: HashMap<Uri, Vec<TextEdit>>,
    proposed: Vec<(PathBuf, Arc<str>)>,
}

pub(super) fn rename_refusal(msg: impl Into<String>) -> NameQueryError {
    NameQueryError::EntityRename(msg.into())
}

fn refused_rename(query: &SourceQuery, refusal: NameQueryError) -> Result<SourceReply> {
    if matches!(query, SourceQuery::PrepareRename(_))
        && matches!(refusal, NameQueryError::EntityRename(_))
    {
        return Ok(SourceReply::PrepareRename(None));
    }
    Err(refusal.into())
}

pub(super) fn rename_answer(
    req: &mut EntityQuery<'_>,
    modules: &BTreeMap<String, LoadedModule>,
    scope: &EntityScope<'_>,
    entity: &Entity,
    range: Option<lsp_types::Range>,
    found: &EntityOccurrences,
) -> Result<SourceReply> {
    let query = req.query;
    let def = match rename_definition(req.site, scope, entity, found) {
        Ok(def) => def,
        Err(refusal) => return refused_rename(query, refusal),
    };
    if let Some(refusal) = refuse_shared_definition(req, modules, scope, entity, def)? {
        return refused_rename(query, refusal);
    }
    if matches!(query, SourceQuery::PrepareRename(_)) {
        return Ok(SourceReply::PrepareRename(range.map(|range| {
            PrepareRenameResponse::RangeWithPlaceholder {
                range,
                placeholder: entity.id.clone(),
            }
        })));
    }
    let SourceQuery::Rename(params) = query else {
        unreachable!("hover, references, and prepare-rename answered the other queries")
    };
    rename_workspace_edit(
        req,
        modules,
        scope,
        entity,
        &params.new_name,
        &found.occurrences,
    )
}

fn rename_definition<'a>(
    site: &QuerySite<'_>,
    scope: &EntityScope<'_>,
    entity: &Entity,
    found: &'a EntityOccurrences,
) -> std::result::Result<&'a Occurrence, NameQueryError> {
    let defs = entity_definitions(&found.occurrences, scope);
    if found.unresolved {
        return Err(rename_refusal(
            "entity references include unresolved arguments; rename requires a complete set of writable literals",
        ));
    }
    let Some(def) = writable_definition(entity, &defs, site) else {
        return Err(rename_refusal(
            "entity rename requires one writable literal definition in its declared source; derived and external identities cannot be renamed",
        ));
    };
    if found
        .occurrences
        .iter()
        .any(|occ| !writable(&occ.src, site.cfg, site.pkgs))
    {
        return Err(rename_refusal(
            "an entity occurrence is outside writable project sources",
        ));
    }
    Ok(def)
}

fn writable_definition<'a>(
    entity: &Entity,
    defs: &[&'a Occurrence],
    site: &QuerySite<'_>,
) -> Option<&'a Occurrence> {
    let [def] = defs else {
        return None;
    };
    (entity.source == Some(def.src.id()) && writable(&def.src, site.cfg, site.pkgs)).then_some(*def)
}

fn rename_workspace_edit(
    req: &mut EntityQuery<'_>,
    modules: &BTreeMap<String, LoadedModule>,
    scope: &EntityScope<'_>,
    entity: &Entity,
    new_name: &str,
    occs: &[Occurrence],
) -> Result<SourceReply> {
    let res = req.resources;
    let cancel = req.site.cancel;
    let EntityScope {
        module,
        query_name,
        model,
        ..
    } = *scope;
    let RenameProposal { changes, proposed } = rename_edits(req, scope, occs, new_name)?;
    // Verify the proposed edits actually update the queried entity identity without altering others.
    let checked = proposed_model(req, module, query_name, &proposed)?;
    let mut expected = entity_sources(&model.entities);
    expected.remove(entity.id.as_str());
    expected.insert(new_name, entity.source);
    if !checked.result.success || entity_sources(&checked.entities) != expected {
        return Err(rename_refusal(
            "the proposed edits must rename only the selected entity and preserve the other entity IDs and sources",
        )
        .into());
    }
    for (other_mod, other_query) in other_queries(modules, module.name.as_str(), query_name) {
        let orig = req.cache.get(other_mod, other_query, res.clone(), cancel)?;
        let checked = proposed_model(req, other_mod, other_query, &proposed)?;
        if !orig.result.success
            || !checked.result.success
            || entity_sources(&checked.entities) != entity_sources(&orig.entities)
        {
            return Err(rename_refusal(format!(
                "the proposed edits change entity IDs or sources in query `{}.{other_query}`; rename would leave its references unchanged",
                other_mod.name
            ))
            .into());
        }
    }
    cancel.ensure_active()?;
    Ok(SourceReply::Rename(Some(WorkspaceEdit {
        changes: Some(changes),
        ..WorkspaceEdit::default()
    })))
}

fn entity_definitions<'a>(occs: &'a [Occurrence], scope: &EntityScope<'_>) -> Vec<&'a Occurrence> {
    let mut defs = occs
        .iter()
        .filter(|occ| scope.bindings[occ.arg.binding].definition)
        .collect::<Vec<_>>();
    defs.sort_by_key(|occ| (occ.uri.as_str(), occ.arg.range.start, occ.arg.range.end));
    defs.dedup_by(|l, r| l.uri == r.uri && l.arg.range == r.arg.range);
    defs
}

fn rename_edits(
    req: &EntityQuery<'_>,
    scope: &EntityScope<'_>,
    occs: &[Occurrence],
    new_name: &str,
) -> Result<RenameProposal> {
    let EntityQuery {
        source,
        query,
        overlays,
        view,
        ..
    } = *req;
    let client_root = req.site.client_root;
    let cfg = req.site.cfg;
    if new_name.is_empty() || scope.model.entities.iter().any(|e| e.id == new_name) {
        return Err(rename_refusal(
            "the new entity ID must be nonempty and must not already exist in this entity query",
        )
        .into());
    }
    let replacement = Str::from(new_name).repr().to_string();
    let mut by_src = BTreeMap::<PathBuf, (Source, Vec<std::ops::Range<usize>>)>::new();
    for occ in occs {
        let path = cfg.root().join(occ.src.id().vpath().get_without_slash());
        let (_, ranges) = by_src
            .entry(path)
            .or_insert_with(|| (occ.src.clone(), Vec::new()));
        ranges.push(occ.arg.range.clone());
    }
    let mut changes = HashMap::new();
    let mut proposed = overlays.to_vec();
    for (path, (text, mut ranges)) in by_src {
        ranges.sort_by_key(|r| (r.start, r.end));
        ranges.dedup();
        if ranges.windows(2).any(|pair| pair[0].end > pair[1].start) {
            return Err(rename_refusal("entity edit ranges overlap").into());
        }
        let edits = ranges
            .iter()
            .map(|r| {
                Ok(TextEdit {
                    range: utf16_range(text.lines(), r.clone()).ok_or_else(|| {
                        anyhow::Error::from(rename_refusal(
                            "entity literal range is not valid in the current source",
                        ))
                    })?,
                    new_text: replacement.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut uris = view
            .versions_for_path(&path)
            .map(|(uri, _)| uri.parse::<Uri>())
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if uris.is_empty() {
            uris.push(if text.id() == source.id() {
                query.document().0.clone()
            } else {
                client_root.address(&path)?
            });
        }
        for uri in uris {
            changes.insert(uri, edits.clone());
        }
        let mut updated = text.text().to_owned();
        for range in ranges.into_iter().rev() {
            updated.replace_range(range, &replacement);
        }
        proposed.retain(|(existing, _)| {
            crate::identity::path_id(existing, cfg.root()) != Some(text.id())
        });
        proposed.push((path, Arc::<str>::from(updated)));
    }
    Ok(RenameProposal { changes, proposed })
}

fn entity_sources(entities: &[Entity]) -> BTreeMap<&str, Option<FileId>> {
    entities.iter().map(|e| (e.id.as_str(), e.source)).collect()
}

fn other_queries<'a>(
    modules: &'a BTreeMap<String, LoadedModule>,
    selected_mod: &'a str,
    selected_query: &'a str,
) -> impl Iterator<Item = (&'a LoadedModule, &'a str)> + 'a {
    modules.values().flat_map(move |module| {
        module
            .queries
            .iter()
            .filter(|(_, q)| q.has_entities())
            .map(|(name, _)| name.as_str())
            .filter(move |q| module.name != selected_mod || *q != selected_query)
            .map(move |q| (module, q))
    })
}

fn proposed_model(
    req: &EntityQuery<'_>,
    module: &LoadedModule,
    query: &str,
    proposed: &[(PathBuf, Arc<str>)],
) -> Result<QueryEvaluation> {
    tola_build::modules::query::run_entities(
        Arc::clone(req.site.cfg),
        module,
        &module.queries[query],
        &format!("{}.{query}", module.name),
        req.resources.clone(),
        proposed.to_vec(),
        req.site.cancel,
    )
}

fn refuse_shared_definition(
    req: &mut EntityQuery<'_>,
    modules: &BTreeMap<String, LoadedModule>,
    scope: &EntityScope<'_>,
    entity: &Entity,
    def: &Occurrence,
) -> Result<Option<NameQueryError>> {
    let EntityQuery {
        site,
        resources: res,
        ..
    } = *req;
    let QuerySite {
        pkgs,
        cancel,
        client_root,
        ..
    } = *site;
    let target =
        tola_input::normalize_existing_prefix(&crate::uri::to_site_path(def.uri.as_str())?);
    for (module, query) in other_queries(modules, scope.module.name.as_str(), scope.query_name) {
        let model = req.cache.get(module, query, res.clone(), cancel)?;
        if !model.result.success {
            return Ok(Some(rename_refusal(format!(
                "cannot establish entity rename ownership while query {}.{query} fails",
                module.name
            ))));
        }
        let Some(src_id) = model
            .entities
            .iter()
            .find(|other| other.id == entity.id)
            .and_then(|other| other.source)
        else {
            continue;
        };
        let Some(comp) = checked_compilation(&model) else {
            return Ok(Some(rename_refusal(
                "another entity query has no checked source world",
            )));
        };
        let mut sem = Semantic::new(comp, cancel);
        let Some(uri) = sem.source_uri(src_id, client_root) else {
            continue;
        };
        let Ok(path) = crate::uri::to_site_path(uri.as_str()) else {
            continue;
        };
        if tola_input::normalize_existing_prefix(&path) != target {
            continue;
        }
        let Ok(src) = comp.world().source(src_id) else {
            return Ok(Some(rename_refusal(
                "a shared entity definition cannot be read",
            )));
        };
        if src.text() != def.src.text() {
            return Ok(Some(NameQueryError::ConflictingSourceText));
        }
        let literals = match module_package(pkgs, module.name.as_str())? {
            Some(pkg) => {
                let bindings = entity_bindings(&mut sem, pkg, module, query)?;
                definition_ranges(&src, &mut sem, &bindings, &entity.id)?
            }
            None => Vec::new(),
        };
        req.cache.observe_accessed(sem.accessed());
        if literals.len() != 1 || literals[0] == def.arg.range {
            return Ok(Some(rename_refusal(format!(
                "entity definition is shared with query {}.{query}; renaming one namespace would leave the other namespace's references unchanged",
                module.name
            ))));
        }
    }
    Ok(None)
}

fn writable(src: &Source, cfg: &SiteConfig, pkgs: &PackageAccess<'_>) -> bool {
    if !matches!(src.id().root(), VirtualRoot::Project) {
        return false;
    }
    let path = cfg.root().join(src.id().vpath().get_without_slash());
    let physical = tola_input::normalize_existing_prefix(&path);
    physical.starts_with(cfg.root()) && pkgs.boundary.check(&physical).is_ok()
}
