//! Editor entity answers use the model's world, including its unsaved source text.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, ensure};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, GotoDefinitionResponse, Hover,
    HoverContents, Location, MarkupContent, MarkupKind, PrepareRenameResponse, TextEdit, Uri,
    WorkspaceEdit,
};
use tola_build::BuildResources;
use tola_build::cancellation::BuildCancelled;
use tola_build::check::SourceCompilation;
use tola_build::config::ResolvedSiteConfig;
use tola_build::modules::query::QueryEvaluation;
use tola_module::declaration::Parameter;
use tola_module::entities::Entity;
use tola_module::module_set::LoadedModule;
use tola_typst::typst::World;
use tola_typst::typst::foundations::{Repr, Str, Value};
use tola_typst::typst::syntax::ast::AstNode;
use tola_typst::typst::syntax::package::PackageSpec;
use tola_typst::typst::syntax::{FileId, LinkedNode, Side, Source, VirtualRoot, ast};

use crate::analysis::NameQueryError;
use crate::compiler::EntityQueries;
use crate::packages::PackageAccess;
use crate::position::utf16_range;
use crate::protocol::{SourceQuery, SourceReply};

use super::QuerySite;
use super::entities::{self, EntityBinding};
use super::semantic::Semantic;

/// One entity query: the editor's request and the session state every answer reads.
pub(super) struct EntityQuery<'a> {
    pub(super) site: &'a QuerySite<'a>,
    /// The document the editor asks about, as the editor last wrote it.
    pub(super) source: &'a Source,
    /// The cursor's byte offset in `source`.
    pub(super) cursor: usize,
    pub(super) query: &'a SourceQuery,
    /// The entity models this revision caches.
    pub(super) cache: &'a mut EntityQueries,
    pub(super) resources: &'a BuildResources,
    /// The source revision the cached models belong to.
    pub(super) revision: u64,
    /// The editor's unsaved text, by path.
    pub(super) overlays: &'a [(PathBuf, Arc<str>)],
    pub(super) view: &'a crate::sources::SourceView,
    pub(super) disk: &'a mut crate::analysis::DiskSources,
}

/// The module query one entity argument belongs to, and the model that proves it.
struct EntityScope<'a> {
    module: &'a LoadedModule,
    query_name: &'a str,
    model: &'a QueryEvaluation,
    compilation: &'a SourceCompilation,
    bindings: &'a [EntityBinding],
}

pub(super) fn respond(mut request: EntityQuery<'_>) -> Result<Option<SourceReply>> {
    let EntityQuery {
        site,
        source,
        cursor,
        query,
        resources,
        overlays,
        ..
    } = request;
    site.cancellation.ensure_active()?;
    if !matches!(
        query,
        SourceQuery::Completion(_)
            | SourceQuery::Definition(_)
            | SourceQuery::Hover(_)
            | SourceQuery::References(_)
            | SourceQuery::PrepareRename(_)
            | SourceQuery::Rename(_)
    ) || !string_argument(source, cursor)
    {
        return Ok(None);
    }
    let modules = tola_build::modules::load_modules(
        site.configuration.get_root(),
        site.configuration.vendor().path.as_deref(),
        site.configuration.modules(),
        resources.input_mode(),
        &Default::default(),
    )?;
    if modules
        .values()
        .all(|module| module.queries.values().all(|query| !query.has_entities()))
    {
        return Ok(None);
    }
    request.cache.prepare(
        site.configuration,
        request.revision,
        resources.input_scope(),
        overlays,
        &modules,
    );
    let mut answers = EntityAnswers::default();
    for (alias, module) in &modules {
        let Some(package) = module_package(site.packages, alias)? else {
            continue;
        };
        for (name, declaration) in &module.queries {
            if !declaration.has_entities() {
                continue;
            }
            let model = request
                .cache
                .get(module, name, resources.clone(), site.cancellation)?;
            let Some(compilation) = checked_compilation(&model) else {
                continue;
            };
            // A model compiled from other packages or text belongs to a superseded revision.
            if compilation.world().local_packages().as_ref()
                != site.packages.local_packages.as_ref()
            {
                return Err(BuildCancelled.into());
            }
            let (_, accessed) = compilation.world().read_file_with_evidence(source.id());
            request.cache.observe_accessed(&accessed);
            let Ok(prepared) = compilation.world().source(source.id()) else {
                continue;
            };
            if prepared.text() != source.text() {
                return Err(BuildCancelled.into());
            }
            let mut semantics = Semantic::new(compilation, site.cancellation);
            let bindings = entity_bindings(&mut semantics, package, module, name)?;
            let arguments = entities::argument_at(&prepared, cursor, &mut semantics, &bindings)?;
            request.cache.observe_accessed(semantics.accessed());
            if arguments.is_empty() {
                continue;
            }
            answers.argument_named = true;
            if !model.result.success {
                let message = model
                    .result
                    .diagnostics
                    .iter()
                    .find(|diagnostic| diagnostic.severity == tola_diagnostic::Severity::Error)
                    .map(|diagnostic| diagnostic.message.as_str())
                    .unwrap_or("the query produced no usable result");
                anyhow::bail!("query `{alias}.{name}` failed: {message}");
            }
            let scope = EntityScope {
                module,
                query_name: name,
                model: &model,
                compilation,
                bindings: &bindings,
            };
            for argument in arguments {
                if let Some(reply) = answer_argument(
                    &mut request,
                    &modules,
                    &scope,
                    &mut semantics,
                    argument,
                    &mut answers,
                )? {
                    return Ok(Some(reply));
                }
            }
        }
    }
    site.cancellation.ensure_active()?;
    if !answers.argument_named {
        return Ok(None);
    }
    Ok(Some(answers.finish(query)?))
}

/// The answers one entity query collects before it replies.
#[derive(Default)]
struct EntityAnswers {
    completions: Vec<CompletionItem>,
    locations: Vec<Location>,
    replies: Vec<SourceReply>,
    /// The literals answered already, by module, query, ID, and range.
    answered: BTreeSet<(String, String, String, usize, usize)>,
    argument_named: bool,
}

