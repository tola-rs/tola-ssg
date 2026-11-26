//! Editor entity answers use the model's world, including its unsaved source text.

mod rename;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, ensure};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, GotoDefinitionResponse, Hover,
    HoverContents, Location, MarkupContent, MarkupKind, TextEdit, Uri, WorkspaceEdit,
};
use tola_build::BuildResources;
use tola_build::cancellation::BuildCancelled;
use tola_build::check::SourceCompilation;
use tola_build::modules::query::QueryEvaluation;
use tola_module::declaration::Parameter;
use tola_module::entities::Entity;
use tola_module::module_set::LoadedModule;
use tola_typst::typst::World;
use tola_typst::typst::foundations::{Repr, Str, Value};
use tola_typst::typst::syntax::ast::AstNode;
use tola_typst::typst::syntax::package::PackageSpec;
use tola_typst::typst::syntax::{FileId, LinkedNode, Side, Source, ast};

use crate::compiler::EntityQueries;
use crate::packages::PackageAccess;
use crate::position::utf16_range;
use crate::protocol::{SourceQuery, SourceReply};

use super::QuerySite;
use super::entities::{self, EntityArgument, EntityBinding};
use super::semantic::Semantic;

pub(super) struct EntityQuery<'a> {
    pub(super) site: &'a QuerySite<'a>,
    pub(super) source: &'a Source,
    pub(super) cursor: usize,
    pub(super) query: &'a SourceQuery,
    pub(super) cache: &'a mut EntityQueries,
    pub(super) resources: &'a BuildResources,
    pub(super) revision: u64,
    pub(super) overlays: &'a [(PathBuf, Arc<str>)],
    pub(super) view: &'a crate::sources::SourceView,
    pub(super) disk: &'a mut crate::analysis::DiskSources,
}

struct EntityScope<'a> {
    module: &'a LoadedModule,
    query_name: &'a str,
    model: &'a QueryEvaluation,
    comp: &'a SourceCompilation,
    bindings: &'a [EntityBinding],
}

pub(super) fn respond(mut req: EntityQuery<'_>) -> Result<Option<SourceReply>> {
    let EntityQuery {
        site,
        source: src,
        cursor: pos,
        query,
        resources: res,
        overlays,
        ..
    } = req;
    site.cancel.ensure_active()?;
    if !matches!(
        query,
        SourceQuery::Completion(_)
            | SourceQuery::Definition(_)
            | SourceQuery::Hover(_)
            | SourceQuery::References(_)
            | SourceQuery::PrepareRename(_)
            | SourceQuery::Rename(_)
    ) || !string_argument(src, pos)
    {
        return Ok(None);
    }
    let modules = tola_build::modules::load_modules(
        site.cfg.root(),
        site.cfg.vendor().path.as_deref(),
        site.cfg.modules(),
        res.input_mode(),
        &Default::default(),
    )?;
    if modules
        .values()
        .all(|module| module.queries.values().all(|q| !q.has_entities()))
    {
        return Ok(None);
    }
    req.cache.prepare(
        site.cfg,
        req.revision,
        res.input_scope(),
        overlays,
        &modules,
    );
    let mut answers = EntityAnswers::default();
    for (alias, module) in &modules {
        let Some(pkg) = module_package(site.pkgs, alias)? else {
            continue;
        };
        for (name, decl) in &module.queries {
            if !decl.has_entities() {
                continue;
            }
            let model = req.cache.get(module, name, res.clone(), site.cancel)?;
            let Some(comp) = checked_compilation(&model) else {
                continue;
            };
            // A model compiled from other packages or text belongs to a superseded revision.
            if comp.world().local_packages().as_ref() != site.pkgs.local_packages.as_ref() {
                return Err(BuildCancelled.into());
            }
            let (_, accessed) = comp.world().read_file_with_evidence(src.id());
            req.cache.observe_accessed(&accessed);
            let Ok(prep) = comp.world().source(src.id()) else {
                continue;
            };
            if prep.text() != src.text() {
                return Err(BuildCancelled.into());
            }
            let mut sem = Semantic::new(comp, site.cancel);
            let bindings = entity_bindings(&mut sem, pkg, module, name)?;
            let args = entities::argument_at(&prep, pos, &mut sem, &bindings)?;
            req.cache.observe_accessed(sem.accessed());
            if args.is_empty() {
                continue;
            }
            answers.argument_named = true;
            if !model.result.success {
                anyhow::bail!("{}", query_failure_message(alias, name, &model));
            }
            let scope = EntityScope {
                module,
                query_name: name,
                model: &model,
                comp,
                bindings: &bindings,
            };
            for arg in args {
                if let Some(reply) =
                    answer_argument(&mut req, &modules, &scope, &mut sem, arg, &mut answers)?
                {
                    return Ok(Some(reply));
                }
            }
        }
    }
    site.cancel.ensure_active()?;
    if !answers.argument_named {
        return Ok(None);
    }
    Ok(Some(answers.finish(query)?))
}

