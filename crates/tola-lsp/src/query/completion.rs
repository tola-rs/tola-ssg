//! What one completion offers at the cursor: names, members, import items, metadata keys,
//! and the parameters a call can still fill.

mod callable;
mod definition;
mod source;
mod syntax;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::Arc;

use anyhow::{Context, Result};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionTextEdit,
    Documentation, InsertTextFormat, MarkupContent, MarkupKind,
};
use tola_build::check::SourceCompilation;
use tola_build::config::SiteConfig;
use tola_typst::typst::foundations::Value;
use tola_typst::typst::syntax::ast;
use tola_typst::typst::syntax::{LinkedNode, Side, Source, Span, SyntaxKind};
use tola_typst_syntax::names::{Declaration, Initializer, SourceNames};

use crate::markdown;
use crate::position;

pub(crate) use source::{
    source_completion_reply, source_import_completion, source_member_completion,
    source_member_graph_needed, source_name_completion, source_name_graph_needed,
    source_name_position,
};
pub(in crate::query) use syntax::{comment_at, completion_edit, opaque_at};
use syntax::{documentation_template, is_path_position, raw_at, typed_text, unparenthesized};

use super::context::{SelectedSyntax, Selection};
use super::expression::ExpressionType;
use super::hover::SECTION_JOIN;
use super::meta::{meta_call, meta_key};
use super::records::origin::{field_chain, record_origin};
use super::schema;
use super::semantic::{self, Semantic};
use super::site_schema::{
    DeclaredKey, declared_documentation, descriptor_line, descriptor_type, rejected_key_note,
};

fn labelled_type(ty: Option<String>) -> CompletionItemLabelDetails {
    CompletionItemLabelDetails {
        detail: None,
        description: ty,
    }
}

pub(super) struct NamePosition<'a> {
    pub(super) source: &'a Source,
    pub(super) cursor: usize,
    pub(super) prefix: &'a str,
    pub(super) replacement: Range<usize>,
}

impl<'a> NamePosition<'a> {
    pub(super) fn at(src: &'a Source, pos: usize, replacement: Range<usize>) -> Self {
        Self {
            source: src,
            cursor: pos,
            prefix: typed_text(src, &replacement, pos),
            replacement,
        }
    }

    pub(super) fn edit(&self, new_text: String) -> Result<CompletionTextEdit> {
        completion_edit(self.source, self.cursor, self.replacement.clone(), new_text)
    }
}

pub(super) fn agreed<T: PartialEq>(mut alts: impl Iterator<Item = T>) -> Option<T> {
    let first = alts.next()?;
    alts.all(|alt| alt == first).then_some(first)
}

pub(super) fn name_matches(name: &str, prefix: &str) -> bool {
    let mut spelled = name.chars();
    prefix
        .chars()
        .all(|typed| spelled.by_ref().any(|c| c == typed))
}

pub(super) fn typed_prefix(src: &Source, pos: usize) -> &str {
    let Some(leaf) = LinkedNode::new(src.root()).leaf_at(pos, Side::Before) else {
        return "";
    };
    let range = match leaf.kind() {
        SyntaxKind::Ident | SyntaxKind::MathIdent => leaf.range(),
        _ => return "",
    };
    typed_text(src, &range, pos)
}

pub(in crate::query) fn markdown_documentation(text: impl Into<String>) -> Documentation {
    Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: markdown::docs(&text.into()),
    })
}

struct DeclaredName {
    names: Arc<SourceNames>,
    index: usize,
}

impl DeclaredName {
    fn declaration(&self) -> &Declaration {
        &self.names.declarations()[self.index]
    }
}

fn declared(
    sem: &mut Semantic<'_>,
    bindings: &BTreeMap<String, semantic::Binding>,
    mut span: Span,
) -> Option<DeclaredName> {
    let mut followed = Vec::new();
    loop {
        let range = sem.span_range(span)?;
        let names = sem.source_names(span.id()?)?;
        let index = names.declared_at(range.start)?;
        if let Initializer::Alias(path) = &names.declarations()[index].initializer
            && let Some(root) = path.segments.first()
            && let Some(target) = bindings.get(names.text(root))
            && !followed.contains(&target.span)
        {
            followed.push(target.span);
            span = target.span;
            continue;
        }
        return Some(DeclaredName { names, index });
    }
}

fn attach_value_docs(
    item: &mut CompletionItem,
    sem: &Semantic<'_>,
    span: Span,
    val: Option<&Value>,
) {
    if item.documentation.is_none() {
        item.documentation = sem.value_docs(span, val).map(markdown_documentation);
    }
}

fn known_types(val: Option<&Value>) -> Vec<ExpressionType> {
    val.cloned()
        .map(ExpressionType::Known)
        .into_iter()
        .collect()
}

