//! What one hover shows, and how its sections part from each other.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use anyhow::Result;
use lsp_types::Hover;
use tola_build::AssetUrls;
use tola_build::cancellation::BuildCancellation;
use tola_build::check::SourceCompilation;
use tola_build::config::ResolvedSiteConfig;
use tola_packages::Signature;
use tola_typst::typst::World;
use tola_typst::typst::foundations::{Func, Styles, Value};
use tola_typst::typst::syntax::ast::{self, AstNode};
use tola_typst::typst::syntax::{LinkedNode, Side, Source, Span, SyntaxKind, VirtualRoot};
use tola_typst_syntax::docs::Documentation;
use tola_typst_syntax::names::{Declaration, DeclarationKind, SourceNames};
use typst_ide::Tooltip;

use crate::markdown;
use crate::position;
use crate::protocol::markdown_hover;
use crate::sentence::{joined, listed};

use super::citations::{self, BibliographyCache};
use super::context::{SelectedSyntax, Selection};
use super::editing;
use super::expression::ExpressionType;
use super::imports::imported_member;
use super::labels;
use super::meta::{MetaCall, meta_call, meta_key_at};
use super::name_graph::bare_name;
use super::records::declared::{DeclaredOutput, declared_output};
use super::records::origin::{field_chain, record_origin};
use super::records::parameters::{ParameterArgument, parameter_argument};
use super::repair::import_star;
use super::schema;
use super::semantic::{self, Semantic};
use super::site_schema::{declared_documentation, rejected_key_note};

/// Whether a hover says anything, which a markdown envelope of whitespace does not.
pub(super) fn says_something(hover: &lsp_types::Hover) -> bool {
    let says = |marked: &lsp_types::MarkedString| match marked {
        lsp_types::MarkedString::String(text) => !text.trim().is_empty(),
        lsp_types::MarkedString::LanguageString(code) => !code.value.trim().is_empty(),
    };
    match &hover.contents {
        lsp_types::HoverContents::Markup(markup) => !markup.value.trim().is_empty(),
        lsp_types::HoverContents::Scalar(marked) => says(marked),
        lsp_types::HoverContents::Array(marked) => marked.iter().any(says),
    }
}

/// The union of every type the world observed at a node.
fn observed_type_union(observed: &[(ExpressionType, Option<Styles>)]) -> Option<String> {
    let types = observed
        .iter()
        .map(|(ty, _)| ty.type_name())
        .collect::<BTreeSet<_>>();
    (!types.is_empty()).then(|| types.into_iter().collect::<Vec<_>>().join(" | "))
}

/// The one value every observation at a node agrees on, when the world observed one there.
fn agreed_value(observed: &[(ExpressionType, Option<Styles>)]) -> Option<&Value> {
    let (ty, _) = observed.first()?;
    let value = ty.value()?;
    observed
        .iter()
        .all(|(ty, _)| ty.value() == Some(value))
        .then_some(value)
}

/// One export as every answer reads it: its value line, then its documentation.
fn described_export(semantics: &Semantic<'_>, name: &str, value: &Value, span: Span) -> String {
    let docs = match value {
        Value::Func(func) => semantic::function_docs(func).map(str::to_owned),
        _ => None,
    }
    .or_else(|| semantics.source_docs(span));
    let line = match value {
        Value::Func(func) => semantic::signature(name, func, None, &BTreeMap::new()).code(),
        _ => semantic::description(value, name),
    };
    let mut text = typc_fence(&line);
    if let Some(docs) = docs {
        text.push_str(SECTION_JOIN);
        text.push_str(&markdown::docs(&docs));
    }
    text
}

/// One Typst code fence, as every hover section writes it.
pub(in crate::query) fn typc_fence(code: &str) -> String {
    format!("```typc\n{code}\n```")
}

/// Hover sections part as Tinymist parts them (Apache-2.0; see licenses/README.md).
pub(in crate::query) const SECTION_JOIN: &str = "\n\n---\n\n";

/// The hover a set of sections makes, or none when they say nothing.
fn sections_hover(sections: Vec<String>, range: Option<lsp_types::Range>) -> Option<Hover> {
    (!sections.is_empty()).then(|| markdown_hover(sections.join(SECTION_JOIN), range))
}

/// How many fields one record shape lists before the remainder is counted.
const SHAPE_FIELD_LIMIT: usize = 20;

/// Function parameters as Tinymist sections: group, name, type, documentation.
fn parameter_section(signature: &Signature) -> Option<String> {
    let mut text = String::new();
    for (heading, kind, group) in [
        (
            "Positional Parameters",
            "positional",
            signature.positional(),
        ),
        ("Rest Parameters", "spread right", signature.rest()),
        ("Named Parameters", "named", signature.named()),
    ] {
        if group.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str("# ");
        text.push_str(heading);
        for (index, parameter) in group.iter().enumerate() {
            text.push_str("\n\n## ");
            text.push_str(&parameter.name);
            if index > 0 {
                text.push_str(" (");
                text.push_str(kind);
                text.push(')');
            }
            text.push_str("\n\n");
            text.push_str(&typc_fence(&format!("type: {}", parameter.ty_spelling())));
            if let Some(docs) = &parameter.docs {
                text.push_str("\n\n");
                text.push_str(&markdown::docs(docs));
            }
        }
    }
    (!text.is_empty()).then_some(text)
}