fn query_failure_message(alias: &str, name: &str, model: &QueryEvaluation) -> String {
    let msg = model
        .result
        .diagnostics
        .iter()
        .find(|d| d.severity == tola_diagnostic::Severity::Error)
        .map(|d| d.message.as_str())
        .unwrap_or("the query produced no usable result");
    let mut details = vec![msg];
    for diag in &model.result.diagnostics {
        if diag.code == tola_diagnostic::codes::query::RESULT {
            details.extend(diag.notes.iter().map(String::as_str));
            details.extend(diag.help.iter().map(|h| h.message.as_str()));
        }
    }
    format!("query `{alias}.{name}` failed: {}", details.join("\n"))
}

#[derive(Default)]
struct EntityAnswers {
    completions: Vec<CompletionItem>,
    locations: Vec<Location>,
    replies: Vec<SourceReply>,
    answered: BTreeSet<(String, String, String, usize, usize)>,
    argument_named: bool,
}

impl EntityAnswers {
    fn finish(mut self, query: &SourceQuery) -> Result<SourceReply> {
        Ok(match query {
            SourceQuery::Completion(_) => {
                self.completions.sort_by(|l, r| l.label.cmp(&r.label));
                self.completions
                    .dedup_by(|l, r| l.label == r.label && l.text_edit == r.text_edit);
                super::completion_reply(self.completions)
            }
            SourceQuery::Definition(_) => {
                self.locations
                    .sort_by_key(|l| (l.uri.as_str().to_owned(), l.range.start));
                self.locations.dedup();
                SourceReply::Definition(
                    (!self.locations.is_empty())
                        .then_some(GotoDefinitionResponse::Array(self.locations)),
                )
            }
            _ if self.replies.len() == 1 => self.replies.remove(0),
            SourceQuery::Rename(_) if self.replies.len() > 1 => {
                return Err(rename::rename_refusal(
                    "the argument names more than one declared entity identity",
                )
                .into());
            }
            _ => super::unavailable(query),
        })
    }
}

