//! Values come from the official Bundle compiler, never the paged IDE tracer.

mod format;
mod locations;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use anyhow::Result;
use lsp_types::{Location, SignatureInformation};
use tola_build::cancellation::BuildCancellation;
use tola_build::check::SourceCompilation;
use tola_packages::Parameter;
use tola_typst::typst::Library;
use tola_typst::typst::World;
use tola_typst::typst::WorldExt;
use tola_typst::typst::comemo::Track;
use tola_typst::typst::engine::{Engine, Route, Sink, Traced};
use tola_typst::typst::foundations::{Arg, Args, Func, Str, Styles, Value};
use tola_typst::typst::introspection::{EmptyIntrospector, Introspector};
use tola_typst::typst::syntax::ast::AstNode;
use tola_typst::typst::syntax::{FileId, LinkedNode, Source, Span, Spanned, ast};
use tola_typst::typst::utils::Protected;
use tola_typst_syntax::names::{
    Declaration, DeclarationKind, ImportSource, Initializer, SourceNames,
};

pub(in crate::query) use format::{
    abbreviated, completion, description, function_docs, inline_signature, parameter_completions,
    sampled_value, signature,
};

use super::expression::{ExpressionType, members};
use super::inference::{
    ExpressionBinding, ExpressionResolver, INITIALIZER_DEPTH, Inference, binding_is_written,
    expression_name, is_name_reference,
};
use format::signature_information;
use locations::last_top_level_declaration;

#[derive(Clone)]
pub(super) struct Binding {
    pub span: Span,
    pub value: Option<Value>,
    pub tola: bool,
    pub parameter: bool,
    pub initializer: Option<Span>,
}

enum ScopeName {
    Declaration(usize),
    Import(usize),
}

pub(super) struct Semantic<'a> {
    comp: &'a SourceCompilation,
    cancel: &'a BuildCancellation,
    traces: HashMap<Span, Vec<(Value, Option<Styles>)>>,
    imports: HashMap<Span, Option<Value>>,
    names: HashMap<FileId, Arc<SourceNames>>,
    inference: Inference,
    accessed: tola_typst::AccessedDeps,
}

impl<'a> Semantic<'a> {
    pub(super) fn new(comp: &'a SourceCompilation, cancel: &'a BuildCancellation) -> Self {
        Self {
            comp,
            cancel,
            traces: HashMap::new(),
            imports: HashMap::new(),
            names: HashMap::new(),
            inference: Inference::default(),
            accessed: tola_typst::AccessedDeps::default(),
        }
    }

    pub(super) fn trace(&mut self, span: Span) -> Result<Vec<(Value, Option<Styles>)>> {
        self.cancel.ensure_active()?;
        // A body span is shared by every invocation, so an inferred call frame has no observations.
        if self.inference.has_call_frame() {
            return Ok(Vec::new());
        }
        if let Some(vals) = self.traces.get(&span) {
            return Ok(vals.clone());
        }
        let (mut vals, accessed) = tola_typst::observe_world(self.comp.world(), |world| {
            tola_typst::typst::trace::<tola_typst::typst_bundle::Bundle>(world, span)
                .into_iter()
                .collect::<Vec<_>>()
        });
        self.record_accessed(accessed);
        if vals.is_empty()
            && let Some(id) = span.id()
        {
            vals = self.trace_source(span, id);
        }
        self.cancel.ensure_active()?;
        self.traces.insert(span, vals.clone());
        Ok(vals)
    }

    fn trace_source(&mut self, span: Span, id: FileId) -> Vec<(Value, Option<Styles>)> {
        let (vals, accessed) = tola_typst::observe_world(self.comp.world(), |world| {
            let traced = Traced::new(span);
            let mut sink = Sink::new();
            let route = Route::default();
            if let Ok(src) = world.source(id) {
                let _ = tola_typst::typst_eval::eval(
                    world.track(),
                    world.library(),
                    traced.track(),
                    sink.track_mut(),
                    route.track(),
                    &src,
                );
            }
            sink.values().into_iter().collect::<Vec<_>>()
        });
        self.record_accessed(accessed);
        vals
    }

