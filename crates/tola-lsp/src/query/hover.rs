//! What one hover shows, and how its sections part from each other.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use anyhow::Result;
use lsp_types::Hover;
use tola_build::AssetUrls;
use tola_build::cancellation::BuildCancellation;
use tola_build::check::SourceCompilation;
use tola_build::config::ResolvedSiteConfig;
use tola_packages::Signature;
use tola_typst::typst::World;
use tola_typst::typst::foundations::Value;
use tola_typst::typst::syntax::ast::{self, AstNode};
use tola_typst::typst::syntax::{LinkedNode, Side, Source, Span, SyntaxKind, VirtualRoot};
use typst_ide::Tooltip;

use crate::markdown;
use crate::position;
use crate::protocol::markdown_hover;
use crate::sentence::{joined, listed};

use super::citations::{self, BibliographyCache};
use super::context::{SelectedSyntax, Selection};
use super::editing;
use super::imports::imported_member;
use super::labels;
use super::meta::{MetaCall, meta_call, meta_key_at};
use super::name_graph::bare_name;
use super::records::declared::declared_output;
use super::records::origin::{field_chain, record_origin};
use super::records::parameters::parameter_argument;
use super::repair::import_star;
use super::schema;
use super::semantic::{self, Semantic};
use super::site_schema::{declared_documentation, rejected_key_note};

/// Whether a hover says anything, which a markdown envelope of whitespace does not.
pub(super) fn says_something(hover: &lsp_types::Hover) -> bool {
    match &hover.contents {
        lsp_types::HoverContents::Markup(markup) => !markup.value.trim().is_empty(),
        lsp_types::HoverContents::Scalar(marked) => match marked {
            lsp_types::MarkedString::String(text) => !text.trim().is_empty(),
            lsp_types::MarkedString::LanguageString(code) => !code.value.trim().is_empty(),
        },
        lsp_types::HoverContents::Array(marked) => marked.iter().any(|marked| match marked {
            lsp_types::MarkedString::String(text) => !text.trim().is_empty(),
            lsp_types::MarkedString::LanguageString(code) => !code.value.trim().is_empty(),
        }),
    }
}
/// The hover markdown for one described name: its signatures, then its documentation.
fn described(descriptions: &[String], docs: Option<&str>) -> String {
    let mut text = if descriptions.is_empty() {
        String::new()
    } else {
        format!("```typc\n{}\n```", descriptions.join("\n"))
    };
    if let Some(docs) = docs {
        if !text.is_empty() {
            text.push_str(SECTION_JOIN);
        }
        text.push_str(&markdown::docs(docs));
    }
    text
}

/// How one hover's sections part from each other, as Tinymist parts them
/// (Apache-2.0; see `licenses/README.md`).
pub(in crate::query) const SECTION_JOIN: &str = "\n\n---\n\n";

/// How many fields one record shape lists before the remainder is counted.
const SHAPE_FIELD_LIMIT: usize = 20;

/// The parameters of a function as the sections Tinymist prints: each group headed, each
/// parameter named, its type, then the documentation that describes it.
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
            text.push_str("\n\n```typc\ntype: ");
            text.push_str(parameter.ty_spelling());
            text.push_str("\n```");
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

