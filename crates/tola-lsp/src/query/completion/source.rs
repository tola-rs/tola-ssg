//! Names and members established by source and static library metadata, without a checked world.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use tola_build::cancellation::BuildCancellation;

use lsp_types::{CompletionItem, CompletionItemKind, CompletionResponse};
use tola_packages::ExportDocumentation;
use tola_typst::GLOBAL_LIBRARY;
use tola_typst::typst::foundations::Value;
use tola_typst::typst::syntax::ast;
use tola_typst::typst::syntax::{FileId, LinkedNode, Side, Source, SyntaxKind};
use tola_typst_syntax::imports::{ModuleExport, NameClass, NameDefinition, NameGraph};
use tola_typst_syntax::names::{Declaration, Initializer, NamePath, OccurrenceKind, SourceNames};

use super::super::expression::{ExpressionType, MemberCall, members, single_value};
use super::super::inference::{ExpressionResolver, SourceResolver};
use super::super::semantic;
use super::callable::{self, Callable};
use super::definition;
use super::{
    comment_at, completion_edit, documentation_template, is_path_position, opaque_at, raw_at,
};
use crate::protocol::SourceReply;

/// Source replies whose answer cannot depend on a checked world. Strings inside calls retain
/// their semantic lane because a callee can declare an asset URL parameter.
pub(crate) fn source_completion_reply(source: &Source, cursor: usize) -> Option<SourceReply> {
    if let Some(reply) = documentation_template(source, cursor) {
        return Some(reply);
    }
    let mut node = LinkedNode::new(source.root()).leaf_at(cursor, Side::Before);
    let mut in_call = false;
    while let Some(current) = node {
        if current.kind() == SyntaxKind::FuncCall {
            in_call = true;
            break;
        }
        node = current.parent().cloned();
    }
    (comment_at(source, cursor)
        || raw_at(source, cursor)
        || (opaque_at(source, cursor) && !in_call && !is_path_position(source, cursor)))
    .then(|| SourceReply::Completion(CompletionResponse::Array(Vec::new())))
}

/// Whether the cursor reads a name the source-only scope can answer.
pub(crate) fn source_name_position(names: &SourceNames, source: &Source, cursor: usize) -> bool {
    if opaque_at(source, cursor) || comment_at(source, cursor) || raw_at(source, cursor) {
        return false;
    }
    if name_range(source, cursor).is_none() {
        return false;
    }
    match names.occurrence(cursor) {
        Some(occurrence) => matches!(occurrence.kind, OccurrenceKind::Name),
        None => LinkedNode::new(source.root())
            .leaf_at(cursor, Side::Before)
            .is_some_and(|leaf| leaf.kind() == SyntaxKind::Hash),
    }
}

/// The names one source's own scopes and the standard library offer without a compiled world.
pub(crate) fn source_name_completion(
    graph: Option<&NameGraph>,
    names: &SourceNames,
    source: &Source,
    cursor: usize,
    cancellation: &BuildCancellation,
) -> Result<Option<Vec<CompletionItem>>> {
    cancellation.ensure_active()?;
    if !source_name_position(names, source, cursor) {
        return Ok(None);
    }
    let Some(range) = name_range(source, cursor) else {
        return Ok(None);
    };
    let replacement = if &source.text()[range.clone()] == "#" {
        cursor..cursor
    } else {
        range
    };
    let prefix = &source.text()[replacement.start..cursor];
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    let mut packages = BTreeMap::new();
    let mut push = |name: String,
                    kind: Option<CompletionItemKind>,
                    definition: Option<NameDefinition>,
                    declaration: Option<&Declaration>,
                    items: &mut Vec<CompletionItem>|
     -> Result<()> {
        cancellation.ensure_active()?;
        if !super::name_matches(&name, prefix) || !seen.insert(name.clone()) {
            return Ok(());
        }
        let Ok(edit) = completion_edit(source, cursor, replacement.clone(), name.clone()) else {
            return Ok(());
        };
        let mut item = CompletionItem::new_simple(name, String::new());
        item.kind = kind;
        item.text_edit = Some(edit);
        let mut followed = Vec::new();
        let callable = if let Some(definition) = definition {
            attach_definition(
                &mut item,
                definition,
                graph,
                &mut packages,
                &mut followed,
                cancellation,
            )?
        } else if let Some(declaration) = declaration {
            attach_source_declaration(
                &mut item,
                names,
                declaration,
                graph,
                &mut packages,
                &mut followed,
                cancellation,
            )?
        } else {
            None
        };
        match callable {
            Some(callable) => items.extend(callable::callable_items(
                item,
                source,
                cursor,
                &replacement,
                callable,
            )),
            None => items.push(item),
        }
        Ok(())
    };
    enum ScopeName<'a> {
        Declaration(&'a Declaration),
        Import(ModuleExport),
    }
    let mut scope = Some(names.scope_at(cursor));
    while let Some(id) = scope {
        let mut candidates = Vec::new();
        for &declaration in names.scopes()[id].declarations.iter().rev() {
            cancellation.ensure_active()?;
            let declaration = &names.declarations()[declaration];
            if super::name_matches(&declaration.name, prefix)
                && (declaration.visible_from <= cursor
                    || declaration
                        .recursive_body
                        .as_ref()
                        .is_some_and(|body| body.contains(&cursor)))
            {
                candidates.push((
                    declaration.visible_from,
                    ScopeName::Declaration(declaration),
                ));
            }
        }
        for (statement, import) in names.imports().iter().enumerate() {
            cancellation.ensure_active()?;
            if import.wildcard && import.scope == id && import.range.end <= cursor {
                let Some(graph) = graph else {
                    return Ok(None);
                };
                if !graph.interface_established(source.id(), statement) {
                    return Ok(None);
                }
                for export in
                    graph.module_exports_matching(source.id(), statement, &[], prefix, || {
                        cancellation.ensure_active()
                    })?
                {
                    candidates.push((import.range.end, ScopeName::Import(export)));
                }
            }
        }
        cancellation.ensure_active()?;
        candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
        for (_, candidate) in candidates {
            cancellation.ensure_active()?;
            match candidate {
                ScopeName::Declaration(declaration) => push(
                    declaration.name.clone(),
                    Some(definition::declaration_kind(&declaration.initializer)),
                    None,
                    Some(declaration),
                    &mut items,
                ),
                ScopeName::Import(export) => push(
                    export.name,
                    Some(class_kind(export.class)),
                    export.definition,
                    None,
                    &mut items,
                ),
            }?;
        }
        scope = names.scopes()[id].parent;
    }
    let math = names
        .occurrence(cursor)
        .is_some_and(|occurrence| occurrence.math);
    let scope = if math {
        GLOBAL_LIBRARY.math.scope()
    } else {
        GLOBAL_LIBRARY.global.scope()
    };
    for (name, binding) in scope
        .iter()
        .filter(|(name, _)| super::name_matches(name, prefix))
    {
        push(
            name.to_string(),
            None,
            Some(NameDefinition::Builtin(binding.read().clone())),
            None,
            &mut items,
        )?;
    }
    push(
        "std".to_owned(),
        None,
        Some(NameDefinition::Builtin(GLOBAL_LIBRARY.std.read().clone())),
        None,
        &mut items,
    )?;
    cancellation.ensure_active()?;
    Ok(Some(items))
}

