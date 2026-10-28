//! What one completion offers at the cursor: names, members, import items, metadata keys, and the
//! parameters a call can still fill; and the signature one call stands in.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use anyhow::{Context, Result};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionResponse,
    CompletionTextEdit, Documentation, InsertReplaceEdit, InsertTextFormat, MarkupContent,
    MarkupKind, TextEdit,
};
use tola_build::check::SourceCompilation;
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::foundations::Value;
use tola_typst::typst::syntax::ast;
use tola_typst::typst::syntax::{LinkedNode, Side, Source, Span, SyntaxKind};
use tola_typst_syntax::names::{Initializer, SourceNames};

use crate::markdown;
use crate::position;
use crate::protocol::SourceReply;

mod callable;
mod definition;
mod source;

pub(crate) use source::{
    source_completion_reply, source_import_completion, source_member_completion,
    source_member_graph_needed, source_name_completion, source_name_graph_needed,
    source_name_position,
};

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

/// The type one completion item shows beside its label.
fn labelled_type(ty: Option<String>) -> CompletionItemLabelDetails {
    CompletionItemLabelDetails {
        detail: None,
        description: ty,
    }
}
/// Whether the cursor stands where a path is written: inside an import or include statement, or
/// in a reader call's path argument.
///
/// A path position names a file or a package, never the file's own bindings, so completion reads
/// the author's own source there: a repair blanks the statement a query copy compiles without.
fn is_path_position(source: &Source, cursor: usize) -> bool {
    if tola_typst_syntax::syntax::path_argument(source, cursor).is_some() {
        return true;
    }
    tola_typst_syntax::syntax::cursor_leaves(source, cursor)
        .into_iter()
        .flatten()
        .any(|leaf| {
            let mut node = leaf;
            loop {
                if matches!(
                    node.kind(),
                    SyntaxKind::ModuleImport | SyntaxKind::ModuleInclude
                ) {
                    return true;
                }
                match node.parent() {
                    Some(parent) => node = parent.clone(),
                    None => return false,
                }
            }
        })
}
/// Whether the cursor stands in text no name completion can answer.
pub(in crate::query) fn opaque_at(source: &Source, cursor: usize) -> bool {
    let mut current = LinkedNode::new(source.root()).leaf_at(cursor, Side::Before);
    while let Some(node) = current {
        if matches!(
            node.kind(),
            SyntaxKind::LineComment | SyntaxKind::BlockComment | SyntaxKind::Str | SyntaxKind::Raw
        ) {
            return true;
        }
        // An interpolation inside a string or raw block is code, not the text around it.
        if !matches!(
            node.kind(),
            SyntaxKind::Text | SyntaxKind::Str | SyntaxKind::Raw
        ) && node.cast::<ast::Expr>().is_some()
        {
            return false;
        }
        current = node.parent().cloned();
    }
    false
}

/// Whether the cursor stands in a raw block, which has no code names to offer.
pub(crate) fn raw_at(source: &Source, cursor: usize) -> bool {
    let mut current = LinkedNode::new(source.root()).leaf_at(cursor, Side::Before);
    while let Some(node) = current {
        match node.kind() {
            SyntaxKind::Raw => return true,
            SyntaxKind::Str | SyntaxKind::LineComment | SyntaxKind::BlockComment => return false,
            _ => {}
        }
        current = node.parent().cloned();
    }
    false
}

/// Whether the cursor stands inside a comment, where only the documentation template answers.
pub(in crate::query) fn comment_at(source: &Source, cursor: usize) -> bool {
    LinkedNode::new(source.root())
        .leaf_at(cursor, Side::Before)
        .is_some_and(|leaf| {
            matches!(
                leaf.kind(),
                SyntaxKind::LineComment | SyntaxKind::BlockComment
            )
        })
}

