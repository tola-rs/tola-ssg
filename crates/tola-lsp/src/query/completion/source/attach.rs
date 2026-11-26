use anyhow::Result;
use lsp_types::{CompletionItem, CompletionItemKind};
use tola_build::cancellation::BuildCancellation;
use tola_typst::typst::foundations::Value;
use tola_typst::typst::syntax::FileId;
use tola_typst_syntax::imports::{ModuleExport, NameClass, NameDefinition, NameGraph};
use tola_typst_syntax::names::{Declaration, Initializer, NamePath, SourceNames};

use crate::query::completion::NamePosition;
use crate::query::completion::callable::Callable;
use crate::query::completion::definition::{self, PackageExports};
use crate::query::expression::single_value;
use crate::query::inference::{ExpressionResolver, SourceResolver};

pub(super) fn attach_definition(
    item: &mut CompletionItem,
    def: NameDefinition,
    graph: Option<&NameGraph>,
    pkgs: &mut PackageExports,
    followed: &mut Vec<(FileId, usize)>,
    cancel: &BuildCancellation,
) -> Result<Option<Callable>> {
    cancel.ensure_active()?;
    let shape = match def {
        NameDefinition::Builtin(val) => definition::attach_value(item, val),
        NameDefinition::Source { file, declaration } => {
            let Some(names) = graph.and_then(|g| g.source(file)) else {
                return Ok(None);
            };
            attach_source_declaration(
                item,
                names,
                &names.declarations()[declaration],
                graph,
                pkgs,
                followed,
                cancel,
            )?
        }
    };
    cancel.ensure_active()?;
    Ok(shape)
}

pub(super) fn attach_source_declaration(
    item: &mut CompletionItem,
    names: &SourceNames,
    decl: &Declaration,
    graph: Option<&NameGraph>,
    pkgs: &mut PackageExports,
    followed: &mut Vec<(FileId, usize)>,
    cancel: &BuildCancellation,
) -> Result<Option<Callable>> {
    let src = names.source();
    let id = (src.id(), decl.range.start);
    if followed.contains(&id) {
        return Ok(None);
    }
    followed.push(id);
    if definition::exporting_package(names, &decl.name).is_some() {
        return definition::attach_declaration(item, names, decl, pkgs, cancel);
    }
    let mut shape = None;
    if let Initializer::Import { import, path } = &decl.initializer
        && let Some(graph) = graph
        && let Some(def) = graph.import_definition(src.id(), *import, path)
    {
        shape = attach_definition(item, def, Some(graph), pkgs, followed, cancel)?;
    }
    if let Initializer::Alias(path) = &decl.initializer {
        shape = attach_alias(item, names, path, graph, pkgs, followed, cancel)?.or(shape);
    }
    Ok(definition::attach_declaration(item, names, decl, pkgs, cancel)?.or(shape))
}

fn attach_alias(
    item: &mut CompletionItem,
    names: &SourceNames,
    path: &NamePath,
    graph: Option<&NameGraph>,
    pkgs: &mut PackageExports,
    followed: &mut Vec<(FileId, usize)>,
    cancel: &BuildCancellation,
) -> Result<Option<Callable>> {
    if let Some(graph) = graph
        && let Some(range) = path.segments.last()
        && let Some(binding) = graph
            .selected(names.source().id(), range.start)
            .and_then(|occ| occ.binding)
        && let Some(def) = graph.binding_definition(binding)
    {
        return attach_definition(item, def, Some(graph), pkgs, followed, cancel);
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
            pkgs,
            followed,
            cancel,
        );
    }
    let Some(val) = library_value(names, path, graph, cancel)? else {
        return Ok(None);
    };
    Ok(definition::attach_value(item, val))
}

fn library_value(
    names: &SourceNames,
    path: &NamePath,
    graph: Option<&NameGraph>,
    cancel: &BuildCancellation,
) -> Result<Option<Value>> {
    cancel.ensure_active()?;
    let (Some(first), Some(last)) = (path.segments.first(), path.segments.last()) else {
        return Ok(None);
    };
    let range = first.start..last.end;
    let Some(node) = tola_typst_syntax::syntax::node_at_range(names.source(), &range) else {
        return Ok(None);
    };
    Ok(single_value(
        SourceResolver::new(names, graph, cancel)
            .known_values(&node)?
            .into_iter()
            .map(|(val, _)| val)
            .collect(),
    ))
}

pub(super) fn class_kind(class: NameClass) -> CompletionItemKind {
    match class {
        NameClass::Function => CompletionItemKind::FUNCTION,
        NameClass::Namespace => CompletionItemKind::MODULE,
        NameClass::Value => CompletionItemKind::VALUE,
    }
}

pub(super) fn export_item(
    pos: &NamePosition<'_>,
    export: ModuleExport,
    graph: Option<&NameGraph>,
    pkgs: &mut PackageExports,
    cancel: &BuildCancellation,
) -> Result<(CompletionItem, Option<Callable>)> {
    let edit = pos.edit(export.name.clone())?;
    let mut item = CompletionItem::new_simple(export.name, String::new());
    item.kind = Some(class_kind(export.class));
    item.text_edit = Some(edit);
    let callable = match export.definition {
        Some(def) => attach_definition(&mut item, def, graph, pkgs, &mut Vec::new(), cancel)?,
        None => None,
    };
    Ok((item, callable))
}