fn wildcard_import_item(edit: CompletionTextEdit) -> CompletionItem {
    CompletionItem {
        label: "*".to_owned(),
        kind: Some(CompletionItemKind::SNIPPET),
        detail: Some("Import everything.".to_owned()),
        text_edit: Some(edit),
        ..CompletionItem::default()
    }
}

fn already_imported(import: &tola_typst_syntax::syntax::Import, name: &str) -> bool {
    import.imported.iter().any(|path| {
        path.len() == import.path.len() + 1
            && path[..import.path.len()] == import.path
            && path.last().map(String::as_str) == Some(name)
    })
}

impl Selection<'_> {
    pub(super) fn completion(
        &self,
        prep: &Source,
        sem: &mut Semantic<'_>,
    ) -> Result<Vec<CompletionItem>> {
        if let SelectedSyntax::Import(import) = &self.syntax {
            return self.import_completion(prep, sem, import);
        }
        if let SelectedSyntax::Member(member) = &self.syntax {
            return self.member_completion(prep, sem, member);
        }
        if is_path_position(self.source, self.cursor) {
            return Ok(Vec::new());
        }
        let params = self.parameter_completion(sem)?;
        let Some(node) = tola_typst_syntax::syntax::expression(self.source, self.cursor)
            .or_else(|| tola_typst_syntax::syntax::before(self.source, self.cursor))
        else {
            return Ok(params);
        };
        let replacement = if node.kind() == SyntaxKind::Ident {
            node.range()
        } else {
            self.cursor..self.cursor
        };
        let pos = self.name_position(replacement)?;
        let mut items = self.bound_name_items(sem, &node, &pos)?;
        let mut seen: BTreeSet<String> = items.iter().map(|i| i.label.clone()).collect();
        items.extend(self.library_name_items(sem, &node, &pos, &mut seen)?);
        items.extend(params.into_iter().filter(|i| seen.insert(i.label.clone())));
        Ok(items)
    }

    fn bound_name_items(
        &self,
        sem: &mut Semantic<'_>,
        node: &LinkedNode<'_>,
        pos: &NamePosition<'_>,
    ) -> Result<Vec<CompletionItem>> {
        let bindings = sem.bindings(node.clone())?;
        let mut pkgs = definition::PackageExports::default();
        let mut items = Vec::new();
        for (name, binding) in &bindings {
            if !name_matches(name, pos.prefix) {
                continue;
            }
            let val = match &binding.value {
                Some(val) => Some(val.clone()),
                None if binding.tola => sem
                    .trace(binding.span)?
                    .into_iter()
                    .next()
                    .map(|(val, _)| val),
                None => None,
            };
            let decl = declared(sem, &bindings, binding.span);
            let mut item = self.name_item(name, &known_types(val.as_ref()), pos)?;
            attach_value_docs(&mut item, sem, binding.span, val.as_ref());
            if val.is_none()
                && let Some(decl) = &decl
            {
                item.kind = Some(definition::declaration_kind(
                    &decl.declaration().initializer,
                ));
            }
            let shape = match &decl {
                Some(decl) => definition::attach_declaration(
                    &mut item,
                    &decl.names,
                    decl.declaration(),
                    &mut pkgs,
                    sem.cancellation(),
                )?,
                None => None,
            };
            let callable = shape.or_else(|| {
                val.as_ref()
                    .and_then(|v| callable::Callable::of_value(v, callable::Receiver::Unbound))
            });
            items.extend(callable::name_items(item, callable, pos));
        }
        Ok(items)
    }

    fn library_name_items(
        &self,
        sem: &mut Semantic<'_>,
        node: &LinkedNode<'_>,
        pos: &NamePosition<'_>,
        seen: &mut BTreeSet<String>,
    ) -> Result<Vec<CompletionItem>> {
        let mut items = Vec::new();
        for (name, val) in sem.library_bindings(node) {
            if !name_matches(&name, pos.prefix) || !seen.insert(name.clone()) {
                continue;
            }
            let mut item = self.name_item(&name, &[ExpressionType::Known(val.clone())], pos)?;
            attach_value_docs(&mut item, sem, Span::detached(), Some(&val));
            items.extend(callable::name_items(
                item,
                callable::Callable::of_value(&val, callable::Receiver::Unbound),
                pos,
            ));
        }
        Ok(items)
    }

    pub(super) fn import_completion(
        &self,
        prep: &Source,
        sem: &mut Semantic<'_>,
        import: &tola_typst_syntax::syntax::Import,
    ) -> Result<Vec<CompletionItem>> {
        let Some(replacement) = &import.replacement else {
            return Ok(Vec::new());
        };
        let Some(src) = tola_typst_syntax::syntax::node_at_range(prep, &import.source)
            .or_else(|| tola_typst_syntax::syntax::node_at_range(self.source, &import.source))
        else {
            return Ok(Vec::new());
        };
        let pos = self.name_position(replacement.clone())?;
        let mut items = Vec::new();
        if import.imported.is_empty() && "*".starts_with(pos.prefix) {
            items.push(wildcard_import_item(pos.edit("*".to_owned())?));
        }
        for (name, binding) in sem.import_bindings(&src, &import.path)? {
            if !name_matches(&name, pos.prefix) || already_imported(import, &name) {
                continue;
            }
            let mut item = self.name_item(&name, &known_types(binding.value.as_ref()), &pos)?;
            attach_value_docs(&mut item, sem, binding.span, binding.value.as_ref());
            items.push(item);
        }
        Ok(items)
    }

    pub(super) fn member_completion(
        &self,
        prep: &Source,
        sem: &mut Semantic<'_>,
        member: &tola_typst_syntax::syntax::Member,
    ) -> Result<Vec<CompletionItem>> {
        let Some(target) = tola_typst_syntax::syntax::node_at_range(prep, &member.target)
            .or_else(|| tola_typst_syntax::syntax::node_at_range(self.source, &member.target))
        else {
            return Ok(Vec::new());
        };
        let fields = sem.member_types(&target)?;
        let receivers = sem.known_values(&target)?;
        let names = self.names_of(self.source);
        let (root, path) = match field_chain(&target) {
            Some((root, path)) => (root, path),
            None => (target.clone(), Vec::new()),
        };
        let declared = if path
            .first()
            .is_some_and(|field| field == tola_build::SourceDescriptorField::Metadata.name())
        {
            self.parsed_schema(&names, root.clone(), sem)?
                .and_then(|s| s.at_path(&path[1..]))
        } else {
            None
        };
        let descriptors = if path.is_empty() {
            record_origin(&names, root, sem)?.and_then(|o| o.descriptors())
        } else {
            None
        };
        let mut declared_fields = declared
            .as_ref()
            .map(schema::SchemaDescription::fields)
            .unwrap_or_default();
        let receiver = callable::receiver_binds(
            receivers
                .iter()
                .map(|(r, _)| !matches!(r, Value::Module(_) | Value::Type(_))),
        );
        let pos = self.name_position(member.replacement.clone())?;
        let mut items = Vec::new();
        for (name, vals) in fields
            .into_iter()
            .filter(|(name, _)| name_matches(name, pos.prefix))
        {
            let mut item = self.name_item(&name, &vals, &pos)?;
            let val = agreed(vals.iter().map(ExpressionType::value)).flatten();
            let span = receivers
                .iter()
                .filter_map(|(r, _)| r.scope()?.get(&name))
                .find(|b| Some(b.read()) == val)
                .map_or_else(Span::detached, |b| b.span());
            attach_value_docs(&mut item, sem, span, val);
            if let Some(idx) = declared_fields.iter().position(|(f, _)| f == &name) {
                let (_, field) = declared_fields.remove(idx);
                item.documentation = Some(markdown_documentation(field.field(&name).section()));
            }
            let callable = receiver.and_then(|r| callable::Callable::of_types(&vals, r));
            items.extend(callable::name_items(item, callable, &pos));
        }
        for (name, field) in declared_fields {
            if !name_matches(&name, pos.prefix) {
                continue;
            }
            let mut item = self.name_item(&name, &[], &pos)?;
            item.label_details = Some(labelled_type(field.ty()));
            item.documentation = Some(markdown_documentation(field.field(&name).section()));
            items.push(item);
        }
        if let Some(descriptors) = descriptors {
            for field in descriptors.fields() {
                let name = field.name();
                if !name_matches(name, pos.prefix) || items.iter().any(|i| i.label == name) {
                    continue;
                }
                let mut item = self.name_item(name, &[], &pos)?;
                item.label_details = Some(labelled_type(Some(
                    descriptor_type(field.kind()).to_owned(),
                )));
                item.documentation = Some(markdown_documentation(descriptor_line(field)));
                items.push(item);
            }
        }
        Ok(items)
    }

    pub(super) fn meta_completion(
        &self,
        comp: &SourceCompilation,
        cfg: &SiteConfig,
        sem: &mut Semantic<'_>,
    ) -> Result<Option<Vec<CompletionItem>>> {
        let Some(call) = meta_call(self.source, self.cursor, &self.names_of(self.source)) else {
            return Ok(None);
        };
        let Some(key) = meta_key(self.source, self.cursor, &call) else {
            return Ok(None);
        };
        let schemas = self.meta_schemas(comp, cfg, sem)?;
        if schemas.shapes.is_empty() {
            return Ok(None);
        }
        let pos = self.name_position(key.typed.clone())?;
        let mut declared: BTreeMap<String, DeclaredKey> = BTreeMap::new();
        for schema in &schemas.shapes {
            for (name, field) in schema.shape.fields() {
                let entry = declared.entry(name.clone()).or_insert_with(|| DeclaredKey {
                    ty: field.ty(),
                    fields: Vec::new(),
                });
                entry
                    .fields
                    .push((field.field(&name), schema.declaration()));
            }
        }
        let mut items = Vec::new();
        for (name, declared) in declared {
            if !name_matches(&name, pos.prefix) || key.written.contains(&name) {
                continue;
            }
            let rejecting = schemas
                .shapes
                .iter()
                .filter(|s| schemas.covered && s.rejects(&name))
                .count();
            let required = schemas.shapes.iter().any(|s| s.requires(&name));
            if rejecting > 0 && !required {
                continue;
            }
            let mut docs = declared_documentation(&declared.fields, &schemas);
            if rejecting > 0 {
                docs.push_str(&format!(
                    "{SECTION_JOIN}{}",
                    rejected_key_note(rejecting, required)
                ));
            }
            let mut item = semantic::completion(
                &name,
                &[],
                pos.edit(crate::completion::typst_snippet(&format!("{name}: ${{}}")))?,
            );
            item.label_details = Some(labelled_type(declared.ty));
            item.documentation = Some(markdown_documentation(docs));
            item.insert_text_format = Some(InsertTextFormat::SNIPPET);
            items.push(item);
        }
        // Return None when empty so general names/labels/citations/postfix still answer.
        Ok((!items.is_empty()).then_some(items))
    }

    fn name_item(
        &self,
        name: &str,
        types: &[ExpressionType],
        pos: &NamePosition<'_>,
    ) -> Result<CompletionItem> {
        Ok(semantic::completion(
            name,
            types,
            self.completion_edit(pos.replacement.clone(), name.to_owned())?,
        ))
    }

    fn name_position(&self, replacement: Range<usize>) -> Result<NamePosition<'_>> {
        let (_, prefix) = self.completion_range(replacement.clone())?;
        Ok(NamePosition {
            source: self.source,
            cursor: self.cursor,
            prefix,
            replacement,
        })
    }

    pub(super) fn completion_edit(
        &self,
        replacement: Range<usize>,
        new_text: String,
    ) -> Result<CompletionTextEdit> {
        completion_edit(self.source, self.cursor, replacement, new_text)
    }

    pub(super) fn completion_range(
        &self,
        replacement: Range<usize>,
    ) -> Result<(lsp_types::Range, &str)> {
        let range = position::utf16_range(self.source.lines(), replacement.clone())
            .context("invalid completion range")?;
        Ok((range, typed_text(self.source, &replacement, self.cursor)))
    }

    pub(super) fn parameter_completion(
        &self,
        sem: &mut Semantic<'_>,
    ) -> Result<Vec<CompletionItem>> {
        let prev = LinkedNode::new(self.source.root()).leaf_at(self.cursor, Side::Before);
        let after_sep = prev.is_some_and(|node| {
            matches!(
                node.kind(),
                SyntaxKind::LeftParen | SyntaxKind::Comma | SyntaxKind::Error
            )
        });
        if !after_sep {
            return Ok(Vec::new());
        }
        let Some(call) = tola_typst_syntax::syntax::call(self.source, self.cursor) else {
            return Ok(Vec::new());
        };
        let funcs = match &call.target {
            tola_typst_syntax::syntax::CallTarget::Function => {
                let Some(callee) =
                    tola_typst_syntax::syntax::node_at_range(self.source, &call.callee)
                else {
                    return Ok(Vec::new());
                };
                sem.callee_functions(&callee)?
            }
            tola_typst_syntax::syntax::CallTarget::Field { receiver, field } => {
                let Some(receiver) =
                    tola_typst_syntax::syntax::node_at_range(self.source, receiver)
                else {
                    return Ok(Vec::new());
                };
                sem.field_functions(&receiver, &self.source.text()[field.clone()])?
            }
        };
        let call_node = tola_typst_syntax::syntax::node_at_range(
            self.source,
            &(call.callee.start..call.suffix.end),
        );
        let taken: BTreeSet<String> = call_node
            .as_ref()
            .and_then(|node| node.cast::<ast::FuncCall>())
            .map(|call| {
                call.args()
                    .items()
                    .filter_map(|arg| match arg {
                        ast::Arg::Named(named) => Some(named.name().get().to_string()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let (range, _) = self.completion_range(self.cursor..self.cursor)?;
        Ok(semantic::parameter_completions(sem, &funcs, &taken, range))
    }
}