fn answer_argument(
    req: &mut EntityQuery<'_>,
    modules: &BTreeMap<String, LoadedModule>,
    scope: &EntityScope<'_>,
    sem: &mut Semantic<'_>,
    arg: EntityArgument,
    answers: &mut EntityAnswers,
) -> Result<Option<SourceReply>> {
    let query = req.query;
    let src = req.source;
    match query {
        SourceQuery::Completion(_) if !scope.bindings[arg.binding].definition => {
            let Some(range) = utf16_range(src.lines(), arg.content_range.clone()) else {
                return Ok(None);
            };
            for entity in &scope.model.entities {
                let quoted = Str::from(entity.id.as_str()).repr();
                answers.completions.push(CompletionItem {
                    label: entity.id.clone(),
                    detail: Some(entity.name.clone()),
                    documentation: entity
                        .description
                        .clone()
                        .map(lsp_types::Documentation::String),
                    kind: Some(CompletionItemKind::REFERENCE),
                    filter_text: Some(format!("{} {}", entity.id, entity.name)),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                        range,
                        new_text: quoted[1..quoted.len() - 1].to_owned(),
                    })),
                    ..CompletionItem::default()
                });
            }
        }
        SourceQuery::Definition(_) => {
            let Some(entity) = scope.model.entities.iter().find(|e| e.id == arg.value) else {
                return Ok(None);
            };
            if let Some(loc) = definition_argument(req, scope, sem, entity)? {
                answers.locations.push(loc);
            }
        }
        SourceQuery::Hover(_)
        | SourceQuery::References(_)
        | SourceQuery::PrepareRename(_)
        | SourceQuery::Rename(_) => {
            let Some(entity) = scope.model.entities.iter().find(|e| e.id == arg.value) else {
                return Ok(None);
            };
            if let SourceQuery::Rename(params) = query
                && params.new_name == entity.id
            {
                req.site.cancel.ensure_active()?;
                return Ok(Some(SourceReply::Rename(Some(WorkspaceEdit::default()))));
            }
            if !answers.answered.insert((
                scope.module.name.clone(),
                scope.query_name.to_owned(),
                arg.value.clone(),
                arg.range.start,
                arg.range.end,
            )) {
                return Ok(None);
            }
            answers
                .replies
                .push(entity_answer(req, modules, scope, entity, &arg, sem)?);
        }
        _ => {}
    }
    Ok(None)
}

fn definition_argument(
    req: &mut EntityQuery<'_>,
    scope: &EntityScope<'_>,
    sem: &mut Semantic<'_>,
    entity: &Entity,
) -> Result<Option<Location>> {
    let Some(target) = entity.source else {
        // Canonical external URLs are not local source definitions.
        return Ok(None);
    };
    let Ok(target_src) = scope.comp.world().source(target) else {
        return Ok(None);
    };
    let Some(uri) = sem.source_uri(target, req.site.client_root) else {
        return Ok(None);
    };
    let decls = definition_ranges(&target_src, sem, scope.bindings, &entity.id)?;
    req.cache.observe_accessed(sem.accessed());
    let range = if decls.len() == 1 {
        utf16_range(target_src.lines(), decls[0].clone()).unwrap_or_default()
    } else {
        lsp_types::Range::default()
    };
    Ok(Some(Location::new(uri, range)))
}

fn definition_ranges(
    src: &Source,
    sem: &mut Semantic<'_>,
    bindings: &[EntityBinding],
    entity_id: &str,
) -> Result<Vec<std::ops::Range<usize>>> {
    let mut ranges = entities::scan(src, sem, bindings)?
        .literals
        .into_iter()
        .filter(|arg| bindings[arg.binding].definition && arg.value == entity_id)
        .map(|arg| arg.range)
        .collect::<Vec<_>>();
    ranges.sort_by_key(|r| (r.start, r.end));
    ranges.dedup();
    Ok(ranges)
}

