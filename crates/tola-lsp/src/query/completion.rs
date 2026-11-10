//! What one completion offers at the cursor: names, members, import items, metadata keys, and the
//! parameters a call can still fill; and the signature one call stands in.

use std::collections::{BTreeMap, BTreeSet};
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
use tola_typst_syntax::names::{Declaration, Initializer, SourceNames};

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

/// Where one completion is written: the author's source, the cursor, the bytes typed before it,
/// and the range an accepted item replaces.
pub(super) struct NamePosition<'a> {
    pub(super) source: &'a Source,
    pub(super) cursor: usize,
    /// The bytes inside [`Self::replacement`] before the cursor, which the client filters on.
    pub(super) prefix: &'a str,
    pub(super) replacement: Range<usize>,
}

impl<'a> NamePosition<'a> {
    /// The name `replacement` covers in `source`.
    pub(super) fn at(source: &'a Source, cursor: usize, replacement: Range<usize>) -> Self {
        Self {
            source,
            cursor,
            prefix: typed_text(source, &replacement, cursor),
            replacement,
        }
    }

    /// The edit `new_text` writes over the written name, keeping what follows the cursor.
    pub(super) fn edit(&self, new_text: String) -> Result<CompletionTextEdit> {
        completion_edit(self.source, self.cursor, self.replacement.clone(), new_text)
    }
}

/// The one alternative every candidate agrees on, or `None` when they disagree or there are none.
pub(super) fn agreed<T: PartialEq>(mut alternatives: impl Iterator<Item = T>) -> Option<T> {
    let first = alternatives.next()?;
    alternatives
        .all(|alternative| alternative == first)
        .then_some(first)
}
/// Whether the cursor stands where a path is written, which completion reads from author text.
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

/// The documentation template an empty `///` above a function definition completes.
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

/// Whether the name still contains the typed characters in order (`al-` reaches `all-sources`).
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
    typed_text(source, &range, cursor)
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

/// The expression a parenthesized group wraps: a group spells what it holds, not itself.
fn unparenthesized(mut expression: Option<ast::Expr>) -> Option<ast::Expr> {
    while let Some(ast::Expr::Parenthesized(group)) = expression {
        expression = Some(group.expr());
    }
    expression
}

/// One completion item's documentation, as the markdown an editor renders.
pub(in crate::query) fn markdown_documentation(text: impl Into<String>) -> Documentation {
    Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: markdown::docs(&text.into()),
    })
}

/// The declaration one name resolves to, in the source that declares it.
struct DeclaredName {
    names: Arc<SourceNames>,
    index: usize,
}

impl DeclaredName {
    fn declaration(&self) -> &Declaration {
        &self.names.declarations()[self.index]
    }
}