    pub(super) fn import(&mut self, src: &LinkedNode<'_>) -> Result<Option<Value>> {
        self.cancel.ensure_active()?;
        // A dynamic source may belong to another invocation; a literal import keeps its identity.
        if self.inference.has_call_frame() && src.cast::<ast::Str>().is_none() {
            return Ok(None);
        }
        if let Some(val) = self.imports.get(&src.span()) {
            return Ok(val.clone());
        }
        let val = if let Some(lit) = src.cast::<ast::Str>() {
            Some(Value::Str(lit.get().into()))
        } else {
            self.trace(src.span())?
                .into_iter()
                .next()
                .map(|(val, _)| val)
        };
        let imported = match val {
            Some(val) if val.scope().is_some() => Some(val),
            Some(Value::Str(path)) => self.imported_module(&path, src.span()),
            _ => None,
        };
        self.cancel.ensure_active()?;
        self.imports.insert(src.span(), imported.clone());
        Ok(imported)
    }

    fn imported_module(&mut self, path: &Str, span: Span) -> Option<Value> {
        let introspector: &dyn Introspector = match self.comp.introspector() {
            Some(introspector) => introspector,
            None => &EmptyIntrospector,
        };
        let (imported, accessed) = tola_typst::observe_world(self.comp.world(), |world| {
            let traced = Traced::default();
            let mut sink = Sink::new();
            let mut engine = Engine {
                library: world.library(),
                world: world.track(),
                introspector: Protected::new(introspector.track()),
                traced: traced.track(),
                sink: sink.track_mut(),
                route: Route::default(),
            };
            tola_typst::typst_eval::import(&mut engine, path, span)
                .ok()
                .map(Value::Module)
        });
        self.record_accessed(accessed);
        imported
    }

    pub(super) fn inspect_schema(&self, decl: &Value, input: bool) -> Result<Option<Value>> {
        self.cancel.ensure_active()?;
        let world = self.comp.world();
        let traced = Traced::default();
        let mut sink = Sink::new();
        let mut engine = Engine {
            library: world.library(),
            world: (world as &dyn World).track(),
            introspector: Protected::new((&EmptyIntrospector as &dyn Introspector).track()),
            traced: traced.track(),
            sink: sink.track_mut(),
            route: Route::default(),
        };
        let desc = tola_typst::typst_eval::import(
            &mut engine,
            &Str::from("@tola/schema:0.0.0"),
            Span::detached(),
        )
        .ok()
        .and_then(|module| {
            let Value::Func(inspect) = module.scope().get("inspect")?.read() else {
                return None;
            };
            let span = Span::detached();
            let mut args = Args::new(span, [decl.clone()]);
            args.items.push(Arg {
                span,
                name: Some("input".into()),
                value: Spanned::new(Value::Bool(input), span),
            });
            inspect
                .call(
                    &mut engine,
                    tola_typst::typst::foundations::Context::none().track(),
                    args,
                )
                .ok()
        });
        self.cancel.ensure_active()?;
        Ok(desc)
    }

    pub(super) fn imported_export(
        &mut self,
        src: &LinkedNode<'_>,
        path: &[String],
    ) -> Result<Option<(Value, Span)>> {
        let Some(mut val) = self.import(src)? else {
            return Ok(None);
        };
        let mut span = Span::detached();
        for part in path {
            self.cancel.ensure_active()?;
            let module = match &val {
                Value::Module(module) => module.file_id(),
                _ => None,
            };
            let Some(binding) = val.scope().and_then(|scope| scope.get(part.as_str())) else {
                return Ok(None);
            };
            span = self.binding_span(module, part, binding.span());
            val = binding.read().clone();
        }
        Ok(Some((val, span)))
    }

    pub(super) fn imported_location(
        &mut self,
        src: &LinkedNode<'_>,
        path: &[String],
        client_root: &crate::uri::ClientRoot,
    ) -> Result<Option<Location>> {
        let Some((_, span)) = self.imported_export(src, path)? else {
            return Ok(None);
        };
        if !crate::identity::is_tola_span(span) {
            return Ok(None);
        }
        Ok(self.location(span, client_root))
    }

    fn binding_span(&mut self, file: Option<FileId>, name: &str, span: Span) -> Span {
        if !span.is_detached() {
            return span;
        }
        let Some(names) = file.and_then(|file| self.source_names(file)) else {
            return span;
        };
        let Some(decl) = last_top_level_declaration(&names, |d| d.name == name) else {
            return span;
        };
        tola_typst_syntax::syntax::node_at_range(names.source(), &decl.range)
            .map_or(span, |node| node.span())
    }