fn name_range(source: &Source, cursor: usize) -> Option<std::ops::Range<usize>> {
    tola_typst_syntax::syntax::name(source, cursor).or_else(|| {
        let leaf = LinkedNode::new(source.root()).leaf_at(cursor, Side::Before)?;
        (leaf.kind() == SyntaxKind::MathIdent).then(|| leaf.range())
    })
}

/// Whether an import visible at the cursor can contribute completion names or documentation.
pub(crate) fn source_name_graph_needed(
    names: &SourceNames,
    cursor: usize,
    cancellation: &BuildCancellation,
) -> Result<bool> {
    cancellation.ensure_active()?;
    let source = names.source();
    let Some(range) = name_range(source, cursor) else {
        return Ok(false);
    };
    let prefix = if &source.text()[range.clone()] == "#" {
        ""
    } else {
        &source.text()[range.start..cursor]
    };
    let mut scope = Some(names.scope_at(cursor));
    while let Some(id) = scope {
        for import in names.imports() {
            cancellation.ensure_active()?;
            if import.wildcard && import.scope == id && import.range.end <= cursor {
                return Ok(true);
            }
        }
        for &declaration in &names.scopes()[id].declarations {
            cancellation.ensure_active()?;
            let declared = &names.declarations()[declaration];
            if super::name_matches(&declared.name, prefix)
                && declared.visible_from <= cursor
                && imported_declaration(names, declaration, &mut BTreeSet::new(), cancellation)?
            {
                return Ok(true);
            }
        }
        scope = names.scopes()[id].parent;
    }
    Ok(false)
}

fn imported_declaration(
    names: &SourceNames,
    declaration: usize,
    followed: &mut BTreeSet<usize>,
    cancellation: &BuildCancellation,
) -> Result<bool> {
    cancellation.ensure_active()?;
    if !followed.insert(declaration) {
        return Ok(false);
    }
    match &names.declarations()[declaration].initializer {
        Initializer::Import { .. } => Ok(true),
        Initializer::Alias(path) => {
            let declaration = path
                .segments
                .first()
                .and_then(|root| names.resolve(names.text(root), path.scope, path.at));
            match declaration {
                Some(declaration) => {
                    imported_declaration(names, declaration, followed, cancellation)
                }
                None => Ok(false),
            }
        }
        _ => Ok(false),
    }
}

/// A member expression may read imports through its receiver or a local function body.
pub(crate) fn source_member_graph_needed(
    names: &SourceNames,
    source: &Source,
    cursor: usize,
    cancellation: &BuildCancellation,
) -> Result<bool> {
    cancellation.ensure_active()?;
    let Some(member) = tola_typst_syntax::syntax::member(source, cursor) else {
        return Ok(false);
    };
    let Some(target) = tola_typst_syntax::syntax::node_at_range(source, &member.target) else {
        return Ok(false);
    };
    if matches!(
        target.cast::<ast::Expr>(),
        Some(
            ast::Expr::None(_)
                | ast::Expr::Auto(_)
                | ast::Expr::Bool(_)
                | ast::Expr::Int(_)
                | ast::Expr::Float(_)
                | ast::Expr::Numeric(_)
                | ast::Expr::Str(_)
        )
    ) {
        return Ok(false);
    }
    Ok(!names.imports().is_empty())
}

/// What a definition resolved through the site's source graph contributes: a library value, or the
/// declaration the graph reaches through imports and aliases.
fn attach_definition(
    item: &mut CompletionItem,
    definition: NameDefinition,
    graph: Option<&NameGraph>,
    packages: &mut BTreeMap<String, Vec<ExportDocumentation>>,
    followed: &mut Vec<(FileId, usize)>,
    cancellation: &BuildCancellation,
) -> Result<Option<Callable>> {
    cancellation.ensure_active()?;
    let shape = match definition {
        NameDefinition::Builtin(value) => definition::attach_value(item, value),
        NameDefinition::Source { file, declaration } => {
            let Some(names) = graph.and_then(|graph| graph.source(file)) else {
                return Ok(None);
            };
            attach_source_declaration(
                item,
                names,
                &names.declarations()[declaration],
                graph,
                packages,
                followed,
                cancellation,
            )?
        }
    };
    cancellation.ensure_active()?;
    Ok(shape)
}