/// The completion a `///` comment above a function definition offers.
///
/// The template names the closure's own parameters, so an author writes the descriptions where
/// the ecosystem reads them; it completes the comment line and inserts at the cursor.
pub(in crate::query) fn documentation_template(
    source: &Source,
    cursor: usize,
) -> Option<SourceReply> {
    let leaf = LinkedNode::new(source.root()).leaf_at(cursor, Side::Before)?;
    if leaf.kind() != SyntaxKind::LineComment || leaf.range().end != cursor {
        return None;
    }
    // The comment is empty and finished: `///` or `/// ` with the cursor at its end.
    let space = match leaf.leaf_text().as_str() {
        "///" => " ",
        "/// " => "",
        _ => return None,
    };
    // The next line defines a function: a `#let` binding whose value is a closure.
    let hash = next_node(&leaf)?;
    if hash.kind() != SyntaxKind::Hash {
        return None;
    }
    let between = &source.text()[leaf.range().end..hash.range().start];
    if between
        .chars()
        .filter(|character| *character == '\n')
        .count()
        > 1
    {
        return None;
    }
    let binding = next_node(&hash)?;
    let closure = match binding.cast::<ast::LetBinding>()?.init() {
        Some(ast::Expr::Closure(closure)) => closure,
        _ => return None,
    };
    // The first stop is the line the author's comment ends on, so the block begins there; the
    // return line is the last stop.
    let mut snippet = format!("{space}$0\n///");
    let mut next = 1;
    for parameter in closure.params().children() {
        let name = match parameter {
            ast::Param::Pos(ast::Pattern::Normal(ast::Expr::Ident(ident))) => {
                ident.get().to_string()
            }
            ast::Param::Named(named) => named.name().get().to_string(),
            ast::Param::Spread(spread) => match spread.sink_ident() {
                Some(ident) => ident.get().to_string(),
                None => continue,
            },
            _ => continue,
        };
        snippet.push_str(&format!("\n/// - {name} (${next}): ${}", next + 1));
        next += 2;
    }
    snippet.push_str(&format!("\n/// -> ${next}"));
    let range = position::utf16_range(source.lines(), cursor..cursor)?;
    Some(SourceReply::Completion(CompletionResponse::Array(vec![
        CompletionItem {
            label: "Document function".to_owned(),
            kind: Some(CompletionItemKind::CONSTANT),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: snippet,
            })),
            ..CompletionItem::default()
        },
    ])))
}

/// Whether one name can still complete what the author typed: every typed character appears in
/// the name, in order. A `-` is part of a kebab-case name, so `al-` still reaches `all-sources`,
/// and a name whose letters are spread out (`m07` for `marker_000_07`) stays reachable.
pub(super) fn name_matches(name: &str, prefix: &str) -> bool {
    let mut spelled = name.chars();
    prefix
        .chars()
        .all(|typed| spelled.by_ref().any(|candidate| candidate == typed))
}

/// The name bytes one cursor has already typed; a name completion has to start with them.
pub(super) fn typed_prefix(source: &Source, cursor: usize) -> &str {
    let Some(leaf) = LinkedNode::new(source.root()).leaf_at(cursor, Side::Before) else {
        return "";
    };
    let range = match leaf.kind() {
        SyntaxKind::Ident | SyntaxKind::MathIdent => leaf.range(),
        _ => return "",
    };
    &source.text()[range.start..cursor]
}

/// The next node after `node` at any depth: its next sibling, or that of its nearest ancestor
/// that has one.
fn next_node<'a>(node: &LinkedNode<'a>) -> Option<LinkedNode<'a>> {
    let mut current = node.clone();
    loop {
        if let Some(sibling) = current.next_sibling() {
            return Some(sibling);
        }
        current = current.parent()?.clone();
    }
}
/// One completion item's documentation, as the markdown an editor renders.
pub(in crate::query) fn markdown_documentation(text: impl Into<String>) -> Documentation {
    Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: markdown::docs(&text.into()),
    })
}

/// The declaration the site's source binds one name by, following the name an alias spells.
fn declared(
    semantics: &mut Semantic<'_>,
    bindings: &BTreeMap<String, semantic::Binding>,
    span: Span,
) -> Option<(Arc<SourceNames>, usize)> {
    let mut span = span;
    let mut followed = Vec::new();
    loop {
        let range = semantics.span_range(span)?;
        let names = semantics.source_names(span.id()?)?;
        let declaration = names.declared_at(range.start)?;
        // An alias names another binding; that binding's declaration is what the author reads.
        if let Initializer::Alias(path) = &names.declarations()[declaration].initializer
            && let Some(root) = path.segments.first()
            && let Some(target) = bindings.get(names.text(root))
            && !followed.contains(&target.span)
        {
            followed.push(target.span);
            span = target.span;
            continue;
        }
        return Some((names, declaration));
    }
}