/// The declaration the site's source binds one name by, following the name an alias spells.
fn declared(
    semantics: &mut Semantic<'_>,
    bindings: &BTreeMap<String, semantic::Binding>,
    span: Span,
) -> Option<DeclaredName> {
    let mut span = span;
    let mut followed = Vec::new();
    loop {
        let range = semantics.span_range(span)?;
        let names = semantics.source_names(span.id()?)?;
        let index = names.declared_at(range.start)?;
        // An alias names another binding; that binding's declaration is what the author reads.
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

/// The expression types one resolved value offers: the value itself, or none when no value
/// reached the name.
fn known_types(value: Option<&Value>) -> Vec<ExpressionType> {
    value
        .cloned()
        .map(ExpressionType::Known)
        .into_iter()
        .collect()
}

/// The item an import statement offers while it asks for every export: `*`, which the client
/// filters on like any other name.
fn wildcard_import_item(edit: CompletionTextEdit) -> CompletionItem {
    CompletionItem {
        label: "*".to_owned(),
        kind: Some(CompletionItemKind::SNIPPET),
        detail: Some("Import everything.".to_owned()),
        text_edit: Some(edit),
        ..CompletionItem::default()
    }
}

/// Whether the statement already imports `name` from the module it selects.
fn already_imported(import: &tola_typst_syntax::syntax::Import, name: &str) -> bool {
    import.imported.iter().any(|path| {
        path.len() == import.path.len() + 1
            && path[..import.path.len()] == import.path
            && path.last().map(String::as_str) == Some(name)
    })
}

impl Selection<'_> {
    /// The names, members, import items, and parameters one completion position offers.
    ///
    /// Edits rewrite the author's text; resolved nodes come from the compiled copy first, whose
    /// offsets the repairs keep aligned with the author's parse.
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
        let position = self.name_position(replacement)?;
        let mut items = self.bound_name_items(semantics, &node, &position)?;
        let mut seen: BTreeSet<String> = items.iter().map(|item| item.label.clone()).collect();
        items.extend(self.library_name_items(semantics, &node, &position, &mut seen)?);
        items.extend(
            parameters
                .into_iter()
                .filter(|item| seen.insert(item.label.clone())),
        );
        Ok(items)
    }

    /// The items this source's own bindings offer at `node`, each with the call its value or its
    /// declaration admits.
    fn bound_name_items(
        &self,
        semantics: &mut Semantic<'_>,
        node: &LinkedNode<'_>,
        position: &NamePosition<'_>,
    ) -> Result<Vec<CompletionItem>> {
        let bindings = semantics.bindings(node.clone())?;
        let mut packages = definition::PackageExports::default();
        let mut items = Vec::new();
        for (name, binding) in &bindings {
            if !name_matches(name, position.prefix) {
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
            let mut item = self.name_item(name, &known_types(value.as_ref()), position)?;
            attach_value_docs(&mut item, semantics, binding.span, value.as_ref());
            if value.is_none()
                && let Some(declaration) = &declaration
            {
                // No value reached this name, so the kind its declaration promises is the only
                // kind the editor can show.
                item.kind = Some(definition::declaration_kind(
                    &declaration.declaration().initializer,
                ));
            }
            let shape = match &declaration {
                Some(declaration) => definition::attach_declaration(
                    &mut item,
                    &declaration.names,
                    declaration.declaration(),
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
            items.extend(callable::name_items(item, callable, position));
        }
        Ok(items)
    }

    /// The items the standard library offers for the names this file does not bind: they answer
    /// even when the site failed before evaluating this position, and the file's own bindings
    /// shadow them.
    fn library_name_items(
        &self,
        semantics: &mut Semantic<'_>,
        node: &LinkedNode<'_>,
        position: &NamePosition<'_>,
        seen: &mut BTreeSet<String>,
    ) -> Result<Vec<CompletionItem>> {
        let mut items = Vec::new();
        for (name, value) in semantics.library_bindings(node) {
            if !name_matches(&name, position.prefix) || !seen.insert(name.clone()) {
                continue;
            }
            let mut item =
                self.name_item(&name, &[ExpressionType::Known(value.clone())], position)?;
            attach_value_docs(&mut item, semantics, Span::detached(), Some(&value));
            items.extend(callable::name_items(
                item,
                callable::Callable::of_value(&value, callable::Receiver::Unbound),
                position,
            ));
        }
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
        // The module node answers by value from the compiled copy, then from the author's parse.
        let Some(source) = tola_typst_syntax::syntax::node_at_range(prepared, &import.source)
            .or_else(|| tola_typst_syntax::syntax::node_at_range(self.source, &import.source))
        else {
            return Ok(Vec::new());
        };
        let position = self.name_position(replacement.clone())?;
        let mut items = Vec::new();
        if import.imported.is_empty() && "*".starts_with(position.prefix) {
            items.push(wildcard_import_item(position.edit("*".to_owned())?));
        }
        for (name, binding) in semantics.import_bindings(&source, &import.path)? {
            if !name_matches(&name, position.prefix) || already_imported(import, &name) {
                continue;
            }
            let mut item =
                self.name_item(&name, &known_types(binding.value.as_ref()), &position)?;
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
        // The receiver answers by value from the compiled copy, then from the author's parse.
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
            .map(schema::SchemaDescription::fields)
            .unwrap_or_default();
        // A member of a value supplies its `self`; a module's or a type's member takes the
        // receiver as its first argument.
        let receiver = callable::receiver_binds(
            receivers
                .iter()
                .map(|(receiver, _)| !matches!(receiver, Value::Module(_) | Value::Type(_))),
        );
        let position = self.name_position(member.replacement.clone())?;
        let mut items = Vec::new();
        for (name, values) in fields
            .into_iter()
            .filter(|(name, _)| name_matches(name, position.prefix))
        {
            let mut item = self.name_item(&name, &values, &position)?;
            let value = agreed(values.iter().map(ExpressionType::value)).flatten();
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
            let callable =
                receiver.and_then(|receiver| callable::Callable::of_types(&values, receiver));
            items.extend(callable::name_items(item, callable, &position));
        }
        for (name, field) in declared_fields {
            if !name_matches(&name, position.prefix) {
                continue;
            }
            let mut item = self.name_item(&name, &[], &position)?;
            item.label_details = Some(labelled_type(field.ty()));
            item.documentation = Some(markdown_documentation(field.field(&name).section()));
            items.push(item);
        }
        if let Some(descriptors) = descriptors {
            for field in descriptors.fields() {
                let name = field.name();
                if !name_matches(name, position.prefix)
                    || items.iter().any(|item| item.label == name)
                {
                    continue;
                }
                let mut item = self.name_item(name, &[], &position)?;
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
        let position = self.name_position(key.typed.clone())?;
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
            if !name_matches(&name, position.prefix) || key.written.contains(&name) {
                continue;
            }
            let rejecting = schemas
                .shapes
                .iter()
                .filter(|schema| schemas.covered && schema.rejects(&name))
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
                position.edit(crate::completion::typst_snippet(&format!("{name}: ${{}}")))?,
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

    /// The item one name inserts at `position`, described by the types the site establishes for it.
    fn name_item(
        &self,
        name: &str,
        types: &[ExpressionType],
        position: &NamePosition<'_>,
    ) -> Result<CompletionItem> {
        Ok(semantic::completion(
            name,
            types,
            self.completion_edit(position.replacement.clone(), name.to_owned())?,
        ))
    }

    /// The position an item accepted at `replacement` writes at.
    fn name_position(&self, replacement: Range<usize>) -> Result<NamePosition<'_>> {
        let (_, prefix) = self.completion_range(replacement.clone())?;
        Ok(NamePosition {
            source: self.source,
            cursor: self.cursor,
            prefix,
            replacement,
        })
    }

    /// The insert-and-replace edit one item has: keep what follows the cursor, replace the token.
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

    /// The named parameters a call can still fill, from the metadata signature help reads.
    pub(super) fn parameter_completion(
        &self,
        semantics: &mut Semantic<'_>,
    ) -> Result<Vec<CompletionItem>> {
        let previous = LinkedNode::new(self.source.root()).leaf_at(self.cursor, Side::Before);
        let after_separator = previous.is_some_and(|node| {
            matches!(
                node.kind(),
                SyntaxKind::LeftParen | SyntaxKind::Comma | SyntaxKind::Error
            )
        });
        if !after_separator {
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
        Ok(semantic::parameter_completions(
            semantics, &functions, &taken, range,
        ))
    }
}

/// The text one completion replaces from its start up to the cursor: the prefix the client filters
/// on, read from the author's own source.
fn typed_text<'a>(source: &'a Source, replacement: &Range<usize>, cursor: usize) -> &'a str {
    &source.text()[replacement.start..cursor]
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
        assert!(!site.config.build().publish_dir.exists());
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
    fn edit_separates_insert_from_replace() {
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
    fn empty_comment_completes_template() {
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

    #[test]
    fn path_prefixes_decode_typst_escapes() {
        let mut site = QuerySession::new();
        site.site.write("templates/title.typ", "Title\n");
        let items = site.completion("#include \"\\u{2f}te|\"\n");
        let item = item_labeled(&items, "/templates/title.typ");
        assert_eq!(insertion_text(item), "/templates/title.typ");
        assert_eq!(
            item.filter_text.as_deref(),
            Some("\\u{2f}templates/title.typ")
        );
    }

    /// The label is the file's own spelling; the inserted text is escaped for Typst.
    #[test]
    #[cfg(unix)]
    fn paths_keep_written_spelling() {
        let mut site = QuerySession::new();
        site.site.write("content/quo\"ted.typ", "Quoted\n");
        let items = site.completion("#include \"|\"\n");
        let item = item_labeled(&items, "quo\"ted.typ");
        assert_eq!(insertion_text(item), "quo\\\"ted.typ");
        let completed = format!("#include \"{}|\"\n", insertion_text(item));
        let Some(lsp_types::GotoDefinitionResponse::Scalar(location)) = site.definition(&completed)
        else {
            panic!("the completed string reaches its file");
        };
        assert_eq!(
            location.uri,
            crate::uri::from_file_path(&site.config.get_root().join("content/quo\"ted.typ"))
                .unwrap()
        );
        let opened = crate::sources::load(
            &location.uri,
            site.config.get_root(),
            &Default::default(),
            &site.session.resources().source_boundary(&site.config),
            None,
            None,
        )
        .expect("the completed file opens");
        assert_eq!(opened.text(), "Quoted\n");

        site.site.write("content/chapter:notes.typ", "Notes\n");
        let items = site.completion("#include \"chapter:|notes.typ\"\n");
        let item = item_labeled(&items, "chapter:notes.typ");
        assert_eq!(insertion_text(item), "chapter:notes.typ");
    }

    #[test]
    fn nonpath_arguments_offer_no_files() {
        let mut site = QuerySession::new();
        site.site.write("assets/cover.svg", "<svg/>");
        for marked in [
            "#image(alt: \"|",
            "#read(encoding: \"|",
            "#read(\"data.csv\", encoding: \"utf|8\")",
            "#bibliography(\"refs.bib\", style: \"ap|a\")",
            "#bibliography(\"refs.bib\", \"m|\")",
            "#let read(text) = text\n#read(\"|\")",
            "#let read(text) = text\n#read(\"|",
            "#let render(read) = read(\"|\")",
        ] {
            let items = site.completion(marked);
            assert!(
                !completion_labels(&items).contains(&"../assets/cover.svg"),
                "{marked}: {items:?}"
            );
        }
    }

    #[test]
    fn bibliographies_complete_each_source() {
        let mut site = QuerySession::new();
        site.site.write("content/more.bib", "");
        for marked in [
            "#bibliography((\"first.bib\", \"m|\"))",
            "#bibliography((\"first.bib\", \"m|",
        ] {
            let items = site.completion(marked);
            assert!(
                completion_labels(&items).contains(&"more.bib"),
                "{marked}: {items:?}"
            );
        }
    }

    #[test]
    fn styles_complete_only_csl() {
        let mut site = QuerySession::new();
        site.site.write("content/styles/custom.csl", "");
        site.site.write("content/styles/refs.bib", "");
        for marked in [
            "#bibliography(\"refs.bib\", style: \"styles/|custom.csl\")",
            "#bibliography(\"refs.bib\", style: \"styles/|\")",
        ] {
            let items = site.completion(marked);
            assert!(completion_labels(&items).contains(&"styles/custom.csl"));
            assert!(
                items
                    .iter()
                    .filter(|item| item.kind == Some(CompletionItemKind::FILE))
                    .all(|item| item.label.ends_with(".csl")),
                "{marked}: {items:?}"
            );
        }
    }

    #[test]
    fn raw_files_complete_by_role() {
        let mut site = QuerySession::new();
        site.site.write("content/my.tmTheme", "");
        site.site.write("content/my.sublime-syntax", "");
        for (marked, expected, excluded) in [
            (
                "#set raw(theme: \"my|\")",
                "my.tmTheme",
                "my.sublime-syntax",
            ),
            (
                "#raw(\"text\", theme: \"my|\")",
                "my.tmTheme",
                "my.sublime-syntax",
            ),
            (
                "#set raw(syntaxes: (\"other.sublime-syntax\", \"my|\"))",
                "my.sublime-syntax",
                "my.tmTheme",
            ),
        ] {
            let items = site.completion(marked);
            let labels = completion_labels(&items);
            assert!(labels.contains(&expected), "{marked}: {labels:?}");
            assert!(!labels.contains(&excluded), "{marked}: {labels:?}");
        }
        let items = site.completion("#let raw(theme) = theme\n#raw(theme: \"my|\")");
        assert!(!completion_labels(&items).contains(&"my.tmTheme"));
    }

    #[test]
    fn reader_paths_replace_own_argument() {
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

        for marked in [
            "#read(\"da|\")\n",
            "#let read(text) = text\n#std.read(\"da|\")\n",
            "#read(\"da|\")\n#let read(text) = text\n",
        ] {
            let items = site.completion(marked);
            assert!(
                completion_labels(&items)
                    .iter()
                    .any(|label| label.ends_with("data/site.json")),
                "{marked}: {items:?}"
            );
        }

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
    fn import_items_only_module_exports() {
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
    fn local_module_items_on_colon() {
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
