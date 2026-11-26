//! What one hover shows, and how its sections part from each other.

mod format;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::ops::Range;

use anyhow::Result;
use lsp_types::Hover;
use tola_build::AssetUrls;
use tola_build::cancellation::BuildCancellation;
use tola_build::check::SourceCompilation;
use tola_build::config::SiteConfig;
use tola_typst::typst::World;
use tola_typst::typst::foundations::{Styles, Value};
use tola_typst::typst::syntax::ast::{self, AstNode};
use tola_typst::typst::syntax::{LinkedNode, Side, Source, Span, SyntaxKind, VirtualRoot};
use tola_typst_syntax::docs::Documentation;
use tola_typst_syntax::names::{Declaration, DeclarationKind, SourceNames};
use typst_ide::Tooltip;

pub(in crate::query) use format::{SECTION_JOIN, says_something, shown_routes, typc_fence};
use format::{
    agreed_value, declaration_sections, described_export, function_sections, observed_type_union,
    parameter_section, sections_hover,
};

use crate::markdown;
use crate::position;
use crate::protocol::markdown_hover;
use crate::sentence::joined;

use super::citations::{self, BibliographyCache};
use super::context::{SelectedSyntax, Selection};
use super::editing;
use super::expression::ExpressionType;
use super::imports::imported_member;
use super::labels;
use super::meta::{MetaCall, meta_call, meta_key_at};
use super::name_graph::bare_name;
use super::records::declared::declared_output;
use super::records::origin::{field_chain, record_origin};
use super::records::parameters::{ParameterArgument, parameter_argument};
use super::repair::import_star;
use super::schema;
use super::semantic::{self, Semantic};
use super::site_schema::{declared_documentation, rejected_key_note};

fn parameter_declaration(names: &SourceNames, at: usize) -> Option<&Declaration> {
    names
        .declared_at(at)
        .map(|id| &names.declarations()[id])
        .filter(|d| matches!(d.kind, DeclarationKind::Parameter))
}

fn name_type(
    prep: &Source,
    observed: &[(ExpressionType, Option<Styles>)],
    decl: Option<&Declaration>,
    provisioned: Option<&ParameterArgument>,
    sem: &mut Semantic<'_>,
) -> Result<String> {
    let ty = match (observed_type_union(observed), decl) {
        (Some(ty), _) => Some(ty),
        (None, Some(decl)) => Selection::default_type(prep, &decl.range, sem)?,
        (None, None) => None,
    }
    .or_else(|| provisioned.and_then(|p| p.ty.clone()));
    Ok(ty.unwrap_or_else(|| "any".to_owned()))
}