impl EntityAnswers {
    /// The reply these answers make for `query`.
    fn finish(self, query: &SourceQuery) -> Result<SourceReply> {
        let mut replies = self.replies;
        Ok(match query {
            SourceQuery::Completion(_) => {
                let mut completions = self.completions;
                completions.sort_by(|left, right| left.label.cmp(&right.label));
                completions.dedup_by(|left, right| {
                    left.label == right.label && left.text_edit == right.text_edit
                });
                super::completion_reply(completions)
            }
            SourceQuery::Definition(_) => {
                let mut locations = self.locations;
                locations.sort_by(|left, right| {
                    left.uri
                        .as_str()
                        .cmp(right.uri.as_str())
                        .then_with(|| left.range.start.line.cmp(&right.range.start.line))
                        .then_with(|| left.range.start.character.cmp(&right.range.start.character))
                });
                locations.dedup();
                SourceReply::Definition(
                    (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations)),
                )
            }
            _ if replies.len() == 1 => replies.remove(0),
            SourceQuery::Rename(_) if replies.len() > 1 => {
                return Err(rename_refusal(
                    "the argument names more than one declared entity identity",
                )
                .into());
            }
            _ => super::unavailable(query),
        })
    }
}

/// The answer one entity argument gets: collected, or the reply that ends the request.
fn answer_argument(
    request: &mut EntityQuery<'_>,
    modules: &BTreeMap<String, LoadedModule>,
    scope: &EntityScope<'_>,
    semantics: &mut Semantic<'_>,
    argument: entities::EntityArgument,
    answers: &mut EntityAnswers,
) -> Result<Option<SourceReply>> {
    let query = request.query;
    let EntityQuery { source, .. } = *request;
    match query {
        SourceQuery::Completion(_) if !scope.bindings[argument.binding].definition => {
            let Some(range) = utf16_range(source.lines(), argument.content_range.clone()) else {
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
            let Some(entity) = scope
                .model
                .entities
                .iter()
                .find(|entity| entity.id == argument.value)
            else {
                return Ok(None);
            };
            if let Some(location) = definition_argument(request, scope, semantics, entity)? {
                answers.locations.push(location);
            }
        }
        SourceQuery::Hover(_)
        | SourceQuery::References(_)
        | SourceQuery::PrepareRename(_)
        | SourceQuery::Rename(_) => {
            let Some(entity) = scope
                .model
                .entities
                .iter()
                .find(|entity| entity.id == argument.value)
            else {
                return Ok(None);
            };
            if let SourceQuery::Rename(parameters) = query
                && parameters.new_name == entity.id
            {
                request.site.cancellation.ensure_active()?;
                return Ok(Some(SourceReply::Rename(Some(WorkspaceEdit::default()))));
            }
            if !answers.answered.insert((
                scope.module.name.clone(),
                scope.query_name.to_owned(),
                argument.value.clone(),
                argument.range.start,
                argument.range.end,
            )) {
                return Ok(None);
            }
            answers.replies.push(entity_answer(
                request, modules, scope, entity, &argument, semantics,
            )?);
        }
        _ => {}
    }
    Ok(None)
}

/// Where one definition argument's entity is declared, in its declared source.
fn definition_argument(
    request: &mut EntityQuery<'_>,
    scope: &EntityScope<'_>,
    semantics: &mut Semantic<'_>,
    entity: &Entity,
) -> Result<Option<Location>> {
    let Some(target) = entity.source else {
        // A canonical web URL is not a local source definition.
        return Ok(None);
    };
    let Ok(target_source) = scope.compilation.world().source(target) else {
        return Ok(None);
    };
    let Some(uri) = semantics.source_uri(target, request.site.client_root) else {
        return Ok(None);
    };
    let declarations = definition_ranges(&target_source, semantics, scope.bindings, &entity.id)?;
    request.cache.observe_accessed(semantics.accessed());
    let range = if declarations.len() == 1 {
        utf16_range(target_source.lines(), declarations[0].clone()).unwrap_or_default()
    } else {
        lsp_types::Range::default()
    };
    Ok(Some(Location::new(uri, range)))
}

/// Every definition literal range of one entity in `source`, sorted.
fn definition_ranges(
    source: &Source,
    semantics: &mut Semantic<'_>,
    bindings: &[EntityBinding],
    entity_id: &str,
) -> Result<Vec<std::ops::Range<usize>>> {
    let mut ranges = entities::scan(source, semantics, bindings)?
        .literals
        .into_iter()
        .filter(|argument| bindings[argument.binding].definition && argument.value == entity_id)
        .map(|argument| argument.range)
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| (range.start, range.end));
    ranges.dedup();
    Ok(ranges)
}

/// Check each declared function parameter and select this query's bindings.
fn entity_bindings(
    semantics: &mut Semantic<'_>,
    package: &tola_typst::LocalPackage,
    module: &LoadedModule,
    selected_query: &str,
) -> Result<Vec<EntityBinding>> {
    let mut bindings: Vec<(&str, String, EntityBinding)> = Vec::new();
    for (query_name, query) in &module.queries {
        for (kind, definition, parameters) in [
            ("references", false, &query.references),
            ("definitions", true, &query.definitions),
        ] {
            for (index, declared) in parameters.iter().enumerate() {
                let key = format!("queries.{query_name}.{kind}[{index}]");
                let function = semantics
                    .public_export(package.entry_id(), &declared.export)?
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "module `{}` `{key}.export`: `{}` is not an available public function",
                            module.name,
                            declared.export,
                        )
                    })?;
                let declares_parameter = match &declared.parameter {
                    Parameter::Named { named } => function.params().any(|parameter| {
                        parameter.named() && parameter.name() == Some(named.as_str())
                    }),
                    Parameter::Positional { positional } => function
                        .params()
                        .filter(|parameter| parameter.positional() && !parameter.variadic())
                        .nth(*positional)
                        .is_some(),
                };
                ensure!(
                    declares_parameter,
                    "module `{}` `{key}.parameter`: `{}` does not declare this parameter",
                    module.name,
                    declared.export,
                );
                if let Some((previous_query, previous_key, previous)) =
                    bindings.iter().find(|(_, _, previous)| {
                        previous.function == function && previous.parameter == declared.parameter
                    })
                {
                    ensure!(
                        *previous_query == query_name.as_str() && previous.definition == definition,
                        "module `{}` `{key}` conflicts with `{previous_key}` for the same function parameter",
                        module.name,
                    );
                    continue;
                }
                bindings.push((
                    query_name,
                    key,
                    EntityBinding {
                        parameter: declared.parameter.clone(),
                        definition,
                        function,
                    },
                ));
            }
        }
    }
    Ok(bindings
        .into_iter()
        .filter(|(query, _, _)| *query == selected_query)
        .map(|(_, _, binding)| binding)
        .collect())
}