impl Selection<'_> {
    /// The hover a citation answers with, or `None` when the cursor is not on a key a bibliography
    /// declares.
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
    /// The text of an `asset-url` argument the cursor sits in, with the bytes it occupies.
    ///
    /// The parameter's declared domain decides whether an argument names a published URL, and the
    /// callee's own metadata decides the parameter, so an alias, an imported binding, or a
    /// re-export keeps the answer.
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
            // A content block is the call's body rather than a value of the parameter it sits in,
            // and a named argument names itself, exactly as the parameter hints read them.
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

    /// The hover an `asset-url` argument earns: where the URL it writes is published, or that no
    /// `assets` declaration of this site publishes it.
    pub(super) fn asset_url_hover(
        &self,
        config: &ResolvedSiteConfig,
        semantics: &mut Semantic<'_>,
        cancellation: &BuildCancellation,
    ) -> Result<Option<Hover>> {
        let Some((typed, bytes)) = self.asset_url_argument(semantics)? else {
            return Ok(None);
        };
        let urls = AssetUrls::for_check(config, cancellation)?;
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
        // A `#tola-meta` call answers from the site's schemas: its callee as the package function,
        // its existing keys as the fields the site declares.
        if let Some(hover) = self.meta_hover(compilation, config, prepared, semantics)? {
            return Ok(Some(hover));
        }
        // A named argument's name answers the parameter it fills; a dict field's key answers the
        // documentation its value's `describe` call has. A parameter declaration spells the
        // same shape, so it keeps its name for the rungs below; anything else names no value of
        // its own, and the value lane must not read it as one.
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
        // A node the query copy repaired is a stand-in: its compiled value answers about a
        // fabricated construct, so this lane answers nothing about it.
        if !self.range_keeps_source_text(prepared, node.range()) {
            return Ok(None);
        }
        // Only a name has a value this lane describes: an identifier in code, or a chain
        // rooted at one. Anything else — a literal, a call, a content block — names no value of
        // its own.
        let rooted = tola_typst_syntax::syntax::root_expression(node.clone());
        if rooted.kind() != SyntaxKind::Ident {
            return Ok(None);
        }
        self.value_hover(prepared, &node, semantics)
    }

    /// Hover for one name the value lane observed.
    ///
    /// A name bound to a function answers its definition line and, when this file declares it, the
    /// types the world observed at its parameters; a name bound to a module answers where the
    /// module is bound and what it is; anything else answers the declaration its author reads and
    /// the type the world observed. A record chain answers the shape its file's schema declares,
    /// observed records or none. A declared name with neither an observation nor a declared shape
    /// still answers `any`: the declaration itself is the fact.
    fn value_hover(
        &self,
        prepared: &Source,
        node: &LinkedNode<'_>,
        semantics: &mut Semantic<'_>,
    ) -> Result<Option<Hover>> {
        let names = Arc::new(tola_typst_syntax::names::SourceNames::new(prepared.clone()));
        let declaration = names
            .declared_at(self.cursor)
            .map(|id| &names.declarations()[id]);
        let observed = semantics.values(node)?;
        // The declared shape of one chain is a fact of the source, not of the realized site: a
        // chain answers it whether or not this execution observed any record the chain reads.
        let (root, fields) = match field_chain(node) {
            Some((root, fields)) => (root, fields),
            None => (node.clone(), Vec::new()),
        };
        let name = &prepared.text()[node.range()];
        // A field chain reads its root's value, and a field names no declaration of its own, so a
        // parameter's proof comes from the root's declaration.
        let parameter = match declaration {
            Some(declaration)
                if matches!(
                    declaration.kind,
                    tola_typst_syntax::names::DeclarationKind::Parameter
                ) =>
            {
                Some(declaration)
            }
            _ => names
                .declared_at(root.range().start)
                .map(|id| &names.declarations()[id])
                .filter(|declaration| {
                    matches!(
                        declaration.kind,
                        tola_typst_syntax::names::DeclarationKind::Parameter
                    )
                }),
        };
        // A parameter's value is the argument each call passes, which the world observed only
        // where a call ran: with no observation, the site's own calls still prove it, and the
        // record that argument names is what a chain over the parameter reads.
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
        // A member path names no declaration of its own, so the signature, module, and type lines
        // answer a bare name only.
        let named = node.kind() == SyntaxKind::Ident;
        // A named argument holds the value its parameter takes, and the world observed that value
        // at the argument itself.
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
        let observed_parameters = match declaration {
            Some(declaration)
                if matches!(
                    declaration.kind,
                    tola_typst_syntax::names::DeclarationKind::Let
                ) =>
            {
                self.observed_parameter_types(prepared, &declaration.range, &arguments, semantics)?
            }
            _ => BTreeMap::new(),
        };
        let mut docs = semantics
            .definition(node)?
            .and_then(|span| semantics.source_docs(span));
        // A declaration the world observed no value at — a parameter, whose value is the
        // argument — still has the block its author wrote above it.
        if docs.is_none()
            && let Some(declaration) = declaration
        {
            docs = tola_typst_syntax::docs::declaration_docs(prepared, declaration.range.start);
        }

        let mut sections = Vec::new();
        match observed.first().map(|(value, _)| value) {
            Some(Value::Func(func)) => {
                docs = semantic::function_docs(func).map(str::to_owned).or(docs);
                let documented = docs
                    .take()
                    .map(|docs| tola_typst_syntax::docs::Documentation::parse(&docs));
                let signature =
                    semantic::signature(name, func, documented.as_ref(), &observed_parameters);
                if named {
                    sections.push(format!("```typc\n{}\n```", signature.code()));
                }
                // The block's summary describes the declaration; its parameter descriptions
                // belong with the parameters the signature section shows.
                if let Some(documentation) = documented
                    && !documentation.summary.is_empty()
                {
                    sections.push(markdown::docs(&documentation.summary));
                }
                if let Some(section) = parameter_section(&signature) {
                    sections.push(section);
                }
            }
            Some(Value::Module(module)) => {
                if named {
                    let bound = module
                        .name()
                        .map_or_else(|| name.to_owned(), ToString::to_string);
                    sections.push(format!("```typc\nlet {bound};\n```"));
                }
            }
            _ => {
                let ty = observed
                    .first()
                    .map(|(value, _)| semantic::type_spelling(&value.ty()))
                    .or_else(|| {
                        declaration.and_then(|declaration| {
                            self.default_type(prepared, &declaration.range, semantics)
                        })
                    })
                    .or_else(|| {
                        provisioned
                            .as_ref()
                            .and_then(|provisioned| provisioned.ty.clone())
                    })
                    .unwrap_or_else(|| "any".to_owned());
                // A bare `any` says nothing next to a declared shape; a type the world observed
                // stays, because it reads what `any` cannot.
                if named && (ty != "any" || declared.is_none()) {
                    sections.push(format!("```typc\nlet {name} = {ty};\n```"));
                }
            }
        }
        // A chain over site records answers the shape of those records, never one dictionary per
        // realized document: with many sources every hover would describe the whole site. The shape
        // is declared by the source — a schema, the descriptors, or a project function's body —
        // so it answers even where the site realized no record at all.
        if let Some(declared) = &declared {
            sections.push(shape_section("Declared fields", &declared.lines));
        }
        if let Some(summary) = docs
            .map(|docs| tola_typst_syntax::docs::Documentation::parse(&docs))
            .map(|documentation| documentation.summary)
            .filter(|summary| !summary.is_empty())
        {
            sections.push(markdown::docs(&summary));
        }
        if let Some(declared) = &declared
            && let Some(description) = &declared.documentation
        {
            sections.push(markdown::docs(description));
        }
        if sections.is_empty() {
            return Ok(None);
        }
        Ok(Some(markdown_hover(
            sections.join(SECTION_JOIN),
            position::utf16_range(self.source.lines(), node.range()),
        )))
    }

    /// The type the world observed at each parameter of the function one declaration binds, keyed
    /// by parameter name: the call's own argument where it names one, the declaration's value
    /// otherwise.
    fn observed_parameter_types(
        &self,
        prepared: &Source,
        range: &std::ops::Range<usize>,
        arguments: &BTreeMap<String, std::ops::Range<usize>>,
        semantics: &mut Semantic<'_>,
    ) -> Result<BTreeMap<String, String>> {
        let Some(ident) = tola_typst_syntax::syntax::node_at_range(prepared, range) else {
            return Ok(BTreeMap::new());
        };
        let Some(parent) = ident.parent().cloned() else {
            return Ok(BTreeMap::new());
        };
        // The sugar `let name(parameters) = body` hangs the parameters on the closure the name
        // belongs to; a closure bound as a value has them on its own node.
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
            if let Some((value, _)) = semantics.values(&node)?.first() {
                observed.insert(name, semantic::type_spelling(&value.ty()));
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
            if node.kind() == SyntaxKind::Named {
                let named = node.cast::<ast::Named>()?;
                let range = prepared.find(named.name().span())?.range();
                return (cursor >= range.start && cursor <= range.end).then_some(node);
            }
            node = node.parent()?.clone();
        }
    }

    /// Hover for the name of a named argument: the parameter it fills, with what the world
    /// observed at the argument itself.
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
        // A function the world observed at the callee is the one whose parameters this name can
        // fill; anything else leaves the name to the rungs below.
        let traced = prepared
            .find(call.callee().span())
            .map(|callee| semantics.values(&callee))
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
            .map(|value| semantics.values(&value))
            .transpose()?
            .unwrap_or_default()
            .first()
            .map(|(value, _)| semantic::type_spelling(&value.ty()));
        // The block above the callee's declaration describes the parameter this argument fills.
        let documented = match prepared.find(call.callee().span()) {
            Some(callee) => semantics
                .definition(&callee)?
                .and_then(|span| semantics.source_docs(span)),
            None => None,
        }
        .or_else(|| func.docs().map(str::to_owned))
        .map(|docs| tola_typst_syntax::docs::Documentation::parse(&docs));
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
        for (value, _) in semantics.values(&value)? {
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

    /// The answer a `#tola-meta` call earns: the package function its callee names, or the field a
    /// key inside its dictionary is declared with.
    fn meta_hover(
        &self,
        compilation: &SourceCompilation,
        config: &ResolvedSiteConfig,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
    ) -> Result<Option<Hover>> {
        let Some(call) = meta_call(self.source, self.cursor) else {
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

    /// The answer a `#tola-meta` callee earns: the package function the file's import binds,
    /// whether or not the call itself evaluated.
    fn meta_callee_hover(
        &self,
        compilation: &SourceCompilation,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
        call: &MetaCall,
    ) -> Result<Option<Hover>> {
        let names = tola_typst_syntax::names::SourceNames::new(self.source.clone());
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
        let docs = match &value {
            Value::Func(func) => semantic::function_docs(func).map(str::to_owned),
            _ => None,
        }
        .or_else(|| semantics.source_docs(span));
        let line = match &value {
            Value::Func(func) => semantic::signature(name, func, None, &BTreeMap::new()).code(),
            _ => semantic::description(&value, name),
        };
        let mut described = described(&[line], docs.as_deref());
        if let Some(home) = self.declaring_file(compilation, span) {
            described.push_str(&format!("\n\nDeclared in `{home}`."));
        }
        Ok(Some(markdown_hover(
            described,
            position::utf16_range(self.source.lines(), call.callee.clone()),
        )))
    }

    /// The type one declaration's default expression gives it, when the world observed it.
    fn default_type(
        &self,
        prepared: &Source,
        range: &std::ops::Range<usize>,
        semantics: &mut Semantic<'_>,
    ) -> Option<String> {
        // A named parameter's default is the last child of its own node; the world observed its
        // literal when it observed the parameter at all.
        let default = tola_typst_syntax::syntax::node_at_range(prepared, range)
            .and_then(|ident| ident.parent().cloned())
            .filter(|param| param.kind() == SyntaxKind::Named)
            .and_then(|param| param.children().next_back())?;
        let observed = semantics.values(&default).ok()?;
        Some(semantic::type_spelling(&observed.first()?.0.ty()))
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
        let names: Vec<String> = semantics
            .import_bindings(&module, &[])?
            .into_keys()
            .collect();
        if names.is_empty() {
            return Ok(None);
        }
        let listed: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
        Ok(Some(markdown_hover(
            format!("This star imports {}.", joined(&listed)),
            position::utf16_range(self.source.lines(), star.range()),
        )))
    }

    /// The site-relative path of the file that declares what `span` names.
    fn declaring_file(&self, compilation: &SourceCompilation, span: Span) -> Option<String> {
        let world = compilation.world();
        let id = span.id()?;
        let path = match id.root() {
            VirtualRoot::Package(spec) => spec.to_string(),
            VirtualRoot::Project => tola_build::filesystem::display_path(
                &world.root().join(id.vpath().get_without_slash()),
                world.root(),
            ),
        };
        Some(path)
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
        let (mut listed, remaining) = listed(
            routes.iter().map(|route| route.route.as_str()),
            crate::routes::NAMED_PAGES,
        );
        if remaining > 0 {
            listed.push_str(&format!(" and {remaining} more"));
        }
        Ok((!listed.is_empty()).then(|| format!("`{listed}`")))
    }

    /// The hover Typst's own reader answers, shaped into the sections an author reads.
    ///
    /// The reader reports what it observed at the cursor, and the library's own metadata names what
    /// the name stands for: a library function answers its signature, then the reader's own
    /// representation of the value the name reads, then the documentation the library or the
    /// reader has.
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
        // The name the cursor stands in. An embedded callee writes `#` directly before its name, so
        // the byte the cursor sits on is the name's own and the reader's leaf sides miss it.
        let name = tola_typst_syntax::syntax::expression(prepared, cursor)
            .filter(|node| node.kind() == SyntaxKind::Ident)
            .map(|node| &prepared.text()[node.range()])
            .or_else(|| bare_name(prepared, cursor));
        // Only a name that reads a value answers by spelling: a declaration, import item, argument
        // label, dict key, or field name reads nothing here, and a name the file itself binds is
        // that binding's, not the standard library's.
        let names = tola_typst_syntax::names::SourceNames::new(prepared.clone());
        let signature = names
            .reads_value_at(cursor)
            .then_some(name)
            .flatten()
            .filter(|_| names.declared_at(cursor).is_none())
            .and_then(|name| {
                let value = world.library().global.scope().get(name)?.read().clone();
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
            sections.push(format!("```typc\n{line}\n```"));
        }
        if let Some(code) = values {
            sections.push(format!("```typc\n{code}\n```"));
        }
        if let Some(docs) = docs {
            sections.push(markdown::docs(&docs));
        }
        (!sections.is_empty()).then(|| markdown_hover(sections.join(SECTION_JOIN), None))
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
        let docs = match &value {
            Value::Func(func) => semantic::function_docs(func).map(str::to_owned),
            _ => None,
        }
        .or_else(|| semantics.source_docs(span));
        let line = match &value {
            Value::Func(func) => semantic::signature(name, func, None, &BTreeMap::new()).code(),
            _ => semantic::description(&value, name),
        };
        let mut described = described(&[line], docs.as_deref());
        if let Some(home) = self.declaring_file(compilation, span) {
            described.push_str(&format!("\n\nDeclared in `{home}`"));
            if let Some(routes) = self.checked_routes(compilation, config, cancellation, span)? {
                described.push_str(&format!(", pages from current source: {routes}"));
            }
            described.push('.');
        }
        Ok(Some(markdown_hover(
            described,
            position::utf16_range(self.source.lines(), item.source.clone()),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{CheckProgress, SourceReply};

    use crate::query::tests::*;
    use lsp_types::request as lsp_request;
    use lsp_types::request::Request as LspRequest;
    use serde_json::json;

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

    /// The cursor on a name's first character answers it, as one inside the name does.
    #[test]
    fn hover_answers_the_name_at_the_cursor() {
        let mut site = QuerySession::new();
        // The byte before the cursor sits in the space before `scale`, so the name it opens is the
        // one the author means; the middle of the name names it outright.
        for marked in ["#let scale = 3\n#sca|le\n", "#let scale = 3\n#sc|ale\n"] {
            let text = site
                .hover_text(marked)
                .unwrap_or_else(|| panic!("{marked:?} asks about `scale`"));
            assert!(
                text.contains("let scale = int;"),
                "{marked:?} answered {text}"
            );
        }
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

    /// A schema field's key answers the declaration and documentation the `@tola/schema`
    /// `describe` call at its value has, however that function reached the file.
    #[test]
    fn described_field_key_answers_its_documentation() {
        let mut site = QuerySession::new();
        for marked in [
            "#import \"@tola/schema:0.0.0\": describe, optional, schema\n#let page = schema((\n  tit|le: describe(optional(str), \"the page title\"),\n))\n",
            "#import \"@tola/schema:0.0.0\": describe as doc, optional, schema\n#let page = schema((\n  tit|le: doc(optional(str), \"the page title\"),\n))\n",
            "#import \"@tola/schema:0.0.0\" as tola-schema\n#let page = tola-schema.schema((\n  tit|le: tola-schema.describe(tola-schema.optional(str), \"the page title\"),\n))\n",
        ] {
            let text = site
                .hover_text(marked)
                .unwrap_or_else(|| panic!("{marked:?} asks about the field"));
            assert!(
                text.contains("```typc\ntitle?: str\n```"),
                "{marked:?} answered {text}"
            );
            assert!(
                text.contains("the page title"),
                "{marked:?} answered {text}"
            );
        }
    }

    /// A hover that would say nothing at all is no hover, which an editor would otherwise open an
    /// empty box for.
    #[test]
    fn blank_hover_answers_nothing() {
        for blank in ["", "\n\n", "   "] {
            assert!(!says_something(&markdown_hover(blank.to_owned(), None)));
        }
        assert!(says_something(&markdown_hover("text".to_owned(), None)));
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

    /// A library call answers in markup and inside a content block, not only in code.
    #[test]
    fn markup_answers_library_call() {
        let mut site = QuerySession::new();
        for marked in [
            "#t|ext(size: 10pt)[Body]\n",
            "#let v = 1\n#align(center)[#t|ext(size: 10pt)[#v]]\n",
        ] {
            let hover = site
                .hover_text(marked)
                .unwrap_or_else(|| panic!("{marked:?} asks about `text`"));
            assert!(hover.contains("text("), "{marked:?} answered {hover}");
        }
    }

    /// A function the file binds reports its signature where markup names it.
    #[test]
    fn markup_answers_local_function_signature() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("#let local-fn(x) = x + 1\n#local-|fn(3)\n")
            .expect("a signature from the file's own function");
        for expected in ["let local-fn(", "x: int,"] {
            assert!(hover.contains(expected), "{hover}");
        }
    }

    /// A math spelling answers what the compiler observed for it.
    #[test]
    fn math_spelling_answers_its_value() {
        let mut site = QuerySession::new();
        for (marked, value) in [
            ("$ s|in(x) $\n", "op(text: [sin], limits: false)"),
            ("$ al|pha $\n", "symbol(\"α\")"),
            ("$ ar|row.r $\n", "symbol(\n  (\"r\", \"→\"),"),
            ("$ arrow.|r $\n", "symbol(\n  \"→\","),
        ] {
            let hover = site
                .hover_text(marked)
                .unwrap_or_else(|| panic!("{marked:?} asks about a math spelling"));
            assert!(hover.contains(value), "{marked:?} answered {hover}");
        }
    }

    /// An `asset-url` argument completes with the site's published URLs — in the declaration's own
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
            .expect("an asset-url argument answers");
        assert!(hover.contains("published at"), "{hover}");
        assert!(hover.contains("/brand/logo.svg"), "{hover}");
    }

    #[test]
    fn package_absence_returns_no_hover() {
        let site = QuerySession::with_program("#let broken = absent\n");
        let package: lsp_types::Uri = "tola-package:/tola/site/0.0.0/lib.typ".parse().unwrap();
        let id = crate::identity::file_id(&package, site.site.root()).expect("a builtin package");
        let text = crate::identity::embedded_source(id)
            .expect("a builtin source")
            .into_owned();
        let source = Source::new(id, text);
        let needle = "@tola/host";
        let at = source
            .text()
            .find(needle)
            .expect("the package imports the host site value")
            + needle.len();
        let range = position::utf16_range(source.lines(), at..at).expect("a position");
        let query = decode(
            lsp_request::HoverRequest::METHOD,
            json!({
                "textDocument": { "uri": package.as_str() },
                "position": range.start,
            }),
        );
        let reply = Selection::new(&source, at, &query, CheckProgress::Failed)
            .source_only_reply(site.config.as_ref(), &query)
            .expect("an answer")
            .expect("a hover reply");
        let SourceReply::Hover(hover) = reply else {
            panic!("{reply:?}")
        };
        assert!(hover.is_none(), "{hover:?}");
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
        assert!(
            hover.starts_with("```typc\nlet text(\n  text: str,"),
            "{hover}"
        );
        assert!(hover.contains("Customizes the look"), "{hover}");
    }

    /// An element function answers the element it builds, so a binding that aliases one hovers the
    /// element's own name rather than the binding's.
    #[test]
    fn aliased_element_answers_the_element() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("#let cells = table\nBindings #cell|s\n")
            .expect("a hover for the alias");
        assert!(hover.contains(") = table;"), "{hover}");
    }
    #[test]
    fn docs_samples_keep_their_indentation() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text(
                "/// Sample:\n/// ```typ\n/// #let inside = 1\n///   #let deeper = 2\n/// ```\n\
                 #let documented = 1\nDocs #docu|mented\n",
            )
            .expect("a hover for the documented name");
        assert!(
            hover.contains("#let inside = 1\n  #let deeper = 2"),
            "{hover}"
        );
    }

    /// Typst's own `example` fence reads as the `typ` an editor has a grammar for, and any other
    /// language a sample names reaches the editor as the documentation wrote it.
    #[test]
    fn docs_example_fences_read_as_typ() {
        let mut site = QuerySession::new();
        for (docs, expected) in [("example", "```typ"), ("typc", "```typc")] {
            let marked = format!(
                "/// ```{docs}\n/// #let x = 1\n/// ```\n#let documented = 1\nDocs #docu|mented\n"
            );
            let hover = site
                .hover_text(&marked)
                .unwrap_or_else(|| panic!("a hover for `{docs}` docs"));
            assert!(
                hover.contains(&format!("{expected}\n#let x = 1\n```")),
                "{hover}"
            );
        }
    }

    /// A docs section starts at the first line the comment block writes.
    #[test]
    fn docs_sections_have_no_blank_edges() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("///\n/// Sample:\n#let documented = 1\nDocs #docu|mented\n")
            .expect("a hover for the documented name");
        assert!(hover.contains("---\n\nSample:"), "{hover}");
    }

    /// A documentation block written as the ecosystem's parameter list answers each description
    /// with the parameter it names, and keeps nothing twice.
    #[test]
    fn documented_parameters_join_their_signature() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text(
                "/// Summary of f.\n/// - x (int): old-style x doc.\n/// -> int\n\
                 #let f|(x) = x\n#f(1)\n",
            )
            .expect("a hover for the documented function");
        assert!(hover.contains("Summary of f."), "{hover}");
        assert!(hover.contains(") = int;"), "{hover}");
        let heading = hover.find("## x").expect("the parameter's heading");
        let description = hover.find("old-style x doc.").expect("the description");
        assert!(heading < description, "{hover}");
        assert_eq!(hover.matches("old-style x doc.").count(), 1, "{hover}");
    }

    /// A parameter the block describes has the type it writes and its description in its own
    /// section, and a named argument answers the description of the parameter it fills.
    #[test]
    fn documented_parameters_reach_every_answer() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("/// Summary of f.\n/// - x (int): the value.\n#let f(x) = x\n#f|\n")
            .expect("a hover for the function's use");
        assert!(hover.contains("x: int,"), "{hover}");
        assert!(
            hover.contains("## x\n\n```typc\ntype: int\n```\n\nthe value."),
            "{hover}"
        );

        let argument = site
            .hover_text(
                "/// Summary of g.\n/// - x (int): the value to scale.\n#let g(x: 1) = x\n#g(x|: 2)\n",
            )
            .expect("a hover for the named argument");
        assert!(argument.contains("the value to scale."), "{argument}");
    }

    /// A local binding answers its declaration line.
    #[test]
    fn local_binding_answers_its_declaration_line() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("#let v = 42\nLocal #v|\n")
            .expect("a hover for the binding");
        assert_eq!(hover, "```typc\nlet v = int;\n```");
    }
    /// A module binding answers where the module is bound.
    #[test]
    fn module_answers_its_binding() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("#let module-value = st|d\n")
            .expect("a hover for the module");
        assert_eq!(hover, "```typc\nlet global;\n```");
    }
    #[test]
    fn literal_method_answers_its_hover() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("#(\"a b\".spl|it())\n")
            .expect("a hover for the method");
        assert!(
            hover.contains("split") || hover.contains("array"),
            "{hover}"
        );
    }
    /// The names the engine supplies answer wherever a site writes them.
    #[test]
    fn engine_names_answer_in_site_sources() {
        let mut site = QuerySession::new();

        let items = site.completion("#let chosen = doc|\n");
        assert!(
            completion_labels(&items).contains(&"document"),
            "{:?}",
            completion_labels(&items)
        );

        let hover = site
            .hover_text("#let chosen = doc|ument\n")
            .expect("a hover for a name the engine supplies");
        assert!(hover.contains("document"), "{hover}");
    }
    /// The hover's first fenced block is the signature the shared model prints, so the editor and
    /// the other surfaces cannot drift apart.
    #[test]
    fn hover_block_matches_the_package_signature() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("#let dead() = [#t|ext(size: 10pt)[x]]\n")
            .expect("a hover for the library callee");
        use tola_typst::typst::LibraryExt as _;
        let library = tola_typst::typst::Library::builder().build();
        let func = library
            .global
            .scope()
            .get("text")
            .expect("the library binds `text`")
            .read()
            .clone();
        let Value::Func(func) = func else {
            panic!("`text` is a function")
        };
        let block = hover
            .strip_prefix("```typc\n")
            .and_then(|hover| hover.split_once("\n```"))
            .map(|(block, _)| block)
            .expect("a fenced signature block");
        assert_eq!(block, tola_packages::Signature::of("text", &func).code());
    }
}