impl Selection<'_> {
    pub(super) fn citation(
        &self,
        comp: &SourceCompilation,
        cfg: &SiteConfig,
        biblios: &mut BibliographyCache,
        cancel: &BuildCancellation,
    ) -> Result<Option<Hover>> {
        let Some(ref_) = labels::referenced(self.source, self.cursor) else {
            return Ok(None);
        };
        let name = &self.source.text()[ref_.name.clone()];
        if labels::declares(comp, name) {
            return Ok(None);
        }
        citations::hover(comp, cfg, self.source.id(), name, biblios, cancel)
    }

    pub(super) fn asset_url_argument(
        &self,
        sem: &mut Semantic<'_>,
    ) -> Result<Option<(String, Range<usize>)>> {
        let Some(call) = tola_typst_syntax::syntax::call(self.source, self.cursor) else {
            return Ok(None);
        };
        let Some(callee) = tola_typst_syntax::syntax::node_at_range(self.source, &call.callee)
        else {
            return Ok(None);
        };
        let Some(Value::Func(func)) = sem
            .callee_functions(&callee)?
            .into_iter()
            .map(|(val, _)| val)
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
        let params = func.params().collect::<Vec<_>>();
        let mut idx = 0usize;
        for item in func_call.args().items() {
            let (param, text) = match item {
                ast::Arg::Pos(ast::Expr::ContentBlock(_)) | ast::Arg::Spread(_) => continue,
                ast::Arg::Pos(ast::Expr::Str(text)) => {
                    let param = params.get(idx).and_then(|p| p.name());
                    idx += 1;
                    (param.map(|name| name.to_string()), text)
                }
                ast::Arg::Pos(_) => {
                    idx += 1;
                    continue;
                }
                ast::Arg::Named(named) => match named.expr() {
                    ast::Expr::Str(text) => (Some(named.name().get().to_string()), text),
                    _ => continue,
                },
            };
            let Some(param) = param else {
                continue;
            };
            let Some(bytes) = call_node.find(text.span()).and_then(|node| {
                let r = node.range();
                (r.end >= r.start + 2).then(|| r.start + 1..r.end - 1)
            }) else {
                continue;
            };
            if !(bytes.start..=bytes.end).contains(&self.cursor) {
                continue;
            }
            return Ok(matches!(
                crate::assets::domain_of(&func, param.as_str()),
                Some(tola_packages::ArgumentDomain::SiteAssetUrl)
            )
            .then(|| (self.source.text()[bytes.clone()].to_owned(), bytes)));
        }
        Ok(None)
    }

    pub(super) fn asset_url_hover(
        &self,
        cfg: &SiteConfig,
        boundary: &tola_typst::SourceBoundary,
        sem: &mut Semantic<'_>,
        cancel: &BuildCancellation,
    ) -> Result<Option<Hover>> {
        let Some((typed, bytes)) = self.asset_url_argument(sem)? else {
            return Ok(None);
        };
        let urls = AssetUrls::for_check_project(cfg, boundary, cancel)?;
        let desc = match crate::assets::written(&urls, &typed) {
            crate::assets::Written::Published(asset) => {
                let declared = asset
                    .origin
                    .map(|origin| {
                        format!(
                            ", declared by {}",
                            crate::assets::declaration_note(origin, cfg.root())
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
            .map(|range| markdown_hover(desc, Some(range))))
    }

    pub(super) fn hover(
        &self,
        comp: &SourceCompilation,
        cfg: &SiteConfig,
        cancel: &BuildCancellation,
        prep: &Source,
        sem: &mut Semantic<'_>,
    ) -> Result<Option<Hover>> {
        if let SelectedSyntax::ImportedName(item) = &self.syntax {
            return self.hover_imported(comp, cfg, cancel, prep, sem, item);
        }
        if let Some(star) = import_star(self.source, self.cursor) {
            return self.hover_star(prep, sem, &star);
        }
        if let Some(hover) = self.meta_hover(comp, cfg, prep, sem)? {
            return Ok(Some(hover));
        }
        if let Some(named) = Self::named_argument(prep, self.cursor) {
            if let Some(hover) = self.parameter_hover(prep, &named, sem)? {
                return Ok(Some(hover));
            }
            let parent = named.parent().map(|p| p.kind());
            if parent == Some(SyntaxKind::Dict) {
                return Self::described_field_hover(prep, &named, sem);
            }
            if parent != Some(SyntaxKind::Params) {
                return Ok(None);
            }
        }
        let Some(node) = tola_typst_syntax::syntax::expression(prep, self.cursor) else {
            return Ok(None);
        };
        // A repaired node is a stand-in; this lane answers nothing about it.
        if !self.range_keeps_source_text(prep, node.range()) {
            return Ok(None);
        }
        let rooted = tola_typst_syntax::syntax::root_expression(node.clone());
        if rooted.kind() != SyntaxKind::Ident {
            return Ok(None);
        }
        self.value_hover(prep, &node, sem)
    }

    fn value_hover(
        &self,
        prep: &Source,
        node: &LinkedNode<'_>,
        sem: &mut Semantic<'_>,
    ) -> Result<Option<Hover>> {
        let names = self.names_of(prep);
        let decl = names
            .declared_at(self.cursor)
            .map(|id| &names.declarations()[id]);
        let observed = sem.types(node)?;
        let (root, fields) = field_chain(node).unwrap_or_else(|| (node.clone(), Vec::new()));
        let name = &prep.text()[node.range()];
        let param = parameter_declaration(&names, self.cursor)
            .or_else(|| parameter_declaration(&names, root.range().start));
        let provisioned = match param {
            Some(decl) if observed.is_empty() => parameter_argument(prep, &names, decl, sem)?,
            _ => None,
        };
        let origin = match (record_origin(&names, root, sem)?, &provisioned) {
            (Some(origin), _) => Some(origin),
            (None, Some(provisioned)) => provisioned.origin.clone(),
            (None, None) => None,
        };
        let mut declared = match &origin {
            Some(origin) => declared_output(origin, &fields, name, sem)?,
            None => None,
        };
        if declared.is_none()
            && fields.is_empty()
            && let Some(provisioned) = &provisioned
        {
            declared = provisioned.declared.clone();
        }
        if observed.is_empty() && decl.is_none() && declared.is_none() {
            return Ok(None);
        }
        let named = node.kind() == SyntaxKind::Ident;
        let observed_params = self.observed_parameters(prep, node, decl, sem)?;
        let mut docs = sem
            .definition(node)?
            .and_then(|span| sem.source_docs(span))
            .or_else(|| {
                decl.and_then(|d| tola_typst_syntax::docs::declaration_docs(prep, d.range.start))
            });

        let mut sections = Vec::new();
        match agreed_value(&observed) {
            Some(Value::Func(func)) => {
                docs = semantic::function_docs(func).map(str::to_owned).or(docs);
                let documented = docs.take().map(|d| Documentation::parse(&d));
                sections.extend(function_sections(
                    name,
                    named,
                    func,
                    documented.as_ref(),
                    &observed_params,
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
                let ty = name_type(prep, &observed, decl, provisioned.as_ref(), sem)?;
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

    fn observed_parameters(
        &self,
        prep: &Source,
        node: &LinkedNode<'_>,
        decl: Option<&Declaration>,
        sem: &mut Semantic<'_>,
    ) -> Result<BTreeMap<String, String>> {
        let Some(decl) = decl.filter(|d| matches!(d.kind, DeclarationKind::Let)) else {
            return Ok(BTreeMap::new());
        };
        let mut args = BTreeMap::new();
        if let Some(parent) = node.parent().cloned()
            && let Some(call) = parent.cast::<ast::FuncCall>()
        {
            for arg in call.args().items() {
                let ast::Arg::Named(named) = arg else {
                    continue;
                };
                if let Some(val) = prep.find(named.expr().span()) {
                    args.insert(named.name().get().to_string(), val.range());
                }
            }
        }
        let Some(ident) = tola_typst_syntax::syntax::node_at_range(prep, &decl.range) else {
            return Ok(BTreeMap::new());
        };
        let Some(parent) = ident.parent().cloned() else {
            return Ok(BTreeMap::new());
        };
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
            let (name, span) = match param {
                ast::Param::Pos(pattern) => {
                    let ast::Pattern::Normal(ast::Expr::Ident(name)) = pattern else {
                        continue;
                    };
                    (name.get().to_string(), pattern.span())
                }
                ast::Param::Named(named) => (named.name().get().to_string(), named.name().span()),
                ast::Param::Spread(spread) => match spread.sink_ident() {
                    Some(name) => (name.get().to_string(), spread.span()),
                    None => continue,
                },
            };
            let node = args
                .get(&name)
                .and_then(|range| tola_typst_syntax::syntax::node_at_range(prep, range))
                .or_else(|| prep.find(span));
            let Some(node) = node else {
                continue;
            };
            let types = sem.types(&node)?;
            if let Some(ty) = observed_type_union(&types) {
                observed.insert(name, ty);
            }
        }
        Ok(observed)
    }

    fn named_argument(prep: &Source, pos: usize) -> Option<LinkedNode<'_>> {
        let root = LinkedNode::new(prep.root());
        let mut node = [Side::Before, Side::After]
            .into_iter()
            .find_map(|side| root.leaf_at(pos, side))?;
        loop {
            if let Some(named) = node.cast::<ast::Named>() {
                let range = prep.find(named.name().span())?.range();
                return (pos >= range.start && pos <= range.end).then_some(node);
            }
            node = node.parent()?.clone();
        }
    }

    fn parameter_hover(
        &self,
        prep: &Source,
        named: &LinkedNode<'_>,
        sem: &mut Semantic<'_>,
    ) -> Result<Option<Hover>> {
        let Some(args) = named.parent() else {
            return Ok(None);
        };
        let Some(call) = args.parent().and_then(|c| c.cast::<ast::FuncCall>()) else {
            return Ok(None);
        };
        let Some(arg) = named.cast::<ast::Named>() else {
            return Ok(None);
        };
        let name = arg.name().get().to_string();
        let traced = prep
            .find(call.callee().span())
            .map(|callee| sem.known_values(&callee))
            .transpose()?
            .and_then(|vals| vals.into_iter().next())
            .and_then(|(val, _)| match val {
                Value::Func(func) => Some(func),
                _ => None,
            });
        let Some(func) = traced else {
            return Ok(None);
        };
        if !func.params().any(|p| p.name() == Some(name.as_str())) {
            return Ok(None);
        }
        let observed = prep
            .find(arg.expr().span())
            .map(|val| sem.types(&val))
            .transpose()?
            .unwrap_or_default();
        let observed = observed_type_union(&observed);
        let documented = match prep.find(call.callee().span()) {
            Some(callee) => sem
                .definition(&callee)?
                .and_then(|span| sem.source_docs(span)),
            None => None,
        }
        .or_else(|| func.docs().map(str::to_owned))
        .map(|docs| Documentation::parse(&docs));
        let observed = match observed {
            Some(observed) => BTreeMap::from([(name.clone(), observed)]),
            None => BTreeMap::new(),
        };
        let mut sig = semantic::signature(
            func.name().unwrap_or_default(),
            &func,
            documented.as_ref(),
            &observed,
        );
        sig.parameters.retain(|p| p.name == name);
        let Some(section) = parameter_section(&sig) else {
            return Ok(None);
        };
        Ok(Some(markdown_hover(
            section,
            position::utf16_range(self.source.lines(), named.range()),
        )))
    }

    fn described_field_hover(
        prep: &Source,
        named: &LinkedNode<'_>,
        sem: &mut Semantic<'_>,
    ) -> Result<Option<Hover>> {
        let Some(field) = named.cast::<ast::Named>() else {
            return Ok(None);
        };
        let Some(val) = prep.find(field.expr().span()) else {
            return Ok(None);
        };
        let name = field.name().get().to_string();
        let mut described: Vec<schema::DeclaredField> = Vec::new();
        for (val, _) in sem.known_values(&val)? {
            if let Some(declared) = sem
                .inspect_schema(&val, false)?
                .and_then(schema::SchemaDescription::read)
                .map(|desc| desc.field(&name))
                && declared.documentation.is_some()
                && !described.contains(&declared)
            {
                described.push(declared);
            }
        }
        if described.is_empty() {
            return Ok(None);
        }
        let range = prep
            .find(field.name().span())
            .and_then(|name| position::utf16_range(prep.lines(), name.range()));
        Ok(Some(markdown_hover(
            described
                .iter()
                .map(schema::DeclaredField::section)
                .collect::<Vec<_>>()
                .join(SECTION_JOIN),
            range,
        )))
    }

    fn meta_hover(
        &self,
        comp: &SourceCompilation,
        cfg: &SiteConfig,
        prep: &Source,
        sem: &mut Semantic<'_>,
    ) -> Result<Option<Hover>> {
        let Some(call) = meta_call(self.source, self.cursor, &self.names_of(self.source)) else {
            return Ok(None);
        };
        if call.callee.start <= self.cursor && self.cursor <= call.callee.end {
            return self.meta_callee_hover(comp, prep, sem, &call);
        }
        self.meta_key_hover(comp, cfg, sem, &call)
    }

    fn meta_key_hover(
        &self,
        comp: &SourceCompilation,
        cfg: &SiteConfig,
        sem: &mut Semantic<'_>,
        call: &MetaCall,
    ) -> Result<Option<Hover>> {
        let Some(name) = meta_key_at(self.source, self.cursor, call) else {
            return Ok(None);
        };
        let schemas = self.meta_schemas(comp, cfg, sem)?;
        let key = &self.source.text()[name.clone()];
        let mut declared = Vec::new();
        for schema in &schemas.shapes {
            for (field, desc) in schema.shape.fields() {
                if field == key {
                    declared.push((desc.field(&field), schema.declaration()));
                }
            }
        }
        if declared.is_empty() {
            return Ok(None);
        }
        let mut docs = declared_documentation(&declared, &schemas);
        let rejecting = schemas
            .shapes
            .iter()
            .filter(|s| schemas.covered && s.rejects(key))
            .count();
        if rejecting > 0 {
            let required = schemas.shapes.iter().any(|s| s.requires(key));
            docs.push_str(&format!(
                "{SECTION_JOIN}{}",
                rejected_key_note(rejecting, required)
            ));
        }
        Ok(Some(markdown_hover(
            docs,
            position::utf16_range(self.source.lines(), name),
        )))
    }

    fn meta_callee_hover(
        &self,
        comp: &SourceCompilation,
        prep: &Source,
        sem: &mut Semantic<'_>,
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
            tola_typst_syntax::syntax::node_at_range(prep, &imported.import.source_range)
        else {
            return Ok(None);
        };
        let Some((val, span)) = sem.imported_export(&module, &imported.path)? else {
            return Ok(None);
        };
        let name = &self.source.text()[call.callee.clone()];
        let mut described = described_export(sem, name, &val, span);
        if let Some(home) = span.id().map(|id| labels::source_path(comp, id)) {
            described.push_str(&format!("\n\nDeclared in `{home}`."));
        }
        Ok(Some(markdown_hover(
            described,
            position::utf16_range(self.source.lines(), call.callee.clone()),
        )))
    }

    fn default_type(
        prep: &Source,
        range: &Range<usize>,
        sem: &mut Semantic<'_>,
    ) -> Result<Option<String>> {
        let Some(default) = tola_typst_syntax::syntax::node_at_range(prep, range)
            .and_then(|ident| ident.parent().cloned())
            .filter(|param| param.kind() == SyntaxKind::Named)
            .and_then(|param| param.children().next_back())
        else {
            return Ok(None);
        };
        let types = sem.types(&default)?;
        Ok(observed_type_union(&types))
    }

    fn hover_star(
        &self,
        prep: &Source,
        sem: &mut Semantic<'_>,
        star: &LinkedNode<'_>,
    ) -> Result<Option<Hover>> {
        let Some(import) = star.parent() else {
            return Ok(None);
        };
        let Some(lit) = import.children().find(|c| c.kind() == SyntaxKind::Str) else {
            return Ok(None);
        };
        let Some(module) = tola_typst_syntax::syntax::node_at_range(prep, &lit.range()) else {
            return Ok(None);
        };
        let listed: Vec<String> = sem
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
        comp: &SourceCompilation,
        cfg: &SiteConfig,
        cancel: &BuildCancellation,
        span: Span,
    ) -> Result<Option<String>> {
        let Some(id) = span.id() else {
            return Ok(None);
        };
        if id.root() != &VirtualRoot::Project {
            return Ok(None);
        }
        let routes = crate::routes::site_routes(comp, cfg, id, cancel)?;
        let listed = shown_routes(routes.iter().map(|r| r.route.as_str()));
        Ok((!listed.is_empty()).then(|| format!("`{listed}`")))
    }

    pub(super) fn builtin_hover(
        &self,
        world: &tola_typst::TypstWorld,
        prep: &Source,
        pos: usize,
    ) -> Option<Hover> {
        let (mut docs, vals) = match editing::tooltip(world, prep, pos) {
            Some(Tooltip::Text(text)) => (Some(text.to_string()), None),
            Some(Tooltip::Code(code)) => (None, Some(code.to_string())),
            None => (None, None),
        };
        let selected = tola_typst_syntax::syntax::expression(prep, pos)
            .filter(|node| matches!(node.kind(), SyntaxKind::Ident | SyntaxKind::MathIdent));
        let name = selected
            .as_ref()
            .map(|node| &prep.text()[node.range()])
            .or_else(|| bare_name(prep, pos));
        let math = selected
            .as_ref()
            .is_some_and(|node| node.kind() == SyntaxKind::MathIdent);
        let names = self.names_of(prep);
        let sig = names
            .reads_value_at(pos)
            .then_some(name)
            .flatten()
            .filter(|_| names.declared_at(pos).is_none())
            .and_then(|name| {
                let val = if math {
                    world.library().math.scope().get(name)
                } else {
                    world.library().global.scope().get(name)
                }?
                .read()
                .clone();
                let Value::Func(func) = val else {
                    return None;
                };
                Some((name.to_owned(), func))
            });
        let line = if let Some((name, func)) = sig {
            docs = docs.or_else(|| semantic::function_docs(&func).map(str::to_owned));
            Some(semantic::signature(&name, &func, None, &BTreeMap::new()).code())
        } else {
            None
        };
        let mut sections = Vec::new();
        if let Some(line) = line {
            sections.push(typc_fence(&line));
        }
        if let Some(code) = vals {
            sections.push(typc_fence(&code));
        }
        if let Some(docs) = docs {
            sections.push(markdown::docs(&docs));
        }
        sections_hover(sections, None)
    }

    fn hover_imported(
        &self,
        comp: &SourceCompilation,
        cfg: &SiteConfig,
        cancel: &BuildCancellation,
        prep: &Source,
        sem: &mut Semantic<'_>,
        item: &tola_typst_syntax::syntax::ImportedName,
    ) -> Result<Option<Hover>> {
        let Some(module) = tola_typst_syntax::syntax::node_at_range(prep, &item.source) else {
            return Ok(None);
        };
        let Some((val, span)) = sem.imported_export(&module, &item.path)? else {
            return Ok(None);
        };
        let Some(name) = item.path.last() else {
            return Ok(None);
        };
        let mut described = described_export(sem, name, &val, span);
        if let Some(home) = span.id().map(|id| labels::source_path(comp, id)) {
            described.push_str(&format!("\n\nDeclared in `{home}`"));
            if let Some(routes) = self.checked_routes(comp, cfg, cancel, span)? {
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