    pub(super) fn source_names(&mut self, file: FileId) -> Option<Arc<SourceNames>> {
        if let Some(names) = self.names.get(&file) {
            return Some(Arc::clone(names));
        }
        let src = self.comp.world().source(file).ok()?;
        let names = Arc::new(SourceNames::new(src));
        self.names.insert(file, Arc::clone(&names));
        Some(names)
    }

    pub(super) fn cancellation(&self) -> &BuildCancellation {
        self.cancel
    }

    pub(super) fn span_range(&self, span: Span) -> Option<std::ops::Range<usize>> {
        self.comp.world().range(span)
    }

    pub(super) fn import_bindings(
        &mut self,
        src: &LinkedNode<'_>,
        path: &[String],
    ) -> Result<BTreeMap<String, Binding>> {
        let mut bindings = BTreeMap::new();
        let Some((val, _)) = self.imported_export(src, path)? else {
            return Ok(bindings);
        };
        self.scope_bindings(&val, &mut bindings);
        Ok(bindings)
    }

    pub(super) fn bindings(&mut self, pos: LinkedNode<'_>) -> Result<BTreeMap<String, Binding>> {
        self.bindings_within(pos, INITIALIZER_DEPTH)
    }

    fn bindings_within(
        &mut self,
        pos: LinkedNode<'_>,
        depth: usize,
    ) -> Result<BTreeMap<String, Binding>> {
        let mut bindings = BTreeMap::new();
        let Some(file) = pos.span().id() else {
            return Ok(bindings);
        };
        let Some(names) = self.source_names(file) else {
            return Ok(bindings);
        };
        let at = pos.range().start;
        let mut scope = names.scope_at(at);
        loop {
            let mut scope_names: Vec<ScopeName> = names.scopes()[scope]
                .declarations
                .iter()
                .copied()
                .filter(|&id| {
                    let decl = &names.declarations()[id];
                    decl.visible_from <= at
                        || decl
                            .recursive_body
                            .as_ref()
                            .is_some_and(|body| body.contains(&at))
                })
                .map(ScopeName::Declaration)
                .chain(
                    names
                        .imports()
                        .iter()
                        .enumerate()
                        .filter(|(_, import)| {
                            import.wildcard && import.scope == scope && import.range.end <= at
                        })
                        .map(|(id, _)| ScopeName::Import(id)),
                )
                .collect();
            scope_names.sort_by_key(|name| {
                std::cmp::Reverse(match name {
                    ScopeName::Declaration(id) => names.declarations()[*id].visible_from,
                    ScopeName::Import(id) => names.imports()[*id].range.end,
                })
            });
            for name in scope_names {
                self.cancel.ensure_active()?;
                match name {
                    ScopeName::Import(id) => {
                        let import = &names.imports()[id];
                        if let Some(src) = tola_typst_syntax::syntax::node_at_range(
                            names.source(),
                            &import.source_range,
                        ) && let Some(val) = self.import(&src)?
                        {
                            self.scope_bindings(&val, &mut bindings);
                        }
                    }
                    ScopeName::Declaration(id) => {
                        let decl = &names.declarations()[id];
                        if bindings.contains_key(&decl.name) {
                            continue;
                        }
                        if let Some(binding) = self.declaration_binding(&names, decl, depth)? {
                            bindings.insert(decl.name.clone(), binding);
                        }
                    }
                }
            }
            let Some(parent) = names.scopes()[scope].parent else {
                break;
            };
            scope = parent;
        }
        Ok(bindings)
    }

    fn declaration_binding(
        &mut self,
        names: &SourceNames,
        decl: &Declaration,
        depth: usize,
    ) -> Result<Option<Binding>> {
        let Some(node) = tola_typst_syntax::syntax::node_at_range(names.source(), &decl.range)
        else {
            return Ok(None);
        };
        let mut binding = Binding {
            span: node.span(),
            value: None,
            tola: false,
            parameter: matches!(decl.kind, DeclarationKind::Parameter),
            initializer: None,
        };
        if let Initializer::Import { import, path } = &decl.initializer {
            let import = &names.imports()[*import];
            if let Some(src) =
                tola_typst_syntax::syntax::node_at_range(names.source(), &import.source_range)
            {
                if path.is_empty() {
                    binding.value = self.import(&src)?;
                    if let Some(Value::Module(module)) = &binding.value
                        && let Some(file) = module.file_id()
                        && let Ok(src) = self.comp.world().source(file)
                    {
                        binding.span = src.root().span();
                    }
                } else if let Some((val, span)) = self.imported_export(&src, path)? {
                    binding.value = Some(val);
                    binding.span = span;
                }
                binding.tola = crate::identity::is_tola_span(binding.span);
            }
        } else if matches!(decl.kind, DeclarationKind::Let) && depth > 0 {
            let mut ancestor = Some(node);
            while let Some(node) = ancestor {
                if let Some(decl) = node.cast::<ast::LetBinding>() {
                    if let Some(init) = decl.init().and_then(|init| node.find(init.span())) {
                        binding.initializer = Some(init.span());
                        binding.tola = self.tola_origin_within(&init, depth - 1)?;
                    }
                    break;
                }
                ancestor = node.parent().cloned();
            }
        }
        Ok(Some(binding))
    }

