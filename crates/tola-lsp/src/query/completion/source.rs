//! Names and members established by source and static library metadata, without a checked world.

mod attach;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use lsp_types::{CompletionItem, CompletionItemKind, CompletionResponse};
use tola_build::cancellation::BuildCancellation;
use tola_typst::GLOBAL_LIBRARY;
use tola_typst::typst::foundations::Value;
use tola_typst::typst::syntax::ast;
use tola_typst::typst::syntax::{LinkedNode, Side, Source, SyntaxKind};
use tola_typst_syntax::imports::{ModuleExport, NameDefinition, NameGraph};
use tola_typst_syntax::names::{Declaration, Initializer, OccurrenceKind, SourceNames};

use super::super::expression::{ExpressionType, MemberCall, members};
use super::super::inference::{ExpressionResolver, SourceResolver};
use super::super::semantic;
use super::callable::{self, Callable};
use super::definition::{self, PackageExports};
use super::{
    NamePosition, already_imported, comment_at, documentation_template, is_path_position,
    opaque_at, raw_at, wildcard_import_item,
};
use crate::protocol::SourceReply;
use attach::{attach_definition, attach_source_declaration, class_kind, export_item};

/// Source replies whose answer cannot depend on a checked world. Strings inside calls retain
/// their semantic lane because a callee can declare an asset URL parameter.
pub(crate) fn source_completion_reply(source: &Source, cursor: usize) -> Option<SourceReply> {
    if let Some(reply) = documentation_template(source, cursor) {
        return Some(reply);
    }
    let mut node = LinkedNode::new(source.root()).leaf_at(cursor, Side::Before);
    let mut in_call = false;
    while let Some(cur) = node {
        if cur.kind() == SyntaxKind::FuncCall {
            in_call = true;
            break;
        }
        node = cur.parent().cloned();
    }
    (comment_at(source, cursor)
        || raw_at(source, cursor)
        || (opaque_at(source, cursor) && !in_call && !is_path_position(source, cursor)))
    .then(|| SourceReply::Completion(CompletionResponse::Array(Vec::new())))
}

pub(crate) fn source_name_position(names: &SourceNames, source: &Source, cursor: usize) -> bool {
    if opaque_at(source, cursor) || name_range(source, cursor).is_none() {
        return false;
    }
    match names.occurrence(cursor) {
        Some(occ) => matches!(occ.kind, OccurrenceKind::Name),
        None => LinkedNode::new(source.root())
            .leaf_at(cursor, Side::Before)
            .is_some_and(|leaf| leaf.kind() == SyntaxKind::Hash),
    }
}

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
    let Some(pos) = typed_name(source, cursor) else {
        return Ok(None);
    };
    let prefix = pos.prefix;
    let mut offered = OfferedNames::new(pos, names, graph, cancellation);
    let mut scope = Some(names.scope_at(cursor));
    while let Some(id) = scope {
        let mut candidates = Vec::new();
        for &decl in names.scopes()[id].declarations.iter().rev() {
            cancellation.ensure_active()?;
            let decl = &names.declarations()[decl];
            if super::name_matches(&decl.name, prefix) && decl.visible_at(cursor) {
                candidates.push(ScopeName::Declaration(decl));
            }
        }
        for (stmt, import) in names.imports().iter().enumerate() {
            cancellation.ensure_active()?;
            if !wildcard_import_visible(import, id, cursor) {
                continue;
            }
            let Some(graph) = graph else {
                return Ok(None);
            };
            if !graph.interface_established(source.id(), stmt) {
                return Ok(None);
            }
            for export in graph.module_exports_matching(source.id(), stmt, &[], prefix, || {
                cancellation.ensure_active()
            })? {
                candidates.push(ScopeName::Import {
                    export,
                    visible_from: import.range.end,
                });
            }
        }
        cancellation.ensure_active()?;
        candidates.sort_by_key(|c| std::cmp::Reverse(c.visible_from()));
        for candidate in candidates {
            cancellation.ensure_active()?;
            match candidate {
                ScopeName::Declaration(decl) => offered.declaration(decl)?,
                ScopeName::Import { export, .. } => offered.export(export)?,
            }
        }
        scope = names.scopes()[id].parent;
    }
    offered.standard_library()?;
    cancellation.ensure_active()?;
    Ok(Some(offered.items))
}