fn entity_bindings(
    sem: &mut Semantic<'_>,
    pkg: &tola_typst::LocalPackage,
    module: &LoadedModule,
    selected_query: &str,
) -> Result<Vec<EntityBinding>> {
    let mut bindings: Vec<(&str, String, EntityBinding)> = Vec::new();
    for (query_name, query) in &module.queries {
        for (kind, definition, params) in [
            ("references", false, &query.references),
            ("definitions", true, &query.definitions),
        ] {
            for (idx, decl) in params.iter().enumerate() {
                let key = format!("queries.{query_name}.{kind}[{idx}]");
                let func = sem
                    .public_export(pkg.entry_id(), &decl.export)?
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "module `{}` `{key}.export`: `{}` is not an available public function",
                            module.name,
                            decl.export,
                        )
                    })?;
                let declares_param = match &decl.parameter {
                    Parameter::Named { named } => func
                        .params()
                        .any(|p| p.named() && p.name() == Some(named.as_str())),
                    Parameter::Positional { positional } => func
                        .params()
                        .filter(|p| p.positional() && !p.variadic())
                        .nth(*positional)
                        .is_some(),
                };
                ensure!(
                    declares_param,
                    "module `{}` `{key}.parameter`: `{}` does not declare this parameter",
                    module.name,
                    decl.export,
                );
                if let Some((prev_query, prev_key, prev)) = bindings
                    .iter()
                    .find(|(_, _, prev)| prev.function == func && prev.parameter == decl.parameter)
                {
                    ensure!(
                        *prev_query == query_name.as_str() && prev.definition == definition,
                        "module `{}` `{key}` conflicts with `{prev_key}` for the same function parameter",
                        module.name,
                    );
                    continue;
                }
                bindings.push((
                    query_name,
                    key,
                    EntityBinding {
                        parameter: decl.parameter.clone(),
                        definition,
                        function: func,
                    },
                ));
            }
        }
    }
    Ok(bindings
        .into_iter()
        .filter(|(q, _, _)| *q == selected_query)
        .map(|(_, _, binding)| binding)
        .collect())
}

#[derive(Debug, thiserror::Error)]
#[error("invalid configured module identity: {message}")]
struct ModuleIdentityError {
    message: String,
}

fn module_package<'a>(
    pkgs: &'a PackageAccess<'_>,
    alias: &str,
) -> Result<Option<&'a tola_typst::LocalPackage>> {
    let spec = format!("@tola-modules/{alias}:0.0.0")
        .parse::<PackageSpec>()
        .map_err(|err| ModuleIdentityError {
            message: err.to_string(),
        })?;
    Ok(pkgs.local_packages.package(&spec))
}

fn checked_compilation(model: &QueryEvaluation) -> Option<&SourceCompilation> {
    model.revision.as_ref().and_then(|rev| rev.checked())
}

struct Occurrence {
    src: Source,
    arg: EntityArgument,
    uri: Uri,
}

struct EntityOccurrences {
    occurrences: Vec<Occurrence>,
    unresolved: bool,
}

fn entity_answer(
    req: &mut EntityQuery<'_>,
    modules: &BTreeMap<String, LoadedModule>,
    scope: &EntityScope<'_>,
    entity: &Entity,
    selected: &EntityArgument,
    sem: &mut Semantic<'_>,
) -> Result<SourceReply> {
    let query = req.query;
    let range = utf16_range(req.source.lines(), selected.content_range.clone());
    if matches!(query, SourceQuery::Hover(_)) {
        return Ok(entity_hover(entity, scope, range));
    }
    let found = collect_occurrences(req, scope, entity, sem)?;
    match query {
        SourceQuery::References(params) => Ok(entity_references(
            &found.occurrences,
            scope,
            params.context.include_declaration,
        )),
        SourceQuery::PrepareRename(_) | SourceQuery::Rename(_) => {
            rename::rename_answer(req, modules, scope, entity, range, &found)
        }
        _ => unreachable!("completion and definition answer in their own arms"),
    }
}

fn entity_hover(
    entity: &Entity,
    scope: &EntityScope<'_>,
    range: Option<lsp_types::Range>,
) -> SourceReply {
    let mut text = format!(
        "{}\n\nID: {}\nEntity query: {}.{}",
        entity.name, entity.id, scope.module.name, scope.query_name
    );
    if let Some(desc) = &entity.description {
        text.push_str(&format!("\n\n{desc}"));
    }
    if let Some(url) = &entity.url {
        text.push_str(&format!("\n\n{url}"));
    }
    SourceReply::Hover(Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::PlainText,
            value: text,
        }),
        range,
    }))
}