    pub(super) fn tola_origin(&mut self, node: &LinkedNode<'_>) -> Result<bool> {
        self.tola_origin_within(node, INITIALIZER_DEPTH)
    }

    fn tola_origin_within(&mut self, node: &LinkedNode<'_>, depth: usize) -> Result<bool> {
        if self
            .definition(node)?
            .is_some_and(crate::identity::is_tola_span)
        {
            return Ok(true);
        }
        let root = tola_typst_syntax::syntax::root_expression(node.clone());
        let Some(ident) = root.cast::<ast::Ident>() else {
            return Ok(false);
        };
        let name = ident.as_str().to_owned();
        Ok(self
            .bindings_within(root, depth)?
            .remove(name.as_str())
            .is_some_and(|b| b.tola))
    }

    fn scope_bindings(&mut self, val: &Value, bindings: &mut BTreeMap<String, Binding>) {
        let Some(scope) = val.scope() else {
            return;
        };
        let module = match val {
            Value::Module(module) => module.file_id(),
            _ => None,
        };
        for (name, binding) in scope.iter() {
            bindings.entry(name.to_string()).or_insert_with(|| {
                let span = self.binding_span(module, name.as_str(), binding.span());
                Binding {
                    tola: crate::identity::is_tola_span(span),
                    span,
                    value: Some(binding.read().clone()),
                    parameter: false,
                    initializer: None,
                }
            });
        }
    }

    pub(super) fn types(
        &mut self,
        node: &LinkedNode<'_>,
    ) -> Result<Vec<(ExpressionType, Option<Styles>)>> {
        ExpressionResolver::types(self, node)
    }

    pub(super) fn known_values(
        &mut self,
        node: &LinkedNode<'_>,
    ) -> Result<Vec<(Value, Option<Styles>)>> {
        ExpressionResolver::known_values(self, node)
    }

    pub(super) fn public_export(&mut self, entry: FileId, export: &str) -> Result<Option<Func>> {
        self.cancel.ensure_active()?;
        let (imported, accessed) = tola_typst::observe_world(self.comp.world(), |world| {
            let src = world.source(entry).ok()?;
            let traced = Traced::default();
            let mut sink = Sink::new();
            let route = Route::default();
            tola_typst::typst_eval::eval(
                world.track(),
                world.library(),
                traced.track(),
                sink.track_mut(),
                route.track(),
                &src,
            )
            .ok()
        });
        self.record_accessed(accessed);
        self.cancel.ensure_active()?;
        let Some(module) = imported else {
            return Ok(None);
        };
        let mut val = Value::Module(module);
        for part in export.split('.') {
            let Ok(field) = val.field(part, ()) else {
                return Ok(None);
            };
            val = field;
        }
        Ok(match val {
            Value::Func(func) => Some(func),
            _ => None,
        })
    }

    pub(super) fn accessed(&self) -> &tola_typst::AccessedDeps {
        &self.accessed
    }

    fn record_accessed(&mut self, accessed: tola_typst::AccessedDeps) {
        self.accessed.reads.extend(accessed.reads);
        self.accessed.disk_reads.extend(accessed.disk_reads);
        self.accessed.package_checks.extend(accessed.package_checks);
    }

    pub(super) fn callable_initializer(
        &mut self,
        node: &LinkedNode<'_>,
    ) -> Result<Option<(Source, Span)>> {
        let Some(binding) = ExpressionResolver::binding(self, node)? else {
            return Ok(None);
        };
        let Some(init) = binding.initializer else {
            return Ok(None);
        };
        Ok(init
            .id()
            .and_then(|file| self.world_source(file))
            .map(|src| (src, init)))
    }