enum ScopeName<'a> {
    Declaration(&'a Declaration),
    Import {
        export: ModuleExport,
        visible_from: usize,
    },
}

impl ScopeName<'_> {
    fn visible_from(&self) -> usize {
        match self {
            Self::Declaration(decl) => decl.visible_from,
            Self::Import { visible_from, .. } => *visible_from,
        }
    }
}

struct OfferedNames<'a> {
    pos: NamePosition<'a>,
    names: &'a SourceNames,
    graph: Option<&'a NameGraph>,
    cancel: &'a BuildCancellation,
    pkgs: PackageExports,
    offered: BTreeSet<String>,
    items: Vec<CompletionItem>,
}

impl<'a> OfferedNames<'a> {
    fn new(
        pos: NamePosition<'a>,
        names: &'a SourceNames,
        graph: Option<&'a NameGraph>,
        cancel: &'a BuildCancellation,
    ) -> Self {
        Self {
            pos,
            names,
            graph,
            cancel,
            pkgs: PackageExports::default(),
            offered: BTreeSet::new(),
            items: Vec::new(),
        }
    }

    fn declaration(&mut self, decl: &Declaration) -> Result<()> {
        self.offer(
            decl.name.clone(),
            Some(definition::declaration_kind(&decl.initializer)),
            None,
            Some(decl),
        )
    }

    fn export(&mut self, export: ModuleExport) -> Result<()> {
        self.offer(
            export.name,
            Some(class_kind(export.class)),
            export.definition,
            None,
        )
    }

    fn standard_library(&mut self) -> Result<()> {
        let math = self
            .names
            .occurrence(self.pos.cursor)
            .is_some_and(|occ| occ.math);
        let lib = if math {
            &GLOBAL_LIBRARY.math
        } else {
            &GLOBAL_LIBRARY.global
        };
        for (name, binding) in lib.scope().iter() {
            if super::name_matches(name, self.pos.prefix) {
                self.builtin(name.to_string(), binding.read().clone())?;
            }
        }
        self.builtin("std".to_owned(), GLOBAL_LIBRARY.std.read().clone())
    }

    fn builtin(&mut self, name: String, val: Value) -> Result<()> {
        self.offer(name, None, Some(NameDefinition::Builtin(val)), None)
    }

    fn offer(
        &mut self,
        name: String,
        kind: Option<CompletionItemKind>,
        def: Option<NameDefinition>,
        decl: Option<&Declaration>,
    ) -> Result<()> {
        self.cancel.ensure_active()?;
        if !super::name_matches(&name, self.pos.prefix) || !self.offered.insert(name.clone()) {
            return Ok(());
        }
        let Ok(edit) = self.pos.edit(name.clone()) else {
            return Ok(());
        };
        let mut item = CompletionItem::new_simple(name, String::new());
        item.kind = kind;
        item.text_edit = Some(edit);
        let mut followed = Vec::new();
        let callable = if let Some(def) = def {
            attach_definition(
                &mut item,
                def,
                self.graph,
                &mut self.pkgs,
                &mut followed,
                self.cancel,
            )?
        } else if let Some(decl) = decl {
            attach_source_declaration(
                &mut item,
                self.names,
                decl,
                self.graph,
                &mut self.pkgs,
                &mut followed,
                self.cancel,
            )?
        } else {
            None
        };
        self.items
            .extend(callable::name_items(item, callable, &self.pos));
        Ok(())
    }
}

fn name_range(src: &Source, pos: usize) -> Option<std::ops::Range<usize>> {
    tola_typst_syntax::syntax::name(src, pos).or_else(|| {
        let leaf = LinkedNode::new(src.root()).leaf_at(pos, Side::Before)?;
        (leaf.kind() == SyntaxKind::MathIdent).then(|| leaf.range())
    })
}

fn typed_name(src: &Source, pos: usize) -> Option<NamePosition<'_>> {
    let range = name_range(src, pos)?;
    let replacement = if &src.text()[range.clone()] == "#" {
        pos..pos
    } else {
        range
    };
    Some(NamePosition::at(src, pos, replacement))
}

fn wildcard_import_visible(
    import: &tola_typst_syntax::names::Import,
    scope: usize,
    pos: usize,
) -> bool {
    import.wildcard && import.scope == scope && import.range.end <= pos
}