/// A configured module alias that cannot name a package identity.
#[derive(Debug, thiserror::Error)]
#[error("invalid configured module identity: {message}")]
struct ModuleIdentityError {
    message: String,
}

/// The local package one configured module alias resolves to.
fn module_package<'a>(
    packages: &'a PackageAccess<'_>,
    alias: &str,
) -> Result<Option<&'a tola_typst::LocalPackage>> {
    let spec = format!("@tola-modules/{alias}:0.0.0")
        .parse::<PackageSpec>()
        .map_err(|error| ModuleIdentityError {
            message: error.to_string(),
        })?;
    Ok(packages.local_packages.package(&spec))
}

/// The checked source world one cached model was compiled in.
fn checked_compilation(model: &QueryEvaluation) -> Option<&SourceCompilation> {
    model
        .revision
        .as_ref()
        .and_then(|revision| revision.checked())
}

struct Occurrence {
    source: Source,
    argument: entities::EntityArgument,
    uri: Uri,
}

fn rename_refusal(message: impl Into<String>) -> NameQueryError {
    NameQueryError::EntityRename(message.into())
}

/// A preparation refuses by answering none; every other request reports the error.
fn refused_rename(query: &SourceQuery, refusal: NameQueryError) -> Result<SourceReply> {
    if matches!(query, SourceQuery::PrepareRename(_))
        && matches!(refusal, NameQueryError::EntityRename(_))
    {
        return Ok(SourceReply::PrepareRename(None));
    }
    Err(refusal.into())
}

/// The reply one entity argument gets, by the question it asks.
fn entity_answer(
    request: &mut EntityQuery<'_>,
    modules: &BTreeMap<String, LoadedModule>,
    scope: &EntityScope<'_>,
    entity: &Entity,
    selected: &entities::EntityArgument,
    semantics: &mut Semantic<'_>,
) -> Result<SourceReply> {
    let query = request.query;
    let range = utf16_range(request.source.lines(), selected.content_range.clone());
    if matches!(query, SourceQuery::Hover(_)) {
        return Ok(entity_hover(entity, scope, range));
    }
    let found = collect_occurrences(request, scope, entity, semantics)?;
    match query {
        SourceQuery::References(parameters) => Ok(entity_references(
            &found.occurrences,
            scope,
            parameters.context.include_declaration,
        )),
        SourceQuery::PrepareRename(_) | SourceQuery::Rename(_) => {
            rename_answer(request, modules, scope, entity, range, &found)
        }
        _ => unreachable!("completion and definition answer in their own arms"),
    }
}

/// The reply a rename or rename preparation answers.
fn rename_answer(
    request: &mut EntityQuery<'_>,
    modules: &BTreeMap<String, LoadedModule>,
    scope: &EntityScope<'_>,
    entity: &Entity,
    range: Option<lsp_types::Range>,
    found: &EntityOccurrences,
) -> Result<SourceReply> {
    let query = request.query;
    let definition = match rename_definition(request.site, scope, entity, found) {
        Ok(definition) => definition,
        Err(refusal) => return refused_rename(query, refusal),
    };
    if let Some(refusal) = refuse_shared_definition(request, modules, scope, entity, definition)? {
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
    let SourceQuery::Rename(parameters) = query else {
        unreachable!("hover, references, and prepare-rename answered the other queries")
    };
    rename_workspace_edit(
        request,
        modules,
        scope,
        entity,
        &parameters.new_name,
        &found.occurrences,
    )
}

/// The literal definition a rename rewrites, once the occurrences are a complete writable set.
fn rename_definition<'a>(
    site: &QuerySite<'_>,
    scope: &EntityScope<'_>,
    entity: &Entity,
    found: &'a EntityOccurrences,
) -> std::result::Result<&'a Occurrence, NameQueryError> {
    let definitions = entity_definitions(&found.occurrences, scope);
    if found.unresolved {
        return Err(rename_refusal(
            "entity references include unresolved arguments; rename requires a complete set of writable literals",
        ));
    }
    let Some(definition) = writable_definition(entity, &definitions, site) else {
        return Err(rename_refusal(
            "entity rename requires one writable literal definition in its declared source; derived and external identities cannot be renamed",
        ));
    };
    if found
        .occurrences
        .iter()
        .any(|occurrence| !writable(&occurrence.source, site.configuration, site.packages))
    {
        return Err(rename_refusal(
            "an entity occurrence is outside writable project sources",
        ));
    }
    Ok(definition)
}

/// The one literal definition of `entity` in its own declared source, when that source is writable.
fn writable_definition<'a>(
    entity: &Entity,
    definitions: &[&'a Occurrence],
    site: &QuerySite<'_>,
) -> Option<&'a Occurrence> {
    let [definition] = definitions else {
        return None;
    };
    (entity.source == Some(definition.source.id())
        && writable(&definition.source, site.configuration, site.packages))
    .then_some(*definition)
}