    pub(super) fn callable_is_stable(&mut self, node: &LinkedNode<'_>) -> Result<bool> {
        let root = tola_typst_syntax::syntax::root_expression(node.clone());
        let Some(file) = root.span().id() else {
            return Ok(false);
        };
        let Some(names) = self.source_names(file) else {
            return Ok(false);
        };
        let Some(decl) = names.declared_at(root.range().start) else {
            return Ok(true);
        };
        Ok(!binding_is_written(
            &names,
            decl,
            node.range().start,
            |_| true,
            || self.ensure_active(),
        )?)
    }

    pub(super) fn library_bindings(&self, node: &LinkedNode<'_>) -> Vec<(String, Value)> {
        if expression_name(node).is_none() || !is_name_reference(node) {
            return Vec::new();
        }
        let Some(mode) = node.mode_after() else {
            return Vec::new();
        };
        let (lib, scope) = self.library_scope(mode);
        let mut names: Vec<(String, Value)> = scope
            .iter()
            .map(|(name, binding)| (name.to_string(), binding.read().clone()))
            .collect();
        names.push(("std".to_owned(), lib.std.read().clone()));
        names.sort_by(|l, r| l.0.cmp(&r.0));
        names
    }

    pub(super) fn callee_functions(
        &mut self,
        callee: &LinkedNode<'_>,
    ) -> Result<Vec<(Value, bool)>> {
        Ok(self
            .known_values(callee)?
            .into_iter()
            .map(|(val, _)| (val, false))
            .collect())
    }

    pub(super) fn field_functions(
        &mut self,
        receiver: &LinkedNode<'_>,
        field: &str,
    ) -> Result<Vec<(Value, bool)>> {
        ExpressionResolver::field_functions(self, receiver, field)
    }

    pub(super) fn signatures(
        &mut self,
        callee: &LinkedNode<'_>,
        name: &str,
    ) -> Result<(Vec<SignatureInformation>, Vec<Parameter>)> {
        let funcs = self.callee_functions(callee)?;
        let def = if funcs
            .iter()
            .any(|(val, _)| matches!(val, Value::Func(func) if function_docs(func).is_none()))
        {
            self.definition(callee)?
        } else {
            None
        };
        Ok(self.reflect_signatures(funcs, name, def))
    }

    pub(super) fn field_signatures(
        &mut self,
        receiver: &LinkedNode<'_>,
        field: &str,
        name: &str,
    ) -> Result<(Vec<SignatureInformation>, Vec<Parameter>)> {
        let funcs = self.field_functions(receiver, field)?;
        Ok(self.reflect_signatures(funcs, name, None))
    }

    fn reflect_signatures(
        &self,
        funcs: impl IntoIterator<Item = (Value, bool)>,
        name: &str,
        def: Option<Span>,
    ) -> (Vec<SignatureInformation>, Vec<Parameter>) {
        let mut sigs = Vec::new();
        let mut params = Vec::new();
        for (val, method) in funcs {
            let Value::Func(func) = val else { continue };
            let docs = self.function_documentation(&func, def);
            let (sig, declared) = signature_information(&func, name, method, docs.as_deref());
            if !sigs.contains(&sig) {
                if sigs.is_empty() {
                    params = declared;
                }
                sigs.push(sig);
            }
        }
        (sigs, params)
    }

    pub(super) fn member_types(
        &mut self,
        node: &LinkedNode<'_>,
    ) -> Result<BTreeMap<String, Vec<ExpressionType>>> {
        let mut fields = BTreeMap::<String, Vec<ExpressionType>>::new();
        for (ty, styles) in self.types(node)? {
            for (name, member) in members(&ty, styles.as_ref(), || self.ensure_active())? {
                let alts = fields.entry(name).or_default();
                if !alts.contains(&member.ty) {
                    alts.push(member.ty);
                }
            }
        }
        Ok(fields)
    }