pub(crate) fn source_name_graph_needed(
    names: &SourceNames,
    cursor: usize,
    cancellation: &BuildCancellation,
) -> Result<bool> {
    cancellation.ensure_active()?;
    let Some(pos) = typed_name(names.source(), cursor) else {
        return Ok(false);
    };
    let mut scope = Some(names.scope_at(cursor));
    while let Some(id) = scope {
        for import in names.imports() {
            cancellation.ensure_active()?;
            if wildcard_import_visible(import, id, cursor) {
                return Ok(true);
            }
        }
        for &decl in &names.scopes()[id].declarations {
            cancellation.ensure_active()?;
            let d = &names.declarations()[decl];
            if super::name_matches(&d.name, pos.prefix)
                && d.visible_from <= cursor
                && imported_declaration(names, decl, &mut BTreeSet::new(), cancellation)?
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
    decl: usize,
    followed: &mut BTreeSet<usize>,
    cancel: &BuildCancellation,
) -> Result<bool> {
    cancel.ensure_active()?;
    if !followed.insert(decl) {
        return Ok(false);
    }
    match &names.declarations()[decl].initializer {
        Initializer::Import { .. } => Ok(true),
        Initializer::Alias(path) => {
            let decl = path
                .segments
                .first()
                .and_then(|root| names.resolve(names.text(root), path.scope, path.at));
            match decl {
                Some(decl) => imported_declaration(names, decl, followed, cancel),
                None => Ok(false),
            }
        }
        _ => Ok(false),
    }
}

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
    let pos = NamePosition::at(source, cursor, member.replacement.clone());
    let mut resolver = SourceResolver::new(names, graph, cancellation);
    let vals = resolver.types(&target)?;
    let mut pkgs = PackageExports::default();
    if vals.is_empty() {
        let Some(exports) = resolver.module_members(&target, pos.prefix)? else {
            return Ok(None);
        };
        let mut items = Vec::new();
        for export in exports {
            cancellation.ensure_active()?;
            let (item, callable) = export_item(&pos, export, graph, &mut pkgs, cancellation)?;
            items.extend(callable::name_items(item, callable, &pos));
        }
        cancellation.ensure_active()?;
        return Ok(Some(items));
    }
    let mut fields = BTreeMap::<String, Vec<(ExpressionType, Option<MemberCall>)>>::new();
    for (val, styles) in vals {
        cancellation.ensure_active()?;
        for (name, member) in members(&val, styles.as_ref(), || Ok(cancellation.ensure_active()?))?
        {
            cancellation.ensure_active()?;
            if super::name_matches(&name, pos.prefix) {
                let alts = fields.entry(name).or_default();
                if !alts.iter().any(|(ty, _)| *ty == member.ty) {
                    alts.push((member.ty, member.call));
                }
            }
        }
    }
    let mut items = Vec::new();
    for (name, alts) in fields {
        cancellation.ensure_active()?;
        let types = alts.iter().map(|(ty, _)| ty.clone()).collect::<Vec<_>>();
        let item = semantic::completion(&name, &types, pos.edit(name.clone())?);
        let receiver = callable::receiver_binds(
            alts.iter()
                .map(|(_, call)| *call == Some(MemberCall::Receiver)),
        );
        let callable = receiver.and_then(|r| Callable::of_types(&types, r));
        items.extend(callable::name_items(item, callable, &pos));
    }
    cancellation.ensure_active()?;
    Ok(Some(items))
}

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
    let stmt = names.imports().iter().enumerate().find_map(|(id, c)| {
        (c.source_range.start <= import.source.start && import.source.end <= c.source_range.end)
            .then_some(id)
    });
    let Some(stmt) = stmt else {
        return Ok(None);
    };
    let pos = NamePosition::at(source, cursor, replacement);
    let exports =
        graph.module_exports_matching(source.id(), stmt, &import.path, pos.prefix, || {
            cancellation.ensure_active()
        })?;
    let mut pkgs = PackageExports::default();
    let mut items = Vec::new();
    if import.imported.is_empty() && "*".starts_with(pos.prefix) {
        items.push(wildcard_import_item(pos.edit("*".to_owned())?));
    }
    for export in exports {
        cancellation.ensure_active()?;
        if already_imported(&import, &export.name) {
            continue;
        }
        let (item, _) = export_item(&pos, export, Some(graph), &mut pkgs, cancellation)?;
        items.push(item);
    }
    cancellation.ensure_active()?;
    Ok(Some(items))
}