/// Leaves documentation the completed value already supplied untouched.
fn attach_value_docs(
    item: &mut CompletionItem,
    semantics: &Semantic<'_>,
    span: Span,
    value: Option<&Value>,
) {
    if item.documentation.is_none() {
        item.documentation = semantics
            .value_docs(span, value)
            .map(markdown_documentation);
    }
}
impl Selection<'_> {
    /// The names, members, import items, and parameters one completion position offers.
    ///
    /// The edit's range and prefix come from the author's own text: the query copy the world
    /// compiled may have repaired an unfinished construct into a different one, and it is the
    /// author's text the returned edit rewrites. A node whose evaluated value answers the
    /// request — a member's receiver, an import's module — is resolved on the world's copy
    /// first, whose parse the traced spans belong to, and falls back to the author's own parse
    /// for a construct the repair dropped; the repairs keep byte offsets aligned, so a
    /// fallback node still answers through its binding and its range.
    pub(super) fn completion(
        &self,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
    ) -> Result<Vec<CompletionItem>> {
        if let SelectedSyntax::Import(import) = &self.syntax {
            return self.import_completion(prepared, semantics, import);
        }
        if let SelectedSyntax::Member(member) = &self.syntax {
            return self.member_completion(prepared, semantics, member);
        }
        // A path position names a file or a package, never the file's own bindings: the editor
        // layer answers paths there, and the package lane answers names.
        if is_path_position(self.source, self.cursor) {
            return Ok(Vec::new());
        }
        let parameters = self.parameter_completion(semantics)?;
        let Some(node) = tola_typst_syntax::syntax::expression(self.source, self.cursor)
            .or_else(|| tola_typst_syntax::syntax::before(self.source, self.cursor))
        else {
            return Ok(parameters);
        };
        let replacement = if node.kind() == SyntaxKind::Ident {
            node.range()
        } else {
            self.cursor..self.cursor
        };
        let (_, prefix) = self.completion_range(replacement.clone())?;
        let mut items = Vec::new();
        let mut packages = BTreeMap::new();
        let bindings = semantics.bindings(node.clone())?;
        for (name, binding) in &bindings {
            if !name_matches(name, prefix) {
                continue;
            }
            let value = match &binding.value {
                Some(value) => Some(value.clone()),
                // Only a Tola binding's value is traced here; an ordinary name answers by label.
                None if binding.tola => semantics
                    .trace(binding.span)?
                    .into_iter()
                    .next()
                    .map(|(value, _)| value),
                None => None,
            };
            let declaration = declared(semantics, &bindings, binding.span);
            let mut item = semantic::completion(
                name,
                &value
                    .iter()
                    .cloned()
                    .map(ExpressionType::Known)
                    .collect::<Vec<_>>(),
                self.completion_edit(replacement.clone(), name.clone())?,
            );
            attach_value_docs(&mut item, semantics, binding.span, value.as_ref());
            if value.is_none()
                && let Some((names, declaration)) = &declaration
            {
                // No value reached this name, so the kind its declaration promises is the only
                // kind the editor can show.
                item.kind = Some(definition::declaration_kind(
                    &names.declarations()[*declaration].initializer,
                ));
            }
            let shape = match &declaration {
                Some((names, declaration)) => definition::attach_declaration(
                    &mut item,
                    names,
                    &names.declarations()[*declaration],
                    &mut packages,
                    semantics.cancellation(),
                )?,
                None => None,
            };
            let callable = shape.or_else(|| {
                value.as_ref().and_then(|value| {
                    callable::Callable::of_value(value, callable::Receiver::Unbound)
                })
            });
            match callable {
                Some(callable) => items.extend(callable::callable_items(
                    item,
                    self.source,
                    self.cursor,
                    &replacement,
                    callable,
                )),
                None => items.push(item),
            }
        }
        // A library name the file did not bind still answers, even when the site failed before
        // evaluating this position; the file's own bindings above shadow it.
        let mut seen: std::collections::BTreeSet<String> =
            items.iter().map(|item| item.label.clone()).collect();
        for (name, value) in semantics.library_bindings(&node) {
            if !name_matches(&name, prefix) || !seen.insert(name.clone()) {
                continue;
            }
            let mut item = semantic::completion(
                &name,
                &[ExpressionType::Known(value.clone())],
                self.completion_edit(replacement.clone(), name.clone())?,
            );
            attach_value_docs(&mut item, semantics, Span::detached(), Some(&value));
            match callable::Callable::of_value(&value, callable::Receiver::Unbound) {
                Some(callable) => items.extend(callable::callable_items(
                    item,
                    self.source,
                    self.cursor,
                    &replacement,
                    callable,
                )),
                None => items.push(item),
            }
        }
        items.extend(
            parameters
                .into_iter()
                .filter(|item| seen.insert(item.label.clone())),
        );
        Ok(items)
    }

    pub(super) fn import_completion(
        &self,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
        import: &tola_typst_syntax::syntax::Import,
    ) -> Result<Vec<CompletionItem>> {
        let Some(replacement) = &import.replacement else {
            return Ok(Vec::new());
        };
        // The module node answers by its value, so the world's parse comes first; a repair that
        // dropped the statement leaves only the author's own node, which still answers by
        // offsets.
        let Some(source) = tola_typst_syntax::syntax::node_at_range(prepared, &import.source)
            .or_else(|| tola_typst_syntax::syntax::node_at_range(self.source, &import.source))
        else {
            return Ok(Vec::new());
        };
        let (_, prefix) = self.completion_range(replacement.clone())?;
        let mut items = Vec::new();
        if import.imported.is_empty() && "*".starts_with(prefix) {
            let mut item = semantic::completion(
                "*",
                &[],
                self.completion_edit(replacement.clone(), "*".to_owned())?,
            );
            item.kind = Some(CompletionItemKind::SNIPPET);
            item.detail = Some("Import everything.".to_owned());
            items.push(item);
        }
        for (name, binding) in semantics.import_bindings(&source, &import.path)? {
            if !name_matches(&name, prefix)
                || import.imported.iter().any(|path| {
                    path.len() == import.path.len() + 1
                        && path[..import.path.len()] == import.path
                        && path.last() == Some(&name)
                })
            {
                continue;
            }
            let mut item = semantic::completion(
                &name,
                &binding
                    .value
                    .iter()
                    .cloned()
                    .map(ExpressionType::Known)
                    .collect::<Vec<_>>(),
                self.completion_edit(replacement.clone(), name.clone())?,
            );
            attach_value_docs(&mut item, semantics, binding.span, binding.value.as_ref());
            items.push(item);
        }
        Ok(items)
    }

    pub(super) fn member_completion(
        &self,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
        member: &tola_typst_syntax::syntax::Member,
    ) -> Result<Vec<CompletionItem>> {
        // The receiver answers by its value, so the world's parse comes first; a repair that
        // dropped the statement leaves only the author's own node, whose binding still resolves
        // its value by offset.
        let Some(target) = tola_typst_syntax::syntax::node_at_range(prepared, &member.target)
            .or_else(|| tola_typst_syntax::syntax::node_at_range(self.source, &member.target))
        else {
            return Ok(Vec::new());
        };
        let fields = semantics.member_types(&target)?;
        let receivers = semantics.known_values(&target)?;
        let names = self.names_of(self.source);
        let (root, path) = match field_chain(&target) {
            Some((root, path)) => (root, path),
            None => (target.clone(), Vec::new()),
        };
        let declared = if path
            .first()
            .is_some_and(|field| field == tola_build::SourceDescriptorField::Metadata.name())
        {
            self.parsed_schema(&names, root.clone(), semantics)?
                .and_then(|schema| schema.at_path(&path[1..]))
        } else {
            None
        };
        // A member read on the site's own descriptors answers the descriptor's declared fields,
        // read from the one authority every producer builds them from.
        let descriptors = if path.is_empty() {
            record_origin(&names, root, semantics)?.and_then(|origin| origin.descriptors())
        } else {
            None
        };
        let mut declared_fields = declared
            .as_ref()
            .map(schema::OutputDescription::fields)
            .unwrap_or_default();
        // A member of a value supplies its `self`; a module's or a type's member takes the
        // receiver as its first argument.
        let receiver = callable::receiver_binds(
            receivers
                .iter()
                .map(|(receiver, _)| !matches!(receiver, Value::Module(_) | Value::Type(_))),
        );
        let (_, prefix) = self.completion_range(member.replacement.clone())?;
        let mut items = Vec::new();
        for (name, values) in fields
            .into_iter()
            .filter(|(name, _)| name_matches(name, prefix))
        {
            let mut item = semantic::completion(
                &name,
                &values,
                self.completion_edit(member.replacement.clone(), name.clone())?,
            );
            let value = values.first().and_then(ExpressionType::value);
            let span = receivers
                .iter()
                .filter_map(|(receiver, _)| receiver.scope()?.get(&name))
                .find(|binding| Some(binding.read()) == value)
                .map_or_else(tola_typst::typst::syntax::Span::detached, |binding| {
                    binding.span()
                });
            attach_value_docs(&mut item, semantics, span, value);
            if let Some(index) = declared_fields.iter().position(|(field, _)| field == &name) {
                let (_, field) = declared_fields.remove(index);
                item.documentation = Some(markdown_documentation(field.field(&name).section()));
            }
            let callable = receiver.and_then(|receiver| {
                values.iter().find_map(|ty| {
                    ty.value()
                        .and_then(|value| callable::Callable::of_value(value, receiver))
                })
            });
            match callable {
                Some(callable) => items.extend(callable::callable_items(
                    item,
                    self.source,
                    self.cursor,
                    &member.replacement,
                    callable,
                )),
                None => items.push(item),
            }
        }
        for (name, field) in declared_fields {
            if !name_matches(&name, prefix) {
                continue;
            }
            let mut item = semantic::completion(
                &name,
                &[],
                self.completion_edit(member.replacement.clone(), name.clone())?,
            );
            item.label_details = Some(labelled_type(field.ty()));
            item.documentation = Some(markdown_documentation(field.field(&name).section()));
            items.push(item);
        }
        if let Some(descriptors) = descriptors {
            for field in descriptors.fields() {
                let name = field.name();
                if !name_matches(name, prefix) || items.iter().any(|item| item.label == name) {
                    continue;
                }
                let mut item = semantic::completion(
                    name,
                    &[],
                    self.completion_edit(member.replacement.clone(), name.to_owned())?,
                );
                item.label_details = Some(labelled_type(Some(
                    descriptor_type(field.kind()).to_owned(),
                )));
                item.documentation = Some(markdown_documentation(descriptor_line(field)));
                items.push(item);
            }
        }
        Ok(items)
    }

    /// The completion a `#tola-meta` dictionary answers: the keys the site's schemas declare.
    pub(super) fn meta_completion(
        &self,
        compilation: &SourceCompilation,
        config: &ResolvedSiteConfig,
        semantics: &mut Semantic<'_>,
    ) -> Result<Option<Vec<CompletionItem>>> {
        let Some(call) = meta_call(self.source, self.cursor, &self.names_of(self.source)) else {
            return Ok(None);
        };
        let Some(key) = meta_key(self.source, self.cursor, &call) else {
            return Ok(None);
        };
        let schemas = self.meta_schemas(compilation, config, semantics)?;
        if schemas.shapes.is_empty() {
            return Ok(None);
        }
        let (_, prefix) = self.completion_range(key.typed.clone())?;
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
            if !name_matches(&name, prefix) || key.written.contains(&name) {
                continue;
            }
            // Every call parsing this source must accept the key: one that neither declares it nor
            // keeps unknown keys rejects it, so the key is offered only where no call rejects it or
            // another call requires it, in which case the conflict is named.
            let rejecting = schemas
                .shapes
                .iter()
                .filter(|schema| !schema.declares(&name) && !schema.accepts_unknown())
                .count();
            let required = schemas.shapes.iter().any(|schema| schema.requires(&name));
            if rejecting > 0 && !required {
                continue;
            }
            let mut documentation = declared_documentation(&declared.fields, &schemas);
            if rejecting > 0 {
                documentation.push_str(&format!(
                    "{SECTION_JOIN}{}",
                    rejected_key_note(rejecting, required)
                ));
            }
            let mut item = semantic::completion(
                &name,
                &[],
                self.completion_edit(
                    key.typed.clone(),
                    crate::completion::typst_snippet(&format!("{name}: ${{}}")),
                )?,
            );
            item.label_details = Some(labelled_type(declared.ty));
            item.documentation = Some(markdown_documentation(documentation));
            item.insert_text_format = Some(InsertTextFormat::SNIPPET);
            items.push(item);
        }
        // A completion that offers nothing must not take the request: the general path still
        // answers names, labels, citations, and postfix wraps at this cursor.
        Ok((!items.is_empty()).then_some(items))
    }
    /// The edit one completion item has.
    ///
    /// The whole token the item replaces, and the prefix the cursor has already typed: a client
    /// that reads insert-and-replace edits keeps the text after the cursor while the item is
    /// inserted, and replaces the token when it is accepted. Both ranges coincide at the end of a
    /// name, where a plain edit says the same.
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
        Ok((range, &self.source.text()[replacement.start..self.cursor]))
    }

    /// The named parameters a call at the cursor can still fill, resolved through the same function
    /// metadata signature help reads. The editor's own reader answers a library callee, but a
    /// member or imported callee only the world resolves.
    pub(super) fn parameter_completion(
        &self,
        semantics: &mut Semantic<'_>,
    ) -> Result<Vec<CompletionItem>> {
        let previous = LinkedNode::new(self.source.root()).leaf_at(self.cursor, Side::Before);
        if !matches!(
            previous.as_ref().map(|node| node.kind()),
            Some(SyntaxKind::LeftParen | SyntaxKind::Comma | SyntaxKind::Error)
        ) {
            return Ok(Vec::new());
        }
        let Some(call) = tola_typst_syntax::syntax::call(self.source, self.cursor) else {
            return Ok(Vec::new());
        };
        let functions = match &call.target {
            tola_typst_syntax::syntax::CallTarget::Function => {
                let Some(callee) =
                    tola_typst_syntax::syntax::node_at_range(self.source, &call.callee)
                else {
                    return Ok(Vec::new());
                };
                semantics.callee_functions(&callee)?
            }
            tola_typst_syntax::syntax::CallTarget::Field { receiver, field } => {
                let Some(receiver) =
                    tola_typst_syntax::syntax::node_at_range(self.source, receiver)
                else {
                    return Ok(Vec::new());
                };
                semantics.field_functions(&receiver, &self.source.text()[field.clone()])?
            }
        };
        let call_node = tola_typst_syntax::syntax::node_at_range(
            self.source,
            &(call.callee.start..call.suffix.end),
        );
        let taken: std::collections::BTreeSet<String> = call_node
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
        Ok(semantic::parameter_completions(
            semantics, &functions, &taken, range,
        ))
    }
}