/// What one declaration the graph reaches contributes: a package's own export documentation, the
/// definition an import or an alias reaches, or the declaration's own text.
fn attach_source_declaration(
    item: &mut CompletionItem,
    names: &SourceNames,
    declaration: &Declaration,
    graph: Option<&NameGraph>,
    packages: &mut BTreeMap<String, Vec<ExportDocumentation>>,
    followed: &mut Vec<(FileId, usize)>,
    cancellation: &BuildCancellation,
) -> Result<Option<Callable>> {
    let source = names.source();
    let identity = (source.id(), declaration.range.start);
    if followed.contains(&identity) {
        return Ok(None);
    }
    followed.push(identity);
    if definition::package_exports(names, &declaration.name) {
        return definition::attach_declaration(item, names, declaration, packages, cancellation);
    }
    let mut shape = None;
    if let Initializer::Import { import, path } = &declaration.initializer
        && let Some(graph) = graph
        && let Some(definition) = graph.import_definition(source.id(), *import, path)
    {
        shape = attach_definition(
            item,
            definition,
            Some(graph),
            packages,
            followed,
            cancellation,
        )?;
    }
    if let Initializer::Alias(path) = &declaration.initializer {
        shape = attach_alias(item, names, path, graph, packages, followed, cancellation)?.or(shape);
    }
    Ok(definition::attach_declaration(item, names, declaration, packages, cancellation)?.or(shape))
}

/// What the name an alias spells declares: the binding the graph reaches, a name this file itself
/// binds, or a library member the path names.
fn attach_alias(
    item: &mut CompletionItem,
    names: &SourceNames,
    path: &NamePath,
    graph: Option<&NameGraph>,
    packages: &mut BTreeMap<String, Vec<ExportDocumentation>>,
    followed: &mut Vec<(FileId, usize)>,
    cancellation: &BuildCancellation,
) -> Result<Option<Callable>> {
    if let Some(graph) = graph
        && let Some(range) = path.segments.last()
        && let Some(binding) = graph
            .selected(names.source().id(), range.start)
            .and_then(|occurrence| occurrence.binding)
        && let Some(definition) = graph.binding_definition(binding)
    {
        return attach_definition(
            item,
            definition,
            Some(graph),
            packages,
            followed,
            cancellation,
        );
    }
    let Some(root) = path.segments.first() else {
        return Ok(None);
    };
    if path.segments.len() == 1
        && let Some(binding) = names.resolve(names.text(root), path.scope, path.at)
    {
        return attach_source_declaration(
            item,
            names,
            &names.declarations()[binding],
            graph,
            packages,
            followed,
            cancellation,
        );
    }
    let Some(value) = library_value(names, path, graph, cancellation)? else {
        return Ok(None);
    };
    Ok(definition::attach_value(item, value))
}

/// The value a library path names, when the alias's file binds none of its segments.
fn library_value(
    names: &SourceNames,
    path: &NamePath,
    graph: Option<&NameGraph>,
    cancellation: &BuildCancellation,
) -> Result<Option<Value>> {
    cancellation.ensure_active()?;
    let (Some(first), Some(last)) = (path.segments.first(), path.segments.last()) else {
        return Ok(None);
    };
    let range = first.start..last.end;
    let Some(node) = tola_typst_syntax::syntax::node_at_range(names.source(), &range) else {
        return Ok(None);
    };
    Ok(single_value(
        SourceResolver::new(names, graph, cancellation)
            .known_values(&node)?
            .into_iter()
            .map(|(value, _)| value)
            .collect(),
    ))
}

fn class_kind(class: NameClass) -> CompletionItemKind {
    match class {
        NameClass::Function => CompletionItemKind::FUNCTION,
        NameClass::Namespace => CompletionItemKind::MODULE,
        NameClass::Value => CompletionItemKind::VALUE,
    }
}