fn shape_section(heading: &str, fields: &[String]) -> String {
    let mut section = format!("### {heading}");
    for field in fields.iter().take(SHAPE_FIELD_LIMIT) {
        section.push_str("\n- ");
        section.push_str(field);
    }
    if fields.len() > SHAPE_FIELD_LIMIT {
        section.push_str(&format!(
            "\n… {} more fields",
            fields.len() - SHAPE_FIELD_LIMIT
        ));
    }
    section
}

/// The routes an answer names, capped at NAMED_PAGES, with the count left out.
pub(in crate::query) fn shown_routes<'a>(routes: impl Iterator<Item = &'a str>) -> String {
    let (mut names, remaining) = listed(routes, crate::routes::NAMED_PAGES);
    if remaining > 0 {
        names.push_str(&format!(" and {remaining} more"));
    }
    names
}

/// The parameter declaration one source binds at `at`, when that name is a parameter.
fn parameter_declaration(names: &SourceNames, at: usize) -> Option<&Declaration> {
    names
        .declared_at(at)
        .map(|id| &names.declarations()[id])
        .filter(|declaration| matches!(declaration.kind, DeclarationKind::Parameter))
}

/// A function's value line when the name is bare, the block's summary, then its parameters.
fn function_sections(
    name: &str,
    named: bool,
    func: &Func,
    documentation: Option<&Documentation>,
    observed_parameters: &BTreeMap<String, String>,
) -> Vec<String> {
    let signature = semantic::signature(name, func, documentation, observed_parameters);
    let mut sections = Vec::new();
    if named {
        sections.push(typc_fence(&signature.code()));
    }
    // The block's summary describes the declaration; parameter docs sit with the parameters.
    if let Some(documentation) = documentation
        && !documentation.summary.is_empty()
    {
        sections.push(markdown::docs(&documentation.summary));
    }
    sections.extend(parameter_section(&signature));
    sections
}

/// The type one name reads: what the world observed at it, the default its declaration spells,
fn name_type(
    prepared: &Source,
    observed: &[(ExpressionType, Option<Styles>)],
    declaration: Option<&Declaration>,
    provisioned: Option<&ParameterArgument>,
    semantics: &mut Semantic<'_>,
) -> Result<String> {
    let ty = match (observed_type_union(observed), declaration) {
        (Some(ty), _) => Some(ty),
        (None, Some(declaration)) => {
            Selection::default_type(prepared, &declaration.range, semantics)?
        }
        (None, None) => None,
    }
    .or_else(|| provisioned.and_then(|provisioned| provisioned.ty.clone()));
    Ok(ty.unwrap_or_else(|| "any".to_owned()))
}

/// The sections a declaration's docs and declared shape add after its value line.
fn declaration_sections(docs: Option<String>, declared: Option<&DeclaredOutput>) -> Vec<String> {
    let mut sections = Vec::new();
    if let Some(declared) = declared {
        sections.push(shape_section("Declared fields", &declared.lines));
    }
    if let Some(summary) = docs
        .map(|docs| Documentation::parse(&docs))
        .map(|documentation| documentation.summary)
        .filter(|summary| !summary.is_empty())
    {
        sections.push(markdown::docs(&summary));
    }
    if let Some(declared) = declared
        && let Some(description) = &declared.documentation
    {
        sections.push(markdown::docs(description));
    }
    sections
}