/// The rename reply, once the proposed text keeps every query's entity identities.
fn rename_workspace_edit(
    request: &mut EntityQuery<'_>,
    modules: &BTreeMap<String, LoadedModule>,
    scope: &EntityScope<'_>,
    entity: &Entity,
    new_name: &str,
    occurrences: &[Occurrence],
) -> Result<SourceReply> {
    let EntityQuery { resources, .. } = *request;
    let cancellation = request.site.cancellation;
    let EntityScope {
        module,
        query_name,
        model,
        ..
    } = *scope;
    let RenameProposal { changes, proposed } = rename_edits(request, scope, occurrences, new_name)?;
    // The declared definition must drive the queried identity, not merely spell it.
    let checked = proposed_model(request, module, query_name, &proposed)?;
    let mut expected = entity_sources(&model.entities);
    expected.remove(entity.id.as_str());
    expected.insert(new_name, entity.source);
    if !checked.result.success || entity_sources(&checked.entities) != expected {
        return Err(rename_refusal(
            "the proposed edits must rename only the selected entity and preserve the other entity IDs and sources",
        )
        .into());
    }
    for (other_module, other_query) in other_queries(modules, module.name.as_str(), query_name) {
        let original =
            request
                .cache
                .get(other_module, other_query, resources.clone(), cancellation)?;
        let checked = proposed_model(request, other_module, other_query, &proposed)?;
        if !original.result.success
            || !checked.result.success
            || entity_sources(&checked.entities) != entity_sources(&original.entities)
        {
            return Err(rename_refusal(format!(
                "the proposed edits change entity IDs or sources in query `{}.{other_query}`; rename would leave its references unchanged",
                other_module.name
            ))
            .into());
        }
    }
    cancellation.ensure_active()?;
    Ok(SourceReply::Rename(Some(WorkspaceEdit {
        changes: Some(changes),
        ..WorkspaceEdit::default()
    })))
}

/// The hover one entity occurrence answers.
fn entity_hover(
    entity: &Entity,
    scope: &EntityScope<'_>,
    range: Option<lsp_types::Range>,
) -> SourceReply {
    let EntityScope {
        module, query_name, ..
    } = *scope;
    let mut text = format!(
        "{}\n\nID: {}\nEntity query: {}.{}",
        entity.name, entity.id, module.name, query_name
    );
    if let Some(description) = &entity.description {
        text.push_str(&format!("\n\n{description}"));
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

/// One entity's literal occurrences in every readable source, and whether the scan stayed unread.
struct EntityOccurrences {
    occurrences: Vec<Occurrence>,
    unresolved: bool,
}

/// Every literal occurrence of one entity in the sources this answer may read.
fn collect_occurrences(
    request: &mut EntityQuery<'_>,
    scope: &EntityScope<'_>,
    entity: &Entity,
    semantics: &mut Semantic<'_>,
) -> Result<EntityOccurrences> {
    let EntityQuery {
        source,
        view,
        overlays,
        ..
    } = *request;
    let client_root = request.site.client_root;
    let configuration = request.site.configuration;
    let packages = request.site.packages;
    let cancellation = request.site.cancellation;
    let compilation = scope.compilation;
    let mut files = crate::analysis::site_sources(
        configuration.get_root(),
        view,
        &packages.boundary,
        request.disk,
        cancellation,
    )?
    .into_iter()
    .map(|names| names.source().id())
    .collect::<Vec<_>>();
    files.push(compilation.world().main());
    files.push(source.id());
    files.extend(
        overlays
            .iter()
            .filter_map(|(path, _)| crate::identity::path_id(path, configuration.get_root())),
    );
    files.extend(entity.source);
    let mut scanned = HashSet::new();
    let mut occurrences = Vec::new();
    let mut unresolved = false;
    while let Some(file) = files.pop() {
        cancellation.ensure_active()?;
        if !scanned.insert(file) {
            continue;
        }
        let (_, accessed) = compilation.world().read_file_with_evidence(file);
        request.cache.observe_accessed(&accessed);
        let Ok(text) = compilation.world().source(file) else {
            unresolved = true;
            continue;
        };
        files.extend(imported_sources(&text, semantics)?);
        let scan = entities::scan(&text, semantics, scope.bindings)?;
        unresolved |= scan.unresolved;
        let Some(uri) = semantics.source_uri(file, client_root) else {
            unresolved |= scan
                .literals
                .iter()
                .any(|argument| argument.value == entity.id);
            continue;
        };
        if let Ok(path) = crate::uri::to_site_path(uri.as_str()) {
            crate::analysis::require_source_text(view, &path, text.text())?;
        }
        for argument in scan
            .literals
            .into_iter()
            .filter(|argument| argument.value == entity.id)
        {
            occurrences.push(Occurrence {
                source: text.clone(),
                argument,
                uri: uri.clone(),
            });
        }
    }
    request.cache.observe_accessed(semantics.accessed());
    Ok(EntityOccurrences {
        occurrences,
        unresolved,
    })
}

/// The references one entity's occurrences answer, in one order.
fn entity_references(
    occurrences: &[Occurrence],
    scope: &EntityScope<'_>,
    include_declaration: bool,
) -> SourceReply {
    let mut locations = occurrences
        .iter()
        .filter(|occurrence| {
            include_declaration || !scope.bindings[occurrence.argument.binding].definition
        })
        .filter_map(|occurrence| {
            Some(Location::new(
                occurrence.uri.clone(),
                utf16_range(occurrence.source.lines(), occurrence.argument.range.clone())?,
            ))
        })
        .collect::<Vec<_>>();
    locations.sort_by(|left, right| {
        (left.uri.as_str(), left.range.start, left.range.end).cmp(&(
            right.uri.as_str(),
            right.range.start,
            right.range.end,
        ))
    });
    locations.dedup();
    SourceReply::References(Some(locations))
}

/// The definitions one entity's occurrences hold, in one order.
fn entity_definitions<'a>(
    occurrences: &'a [Occurrence],
    scope: &EntityScope<'_>,
) -> Vec<&'a Occurrence> {
    let mut definitions = occurrences
        .iter()
        .filter(|occurrence| scope.bindings[occurrence.argument.binding].definition)
        .collect::<Vec<_>>();
    definitions.sort_by(|left, right| {
        (
            left.uri.as_str(),
            left.argument.range.start,
            left.argument.range.end,
        )
            .cmp(&(
                right.uri.as_str(),
                right.argument.range.start,
                right.argument.range.end,
            ))
    });
    definitions.dedup_by(|left, right| {
        left.uri == right.uri && left.argument.range == right.argument.range
    });
    definitions
}