/// The members a source expression establishes without evaluating the site.
pub(crate) fn source_member_completion(
    graph: Option<&NameGraph>,
    names: &SourceNames,
    source: &Source,
    cursor: usize,
    cancellation: &BuildCancellation,
) -> Result<Option<Vec<CompletionItem>>> {
    cancellation.ensure_active()?;
    let Some(member) = tola_typst_syntax::syntax::member(source, cursor) else {
        return Ok(None);
    };
    let Some(target) = tola_typst_syntax::syntax::node_at_range(source, &member.target) else {
        return Ok(None);
    };
    let prefix = &source.text()[member.replacement.start..cursor];
    let mut resolver = SourceResolver::new(names, graph, cancellation);
    let values = resolver.types(&target)?;
    let mut packages = BTreeMap::new();
    if values.is_empty() {
        let Some(exports) = resolver.module_members(&target, prefix)? else {
            return Ok(None);
        };
        let mut items = Vec::new();
        for export in exports {
            cancellation.ensure_active()?;
            let edit = completion_edit(
                source,
                cursor,
                member.replacement.clone(),
                export.name.clone(),
            )?;
            let mut item = CompletionItem::new_simple(export.name, String::new());
            item.kind = Some(class_kind(export.class));
            item.text_edit = Some(edit);
            let callable = match export.definition {
                Some(definition) => attach_definition(
                    &mut item,
                    definition,
                    graph,
                    &mut packages,
                    &mut Vec::new(),
                    cancellation,
                )?,
                None => None,
            };
            match callable {
                Some(callable) => items.extend(callable::callable_items(
                    item,
                    source,
                    cursor,
                    &member.replacement,
                    callable,
                )),
                None => items.push(item),
            }
        }
        cancellation.ensure_active()?;
        return Ok(Some(items));
    }
    let mut fields = BTreeMap::<String, Vec<(ExpressionType, Option<MemberCall>)>>::new();
    for (value, styles) in values {
        cancellation.ensure_active()?;
        for (name, member) in members(&value, styles.as_ref(), || {
            cancellation.ensure_active()?;
            Ok(())
        })? {
            cancellation.ensure_active()?;
            if super::name_matches(&name, prefix) {
                let alternatives = fields.entry(name).or_default();
                if !alternatives.iter().any(|(ty, _)| *ty == member.ty) {
                    alternatives.push((member.ty, member.call));
                }
            }
        }
    }
    let mut items = Vec::new();
    for (name, alternatives) in fields {
        cancellation.ensure_active()?;
        let edit = completion_edit(source, cursor, member.replacement.clone(), name.clone())?;
        let types = alternatives
            .iter()
            .map(|(ty, _)| ty.clone())
            .collect::<Vec<_>>();
        let item = semantic::completion(&name, &types, edit);
        let binds = callable::receiver_binds(
            alternatives
                .iter()
                .map(|(_, call)| *call == Some(MemberCall::Receiver)),
        );
        let callable = binds.and_then(|bound| {
            alternatives.iter().find_map(|(ty, _)| {
                ty.value()
                    .and_then(|value| Callable::of_value(value, bound))
            })
        });
        match callable {
            Some(callable) => items.extend(callable::callable_items(
                item,
                source,
                cursor,
                &member.replacement,
                callable,
            )),
            None => items.push(item),
        }
    }
    cancellation.ensure_active()?;
    Ok(Some(items))
}