fn collect_occurrences(
    req: &mut EntityQuery<'_>,
    scope: &EntityScope<'_>,
    entity: &Entity,
    sem: &mut Semantic<'_>,
) -> Result<EntityOccurrences> {
    let EntityQuery {
        source: src,
        view,
        overlays,
        ..
    } = *req;
    let client_root = req.site.client_root;
    let cfg = req.site.cfg;
    let pkgs = req.site.pkgs;
    let cancel = req.site.cancel;
    let comp = scope.comp;
    let mut files =
        crate::analysis::site_sources(cfg.root(), view, &pkgs.boundary, req.disk, cancel)?
            .into_iter()
            .map(|names| names.source().id())
            .collect::<Vec<_>>();
    files.push(comp.world().main());
    files.push(src.id());
    files.extend(
        overlays
            .iter()
            .filter_map(|(path, _)| crate::identity::path_id(path, cfg.root())),
    );
    files.extend(entity.source);
    let mut scanned = HashSet::new();
    let mut occs = Vec::new();
    let mut unresolved = false;
    while let Some(file) = files.pop() {
        cancel.ensure_active()?;
        if !scanned.insert(file) {
            continue;
        }
        let (_, accessed) = comp.world().read_file_with_evidence(file);
        req.cache.observe_accessed(&accessed);
        let Ok(text) = comp.world().source(file) else {
            unresolved = true;
            continue;
        };
        files.extend(imported_sources(&text, sem)?);
        let scan = entities::scan(&text, sem, scope.bindings)?;
        unresolved |= scan.unresolved;
        let Some(uri) = sem.source_uri(file, client_root) else {
            unresolved |= scan.literals.iter().any(|arg| arg.value == entity.id);
            continue;
        };
        if let Ok(path) = crate::uri::to_site_path(uri.as_str()) {
            crate::analysis::require_source_text(view, &path, text.text())?;
        }
        for arg in scan
            .literals
            .into_iter()
            .filter(|arg| arg.value == entity.id)
        {
            occs.push(Occurrence {
                src: text.clone(),
                arg,
                uri: uri.clone(),
            });
        }
    }
    req.cache.observe_accessed(sem.accessed());
    Ok(EntityOccurrences {
        occurrences: occs,
        unresolved,
    })
}

fn entity_references(
    occs: &[Occurrence],
    scope: &EntityScope<'_>,
    include_declaration: bool,
) -> SourceReply {
    let mut locs = occs
        .iter()
        .filter(|occ| include_declaration || !scope.bindings[occ.arg.binding].definition)
        .filter_map(|occ| {
            Some(Location::new(
                occ.uri.clone(),
                utf16_range(occ.src.lines(), occ.arg.range.clone())?,
            ))
        })
        .collect::<Vec<_>>();
    locs.sort_by_key(|l| (l.uri.as_str().to_owned(), l.range.start, l.range.end));
    locs.dedup();
    SourceReply::References(Some(locs))
}

fn imported_sources(src: &Source, sem: &mut Semantic<'_>) -> Result<Vec<FileId>> {
    let mut files = Vec::new();
    let mut pending = vec![LinkedNode::new(src.root())];
    while let Some(node) = pending.pop() {
        sem.cancellation().ensure_active()?;
        let imported = node
            .cast::<ast::ModuleImport>()
            .map(|import| import.source())
            .or_else(|| node.cast::<ast::ModuleInclude>().map(|inc| inc.source()));
        if let Some(ast::Expr::Str(lit)) = imported
            && let Some(expr) = node.find(lit.span())
            && let Some(Value::Module(module)) = sem.import(&expr)?
            && let Some(file) = module.file_id()
        {
            files.push(file);
        }
        pending.extend(node.children());
    }
    Ok(files)
}

fn string_argument(src: &Source, pos: usize) -> bool {
    let root = LinkedNode::new(src.root());
    [Side::Before, Side::After].into_iter().any(|side| {
        let Some(node) = root.leaf_at(pos, side) else {
            return false;
        };
        node.cast::<ast::Str>().is_some() && node.range().start < pos && pos < node.range().end
    })
}