/// The workspace edits one rename proposes, and the unsaved text those edits produce.
struct RenameProposal {
    changes: HashMap<Uri, Vec<TextEdit>>,
    proposed: Vec<(PathBuf, Arc<str>)>,
}

/// The protocol edits one rename proposes, with the overlays they would produce.
fn rename_edits(
    request: &EntityQuery<'_>,
    scope: &EntityScope<'_>,
    occurrences: &[Occurrence],
    new_name: &str,
) -> Result<RenameProposal> {
    let EntityQuery {
        source,
        query,
        overlays,
        view,
        ..
    } = *request;
    let client_root = request.site.client_root;
    let configuration = request.site.configuration;
    if new_name.is_empty()
        || scope
            .model
            .entities
            .iter()
            .any(|entity| entity.id == new_name)
    {
        return Err(rename_refusal(
            "the new entity ID must be nonempty and must not already exist in this entity query",
        )
        .into());
    }
    let replacement = Str::from(new_name).repr().to_string();
    let mut by_source = BTreeMap::<PathBuf, (Source, Vec<std::ops::Range<usize>>)>::new();
    for occurrence in occurrences {
        let path = configuration
            .get_root()
            .join(occurrence.source.id().vpath().get_without_slash());
        let (_, ranges) = by_source
            .entry(path)
            .or_insert_with(|| (occurrence.source.clone(), Vec::new()));
        ranges.push(occurrence.argument.range.clone());
    }
    let mut changes = HashMap::new();
    let mut proposed = overlays.to_vec();
    for (path, (text, mut ranges)) in by_source {
        ranges.sort_by_key(|range| (range.start, range.end));
        ranges.dedup();
        if ranges.windows(2).any(|pair| pair[0].end > pair[1].start) {
            return Err(rename_refusal("entity edit ranges overlap").into());
        }
        let edits = ranges
            .iter()
            .map(|range| {
                Ok(TextEdit {
                    range: utf16_range(text.lines(), range.clone()).ok_or_else(|| {
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
            crate::identity::path_id(existing, configuration.get_root()) != Some(text.id())
        });
        proposed.push((path, Arc::<str>::from(updated)));
    }
    Ok(RenameProposal { changes, proposed })
}

fn imported_sources(source: &Source, semantics: &mut Semantic<'_>) -> Result<Vec<FileId>> {
    let mut files = Vec::new();
    let mut pending = vec![LinkedNode::new(source.root())];
    while let Some(node) = pending.pop() {
        semantics.cancellation().ensure_active()?;
        let imported = node
            .cast::<ast::ModuleImport>()
            .map(|import| import.source())
            .or_else(|| {
                node.cast::<ast::ModuleInclude>()
                    .map(|include| include.source())
            });
        if let Some(ast::Expr::Str(literal)) = imported
            && let Some(expression) = node.find(literal.span())
            && let Some(Value::Module(module)) = semantics.import(&expression)?
            && let Some(file) = module.file_id()
        {
            files.push(file);
        }
        pending.extend(node.children());
    }
    Ok(files)
}

fn entity_sources(
    entities: &[Entity],
) -> BTreeMap<&str, Option<tola_typst::typst::syntax::FileId>> {
    entities
        .iter()
        .map(|entity| (entity.id.as_str(), entity.source))
        .collect()
}

/// Every configured query except the one the editor selected.
fn other_queries<'a>(
    modules: &'a BTreeMap<String, LoadedModule>,
    selected_module: &'a str,
    selected_query: &'a str,
) -> impl Iterator<Item = (&'a LoadedModule, &'a str)> + 'a {
    modules.values().flat_map(move |module| {
        module
            .queries
            .iter()
            .filter(|(_, query)| query.has_entities())
            .map(|(name, _)| name.as_str())
            .filter(move |query| module.name != selected_module || *query != selected_query)
            .map(move |query| (module, query))
    })
}

fn proposed_model(
    request: &EntityQuery<'_>,
    module: &LoadedModule,
    query: &str,
    proposed: &[(PathBuf, Arc<str>)],
) -> Result<QueryEvaluation> {
    tola_build::modules::query::run_entities(
        Arc::clone(request.site.configuration),
        module,
        &module.queries[query],
        &format!("{}.{query}", module.name),
        request.resources.clone(),
        proposed.to_vec(),
        request.site.cancellation,
    )
}

/// A shared source does not make configured query namespaces interchangeable.
fn refuse_shared_definition(
    request: &mut EntityQuery<'_>,
    modules: &BTreeMap<String, LoadedModule>,
    scope: &EntityScope<'_>,
    entity: &Entity,
    definition: &Occurrence,
) -> Result<Option<NameQueryError>> {
    let EntityQuery {
        site, resources, ..
    } = *request;
    let QuerySite {
        packages,
        cancellation,
        client_root,
        ..
    } = *site;
    let EntityScope {
        module: selected_module,
        query_name: selected_query,
        ..
    } = *scope;
    let target = crate::uri::to_site_path(definition.uri.as_str())?;
    let target = tola_input::normalize_existing_prefix(&target);
    for (module, query) in other_queries(modules, selected_module.name.as_str(), selected_query) {
        let model = request
            .cache
            .get(module, query, resources.clone(), cancellation)?;
        if !model.result.success {
            return Ok(Some(rename_refusal(format!(
                "cannot establish entity rename ownership while query {}.{query} fails",
                module.name
            ))));
        }
        let Some(candidate) = model.entities.iter().find(|other| other.id == entity.id) else {
            continue;
        };
        let Some(source_id) = candidate.source else {
            continue;
        };
        let Some(compilation) = checked_compilation(&model) else {
            return Ok(Some(rename_refusal(
                "another entity query has no checked source world",
            )));
        };
        let mut semantics = Semantic::new(compilation, cancellation);
        let Some(uri) = semantics.source_uri(source_id, client_root) else {
            continue;
        };
        let Ok(path) = crate::uri::to_site_path(uri.as_str()) else {
            continue;
        };
        if tola_input::normalize_existing_prefix(&path) != target {
            continue;
        }
        let Ok(source) = compilation.world().source(source_id) else {
            return Ok(Some(rename_refusal(
                "a shared entity definition cannot be read",
            )));
        };
        if source.text() != definition.source.text() {
            return Ok(Some(NameQueryError::ConflictingSourceText));
        }
        let literals = match module_package(packages, &module.name)? {
            Some(package) => {
                let bindings = entity_bindings(&mut semantics, package, module, query)?;
                definition_ranges(&source, &mut semantics, &bindings, &entity.id)?
            }
            None => Vec::new(),
        };
        request.cache.observe_accessed(semantics.accessed());
        if literals.len() != 1 || literals[0] == definition.argument.range {
            return Ok(Some(rename_refusal(format!(
                "entity definition is shared with query {}.{query}; renaming one namespace would leave the other namespace's references unchanged",
                module.name
            ))));
        }
    }
    Ok(None)
}

fn writable(source: &Source, config: &ResolvedSiteConfig, packages: &PackageAccess<'_>) -> bool {
    if !matches!(source.id().root(), VirtualRoot::Project) {
        return false;
    }
    let path = config
        .get_root()
        .join(source.id().vpath().get_without_slash());
    let physical = tola_input::normalize_existing_prefix(&path);
    physical.starts_with(config.get_root()) && packages.boundary.check(&physical).is_ok()
}

fn string_argument(source: &Source, cursor: usize) -> bool {
    let root = LinkedNode::new(source.root());
    [Side::Before, Side::After].into_iter().any(|side| {
        let Some(node) = root.leaf_at(cursor, side) else {
            return false;
        };
        node.cast::<ast::Str>().is_some()
            && node.range().start < cursor
            && cursor < node.range().end
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::tests::{QuerySession, marked_cursor};

    const ENTITY_QUERY: &str = "entry=\"model.typ\"\nlabel=\"entities\"\n";

    fn configure_modules(site: &mut QuerySession, modules: &str) {
        let config = std::fs::read_to_string(site.site.root().join("tola.toml")).unwrap();
        site.site
            .write("tola.toml", &format!("{config}\n{modules}"));
        site.reload();
    }

    fn scenario() -> QuerySession {
        let mut site = QuerySession::with_program("#invalid_page_template()");
        site.site.write(
            "theme/lib.typ",
            "#let local(address) = []\n#let subtree(body, address: none) = []\n",
        );
        site.site.write("theme/model.typ", r#"
#import "@tola/site:0.0.0": project-path
#metadata((
  (id: "ROOT", name: "Root page", source: project-path("content/target.typ")),
  (id: "INLINE", name: "Inline entity", source: project-path("content/target.typ")),
  (id: "https://foreign.example/ROOT/", name: "Foreign entity", url: "https://foreign.example/ROOT/"),
))<entities>
"#);
        site.site.write(
            "theme/tola-module.toml",
            &format!(
                r#"

[typst]
entry="lib.typ"
[queries.entities]
{ENTITY_QUERY}[[queries.entities.references]]
export="local"
parameter={{positional=0}}
[[queries.entities.definitions]]
export="subtree"
parameter={{named="address"}}
"#
            ),
        );
        site.site.write("content/target.typ", "#import \"@tola-modules/notes:0.0.0\": subtree\n😀 #subtree(address: \"INLINE\")[Body]\n");
        configure_modules(&mut site, "[modules.notes]\npath=\"theme\"\n");
        site
    }

    fn module_scenario() -> QuerySession {
        let mut site = QuerySession::with_program("#unrelated_broken_page_template()");
        for (alias, directory, source, name) in [
            ("shop", "products", "catalog", "Torque wrench"),
            ("terms", "glossary", "glossary", "Glossary entry"),
        ] {
            site.site.write(&format!("{directory}/lib.typ"),
                "#let link(code) = []\n#let alias-link = link\n#let define(code, name: \"item\") = (id: code, name: name)\n");
            site.site.write(&format!("{directory}/model.typ"), &format!(r#"
#import "@tola/site:0.0.0": project-path
#import project-path("content/{source}.typ"): product
#metadata((product + (description: "A model-owned description", source: project-path("content/{source}.typ")),))<entities>
"#));
            site.site.write(
                &format!("{directory}/tola-module.toml"),
                &format!(
                    r#"

[typst]
entry="lib.typ"
[queries.catalog]
{ENTITY_QUERY}[[queries.catalog.references]]
export="link"
parameter={{positional=0}}
[[queries.catalog.references]]
export="alias-link"
parameter={{positional=0}}
[[queries.catalog.definitions]]
export="define"
parameter={{positional=0}}
"#
                ),
            );
            site.site.write(&format!("content/{source}.typ"), &format!(
                "#import \"@tola-modules/{alias}:0.0.0\": define\n#let product = define(\"sku:α-42\", name: \"{name}\")\n"
            ));
        }
        site.site.write("content/uses.typ", "#import \"@tola-modules/shop:0.0.0\": link as cite\n#cite(\"sku:α-42\")\n#let partial = cite.with(\"sku:α-42\")\n#partial()\n#{ let cite(code) = []; cite(\"sku:α-42\") }\n#import \"@tola-modules/terms:0.0.0\" as glossary\n#glossary.link(\"sku:α-42\")\n");
        configure_modules(
            &mut site,
            "[modules.shop]\npath='products'\n[modules.terms]\npath='glossary'\n",
        );
        site
    }

    const PRODUCT_REFERENCE: &str =
        "#import \"@tola-modules/shop:0.0.0\": link\n😀 #link(\"sku:α|-42\")";

    #[test]
    fn hover_and_references_use_declared_identity() {
        let mut site = module_scenario();
        let hover = site.hover_text(PRODUCT_REFERENCE).expect("entity hover");
        assert!(
            hover.contains("Torque wrench") && hover.contains("sku:α-42"),
            "{hover}"
        );
        assert!(
            hover.contains("shop.catalog") && hover.contains("model-owned"),
            "{hover}"
        );
        let references = site
            .references(PRODUCT_REFERENCE, false)
            .expect("entity references");
        assert_eq!(references.len(), 3, "{references:?}");
        assert!(
            references
                .iter()
                .all(|location| !location.uri.as_str().contains("glossary"))
        );
        let declarations = site.references(PRODUCT_REFERENCE, true).unwrap();
        assert_eq!(declarations.len(), references.len() + 1, "{declarations:?}");
        let definition = declarations
            .iter()
            .find(|location| location.uri.as_str().ends_with("content/catalog.typ"))
            .unwrap();
        assert_eq!(definition.range.start.line, 1);
        assert_eq!(definition.range.start.character, 22);
    }

    #[test]
    fn one_function_parameter_cannot_reference_two_queries() {
        let mut site = module_scenario();
        let library = std::fs::read_to_string(site.site.root().join("products/lib.typ")).unwrap();
        site.site.write(
            "products/lib.typ",
            &format!("{library}\n#let other-link = link\n"),
        );
        let manifest =
            std::fs::read_to_string(site.site.root().join("products/tola-module.toml")).unwrap();
        site.site.write(
            "products/tola-module.toml",
            &format!(
                r#"{manifest}
[queries.other]
entry = "model.typ"
label = "entities"
[[queries.other.references]]
export = "other-link"
parameter = {{ positional = 0 }}
"#
            ),
        );
        site.reload();
        let error = site
            .try_reply_with(
                "textDocument/hover",
                PRODUCT_REFERENCE,
                serde_json::json!({}),
            )
            .expect_err("one argument cannot select two query namespaces");
        let message = error.to_string();
        assert!(message.contains("queries.other.references[0]"));
        assert!(message.contains("queries.catalog.references[0]"));
    }

    #[test]
    fn rename_checks_proposed_model() {
        let mut site = module_scenario();
        let prepared = site
            .prepare_rename(PRODUCT_REFERENCE)
            .expect("literal definition supports rename");
        let PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. } = prepared else {
            panic!("placeholder")
        };
        assert_eq!(placeholder, "sku:α-42");
        let edit = site
            .rename(PRODUCT_REFERENCE, "part:\"β\\雪")
            .expect("checked entity edits");
        let changes = edit
            .changes
            .expect("connection versions the edits before delivery");
        assert_eq!(changes.values().map(Vec::len).sum::<usize>(), 4);
        let quoted = Str::from("part:\"β\\雪").repr();
        assert!(
            changes
                .values()
                .flatten()
                .all(|edit| edit.new_text == quoted.as_str())
        );
        assert!(
            changes
                .keys()
                .all(|uri| !uri.as_str().ends_with("glossary.typ"))
        );
        assert!(
            std::fs::read_to_string(site.site.root().join("content/catalog.typ"))
                .unwrap()
                .contains("sku:α-42"),
            "rename only proposes edits"
        );
    }

    #[test]
    fn rename_preserves_unselected_entity_identities() {
        for (path, records) in [
            (
                "products/model.typ",
                "(product + (source: source), product + (id: \"related:\" + product.id, source: source))",
            ),
            (
                "glossary/model.typ",
                "(product + (id: \"term:\" + product.id, source: source),)",
            ),
        ] {
            let mut site = module_scenario();
            site.site.write(
                path,
                &format!(
                    "#import \"@tola/site:0.0.0\": project-path\n\
                     #import project-path(\"content/catalog.typ\"): product\n\
                     #let source = project-path(\"content/catalog.typ\")\n\
                     #metadata({records})<entities>"
                ),
            );
            assert!(site.hover_text(PRODUCT_REFERENCE).is_some(), "{path}");
            assert_eq!(
                site.rename_refusal(PRODUCT_REFERENCE, "new:id"),
                "entityRenameRefused",
                "{path}"
            );
        }
    }

    #[test]
    fn derived_computed_entity_edits_refused() {
        let mut notes = scenario();
        assert!(
            notes
                .prepare_rename("#import \"@tola-modules/notes:0.0.0\": local\n#local(\"RO|OT\")")
                .is_none()
        );
        assert!(notes.prepare_rename("#import \"@tola-modules/notes:0.0.0\": local\n#local(\"https://foreign.example/RO|OT/\")").is_none());
        let refusal = notes
            .rename_error(
                "#import \"@tola-modules/notes:0.0.0\": local\n#local(\"IN|LINE\")",
                "changed",
            )
            .expect("hard-coded model cannot be renamed");
        assert!(
            matches!(
                refusal.downcast_ref::<NameQueryError>(),
                Some(NameQueryError::EntityRename(_))
            ),
            "{refusal:?}"
        );
        let mut site = module_scenario();
        site.site.write(
            "content/dynamic.typ",
            "#import \"@tola-modules/shop:0.0.0\": link\n#let code = \"sku:α-42\"\n#link(code)\n",
        );
        assert!(site.prepare_rename(PRODUCT_REFERENCE).is_none());
        assert_eq!(
            site.rename_refusal(PRODUCT_REFERENCE, "new"),
            "entityRenameRefused"
        );
    }

    #[test]
    fn shared_definition_refuses_partial_rename() {
        let mut site = module_scenario();
        configure_modules(&mut site, "[modules.shop-again]\npath='products'\n");
        site.site.write(
            "content/second-alias.typ",
            "#import \"@tola-modules/shop-again:0.0.0\": link\n#link(\"sku:α-42\")\n",
        );
        assert!(
            site.hover_text(PRODUCT_REFERENCE)
                .unwrap()
                .contains("Torque wrench")
        );
        assert_eq!(site.references(PRODUCT_REFERENCE, false).unwrap().len(), 3);
        let unchanged = site.rename(PRODUCT_REFERENCE, "sku:α-42").unwrap();
        assert!(unchanged.changes.is_none() && unchanged.document_changes.is_none());
        assert!(site.prepare_rename(PRODUCT_REFERENCE).is_none());
        let error = site
            .rename_error(PRODUCT_REFERENCE, "new:id")
            .expect("shared definition must not produce partial edits");
        assert!(
            matches!(
                error.downcast_ref::<NameQueryError>(),
                Some(NameQueryError::EntityRename(_))
            ),
            "{error:?}"
        );
        assert!(error.to_string().contains("shop-again.catalog"), "{error}");
        assert!(
            std::fs::read_to_string(site.site.root().join("content/catalog.typ"))
                .unwrap()
                .contains("sku:α-42")
        );
    }

    /// Two modules declare one entity in one shared source; a variadic `nodes` names no slot.
    fn shared_definition_scenario(nodes: &str) -> QuerySession {
        let mut site = QuerySession::with_program("#unrelated_broken_page_template()");
        site.site.write(
            "alpha/lib.typ",
            "#let define(code, name: \"item\") = (id: code, name: name)\n#let cite(code) = []\n",
        );
        site.site.write(
            "alpha/model.typ",
            r#"
#import "@tola/site:0.0.0": project-path
#import project-path("content/shared.typ"): entry
#metadata((entry + (source: project-path("content/shared.typ")),))<entities>
"#,
        );
        site.site.write(
            "alpha/tola-module.toml",
            &format!(
                r#"

[typst]
entry="lib.typ"
[queries.nodes]
{ENTITY_QUERY}[[queries.nodes.references]]
export="cite"
parameter={{positional=0}}
[[queries.nodes.definitions]]
export="define"
parameter={{positional=0}}
"#
            ),
        );
        site.site.write(
            "beta/lib.typ",
            &format!("#let nodes{nodes} = items\n#let mark(code) = (id: code, name: \"marked\")\n"),
        );
        site.site.write(
            "beta/model.typ",
            r#"
#import "@tola/site:0.0.0": project-path
#metadata(((id: "SHARED", name: "Beta", source: project-path("content/shared.typ")),))<entities>
"#,
        );
        site.site.write(
            "beta/tola-module.toml",
            &format!(
                r#"

[typst]
entry="lib.typ"
[queries.nodes]
{ENTITY_QUERY}[[queries.nodes.definitions]]
export="mark"
parameter={{positional=0}}
[[queries.nodes.definitions]]
export="nodes"
parameter={{positional=0}}
"#
            ),
        );
        site.site.write(
            "content/shared.typ",
            r#"#import "@tola-modules/alpha:0.0.0": define
#import "@tola-modules/beta:0.0.0": mark, nodes
#let entry = define("SHARED", name: "Shared")
#{ let _ = mark("SHARED"); let _ = nodes("SHARED") }
"#,
        );
        configure_modules(
            &mut site,
            "[modules.alpha]\npath='alpha'\n[modules.beta]\npath='beta'\n",
        );
        site
    }

    const SHARED_REFERENCE: &str =
        "#import \"@tola-modules/alpha:0.0.0\": cite\n😀 #cite(\"SHAR|ED\")";

    #[test]
    fn shared_definitions_require_declared_parameters() {
        let mut site = shared_definition_scenario("(..items)");
        let error = site
            .rename_error(SHARED_REFERENCE, "public:β")
            .expect("a nonexistent definition parameter must be diagnosed");
        assert!(
            error
                .to_string()
                .contains("queries.nodes.definitions[1].parameter")
        );

        let mut site = shared_definition_scenario("(first, ..items)");
        assert_eq!(
            site.rename_refusal(SHARED_REFERENCE, "public:β"),
            "entityRenameRefused"
        );
    }

    #[test]
    fn unsaved_buffers_share_rename_world() {
        use crate::sources::OpenSources;
        use serde_json::json;
        let mut site = module_scenario();
        let root = site.config.get_root();
        let target_path = root.join("content/catalog.typ");
        let caller_path = root.join("content/new-buffer.typ");
        let target_uri = crate::uri::from_file_path(&target_path).unwrap();
        let caller_uri = crate::uri::from_file_path(&caller_path).unwrap();
        let target = "// Unsaved 😀 heading\n#import \"@tola-modules/shop:0.0.0\": define\n#let note = \"🦊\"; #let product = define(\"draft:雪\", name: \"Unsaved product\")";
        let (caller, position) =
            marked_cursor("#import \"@tola-modules/shop:0.0.0\": link\n😀 #link(\"draft:|雪\")");
        let mut open = OpenSources::new(root);
        for (uri, text, version) in [(&target_uri, target, 7), (&caller_uri, caller.as_str(), 3)] {
            open.apply(lsp_server::Notification::new("textDocument/didOpen".into(), json!({
                "textDocument": {"uri": uri, "languageId": "typst", "version": version, "text": text}
            }))).unwrap();
        }
        let query = crate::query::tests::decode(
            "textDocument/rename",
            json!({
                "textDocument": {"uri": caller_uri}, "position": position, "newName": "public:β"
            }),
        );
        let reply = crate::query::respond(crate::query::QueryRequest {
            session: &mut site.session,
            compilations: &mut site.compilations,
            disk: &mut crate::analysis::DiskSources::default(),
            graphs: &mut crate::analysis::GraphCache::default(),
            bibliographies: &mut site.bibliographies,
            entity_queries: &mut site.entity_queries,
            config: &site.config,
            source_revision: open.revision(),
            overrides: open.overrides(),
            view: &open.view(),
            published: &[],
            query: &query,
            routes_as_hints: false,
            check_progress: crate::protocol::CheckProgress::Failed,
            cancellation: &tola_build::cancellation::BuildCancellation::default(),
            client_root: &crate::uri::ClientRoot::new(root),
        })
        .unwrap();
        let SourceReply::Rename(Some(edit)) = reply else {
            panic!("rename reply")
        };
        let changes = edit.changes.unwrap();
        assert_eq!(changes.len(), 2, "{changes:?}");
        assert_eq!(changes[&target_uri][0].range.start.line, 2);
        assert_eq!(changes[&target_uri][0].range.start.character, 40);
        assert_eq!(changes[&caller_uri][0].range.start.character, 9);
        assert!(
            changes
                .values()
                .flatten()
                .all(|edit| edit.new_text == "\"public:β\"")
        );
        assert!(!caller_path.exists());
        assert!(
            std::fs::read_to_string(target_path)
                .unwrap()
                .contains("sku:α-42")
        );
    }
}