impl Selection<'_> {
    /// The hover a citation answers, when the cursor stands on a declared key.
    pub(super) fn citation(
        &self,
        compilation: &SourceCompilation,
        config: &ResolvedSiteConfig,
        bibliographies: &mut BibliographyCache,
        cancellation: &BuildCancellation,
    ) -> Result<Option<lsp_types::Hover>> {
        let Some(reference) = labels::referenced(self.source, self.cursor) else {
            return Ok(None);
        };
        let name = &self.source.text()[reference.name.clone()];
        // A label answers its own reference first, exactly as Typst resolves the two.
        if labels::declares(compilation, name) {
            return Ok(None);
        }
        citations::hover(
            compilation,
            config,
            self.source.id(),
            name,
            bibliographies,
            cancellation,
        )
    }
    /// The text of an asset-url argument the cursor sits in, with its bytes.
    pub(super) fn asset_url_argument(
        &self,
        semantics: &mut Semantic<'_>,
    ) -> Result<Option<(String, Range<usize>)>> {
        let Some(call) = tola_typst_syntax::syntax::call(self.source, self.cursor) else {
            return Ok(None);
        };
        let Some(callee) = tola_typst_syntax::syntax::node_at_range(self.source, &call.callee)
        else {
            return Ok(None);
        };
        let Some(Value::Func(func)) = semantics
            .callee_functions(&callee)?
            .into_iter()
            .map(|(value, _)| value)
            .next()
        else {
            return Ok(None);
        };
        let Some(call_node) = tola_typst_syntax::syntax::node_at_range(
            self.source,
            &(call.callee.start..call.suffix.end),
        ) else {
            return Ok(None);
        };
        let Some(func_call) = call_node.cast::<ast::FuncCall>() else {
            return Ok(None);
        };
        let parameters = func.params().collect::<Vec<_>>();
        let mut index = 0usize;
        for item in func_call.args().items() {
            // A content block is the call's body, not a parameter value; a named argument names
            let (parameter, text) = match item {
                ast::Arg::Pos(ast::Expr::ContentBlock(_)) | ast::Arg::Spread(_) => continue,
                ast::Arg::Pos(ast::Expr::Str(text)) => {
                    let parameter = parameters.get(index).and_then(|parameter| parameter.name());
                    index += 1;
                    (parameter.map(|name| name.to_string()), text)
                }
                ast::Arg::Pos(_) => {
                    index += 1;
                    continue;
                }
                ast::Arg::Named(named) => match named.expr() {
                    ast::Expr::Str(text) => (Some(named.name().get().to_string()), text),
                    _ => continue,
                },
            };
            let Some(parameter) = parameter else {
                continue;
            };
            let Some(bytes) = call_node.find(text.span()).and_then(|node| {
                let range = node.range();
                (range.end >= range.start + 2).then(|| range.start + 1..range.end - 1)
            }) else {
                continue;
            };
            if !(bytes.start..=bytes.end).contains(&self.cursor) {
                continue;
            }
            return Ok(matches!(
                crate::assets::domain_of(&func, parameter.as_str()),
                Some(tola_packages::ArgumentDomain::SiteAssetUrl)
            )
            .then(|| (self.source.text()[bytes.clone()].to_owned(), bytes)));
        }
        Ok(None)
    }

    /// The hover an asset-url argument earns: where the URL is published, or that nothing
    pub(super) fn asset_url_hover(
        &self,
        config: &ResolvedSiteConfig,
        semantics: &mut Semantic<'_>,
        cancellation: &BuildCancellation,
    ) -> Result<Option<Hover>> {
        let Some((typed, bytes)) = self.asset_url_argument(semantics)? else {
            return Ok(None);
        };
        let urls = AssetUrls::for_check_project(config, cancellation)?;
        let described = match crate::assets::written(&urls, &typed) {
            crate::assets::Written::Published(asset) => {
                let declared = asset
                    .origin
                    .map(|origin| {
                        format!(
                            ", declared by {}",
                            crate::assets::declaration_note(origin, config.get_root())
                        )
                    })
                    .unwrap_or_default();
                format!(
                    "`{}` is published at `{}`{declared}.",
                    asset.declared, asset.address
                )
            }
            crate::assets::Written::Unpublished => {
                format!("No `assets` declaration of this site publishes `{typed}`.")
            }
        };
        Ok(position::utf16_range(self.source.lines(), bytes)
            .map(|range| markdown_hover(described, Some(range))))
    }
    pub(super) fn hover(
        &self,
        compilation: &SourceCompilation,
        config: &ResolvedSiteConfig,
        cancellation: &BuildCancellation,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
    ) -> Result<Option<Hover>> {
        if let SelectedSyntax::ImportedName(item) = &self.syntax {
            return self.hover_imported(
                compilation,
                config,
                cancellation,
                prepared,
                semantics,
                item,
            );
        }
        if let Some(star) = import_star(self.source, self.cursor) {
            return self.hover_star(prepared, semantics, &star);
        }
        // A tola-meta call answers from the site's schemas: its callee, then its declared keys.
        if let Some(hover) = self.meta_hover(compilation, config, prepared, semantics)? {
            return Ok(Some(hover));
        }
        // A named argument's name answers its parameter; a dict key answers its value's docs.
        if let Some(named) = Self::named_argument(prepared, self.cursor) {
            if let Some(hover) = self.parameter_hover(prepared, &named, semantics)? {
                return Ok(Some(hover));
            }
            let parent = named.parent().map(|parent| parent.kind());
            if parent == Some(SyntaxKind::Dict) {
                return Self::described_field_hover(prepared, &named, semantics);
            }
            if parent != Some(SyntaxKind::Params) {
                return Ok(None);
            }
        }
        let Some(node) = tola_typst_syntax::syntax::expression(prepared, self.cursor) else {
            return Ok(None);
        };
        // A repaired node is a stand-in; this lane answers nothing about it.
        if !self.range_keeps_source_text(prepared, node.range()) {
            return Ok(None);
        }
        // Only a name or a chain rooted at one has a value this lane describes.
        let rooted = tola_typst_syntax::syntax::root_expression(node.clone());
        if rooted.kind() != SyntaxKind::Ident {
            return Ok(None);
        }
        self.value_hover(prepared, &node, semantics)
    }

    /// Hover for one observed name: signature or module line, type, declared shape, docs.
    fn value_hover(
        &self,
        prepared: &Source,
        node: &LinkedNode<'_>,
        semantics: &mut Semantic<'_>,
    ) -> Result<Option<Hover>> {
        let names = self.names_of(prepared);
        let declaration = names
            .declared_at(self.cursor)
            .map(|id| &names.declarations()[id]);
        let observed = semantics.types(node)?;
        // A chain's declared shape is a source fact, answered without any realized record.
        let (root, fields) = field_chain(node).unwrap_or_else(|| (node.clone(), Vec::new()));
        let name = &prepared.text()[node.range()];
        // A field reads no declaration of its own, so a parameter proof comes from the chain's
        let parameter = parameter_declaration(&names, self.cursor)
            .or_else(|| parameter_declaration(&names, root.range().start));
        // A parameter's value is the argument each call passes; the site's calls prove it without
        let provisioned = match parameter {
            Some(declaration) if observed.is_empty() => {
                parameter_argument(prepared, &names, declaration, semantics)?
            }
            _ => None,
        };
        let origin = match (record_origin(&names, root, semantics)?, &provisioned) {
            (Some(origin), _) => Some(origin),
            (None, Some(provisioned)) => provisioned.origin.clone(),
            (None, None) => None,
        };
        let mut declared = match &origin {
            Some(origin) => declared_output(origin, &fields, name, semantics)?,
            None => None,
        };
        // A parameter's own shape answers at the parameter itself, where no chain reads through it.
        if declared.is_none()
            && fields.is_empty()
            && let Some(provisioned) = &provisioned
        {
            declared = provisioned.declared.clone();
        }
        if observed.is_empty() && declaration.is_none() && declared.is_none() {
            return Ok(None);
        }
        // A member path names no declaration, so signature, module, and type lines answer a bare
        let named = node.kind() == SyntaxKind::Ident;
        let observed_parameters =
            self.observed_parameters(prepared, node, declaration, semantics)?;
        // A declaration with no observed value still has the block its author wrote above it.
        let mut docs = semantics
            .definition(node)?
            .and_then(|span| semantics.source_docs(span))
            .or_else(|| {
                declaration.and_then(|declaration| {
                    tola_typst_syntax::docs::declaration_docs(prepared, declaration.range.start)
                })
            });

        let mut sections = Vec::new();
        match agreed_value(&observed) {
            Some(Value::Func(func)) => {
                docs = semantic::function_docs(func).map(str::to_owned).or(docs);
                // The summary sits with the function's own line, so the tail writes none.
                let documented = docs.take().map(|docs| Documentation::parse(&docs));
                sections.extend(function_sections(
                    name,
                    named,
                    func,
                    documented.as_ref(),
                    &observed_parameters,
                ));
            }
            Some(Value::Module(module)) => {
                if named {
                    let bound = module
                        .name()
                        .map_or_else(|| name.to_owned(), ToString::to_string);
                    sections.push(typc_fence(&format!("let {bound};")));
                }
            }
            _ => {
                let ty = name_type(
                    prepared,
                    &observed,
                    declaration,
                    provisioned.as_ref(),
                    semantics,
                )?;
                // A bare any adds nothing beside a declared shape; an observed type stays.
                if named && (ty != "any" || declared.is_none()) {
                    sections.push(typc_fence(&format!("let {name} = {ty};")));
                }
            }
        }
        sections.extend(declaration_sections(docs, declared.as_ref()));
        Ok(sections_hover(
            sections,
            position::utf16_range(self.source.lines(), node.range()),
        ))
    }

    /// The observed type of each parameter, by name, from the call or the declaration.
    fn observed_parameters(
        &self,
        prepared: &Source,
        node: &LinkedNode<'_>,
        declaration: Option<&Declaration>,
        semantics: &mut Semantic<'_>,
    ) -> Result<BTreeMap<String, String>> {
        let Some(declaration) =
            declaration.filter(|declaration| matches!(declaration.kind, DeclarationKind::Let))
        else {
            return Ok(BTreeMap::new());
        };
        // A named argument holds its parameter's value, observed at the argument itself.
        let mut arguments = BTreeMap::new();
        if let Some(parent) = node.parent().cloned()
            && let Some(call) = parent.cast::<ast::FuncCall>()
        {
            for arg in call.args().items() {
                let ast::Arg::Named(named) = arg else {
                    continue;
                };
                let Some(value) = prepared.find(named.expr().span()) else {
                    continue;
                };
                arguments.insert(named.name().get().to_string(), value.range());
            }
        }
        let Some(ident) = tola_typst_syntax::syntax::node_at_range(prepared, &declaration.range)
        else {
            return Ok(BTreeMap::new());
        };
        let Some(parent) = ident.parent().cloned() else {
            return Ok(BTreeMap::new());
        };
        // The let-name sugar hangs parameters on the closure the name belongs to.
        let closure = parent.cast::<ast::Closure>().or_else(|| {
            parent
                .cast::<ast::LetBinding>()
                .and_then(|binding| match binding.init() {
                    Some(ast::Expr::Closure(closure)) => Some(closure),
                    _ => None,
                })
        });
        let Some(closure) = closure else {
            return Ok(BTreeMap::new());
        };
        let mut observed = BTreeMap::new();
        for param in closure.params().children() {
            let name = match param {
                ast::Param::Pos(ast::Pattern::Normal(ast::Expr::Ident(name))) => {
                    name.get().to_string()
                }
                ast::Param::Named(named) => named.name().get().to_string(),
                ast::Param::Spread(spread) => match spread.sink_ident() {
                    Some(name) => name.get().to_string(),
                    None => continue,
                },
                _ => continue,
            };
            let span = match param {
                ast::Param::Pos(pattern) => pattern.span(),
                ast::Param::Named(named) => named.name().span(),
                ast::Param::Spread(spread) => spread.span(),
            };
            let node = arguments
                .get(&name)
                .and_then(|range| tola_typst_syntax::syntax::node_at_range(prepared, range))
                .or_else(|| prepared.find(span));
            let Some(node) = node else {
                continue;
            };
            let types = semantics.types(&node)?;
            if let Some(ty) = observed_type_union(&types) {
                observed.insert(name, ty);
            }
        }
        Ok(observed)
    }

    /// The named argument whose own name the cursor stands in, as `#f(name: …)` writes it.
    fn named_argument(prepared: &Source, cursor: usize) -> Option<LinkedNode<'_>> {
        let root = LinkedNode::new(prepared.root());
        let mut node = [Side::Before, Side::After]
            .into_iter()
            .find_map(|side| root.leaf_at(cursor, side))?;
        loop {
            if let Some(named) = node.cast::<ast::Named>() {
                let range = prepared.find(named.name().span())?.range();
                return (cursor >= range.start && cursor <= range.end).then_some(node);
            }
            node = node.parent()?.clone();
        }
    }

    /// Hover for a named argument's name: its parameter and the observed argument value.
    fn parameter_hover(
        &self,
        prepared: &Source,
        named: &LinkedNode<'_>,
        semantics: &mut Semantic<'_>,
    ) -> Result<Option<Hover>> {
        let Some(arguments) = named.parent() else {
            return Ok(None);
        };
        let Some(call) = arguments
            .parent()
            .and_then(|call| call.cast::<ast::FuncCall>())
        else {
            return Ok(None);
        };
        let Some(argument) = named.cast::<ast::Named>() else {
            return Ok(None);
        };
        let name = argument.name().get().to_string();
        // Only a function observed at the callee has parameters this name can fill.
        let traced = prepared
            .find(call.callee().span())
            .map(|callee| semantics.known_values(&callee))
            .transpose()?
            .and_then(|values| values.into_iter().next())
            .and_then(|(value, _)| match value {
                Value::Func(func) => Some(func),
                _ => None,
            });
        let Some(func) = traced else {
            return Ok(None);
        };
        if !func
            .params()
            .any(|param| param.name() == Some(name.as_str()))
        {
            return Ok(None);
        }
        let observed = prepared
            .find(argument.expr().span())
            .map(|value| semantics.types(&value))
            .transpose()?
            .unwrap_or_default();
        let observed = observed_type_union(&observed);
        // The block above the callee's declaration describes the parameter this argument fills.
        let documented = match prepared.find(call.callee().span()) {
            Some(callee) => semantics
                .definition(&callee)?
                .and_then(|span| semantics.source_docs(span)),
            None => None,
        }
        .or_else(|| func.docs().map(str::to_owned))
        .map(|docs| Documentation::parse(&docs));
        let observed = match observed {
            Some(observed) => BTreeMap::from([(name.clone(), observed)]),
            None => BTreeMap::new(),
        };
        let mut signature = semantic::signature(
            func.name().unwrap_or_default(),
            &func,
            documented.as_ref(),
            &observed,
        );
        signature
            .parameters
            .retain(|parameter| parameter.name == name);
        let Some(section) = parameter_section(&signature) else {
            return Ok(None);
        };
        Ok(Some(markdown_hover(
            section,
            position::utf16_range(self.source.lines(), named.range()),
        )))
    }

    fn described_field_hover(
        prepared: &Source,
        named: &LinkedNode<'_>,
        semantics: &mut Semantic<'_>,
    ) -> Result<Option<Hover>> {
        let Some(field) = named.cast::<ast::Named>() else {
            return Ok(None);
        };
        let Some(value) = prepared.find(field.expr().span()) else {
            return Ok(None);
        };
        let name = field.name().get().to_string();
        let mut described: Vec<schema::DeclaredField> = Vec::new();
        for (value, _) in semantics.known_values(&value)? {
            if let Some(declared) = semantics
                .inspect_schema(&value)?
                .and_then(schema::OutputDescription::read)
                .map(|description| description.field(&name))
                && declared.documentation.is_some()
                && !described.contains(&declared)
            {
                described.push(declared);
            }
        }
        if described.is_empty() {
            return Ok(None);
        }
        let range = prepared
            .find(field.name().span())
            .and_then(|name| position::utf16_range(prepared.lines(), name.range()));
        Ok(Some(markdown_hover(
            described
                .iter()
                .map(schema::DeclaredField::section)
                .collect::<Vec<_>>()
                .join(SECTION_JOIN),
            range,
        )))
    }

    /// The answer a tola-meta call earns: its package function, or a declared key's field.
    fn meta_hover(
        &self,
        compilation: &SourceCompilation,
        config: &ResolvedSiteConfig,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
    ) -> Result<Option<Hover>> {
        let Some(call) = meta_call(self.source, self.cursor, &self.names_of(self.source)) else {
            return Ok(None);
        };
        if call.callee.start <= self.cursor && self.cursor <= call.callee.end {
            return self.meta_callee_hover(compilation, prepared, semantics, &call);
        }
        self.meta_key_hover(compilation, config, semantics, &call)
    }

    /// The answer a `#tola-meta` key earns: the declared field the site's schemas write for it.
    fn meta_key_hover(
        &self,
        compilation: &SourceCompilation,
        config: &ResolvedSiteConfig,
        semantics: &mut Semantic<'_>,
        call: &MetaCall,
    ) -> Result<Option<Hover>> {
        let Some(name) = meta_key_at(self.source, self.cursor, call) else {
            return Ok(None);
        };
        let schemas = self.meta_schemas(compilation, config, semantics)?;
        let key = &self.source.text()[name.clone()];
        let mut declared = Vec::new();
        for schema in &schemas.shapes {
            for (field, description) in schema.shape.fields() {
                if field == key {
                    declared.push((description.field(&field), schema.declaration()));
                }
            }
        }
        if declared.is_empty() {
            return Ok(None);
        }
        let mut documentation = declared_documentation(&declared, &schemas);
        let rejecting = schemas
            .shapes
            .iter()
            .filter(|schema| !schema.declares(key) && !schema.accepts_unknown())
            .count();
        if rejecting > 0 {
            let required = schemas.shapes.iter().any(|schema| schema.requires(key));
            documentation.push_str(&format!(
                "{SECTION_JOIN}{}",
                rejected_key_note(rejecting, required)
            ));
        }
        Ok(Some(markdown_hover(
            documentation,
            position::utf16_range(self.source.lines(), name),
        )))
    }

    /// The package function the file's import binds at a tola-meta callee.
    fn meta_callee_hover(
        &self,
        compilation: &SourceCompilation,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
        call: &MetaCall,
    ) -> Result<Option<Hover>> {
        let names = self.names_of(self.source);
        let Some(callee) = tola_typst_syntax::syntax::node_at_range(self.source, &call.callee)
        else {
            return Ok(None);
        };
        let Some(imported) = imported_member(&names, &callee) else {
            return Ok(None);
        };
        let Some(module) =
            tola_typst_syntax::syntax::node_at_range(prepared, &imported.import.source_range)
        else {
            return Ok(None);
        };
        let Some((value, span)) = semantics.imported_export(&module, &imported.path)? else {
            return Ok(None);
        };
        let name = &self.source.text()[call.callee.clone()];
        let mut described = described_export(semantics, name, &value, span);
        if let Some(home) = span.id().map(|id| labels::source_path(compilation, id)) {
            described.push_str(&format!("\n\nDeclared in `{home}`."));
        }
        Ok(Some(markdown_hover(
            described,
            position::utf16_range(self.source.lines(), call.callee.clone()),
        )))
    }

    /// The type one declaration's default expression gives it, when the world observed it.
    fn default_type(
        prepared: &Source,
        range: &std::ops::Range<usize>,
        semantics: &mut Semantic<'_>,
    ) -> Result<Option<String>> {
        // A named parameter's default is its last child, observed when the parameter was.
        let Some(default) = tola_typst_syntax::syntax::node_at_range(prepared, range)
            .and_then(|ident| ident.parent().cloned())
            .filter(|param| param.kind() == SyntaxKind::Named)
            .and_then(|param| param.children().next_back())
        else {
            return Ok(None);
        };
        let types = semantics.types(&default)?;
        Ok(observed_type_union(&types))
    }

    /// Hover for the star of a wildcard import: every name the module publishes.
    fn hover_star(
        &self,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
        star: &LinkedNode<'_>,
    ) -> Result<Option<Hover>> {
        let Some(import) = star.parent() else {
            return Ok(None);
        };
        let Some(literal) = import
            .children()
            .find(|child| child.kind() == SyntaxKind::Str)
        else {
            return Ok(None);
        };
        let Some(module) = tola_typst_syntax::syntax::node_at_range(prepared, &literal.range())
        else {
            return Ok(None);
        };
        let listed: Vec<String> = semantics
            .import_bindings(&module, &[])?
            .into_keys()
            .map(|name| format!("`{name}`"))
            .collect();
        if listed.is_empty() {
            return Ok(None);
        }
        Ok(Some(markdown_hover(
            format!("This star imports {}.", joined(&listed)),
            position::utf16_range(self.source.lines(), star.range()),
        )))
    }

    fn checked_routes(
        &self,
        compilation: &SourceCompilation,
        config: &ResolvedSiteConfig,
        cancellation: &BuildCancellation,
        span: Span,
    ) -> Result<Option<String>> {
        let Some(id) = span.id() else {
            return Ok(None);
        };
        if id.root() != &VirtualRoot::Project {
            return Ok(None);
        }
        let routes = crate::routes::site_routes(compilation, config, id, cancellation)?;
        let listed = shown_routes(routes.iter().map(|route| route.route.as_str()));
        Ok((!listed.is_empty()).then(|| format!("`{listed}`")))
    }

    /// Typst's own reader hover, shaped into sections: signature, value, docs.
    pub(super) fn builtin_hover(
        &self,
        world: &tola_typst::TypstWorld,
        prepared: &Source,
        cursor: usize,
    ) -> Option<Hover> {
        let (mut docs, values) = match editing::tooltip(world, prepared, cursor) {
            Some(Tooltip::Text(text)) => (Some(text.to_string()), None),
            Some(Tooltip::Code(code)) => (None, Some(code.to_string())),
            None => (None, None),
        };
        // The name the cursor stands in; an embedded callee's hash leaves no leaf at the name.
        let selected = tola_typst_syntax::syntax::expression(prepared, cursor)
            .filter(|node| matches!(node.kind(), SyntaxKind::Ident | SyntaxKind::MathIdent));
        let name = selected
            .as_ref()
            .map(|node| &prepared.text()[node.range()])
            .or_else(|| bare_name(prepared, cursor));
        let math = selected
            .as_ref()
            .is_some_and(|node| node.kind() == SyntaxKind::MathIdent);
        // Math names read the math scope; code names read the global scope, and only value reads
        let names = self.names_of(prepared);
        let signature = names
            .reads_value_at(cursor)
            .then_some(name)
            .flatten()
            .filter(|_| names.declared_at(cursor).is_none())
            .and_then(|name| {
                let value = if math {
                    world.library().math.scope().get(name)
                } else {
                    world.library().global.scope().get(name)
                }?
                .read()
                .clone();
                let Value::Func(func) = value else {
                    return None;
                };
                Some((name.to_owned(), func))
            });
        let line = match signature {
            Some((name, func)) => {
                docs = docs.or_else(|| semantic::function_docs(&func).map(str::to_owned));
                Some(semantic::signature(&name, &func, None, &BTreeMap::new()).code())
            }
            None => None,
        };
        let mut sections: Vec<String> = Vec::new();
        if let Some(line) = line {
            sections.push(typc_fence(&line));
        }
        if let Some(code) = values {
            sections.push(typc_fence(&code));
        }
        if let Some(docs) = docs {
            sections.push(markdown::docs(&docs));
        }
        sections_hover(sections, None)
    }

    /// Hover for one import item's own name, which the import binds under another name at most.
    fn hover_imported(
        &self,
        compilation: &SourceCompilation,
        config: &ResolvedSiteConfig,
        cancellation: &BuildCancellation,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
        item: &tola_typst_syntax::syntax::ImportedName,
    ) -> Result<Option<Hover>> {
        let Some(module) = tola_typst_syntax::syntax::node_at_range(prepared, &item.source) else {
            return Ok(None);
        };
        let Some((value, span)) = semantics.imported_export(&module, &item.path)? else {
            return Ok(None);
        };
        let Some(name) = item.path.last() else {
            return Ok(None);
        };
        let mut described = described_export(semantics, name, &value, span);
        if let Some(home) = span.id().map(|id| labels::source_path(compilation, id)) {
            described.push_str(&format!("\n\nDeclared in `{home}`"));
            if let Some(routes) = self.checked_routes(compilation, config, cancellation, span)? {
                described.push_str(&format!(", pages from current source: {routes}"));
            }
            described.push('.');
        }
        Ok(Some(markdown_hover(
            described,
            position::utf16_range(self.source.lines(), item.name.clone()),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::query::tests::*;

    #[test]
    fn parameter_hover_lists_observed_types() {
        for (first, second, expected) in [
            ("1", "\"text\"", "int | str"),
            ("x => x", "\"text\"", "function | str"),
            ("x => x", "(x, y) => x + y", "function"),
            ("std", "math", "module"),
        ] {
            let mut site = QuerySession::new();
            let calls = format!("\n#render({first})\n#render({second})\n");
            let hover = site
                .hover_text(&format!(
                    "#let render(value) = {{ let kept = val|ue; none }}{calls}",
                ))
                .expect("the observed parameter has a hover");
            assert!(
                hover.contains(&format!("let value = {expected};")),
                "{hover}"
            );
            let signature = site
                .hover_text(&format!(
                    "#let ren|der(value) = {{ let kept = value; none }}{calls}",
                ))
                .expect("the function has a hover");
            assert!(
                signature.contains(&format!("value: {expected}")),
                "{signature}"
            );
        }
    }

    #[test]
    fn whitespace_hover_answers_nothing() {
        let mut site = QuerySession::new();
        for marked in ["#let value = 1\n#value   | \n", "#let value = 1\n|    \n"] {
            assert!(site.hover(marked).is_none(), "{marked:?}");
        }
    }

    #[test]
    fn imported_hover_ranges_cover_selected_names() {
        let mut site = QuerySession::new();
        for marked in [
            "#import \"@tola/address:0.0.0\": out|put-of\n",
            "🦊 #import \"@tola/address:0.0.0\": |output-of as address\n",
        ] {
            let hover = site.hover(marked).expect("an imported export's hover");
            let source = Source::detached(marked.replace('|', ""));
            let range = hover.range.expect("the selected name's range");
            let start = position::byte_offset(source.lines(), range.start).unwrap();
            let end = position::byte_offset(source.lines(), range.end).unwrap();
            assert_eq!(&source.text()[start..end], "output-of");
        }
    }

    /// A reference the query copy repaired is a stand-in: hover answers the label's own fact,
    /// never a value read from the enclosing construct the repair left behind.
    #[test]
    fn repaired_reference_answers_no_hover() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("#let f(..xs) = xs.len()\n#f(<nope|)\n")
            .expect("a hover about the label");
        assert!(hover.contains("declares the label `nope`"), "{hover}");
        assert!(!hover.contains("Sampled"), "{hover}");
    }

    /// The library answers a name it spells only where that name reads a value; a dict key reads
    /// none, so it must answer nothing rather than the function the key happens to spell.
    #[test]
    fn dict_key_answers_no_library_function() {
        let mut site = QuerySession::new();
        assert!(
            site.hover_text("#let page = (tit|le: \"Hello\", text: \"Body\")\n")
                .is_none()
        );
    }

    /// A name the file binds answers where markup embeds it, on the name and directly after it.
    ///
    /// Typst's own reader answers neither position for an embedded expression, so the type the
    /// Bundle compiler observed at the name is the answer the author reads.
    #[test]
    fn markup_answers_local_binding() {
        let mut site = QuerySession::new();
        for marked in [
            "#let v = 1\nMarkup with #|v inside\n",
            "#let v = 1\nMarkup with #v| inside\n",
        ] {
            let hover = site
                .hover_text(marked)
                .unwrap_or_else(|| panic!("{marked:?} asks about `v`"));
            assert!(
                hover.contains("```typc\nlet v = int;\n```"),
                "{marked:?} answered {hover}"
            );
        }
    }

    /// A `asset-url` argument completes with the site's published URLs — in the declaration's own
    /// order — and a hover over one names the address a browser fetches it at.
    #[test]
    fn asset_url_argument_answers_published_urls() {
        let mut site = QuerySession::with_published_assets();
        let items = site.completion(
            "#import \"@tola/host:0.0.0\": asset-url\n#metadata(asset-url(\"/brand/|\"))\n",
        );
        let labels = completion_labels(&items);
        assert_eq!(labels, ["/brand/logo.svg"]);

        let hover = site
            .hover_text(
                "#import \"@tola/host:0.0.0\": asset-url\n#metadata(asset-url(\"/brand/logo.sv|g\"))\n",
            )
            .expect("a asset-url argument answers");
        assert!(hover.contains("published at"), "{hover}");
        assert!(hover.contains("/brand/logo.svg"), "{hover}");
    }

    /// A library callee answers where the world observed nothing at its own span.
    ///
    /// The name reaches the library, so the signature is the first section and the documentation
    /// follows it, whether or not Typst's reader had anything to report.
    #[test]
    fn library_callee_answers_without_observation() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("#let dead() = [#t|ext(size: 10pt)[x]]\n")
            .expect("a hover for the library callee");
        assert!(hover.starts_with("```typc\nlet text("), "{hover}");
        assert!(hover.contains("Customizes the look"), "{hover}");
    }

    #[test]
    fn math_function_hover_shows_its_signature() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("$sqr|t(4)$")
            .expect("a hover for a math function");
        assert!(hover.contains("let sqrt("), "{hover}");
        assert!(hover.contains("A square root."), "{hover}");
        let shadowed = site
            .hover_text("#let sqrt(x) = x\n$sqr|t(4)$")
            .expect("a hover for the local function");
        assert!(!shadowed.contains("A square root."), "{shadowed}");
    }
}