pub(super) fn completion_edit(
    source: &Source,
    cursor: usize,
    replacement: Range<usize>,
    new_text: String,
) -> Result<CompletionTextEdit> {
    let replace =
        position::utf16_range(source.lines(), replacement.clone()).context("invalid range")?;
    let insert = position::utf16_range(
        source.lines(),
        replacement.start..cursor.min(replacement.end),
    )
    .context("invalid range")?;
    Ok(if insert == replace {
        CompletionTextEdit::Edit(TextEdit {
            range: replace,
            new_text,
        })
    } else {
        CompletionTextEdit::InsertAndReplace(InsertReplaceEdit {
            insert,
            replace,
            new_text,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::tests::*;
    use serde_json::json;

    /// A half-typed call the site cannot yet compile still completes the members of what it returns.
    #[test]
    fn incomplete_call_completes_its_members() {
        let mut site = QuerySession::two_documents();
        let items = site.completion(
            r#"#import "@tola/document:0.0.0": current-document
    #context current-document().|
    #metadata((title: [Later])) <tola-meta>"#,
        );
        for field in ["output", "route", "location"] {
            let labels = completion_labels(&items);
            assert!(labels.contains(&field), "{labels:?}");
        }
        let output = item_labeled(&items, "output")
            .label_details
            .as_ref()
            .and_then(|details| details.description.as_deref())
            .unwrap();
        assert!(
            output.contains("a.html") && output.contains("b.html"),
            "{output}"
        );
        assert!(
            !site
                .config
                .build()
                .content_dir
                .join("document.typ")
                .exists()
        );
        assert!(!site.config.build().publish_dir.exists());
        assert!(!site.site.path(".tola/builtin-packages").exists());
    }

    #[test]
    fn import_aliases_resolve_without_shadowing() {
        let mut site = QuerySession::new();
        let items = site.completion(
            r#"#import "@tola/document:0.0.0": current-document as identity
    #let current-document() = (not-native: true)
    #context identity().|"#,
        );
        let alias = completion_labels(&items);
        assert!(alias.contains(&"output"), "{alias:?}");
        assert!(!alias.contains(&"not-native"), "{alias:?}");

        // A shadowed name is ordinary site code: it answers from the world, never as Tola's
        // own value.
        let shadow = site.completion(
            r#"#import "@tola/document:0.0.0": current-document
    #let render(current-document) = [#context current-document().|]
    #render(() => (custom: 7))"#,
        );
        assert!(
            !completion_labels(&shadow).contains(&"output"),
            "{shadow:?}"
        );
    }
    /// An item completed inside a name keeps the text after the cursor while it is inserted, and
    /// replaces the whole name when it is accepted.
    #[test]
    fn completion_edit_separates_insert_from_replace() {
        let mut site = QuerySession::new();
        let completion = site.reply(
            "textDocument/completion",
            "#let keeper = 1\n#let k = kee|per\n",
        );
        let item = completion
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["label"] == "keeper")
            .expect("a `keeper` completion");
        assert_eq!(
            item["textEdit"]["insert"],
            json!({ "start": { "line": 1, "character": 9 }, "end": { "line": 1, "character": 12 } }),
            "{item}"
        );
        assert_eq!(
            item["textEdit"]["replace"],
            json!({ "start": { "line": 1, "character": 9 }, "end": { "line": 1, "character": 15 } }),
            "{item}"
        );
        assert_eq!(item["textEdit"]["newText"], "keeper");

        // At the end of the name both ranges coincide, and one plain edit says the same.
        let completion = site.reply(
            "textDocument/completion",
            "#let keeper = 1\n#let k = keeper|\n",
        );
        let item = completion
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["label"] == "keeper")
            .expect("a `keeper` completion");
        assert_eq!(
            item["textEdit"]["range"],
            json!({ "start": { "line": 1, "character": 9 }, "end": { "line": 1, "character": 15 } }),
            "{item}"
        );
        assert!(item["textEdit"].get("insert").is_none(), "{item}");
    }

    /// A program the site cannot compile still completes the imports the author is writing.
    #[test]
    fn broken_programs_still_complete_imports() {
        let mut site = QuerySession::new();
        let items = site.completion("#import \"@tola/do|\"\n#unknown_after_import");
        let completions = completion_labels(&items);
        assert!(
            completions.contains(&"@tola/document:0.0.0"),
            "{completions:?}"
        );
    }

    /// An empty `///` comment above a function definition completes the template that documents
    /// it: the parameters the closure declares, then a return line.
    #[test]
    fn empty_comment_completes_the_function_template() {
        let mut site = QuerySession::new();
        let items = site.completion("///|\n#let f(a, b) = a\n");
        let item = item_labeled(&items, "Document function");
        assert_eq!(
            item.text_edit,
            Some(CompletionTextEdit::Edit(TextEdit {
                range: lsp_types::Range::new(
                    lsp_types::Position::new(0, 3),
                    lsp_types::Position::new(0, 3),
                ),
                new_text: " $0\n///\n/// - a ($1): $2\n/// - b ($3): $4\n/// -> $5".to_owned(),
            }))
        );
        // A definition that binds no closure takes no template, and neither does a comment
        // separated from its definition by a blank line.
        for marked in ["///|\n#let x = 1\n", "///|\n\n#let f(a) = a\n"] {
            let items = site.completion(marked);
            assert!(
                items.iter().all(|item| item.label != "Document function"),
                "{marked:?} completed {:?}",
                completion_labels(&items)
            );
        }
    }

    #[test]
    fn ignored_files_do_not_complete() {
        let mut site = QuerySession::new();
        site.site.write("templates/page.typ", "");
        site.site.write("scratch/notes.typ", "");
        site.site.write(".gitignore", "scratch/\n");
        let items = site.completion("#include \"scr|\"\n");
        let labels: Vec<&str> = completion_labels(&items);
        assert!(
            !labels.iter().any(|label| label.contains("scratch")),
            "{items:?}"
        );
    }

    /// A call's arguments complete the named parameters its callee declares, however the callee
    /// resolves: a library call, a module member, a dictionary method, a local function.
    #[test]
    fn call_arguments_complete_named_parameters() {
        /// One completion case: the site files it needs, the marked source, and the labels the
        /// call's arguments must offer.
        type Case<'a> = (&'a [(&'a str, &'a str)], &'a str, &'a [&'a str]);
        let cases: [Case; 5] = [
            // A call the site cannot resolve yet must not silence its own parameter completion.
            (&[], "#align(|)\n", &["alignment"]),
            (&[], "#align(a|)\n", &["alignment"]),
            (
                &[
                    (
                        "content/shapes.typ",
                        "#let line(close: false, name: \"x\") = close\n",
                    ),
                    (
                        "content/draw.typ",
                        "#import \"shapes.typ\"\n#let draw = shapes\n",
                    ),
                ],
                "#import \"draw.typ\": draw\n#draw.line(|\n",
                &["close", "name"],
            ),
            (&[], "#let d = (a: 1)\n#d.at(|\n", &["default"]),
            (&[], "#let f(value: 1) = value\n#f(|\n", &["value"]),
        ];
        for (files, marked, expected) in cases {
            let mut site = QuerySession::new();
            for (relative, text) in files.iter().copied() {
                site.site.write(relative, text);
            }
            let items = site.completion(marked);
            let labels = completion_labels(&items);
            for label in expected {
                assert!(labels.contains(label), "{marked:?}: {labels:?}");
            }
        }
    }

    /// A reader's path argument is completed from the file that writes it, replacing only the
    /// argument itself rather than the quotation marks around it.
    #[test]
    fn reader_paths_replace_their_own_argument() {
        let mut site = QuerySession::new();
        site.site.write("assets/cover.svg", "<svg/>");
        site.site.write("data/site.json", "{}");
        let items = site.completion("#image(\"asse|\")\n");
        let item = items
            .iter()
            .find(|item| item.label.ends_with("assets/cover.svg"))
            .expect("the cover image");
        let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
            panic!("a plain path edit: {:?}", item.text_edit);
        };
        assert_eq!(edit.range.start.character, 8);
        assert_eq!(edit.range.end.character, 12);
        assert_eq!(edit.new_text, "../assets/cover.svg");

        let items = site.completion("#read(\"da|\")\n");
        assert!(
            completion_labels(&items)
                .iter()
                .any(|label| label.ends_with("data/site.json")),
            "{items:?}"
        );

        for marked in ["#image(\"asse|\nBody", "#image(\"asse|\nBody \"quoted\""] {
            let items = site.completion(marked);
            let item = items
                .iter()
                .find(|item| item.label.ends_with("assets/cover.svg"))
                .expect("the unfinished cover path");
            let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
                panic!("a plain path edit: {:?}", item.text_edit);
            };
            assert_eq!(edit.range.end.line, 0, "{marked:?}");
            assert_eq!(edit.new_text, "../assets/cover.svg\"", "{marked:?}");
        }
    }

    /// An import item list answers only the module's exports, narrowed by what the author typed.
    #[test]
    fn package_import_items_are_the_only_completion() {
        let package = tola_packages::builtin_package(&tola_packages::TolaPackage::Web.spec())
            .expect("the web package");
        let mut site = QuerySession::with_program(FAILED_PROGRAM);
        for marked in [
            "#import \"@tola/web:0.0.0\": p|",
            "#import \"@tola/web:0.0.0\": pl|",
        ] {
            let items = site.completion(marked);
            let labels = completion_labels(&items);
            assert!(labels.contains(&"plain-text"), "{marked:?}: {labels:?}");
            assert!(
                labels
                    .iter()
                    .all(|label| package.exports().any(|export| export == *label)),
                "{marked:?}: {labels:?}"
            );
        }
        let items = site.completion("#import \"@tola/web:0.0.0\": plain-t|");
        assert_eq!(completion_labels(&items), ["plain-text"]);
        for marked in [
            "#import \"@tola/web:0.0.0\":|",
            "#import \"@tola/web:0.0.0\": |",
            "#import \"@tola/web:0.0.0\": pl |",
            "#import \"@tola/web:0.0.0\": pl, |",
        ] {
            let items = site.completion(marked);
            let labels = completion_labels(&items);
            assert!(labels.contains(&"plain-text"), "{marked:?}: {labels:?}");
            assert!(
                !labels.contains(&"map") && !labels.contains(&"mark"),
                "{marked:?}: {labels:?}"
            );
        }
    }

    /// A local module answers every export on the colon itself, and offers the wildcard the
    /// statement can still use.
    #[test]
    fn local_module_items_complete_on_the_colon() {
        let mut site = QuerySession::with_program(FAILED_PROGRAM);
        site.site.write(
            "components/home.typ",
            "#let home-features() = [Features]\n#let home-hero() = [Hero]\n",
        );
        let items = site.completion("#import \"/components/home.typ\":|");
        let labels = completion_labels(&items);
        for label in ["*", "home-features", "home-hero"] {
            assert!(labels.contains(&label), "{label:?} missing from {labels:?}");
        }
    }
}