/// The import items one source answers without a compiled world.
///
/// The module's own source decides its exports; the reader's general names cannot answer an import item.
pub(crate) fn source_import_completion(
    graph: &NameGraph,
    names: &SourceNames,
    source: &Source,
    cursor: usize,
    cancellation: &BuildCancellation,
) -> Result<Option<Vec<CompletionItem>>> {
    cancellation.ensure_active()?;
    let Some(import) = tola_typst_syntax::syntax::import(source, cursor) else {
        return Ok(None);
    };
    let Some(replacement) = import.replacement.clone() else {
        return Ok(None);
    };
    if cursor < replacement.start || cursor > replacement.end {
        return Ok(None);
    }
    let mut statement = None;
    for (id, candidate) in names.imports().iter().enumerate() {
        cancellation.ensure_active()?;
        if candidate.source_range.start <= import.source.start
            && import.source.end <= candidate.source_range.end
        {
            statement = Some(id);
            break;
        }
    }
    let Some(statement) = statement else {
        return Ok(None);
    };
    let prefix = &source.text()[replacement.start..cursor];
    let exports =
        graph.module_exports_matching(source.id(), statement, &import.path, prefix, || {
            cancellation.ensure_active()
        })?;
    let mut packages = BTreeMap::new();
    let mut items = Vec::new();
    if import.imported.is_empty() && "*".starts_with(prefix) {
        let mut item = CompletionItem::new_simple("*".to_owned(), "Import everything.".to_owned());
        item.kind = Some(CompletionItemKind::SNIPPET);
        item.text_edit = Some(completion_edit(
            source,
            cursor,
            replacement.clone(),
            "*".to_owned(),
        )?);
        items.push(item);
    }
    for export in exports {
        cancellation.ensure_active()?;
        let name = export.name;
        let mut imported = false;
        for path in &import.imported {
            cancellation.ensure_active()?;
            if path.len() == import.path.len() + 1
                && path[..import.path.len()] == import.path
                && path.last() == Some(&name)
            {
                imported = true;
                break;
            }
        }
        if imported {
            continue;
        }
        let mut item = CompletionItem::new_simple(name.clone(), String::new());
        item.kind = Some(class_kind(export.class));
        item.text_edit = Some(completion_edit(source, cursor, replacement.clone(), name)?);
        if let Some(definition) = export.definition {
            let _ = attach_definition(
                &mut item,
                definition,
                Some(graph),
                &mut packages,
                &mut Vec::new(),
                cancellation,
            )?;
        }
        items.push(item);
    }
    cancellation.ensure_active()?;
    Ok(Some(items))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tola_typst::typst::foundations::PathOrStr;
    use tola_typst::typst::syntax::{FileId, RootedPath, VirtualPath, VirtualRoot};
    use tola_typst_syntax::imports::ImportQuery;

    fn file(path: &str) -> FileId {
        RootedPath::new(VirtualRoot::Project, VirtualPath::new(path).unwrap()).intern()
    }

    fn graph(source: &Source, libraries: &[Source]) -> NameGraph {
        NameGraph::new(
            std::iter::once(source)
                .chain(libraries)
                .map(|source| Arc::new(SourceNames::new(source.clone()))),
            |query| match query {
                ImportQuery::Path { path, .. } if path.starts_with('@') => {
                    let package = path.parse().ok()?;
                    Some(
                        RootedPath::new(
                            VirtualRoot::Package(package),
                            VirtualPath::new("lib.typ").unwrap(),
                        )
                        .intern(),
                    )
                }
                ImportQuery::Path { file, path } => PathOrStr::Str(path.into())
                    .resolve(file)
                    .ok()
                    .map(|path| path.intern()),
                ImportQuery::Expression { .. } => None,
            },
            Some(&GLOBAL_LIBRARY),
            || Ok::<(), std::convert::Infallible>(()),
        )
        .unwrap()
    }

    fn named<'a>(items: &'a [CompletionItem], name: &str) -> &'a CompletionItem {
        items.iter().find(|item| item.label == name).unwrap()
    }

    fn inserted_text(item: &CompletionItem) -> &str {
        match item.text_edit.as_ref().unwrap() {
            lsp_types::CompletionTextEdit::Edit(edit) => &edit.new_text,
            lsp_types::CompletionTextEdit::InsertAndReplace(edit) => &edit.new_text,
        }
    }

    fn description(item: &CompletionItem) -> &str {
        item.label_details
            .as_ref()
            .and_then(|details| details.description.as_deref())
            .unwrap()
    }

    fn documentation(item: &CompletionItem) -> &str {
        let Some(lsp_types::Documentation::MarkupContent(markup)) = &item.documentation else {
            panic!("completion carries Markdown documentation");
        };
        &markup.value
    }

    /// The items the lane that reads the source alone answers with, for one marked source.
    fn cold_items(marked: &str) -> Vec<CompletionItem> {
        let cursor = marked.find('|').unwrap();
        let source = Source::detached(marked.replace('|', ""));
        let names = SourceNames::new(source.clone());
        let cancellation = BuildCancellation::new();
        if marked.starts_with("#import") {
            source_import_completion(&graph(&source, &[]), &names, &source, cursor, &cancellation)
        } else {
            source_name_completion(None, &names, &source, cursor, &cancellation)
        }
        .unwrap()
        .unwrap()
    }

    /// A callable completes as a call snippet only where the position calls it; anywhere else the
    /// completion inserts the bare name.
    #[test]
    fn call_snippets_follow_the_typed_position() {
        for (marked, label, expected) in [
            ("𝄞 #rep|\n", "repr", "repr(${1})"),
            ("#ran|", "range", "range(${1})"),
            ("#her|", "here", "here()${1}"),
            ("#let local(x) = rep|", "repr", "repr(${1})"),
            ("#rep|r(1)", "repr", "repr"),
            ("#rep|[body]", "repr", "repr"),
            ("#let alias = rep|", "repr", "repr"),
            ("#let result = 1 + rep|", "repr", "repr(${1})"),
            ("#let f(x) = x\n#f(rep|)", "repr", "repr"),
            ("#show raw: rep|", "repr", "repr"),
            ("#show stron|: it => it", "strong", "strong"),
            ("#import math: ab|", "abs", "abs"),
        ] {
            let items = cold_items(marked);
            let item = named(&items, label);
            let text = inserted_text(item);
            assert_eq!(text, expected, "{marked}");
            if expected.contains("${") {
                assert_eq!(
                    item.insert_text_format,
                    Some(lsp_types::InsertTextFormat::SNIPPET),
                    "{marked}"
                );
                assert_eq!(crate::completion::plain_text(text), format!("{label}()"));
            } else {
                assert_ne!(
                    item.insert_text_format,
                    Some(lsp_types::InsertTextFormat::SNIPPET),
                    "{marked}"
                );
            }
        }
    }

    /// The lane that reads the source alone and the lane that reads the checked site insert the
    /// same call.
    #[test]
    fn cold_and_warm_lanes_agree() {
        for (marked, label) in [
            ("𝄞 #rep|\n", "repr"),
            ("#let result = 1 + rep|", "repr"),
            ("#show stron|: it => it", "strong"),
            ("#import math: ab|", "abs"),
        ] {
            let cold = cold_items(marked);
            let mut site = crate::query::tests::QuerySession::new();
            let warm = site.completion(marked);
            let (cold, warm) = (named(&cold, label), named(&warm, label));
            assert_eq!(inserted_text(cold), inserted_text(warm), "{marked}");
            assert_eq!(cold.insert_text_format, warm.insert_text_format, "{marked}");
        }
    }

    /// What a site's own declarations say about a callable is one answer: the lane that reads the
    /// source alone and the lane that reads the checked world offer the same kind, signature, and
    /// inserted call.
    #[test]
    fn declared_callables_read_one_description() {
        let library = Source::new(
            file("lib.typ"),
            "/// Render meaning.\n/// - value (int): Input meaning.\n#let render(value, label: \"x\") = value"
                .to_owned(),
        );
        for (marked, label, inserted, signature) in [
            (
                "#let render(value, label: \"x\") = value\n#ren|",
                "render",
                "render(${1})",
                "render(value: any, label: any)",
            ),
            (
                "#import \"lib.typ\": render\n#ren|",
                "render",
                "render(${1})",
                "render(value: int, label: any)",
            ),
            (
                "#import \"lib.typ\": render\n#let alias = render\n#ali|",
                "alias",
                "alias(${1})",
                "alias(value: int, label: any)",
            ),
            ("#let stamp() = 1\n#sta|", "stamp", "stamp()${1}", "stamp()"),
        ] {
            let cursor = marked.find('|').unwrap();
            let source = Source::new(file("main.typ"), marked.replace('|', ""));
            let names = SourceNames::new(source.clone());
            let graph = graph(&source, std::slice::from_ref(&library));
            let cold = source_name_completion(
                Some(&graph),
                &names,
                &source,
                cursor,
                &BuildCancellation::new(),
            )
            .unwrap()
            .unwrap();
            let mut site = crate::query::tests::QuerySession::new();
            site.site.write("content/lib.typ", library.text());
            site.site
                .write("content/document.typ", &marked.replace('|', ""));
            let warm = site.completion(marked);
            for item in [named(&cold, label), named(&warm, label)] {
                assert_eq!(item.kind, Some(CompletionItemKind::FUNCTION), "{marked}");
                assert_eq!(description(item), signature, "{marked}");
                assert_eq!(inserted_text(item), inserted, "{marked}");
                assert_eq!(
                    item.insert_text_format,
                    Some(lsp_types::InsertTextFormat::SNIPPET),
                    "{marked}"
                );
            }
        }
    }

    /// A member's call follows its receiver: a module or a type passes the receiver as the first
    /// argument, while a value already supplies `self` and leaves the call empty.
    #[test]
    fn member_calls_follow_their_receiver() {
        let library = Source::new(file("lib.typ"), "#let render(value) = value".to_owned());
        for (marked, label, inserted) in [
            (
                "#import \"lib.typ\" as lib\n#lib.ren|",
                "render",
                "render(${1})",
            ),
            ("#str.le|", "len", "len(${1})"),
            ("#let text = \"x\"\n#text.le|", "len", "len()${1}"),
        ] {
            let cursor = marked.find('|').unwrap();
            let source = Source::new(file("main.typ"), marked.replace('|', ""));
            let names = SourceNames::new(source.clone());
            let graph = graph(&source, std::slice::from_ref(&library));
            let cold = source_member_completion(
                Some(&graph),
                &names,
                &source,
                cursor,
                &BuildCancellation::new(),
            )
            .unwrap()
            .unwrap();
            let mut site = crate::query::tests::QuerySession::new();
            site.site.write("content/lib.typ", library.text());
            site.site
                .write("content/document.typ", &marked.replace('|', ""));
            let warm = site.completion(marked);
            for item in [named(&cold, label), named(&warm, label)] {
                assert_eq!(inserted_text(item), inserted, "{marked}");
            }
        }
    }

    #[test]
    fn local_completions_carry_declared_signatures() {
        for declaration in [
            "#let render(value, label: \"x\", ..args) = value",
            "#let render = (\n (value, label: \"x\", ..args) => value\n)",
        ] {
            for suffix in ["#ren", "#let alias = render\n#ali"] {
                let source = Source::detached(format!(
                    "/// Render meaning.\n/// - value (int): Input meaning.\n/// - label (str): Label meaning.\n/// -> int\n{declaration}\n{suffix}"
                ));
                let names = SourceNames::new(source.clone());
                let items = source_name_completion(
                    None,
                    &names,
                    &source,
                    source.text().len(),
                    &BuildCancellation::new(),
                )
                .unwrap()
                .unwrap();
                let label = if suffix == "#ren" { "render" } else { "alias" };
                let completed = named(&items, label);
                assert_eq!(
                    completed.kind,
                    Some(CompletionItemKind::FUNCTION),
                    "{declaration}: {suffix}"
                );
                for spelling in [
                    label,
                    "value: int",
                    "label: str",
                    "args: arguments",
                    "-> int",
                ] {
                    assert!(description(completed).contains(spelling), "{completed:?}");
                }
                assert!(documentation(completed).contains("Render meaning."));
                assert!(documentation(completed).contains("Input meaning."));
            }
        }
    }

    #[test]
    fn imported_completions_keep_export_documentation() {
        let library = Source::new(file("library.typ"), "/// Library meaning.\n/// - value (int): Input meaning.\n/// -> int\n#let render(value: 1) = value".to_owned());
        let reexport = Source::new(
            file("exports.typ"),
            "/// Export meaning.\n#import \"library.typ\": render as external".to_owned(),
        );
        for (text, label, meaning, import) in [
            (
                "#import \"library.typ\": ren",
                "render",
                "Library meaning.",
                true,
            ),
            (
                "#import \"library.typ\": render as alias\n#ali",
                "alias",
                "Library meaning.",
                false,
            ),
            (
                "#import \"library.typ\": *\n#ren",
                "render",
                "Library meaning.",
                false,
            ),
            (
                "#import \"exports.typ\": ext",
                "external",
                "Export meaning.",
                true,
            ),
        ] {
            let source = Source::new(file("main.typ"), text.to_owned());
            let names = SourceNames::new(source.clone());
            let graph = graph(&source, &[library.clone(), reexport.clone()]);
            let items = if import {
                source_import_completion(
                    &graph,
                    &names,
                    &source,
                    source.text().len(),
                    &BuildCancellation::new(),
                )
                .unwrap()
                .unwrap()
            } else {
                source_name_completion(
                    Some(&graph),
                    &names,
                    &source,
                    source.text().len(),
                    &BuildCancellation::new(),
                )
                .unwrap()
                .unwrap()
            };
            let completed = named(&items, label);
            assert!(
                description(completed).contains(&format!("{label}(")),
                "{completed:?}"
            );
            assert!(
                description(completed).contains("value: int"),
                "{completed:?}"
            );
            assert!(documentation(completed).contains(meaning), "{completed:?}");
        }
    }

    #[test]
    fn bundled_completions_use_package_declarations() {
        let package =
            tola_packages::builtin_package(&tola_packages::TolaPackage::Document.spec()).unwrap();
        let libraries = package
            .files()
            .filter(|(path, _)| path.ends_with(".typ"))
            .map(|(path, source)| {
                let id = RootedPath::new(
                    VirtualRoot::Package(package.spec()),
                    VirtualPath::new(path).unwrap(),
                )
                .intern();
                Source::new(id, source.into_owned())
            })
            .collect::<Vec<_>>();
        let documented = package
            .export_documentation(&["current-document".to_owned()])
            .unwrap()
            .remove(0);
        for (text, label) in [
            (
                "#import \"@tola/document:0.0.0\": current-doc",
                "current-document",
            ),
            (
                "#import \"@tola/document:0.0.0\": current-document as identity\n#iden",
                "identity",
            ),
        ] {
            let source = Source::new(file("main.typ"), text.to_owned());
            let names = SourceNames::new(source.clone());
            let graph = graph(&source, &libraries);
            let items = if label == "identity" {
                source_name_completion(
                    Some(&graph),
                    &names,
                    &source,
                    source.text().len(),
                    &BuildCancellation::new(),
                )
                .unwrap()
                .unwrap()
            } else {
                source_import_completion(
                    &graph,
                    &names,
                    &source,
                    source.text().len(),
                    &BuildCancellation::new(),
                )
                .unwrap()
                .unwrap()
            };
            let completed = named(&items, label);
            assert!(
                description(completed).contains(&format!("{label}(")),
                "{completed:?}"
            );
            assert!(documentation(completed).contains(&documented.documentation.summary));
        }
    }

    #[test]
    fn library_names_follow_math_namespace() {
        for (marked, expected) in [("$sq|$", "sqrt"), ("$st|$", "std"), ("#st|", "std")] {
            let cursor = marked.find('|').unwrap();
            let source = Source::detached(marked.replace('|', ""));
            let names = SourceNames::new(source.clone());
            let items =
                source_name_completion(None, &names, &source, cursor, &BuildCancellation::new())
                    .unwrap()
                    .unwrap();
            assert!(
                items.iter().any(|item| item.label == expected),
                "{marked}: {items:?}"
            );
        }
        for (marked, expected) in [
            ("#std.math.eq|", "equation"),
            ("#std.str.le|", "len"),
            ("$sqrt.wh|$", "where"),
        ] {
            let cursor = marked.find('|').unwrap();
            let source = Source::detached(marked.replace('|', ""));
            let names = SourceNames::new(source.clone());
            let items =
                source_member_completion(None, &names, &source, cursor, &BuildCancellation::new())
                    .unwrap()
                    .unwrap();
            assert!(
                items.iter().any(|item| item.label == expected),
                "{marked}: {items:?}"
            );
        }
    }

    #[test]
    fn completion_edits_follow_utf16_cursor() {
        let marked = "#let café(value) = value\n😀 #ca|fé";
        let cursor = marked.find('|').unwrap();
        let source = Source::detached(marked.replace('|', ""));
        let names = SourceNames::new(source.clone());
        let items =
            source_name_completion(None, &names, &source, cursor, &BuildCancellation::new())
                .unwrap()
                .unwrap();
        let completed = named(&items, "café");
        let Some(lsp_types::CompletionTextEdit::InsertAndReplace(edit)) = &completed.text_edit
        else {
            panic!("the cursor splits the inserted prefix from the replaced name");
        };
        assert_eq!(edit.insert.start, lsp_types::Position::new(1, 4));
        assert_eq!(edit.insert.end, lsp_types::Position::new(1, 6));
        assert_eq!(edit.replace.end, lsp_types::Position::new(1, 8));
        assert_eq!(edit.new_text, "café(${1})");
    }

    #[test]
    fn source_members_follow_expression_values() {
        for (marked, expected) in [
            ("#\"text\".|", "split"),
            ("#let value = \"text\"\n#value.|", "split"),
            ("#let value = (1, 2)\n#value.|", "map"),
            ("#let value = (custom: 1)\n#value.|", "custom"),
            ("#let value = 1 + 2\n#value.|", "signum"),
            (
                "#let value = if true { (left: 1) } else { (right: 2) }\n#value.|",
                "right",
            ),
            ("#let f(x) = (custom: x)\n#f(1).|", "custom"),
            ("#let f(x: 1) = (custom: x)\n#f().|", "custom"),
            ("#let f((x, y)) = (custom: x)\n#f((1, 2)).|", "custom"),
            ("#let f(..args) = args\n#f(extra: 1).|", "extra"),
            (
                "#let f(x) = (custom: x)\n#let alias = f\n#alias(1).|",
                "custom",
            ),
            ("#let f = ((x) => (custom: x))\n#f(1).|", "custom"),
            (
                "#let outer(x) = { let inner() = x; inner() }\n#outer((captured: 1)).|",
                "captured",
            ),
            ("#let value = str(1)\n#value.|", "split"),
            ("#let convert = str\n#convert(1).|", "split"),
            ("#let value = \"a,b\".split(\",\")\n#value.|", "map"),
        ] {
            let cursor = marked.find('|').unwrap();
            let source = Source::detached(marked.replace('|', ""));
            let names = SourceNames::new(source.clone());
            let items =
                source_member_completion(None, &names, &source, cursor, &BuildCancellation::new())
                    .unwrap()
                    .unwrap_or_else(|| panic!("{marked}"));
            assert!(
                items.iter().any(|item| item.label == expected),
                "{marked}: {items:?}"
            );
        }
    }

    #[test]
    fn source_imports_preserve_module_identity() {
        let library = Source::new(
            file("library.typ"),
            "/// Render meaning.\n#let render(value: 1) = (custom: value)".to_owned(),
        );
        for (marked, expected) in [
            (
                "#import \"library.typ\": render as local\n#local().|",
                "custom",
            ),
            (
                "#import \"library.typ\" as library\n#library.render().|",
                "custom",
            ),
            ("#import \"library.typ\" as library\n#library.|", "render"),
            (
                "#let f() = { import \"library.typ\": render; render() }\n#f().|",
                "custom",
            ),
        ] {
            let cursor = marked.find('|').unwrap();
            let source = Source::new(file("main.typ"), marked.replace('|', ""));
            let names = SourceNames::new(source.clone());
            let graph = graph(&source, std::slice::from_ref(&library));
            let items = source_member_completion(
                Some(&graph),
                &names,
                &source,
                cursor,
                &BuildCancellation::new(),
            )
            .unwrap()
            .unwrap_or_else(|| panic!("{marked}"));
            assert!(
                items.iter().any(|item| item.label == expected),
                "{marked}: {items:?}"
            );
        }
    }

    #[test]
    fn writes_invalidate_initializers() {
        for (marked, expected) in [
            (
                "#let value = (old: 1)\n#(value = (current: 2))\n#value.|",
                None,
            ),
            (
                "#let value = (old: 1)\n#value.remove(\"old\")\n#value.|",
                None,
            ),
            (
                "#let value = (old: 1)\n#value.o|ld\n#(value = (current: 2))",
                Some("old"),
            ),
            (
                "#let value = (old: 1)\n#{ let value = (inner: 1); value = (current: 2) }\n#value.o|ld",
                Some("old"),
            ),
        ] {
            let cursor = marked.find('|').unwrap();
            let source = Source::detached(marked.replace('|', ""));
            let names = SourceNames::new(source.clone());
            let cold =
                source_member_completion(None, &names, &source, cursor, &BuildCancellation::new())
                    .unwrap();
            if let Some(expected) = expected {
                assert!(
                    cold.unwrap().iter().any(|item| item.label == expected),
                    "{marked}"
                );
            } else {
                assert!(cold.is_none(), "{marked}: {cold:?}");
                let mut site = crate::query::tests::QuerySession::with_program(
                    crate::query::tests::FAILED_PROGRAM,
                );
                let warm = site.completion(&format!("#panic(\"page\")\n{marked}"));
                assert!(
                    !warm.iter().any(|item| item.label == "old"),
                    "{marked}: {warm:?}"
                );
            }
        }
    }

    #[test]
    fn typed_containers_preserve_fields() {
        for (marked, expected) in [
            ("#let f(x) = (current: x.len())\n#f(\"test\").|", "current"),
            (
                "#let f(x) = (current: x.len())\n#f(\"test\").current.|",
                "signum",
            ),
            ("#let value = (current: unknown)\n#value.|", "current"),
            (
                "#let f((..rest, value)) = value\n#f((0, str(1))).|",
                "split",
            ),
            (
                "#let (ignored, value) = (0, (current: str(1)))\n#value.current.|",
                "split",
            ),
            (
                "#let (ignored, ..rest) = (ignored: 0, current: str(1))\n#rest.|",
                "current",
            ),
            (
                "#let outer(x) = { let inner() = (current: x.len()); inner() }\n#outer(\"test\").current.|",
                "signum",
            ),
        ] {
            let cursor = marked.find('|').unwrap();
            let source = Source::detached(marked.replace('|', ""));
            let names = SourceNames::new(source.clone());
            let cold =
                source_member_completion(None, &names, &source, cursor, &BuildCancellation::new())
                    .unwrap()
                    .expect(marked);
            let mut site = crate::query::tests::QuerySession::with_program(
                crate::query::tests::FAILED_PROGRAM,
            );
            for items in [
                cold,
                site.completion(&format!("#panic(\"page\")\n{marked}")),
            ] {
                assert!(
                    items.iter().any(|item| item.label == expected),
                    "{marked}: {items:?}"
                );
            }
        }
    }

    #[test]
    fn source_calls_preserve_uncertainty() {
        for (marked, expected) in [
            (
                "#let last((..rest, x)) = x\n#last((0, (wrong: 1), (right: 1))).|",
                Some("right"),
            ),
            (
                "#let last((x, ..rest, y)) = y\n#last((0, 1, (wrong: 1), (right: 1))).|",
                Some("right"),
            ),
            (
                "#let choose(flag) = if flag { (1, 2).at(0) } else { (wrong: 1) }\n#choose(true).|",
                None,
            ),
            ("#let take(x) = x\n#take(if false { (wrong: 1) }).|", None),
            (
                "#let take(x) = x\n#take(if true { (left: 1) } else { (right: 1) }).|",
                None,
            ),
            (
                "#let f() = { let ignored = { return (right: 1) }; (wrong: 1) }\n#f().|",
                None,
            ),
            (
                "#let f() = { let inner(x: { return (right: 1) }) = x; (wrong: 1) }\n#f().|",
                None,
            ),
            (
                "#let f() = { let inner() = { return (unused: 1) }; (right: 1) }\n#f().|",
                Some("right"),
            ),
        ] {
            let cursor = marked.find('|').unwrap();
            let source = Source::detached(marked.replace('|', ""));
            let names = SourceNames::new(source.clone());
            let items =
                source_member_completion(None, &names, &source, cursor, &BuildCancellation::new())
                    .unwrap();
            let labels = items.as_ref().map(|items| {
                items
                    .iter()
                    .map(|item| item.label.as_str())
                    .collect::<Vec<_>>()
            });
            match expected {
                Some(expected) => assert!(
                    labels
                        .as_ref()
                        .is_some_and(|labels| labels.contains(&expected)),
                    "{marked}: {labels:?}"
                ),
                None => assert!(items.is_none(), "{marked}: {labels:?}"),
            }
        }
    }
}