    pub(super) fn definition(&mut self, node: &LinkedNode<'_>) -> Result<Option<Span>> {
        if let Some(access) = node.cast::<ast::FieldAccess>()
            && let Some(target) = node.find(access.target().span())
        {
            for (val, _) in self.known_values(&target)? {
                if let Some(binding) = val.scope().and_then(|scope| scope.get(&access.field()))
                    && !binding.span().is_detached()
                {
                    return Ok(Some(binding.span()));
                }
            }
        }
        if let Some(ident) = node.cast::<ast::Ident>()
            && let Some(binding) = self.bindings(node.clone())?.remove(ident.as_str())
            && !binding.span.is_detached()
        {
            return Ok(Some(binding.span));
        }
        for (val, _) in self.known_values(node)? {
            let span = match val {
                Value::Func(func) => func.span(),
                Value::Content(content) => content.span(),
                _ => Span::detached(),
            };
            if !span.is_detached() && span != node.span() {
                return Ok(Some(span));
            }
        }
        Ok(None)
    }

    pub(super) fn definition_location(
        &mut self,
        node: &LinkedNode<'_>,
        client_root: &crate::uri::ClientRoot,
    ) -> Result<Option<Location>> {
        let span = if tola_typst_syntax::syntax::is_import_path(node) {
            match self.import(node)? {
                Some(Value::Module(module)) => module
                    .file_id()
                    .and_then(|id| self.comp.world().source(id).ok())
                    .map(|src| src.root().span()),
                _ => None,
            }
        } else {
            self.definition(node)?
        };
        Ok(span.and_then(|span| self.location(span, client_root)))
    }
}

impl ExpressionResolver for Semantic<'_> {
    fn source(&mut self, file: FileId) -> Option<Source> {
        self.world_source(file)
    }

    fn binding(&mut self, node: &LinkedNode<'_>) -> Result<Option<ExpressionBinding>> {
        let Some(name) = expression_name(node) else {
            return Ok(None);
        };
        if let Some(file) = node.span().id()
            && let Some(names) = self.source_names(file)
        {
            let at = node.range().start;
            let written = match names.declared_at(at) {
                Some(decl) => {
                    binding_is_written(&names, decl, at, |_| false, || self.ensure_active())?
                }
                None => false,
            };
            let mut unresolved_wildcard = false;
            if self.inference.has_call_frame() {
                let local = names
                    .resolve(&name, names.scope_at(at), at)
                    .map(|id| &names.declarations()[id]);
                let mut scope = names.scope_at(at);
                loop {
                    unresolved_wildcard = names.imports().iter().any(|import| {
                        import.wildcard
                            && import.scope == scope
                            && import.range.end <= at
                            && !matches!(import.source, ImportSource::Path(_))
                            && !local.is_some_and(|local| {
                                local.scope == scope && local.visible_from >= import.range.end
                            })
                    });
                    if unresolved_wildcard || local.is_some_and(|local| local.scope == scope) {
                        break;
                    }
                    let Some(parent) = names.scopes()[scope].parent else {
                        break;
                    };
                    scope = parent;
                }
            }
            if written || unresolved_wildcard {
                return Ok(Some(ExpressionBinding::unknown(node.span())));
            }
        }
        Ok(self
            .bindings(node.clone())?
            .remove(&name)
            .map(|b| ExpressionBinding {
                span: b.span,
                value: b.value,
                parameter: b.parameter,
                initializer: b.initializer,
            }))
    }

    fn observed(&mut self, span: Span) -> Result<Vec<(Value, Option<Styles>)>> {
        self.trace(span)
    }

    fn library(&self) -> &Library {
        self.comp.world().library()
    }

    fn inference(&mut self) -> &mut Inference {
        &mut self.inference
    }

    fn ensure_active(&self) -> Result<()> {
        self.cancel.ensure_active()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::tests::*;

    #[test]
    fn receiver_type_methods() {
        let mut site = QuerySession::with_program(FAILED_PROGRAM);
        let items = site.completion("#panic(\"page\")\n#let value = \"a\"\n#value.|");
        let labels = completion_labels(&items);
        assert!(labels.contains(&"to-unicode"), "{labels:?}");
        assert!(!labels.contains(&"from-unicode"), "{labels:?}");
    }

    #[test]
    fn abbreviated_cuts_at_entry_boundary() {
        let entry = format!("({}, {})", "x".repeat(300), "y".repeat(300));
        let repr = std::iter::repeat_n(entry.as_str(), 40)
            .collect::<Vec<_>>()
            .join(",\n");
        let elided = abbreviated(&repr);
        assert!(elided.starts_with("(xxx"), "{elided}");
        assert!(elided.ends_with(" characters omitted)"), "{elided}");
        let last = elided.lines().rev().nth(1).expect("a kept line");
        assert!(last.ends_with("),"), "{last}");
    }
}
