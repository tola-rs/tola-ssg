//! Values come from the official Bundle compiler, never the paged IDE tracer.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionTextEdit, Location,
    ParameterInformation, ParameterLabel, Range, SignatureInformation, TextEdit, Uri,
};
use tola_build::cancellation::BuildCancellation;
use tola_build::check::SourceCompilation;
pub(super) use tola_packages::type_spelling;
use tola_packages::{ParameterKind, Signature};
use tola_typst::typst::World;
use tola_typst::typst::WorldExt;
use tola_typst::typst::comemo::Track;
use tola_typst::typst::engine::{Engine, Route, Sink, Traced};
use tola_typst::typst::foundations::{Func, Repr, Str, Styles, Value, fields_on};
use tola_typst::typst::introspection::{EmptyIntrospector, Introspector};
use tola_typst::typst::syntax::ast::AstNode;
use tola_typst::typst::syntax::{
    FileId, LinkedNode, RootedPath, Source, Span, SyntaxKind, VirtualRoot, ast,
};
use tola_typst::typst::utils::Protected;
use tola_typst_syntax::names::{DeclarationKind, Initializer, SourceNames};

use super::editing;
use crate::position;

/// How far a name's origin follows its initializers; the bound keeps resolution
/// finite where a well-formed program cannot cycle.
const TOLA_ORIGIN_DEPTH: usize = 8;

#[derive(Clone)]
pub(super) struct Binding {
    pub span: Span,
    pub value: Option<Value>,
    pub tola: bool,
}

/// One revision's semantic answers, and the traces they came from.
///
/// Both caches are keyed by span and valid only for `compilation`: a span belongs to one parsed
/// source, and `SourceCompilation` owns the world that produced it. A caller answering an older
/// revision builds a new `Semantic`, never reuses one across compilations.
pub(super) struct Semantic<'a> {
    compilation: &'a SourceCompilation,
    cancellation: &'a BuildCancellation,
    traces: HashMap<Span, Vec<(Value, Option<Styles>)>>,
    imports: HashMap<Span, Option<Value>>,
    names: HashMap<FileId, Arc<SourceNames>>,
}

impl<'a> Semantic<'a> {
    pub(super) fn new(
        compilation: &'a SourceCompilation,
        cancellation: &'a BuildCancellation,
    ) -> Self {
        Self {
            compilation,
            cancellation,
            traces: HashMap::new(),
            imports: HashMap::new(),
            names: HashMap::new(),
        }
    }

    pub(super) fn trace(&mut self, span: Span) -> Result<Vec<(Value, Option<Styles>)>> {
        self.cancellation.ensure_active()?;
        if let Some(values) = self.traces.get(&span) {
            return Ok(values.clone());
        }
        // `trace` swallows compiler errors, so it needs a session whose source
        // analysis and root Bundle both succeeded.
        let values = tola_typst::typst::trace::<tola_typst::typst_bundle::Bundle>(
            self.compilation.world(),
            span,
        )
        .into_iter()
        .collect::<Vec<_>>();
        self.cancellation.ensure_active()?;
        self.traces.insert(span, values.clone());
        Ok(values)
    }

    pub(super) fn import(&mut self, source: &LinkedNode<'_>) -> Result<Option<Value>> {
        self.cancellation.ensure_active()?;
        if let Some(value) = self.imports.get(&source.span()) {
            return Ok(value.clone());
        }
        let value = if let Some(literal) = source.cast::<ast::Str>() {
            Some(Value::Str(literal.get().into()))
        } else {
            self.trace(source.span())?
                .into_iter()
                .next()
                .map(|(value, _)| value)
        };
        let imported = match value {
            Some(value) if value.scope().is_some() => Some(value),
            Some(Value::Str(path)) => {
                let world = self.compilation.world();
                let traced = Traced::default();
                let mut sink = Sink::new();
                // An imported module's own value does not read the site's compiled introspection,
                // so a site that did not compile evaluates the path all the same.
                let introspector: &dyn Introspector = match self.compilation.introspector() {
                    Some(introspector) => introspector,
                    None => &EmptyIntrospector,
                };
                let mut engine = Engine {
                    library: world.library(),
                    world: (world as &dyn World).track(),
                    introspector: Protected::new(introspector.track()),
                    traced: traced.track(),
                    sink: sink.track_mut(),
                    route: Route::default(),
                };
                tola_typst::typst_eval::import(&mut engine, &path, source.span())
                    .ok()
                    .map(Value::Module)
            }
            _ => None,
        };
        self.cancellation.ensure_active()?;
        self.imports.insert(source.span(), imported.clone());
        Ok(imported)
    }

    /// The package owns schema semantics. Its pure inspection API never parses defaults or
    /// invokes user predicates, converters, or lazy factories during an editor request.
    pub(super) fn inspect_schema(&self, declaration: &Value) -> Result<Option<Value>> {
        self.cancellation.ensure_active()?;
        let world = self.compilation.world();
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
        let description = tola_typst::typst_eval::import(
            &mut engine,
            &Str::from("@tola/schema:0.0.0"),
            Span::detached(),
        )
        .ok()
        .and_then(|module| {
            let Value::Func(inspect) = module.scope().get("inspect")?.read() else {
                return None;
            };
            inspect
                .call(
                    &mut engine,
                    tola_typst::typst::foundations::Context::none().track(),
                    tola_typst::typst::foundations::Args::new(
                        Span::detached(),
                        [declaration.clone()],
                    ),
                )
                .ok()
        });
        self.cancellation.ensure_active()?;
        Ok(description)
    }

    /// The export one import item's path names, with the span the package binds it at.
    pub(super) fn imported_export(
        &mut self,
        source: &LinkedNode<'_>,
        path: &[String],
    ) -> Result<Option<(Value, Span)>> {
        self.imported_at(source, path)
    }

    /// The location of the export one import item names, when Tola supplies it.
    pub(super) fn imported_location(
        &mut self,
        source: &LinkedNode<'_>,
        path: &[String],
        client_root: &crate::uri::ClientRoot,
    ) -> Result<Option<Location>> {
        let Some((_, span)) = self.imported_at(source, path)? else {
            return Ok(None);
        };
        if !crate::identity::is_tola_span(span) {
            return Ok(None);
        }
        Ok(self.location(span, client_root))
    }

    /// Walk one import path from the module the import names.
    fn imported_at(
        &mut self,
        source: &LinkedNode<'_>,
        path: &[String],
    ) -> Result<Option<(Value, Span)>> {
        let Some(mut value) = self.import(source)? else {
            return Ok(None);
        };
        let mut span = Span::detached();
        for part in path {
            self.cancellation.ensure_active()?;
            let module = match &value {
                Value::Module(module) => module.file_id(),
                _ => None,
            };
            let Some(binding) = value.scope().and_then(|scope| scope.get(part.as_str())) else {
                return Ok(None);
            };
            span = self.binding_span(module, part, binding.span());
            value = binding.read().clone();
        }
        Ok(Some((value, span)))
    }

    /// Locates detached native bindings in their exporting source.
    fn binding_span(&mut self, file: Option<FileId>, name: &str, span: Span) -> Span {
        if !span.is_detached() {
            return span;
        }
        let Some(names) = file.and_then(|file| self.source_names(file)) else {
            return span;
        };
        let Some(scope) = names.scopes().first() else {
            return span;
        };
        let Some(declaration) = scope
            .declarations
            .iter()
            .rev()
            .map(|&id| &names.declarations()[id])
            .find(|declaration| declaration.name == name)
        else {
            return span;
        };
        tola_typst_syntax::syntax::node_at_range(names.source(), &declaration.range)
            .map_or(span, |node| node.span())
    }

    /// One file's declarations and scopes, parsed at most once per compilation.
    pub(super) fn source_names(&mut self, file: FileId) -> Option<Arc<SourceNames>> {
        if let Some(names) = self.names.get(&file) {
            return Some(Arc::clone(names));
        }
        let source = self.compilation.world().source(file).ok()?;
        let names = Arc::new(SourceNames::new(source));
        self.names.insert(file, Arc::clone(&names));
        Some(names)
    }

    /// The bytes one span covers in its own file, when the world can address it.
    pub(super) fn span_range(&self, span: Span) -> Option<std::ops::Range<usize>> {
        self.compilation.world().range(span)
    }

    /// The site's own Typst sources this compilation read.
    ///
    /// A call site lives in a file the compiler read, which need not be the file an editor asks
    /// about, so an answer that searches call sites searches these.
    pub(super) fn project_sources(&self) -> Vec<FileId> {
        let mut files = Vec::new();
        for read in self.compilation.file_reads() {
            let path = match read.evidence().locator() {
                tola_typst::ReadLocator::Root(path)
                | tola_typst::ReadLocator::ProvidedRoot(path) => path,
                _ => continue,
            };
            if path.extension().is_none_or(|extension| extension != "typ") {
                continue;
            }
            let Ok(vpath) =
                tola_typst::typst::syntax::VirtualPath::new(path.to_string_lossy().as_ref())
            else {
                continue;
            };
            let id = RootedPath::new(VirtualRoot::Project, vpath).intern();
            if !files.contains(&id) {
                files.push(id);
            }
        }
        files
    }

    /// One source of the world this compilation read.
    pub(super) fn world_source(&self, file: FileId) -> Option<Source> {
        self.compilation.world().source(file).ok()
    }

    pub(super) fn import_bindings(
        &mut self,
        source: &LinkedNode<'_>,
        path: &[String],
    ) -> Result<BTreeMap<String, Binding>> {
        let mut result = BTreeMap::new();
        let Some((value, _)) = self.imported_at(source, path)? else {
            return Ok(result);
        };
        self.scope_bindings(&value, &mut result);
        Ok(result)
    }

    /// Nearest lexical bindings first; imports use the Bundle world.
    pub(super) fn bindings(
        &mut self,
        position: LinkedNode<'_>,
    ) -> Result<BTreeMap<String, Binding>> {
        self.bindings_within(position, TOLA_ORIGIN_DEPTH)
    }

    fn bindings_within(
        &mut self,
        position: LinkedNode<'_>,
        depth: usize,
    ) -> Result<BTreeMap<String, Binding>> {
        let mut result = BTreeMap::new();
        let Some(file) = position.span().id() else {
            return Ok(result);
        };
        let Some(names) = self.source_names(file) else {
            return Ok(result);
        };
        let at = position.range().start;
        let mut scope = names.scope_at(at);
        loop {
            let mut declarations: Vec<_> = names.scopes()[scope]
                .declarations
                .iter()
                .copied()
                .filter(|&id| {
                    let declaration = &names.declarations()[id];
                    declaration.visible_from <= at
                        || declaration
                            .recursive_body
                            .as_ref()
                            .is_some_and(|body| body.contains(&at))
                })
                .map(|id| (names.declarations()[id].visible_from, Some(id), None))
                .chain(
                    names
                        .imports()
                        .iter()
                        .enumerate()
                        .filter(|(_, import)| {
                            import.wildcard && import.scope == scope && import.range.end <= at
                        })
                        .map(|(id, import)| (import.range.end, None, Some(id))),
                )
                .collect();
            declarations.sort_by_key(|(at, _, _)| std::cmp::Reverse(*at));
            for (_, declaration, wildcard) in declarations {
                self.cancellation.ensure_active()?;
                if let Some(import) = wildcard {
                    let import = &names.imports()[import];
                    if let Some(source) = tola_typst_syntax::syntax::node_at_range(
                        names.source(),
                        &import.source_range,
                    ) && let Some(value) = self.import(&source)?
                    {
                        self.scope_bindings(&value, &mut result);
                    }
                    continue;
                }
                let declaration = &names.declarations()[declaration.unwrap()];
                if result.contains_key(&declaration.name) {
                    continue;
                }
                let Some(node) =
                    tola_typst_syntax::syntax::node_at_range(names.source(), &declaration.range)
                else {
                    continue;
                };
                let mut binding = Binding {
                    span: node.span(),
                    value: None,
                    tola: false,
                };
                if let Initializer::Import { import, path } = &declaration.initializer {
                    let import = &names.imports()[*import];
                    if let Some(source) = tola_typst_syntax::syntax::node_at_range(
                        names.source(),
                        &import.source_range,
                    ) {
                        if path.is_empty() {
                            binding.value = self.import(&source)?;
                            if let Some(Value::Module(module)) = &binding.value
                                && let Some(file) = module.file_id()
                                && let Ok(source) = self.compilation.world().source(file)
                            {
                                binding.span = source.root().span();
                            }
                        } else if let Some((value, span)) = self.imported_at(&source, path)? {
                            binding.value = Some(value);
                            binding.span = span;
                        }
                        binding.tola = crate::identity::is_tola_span(binding.span);
                    }
                } else if matches!(declaration.kind, DeclarationKind::Let) && depth > 0 {
                    let mut ancestor = Some(node);
                    while let Some(node) = ancestor {
                        if let Some(declaration) = node.cast::<ast::LetBinding>() {
                            if let Some(init) =
                                declaration.init().and_then(|init| node.find(init.span()))
                            {
                                binding.tola = self.tola_origin_within(&init, depth - 1)?;
                            }
                            break;
                        }
                        ancestor = node.parent().cloned();
                    }
                }
                result.insert(declaration.name.clone(), binding);
            }
            let Some(parent) = names.scopes()[scope].parent else {
                break;
            };
            scope = parent;
        }
        Ok(result)
    }

    pub(super) fn tola_origin(&mut self, node: &LinkedNode<'_>) -> Result<bool> {
        self.tola_origin_within(node, TOLA_ORIGIN_DEPTH)
    }

    fn tola_origin_within(&mut self, node: &LinkedNode<'_>, depth: usize) -> Result<bool> {
        // A member of a Tola value resolves to the binding inside the package
        // source; otherwise the leftmost name decides.
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
            .is_some_and(|binding| binding.tola))
    }

    fn scope_bindings(&mut self, value: &Value, result: &mut BTreeMap<String, Binding>) {
        let Some(scope) = value.scope() else {
            return;
        };
        let module = match value {
            Value::Module(module) => module.file_id(),
            _ => None,
        };
        for (name, binding) in scope.iter() {
            result.entry(name.to_string()).or_insert_with(|| {
                let span = self.binding_span(module, name.as_str(), binding.span());
                Binding {
                    tola: crate::identity::is_tola_span(span),
                    span,
                    value: Some(binding.read().clone()),
                }
            });
        }
    }

    pub(super) fn values(&mut self, node: &LinkedNode<'_>) -> Result<Vec<(Value, Option<Styles>)>> {
        if node.kind() == SyntaxKind::Contextual
            && let Some(body) = node.children().next_back()
        {
            return self.values(&body);
        }
        let values = self.trace(node.span())?;
        if !values.is_empty() {
            return Ok(values);
        }
        if let Some(ident) = node.cast::<ast::Ident>()
            && let Some(binding) = self.bindings(node.clone())?.remove(ident.as_str())
        {
            if let Some(value) = binding.value {
                return Ok(vec![(value, None)]);
            }
            return self.trace(binding.span);
        }
        Ok(Vec::new())
    }

    /// The functions a call's callee resolves to, which completion and signature help read.
    pub(super) fn callee_functions(
        &mut self,
        callee: &LinkedNode<'_>,
    ) -> Result<Vec<(Value, bool)>> {
        Ok(self
            .values(callee)?
            .into_iter()
            .map(|(value, _)| (value, false))
            .collect())
    }

    /// The functions a member read resolves to on its receiver, with whether Typst inserts a
    /// receiver.
    pub(super) fn field_functions(
        &mut self,
        receiver: &LinkedNode<'_>,
        field: &str,
    ) -> Result<Vec<(Value, bool)>> {
        let receivers = self.values(receiver)?;
        Ok(receivers
            .into_iter()
            .filter_map(|(receiver, _)| {
                // Typst inserts a receiver only for type and element methods.
                let method = receiver
                    .ty()
                    .scope()
                    .get(field)
                    .or_else(|| match &receiver {
                        Value::Content(content) => content.elem().scope().get(field),
                        _ => None,
                    });
                if let Some(method) = method {
                    Some((method.read().clone(), true))
                } else if matches!(
                    receiver,
                    Value::Symbol(_) | Value::Func(_) | Value::Type(_) | Value::Module(_)
                ) {
                    receiver
                        .field(field, ())
                        .ok()
                        .map(|function| (function, false))
                } else {
                    None
                }
            })
            .collect())
    }

    pub(super) fn signatures(
        &mut self,
        callee: &LinkedNode<'_>,
        name: &str,
    ) -> Result<Vec<SignatureInformation>> {
        let functions = self.callee_functions(callee)?;
        let definition = if functions
            .iter()
            .any(|(value, _)| matches!(value, Value::Func(func) if function_docs(func).is_none()))
        {
            self.definition(callee)?
        } else {
            None
        };
        Ok(self.reflect_signatures(functions, name, definition))
    }

    pub(super) fn field_signatures(
        &mut self,
        receiver: &LinkedNode<'_>,
        field: &str,
        name: &str,
    ) -> Result<Vec<SignatureInformation>> {
        let functions = self.field_functions(receiver, field)?;
        Ok(self.reflect_signatures(functions, name, None))
    }

    fn reflect_signatures(
        &self,
        functions: impl IntoIterator<Item = (Value, bool)>,
        name: &str,
        definition: Option<Span>,
    ) -> Vec<SignatureInformation> {
        let mut signatures = Vec::new();
        for (value, method) in functions {
            let Value::Func(func) = value else { continue };
            let docs = self.function_documentation(&func, definition);
            let signature = signature_information(&func, name, method, docs.as_deref());
            if !signatures.contains(&signature) {
                signatures.push(signature);
            }
        }
        signatures
    }

    /// Merge distinct field values observed in every realized document context.
    pub(super) fn member_values(
        &mut self,
        node: &LinkedNode<'_>,
    ) -> Result<BTreeMap<String, Vec<Value>>> {
        let mut fields = BTreeMap::<String, Vec<Value>>::new();
        for (value, styles) in self.values(node)? {
            for (name, value) in self.fields(&value, styles.as_ref()) {
                let alternatives = fields.entry(name).or_default();
                if !alternatives.contains(&value) {
                    alternatives.push(value);
                }
            }
        }
        Ok(fields)
    }

    fn fields(&self, value: &Value, styles: Option<&Styles>) -> BTreeMap<String, Value> {
        let mut fields = BTreeMap::new();
        let content_scope = match value {
            Value::Content(content) => Some(content.elem().scope()),
            _ => None,
        };
        for scope in content_scope.into_iter().chain(Some(value.ty().scope())) {
            for (name, binding) in scope.iter() {
                if let Value::Func(func) = binding.read()
                    && func
                        .params()
                        .next()
                        .is_some_and(|param| param.name() == Some("self"))
                {
                    fields.insert(name.to_string(), Value::Func(func.clone()));
                }
            }
        }
        if let Some(scope) = value.scope() {
            for (name, binding) in scope.iter() {
                fields.insert(name.to_string(), binding.read().clone());
            }
        }
        for field in fields_on(value.ty()) {
            if let Ok(value) = value.field(field, ()) {
                fields.insert((*field).into(), value);
            }
        }
        match value {
            Value::Dict(dict) => fields.extend(
                dict.iter()
                    .map(|(name, value)| (name.to_string(), value.clone())),
            ),
            Value::Content(content) => fields.extend(
                content
                    .fields()
                    .into_iter()
                    .map(|(name, value)| (name.to_string(), value)),
            ),
            Value::Args(args) => fields.extend(
                args.to_named()
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.clone())),
            ),
            Value::Func(func) => {
                if let Some((elem, styles)) = func.to_element().zip(styles) {
                    for param in elem.params() {
                        if let Some(id) = elem.field_id(param.name)
                            && let Ok(value) = elem.field_from_styles(
                                id,
                                tola_typst::typst::foundations::StyleChain::new(styles),
                            )
                        {
                            fields.insert(param.name.into(), value);
                        }
                    }
                }
            }
            _ => {}
        }
        fields
    }

    pub(super) fn definition(&mut self, node: &LinkedNode<'_>) -> Result<Option<Span>> {
        if let Some(access) = node.cast::<ast::FieldAccess>()
            && let Some(target) = node.find(access.target().span())
        {
            for (value, _) in self.values(&target)? {
                if let Some(binding) = value.scope().and_then(|scope| scope.get(&access.field()))
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
        for (value, _) in self.values(node)? {
            let span = match value {
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
                    .and_then(|id| self.compilation.world().source(id).ok())
                    .map(|source| source.root().span()),
                _ => None,
            }
        } else {
            self.definition(node)?
        };
        // The editor can open any source this world reads, so a location is the whole condition.
        Ok(span.and_then(|span| self.location(span, client_root)))
    }

    /// The site sources one metadata chain's observed records name.
    ///
    /// A chain of record reads observes one value per realized document, and the files those
    /// records name are the sources the chain is about. Only a record naming a file below the site
    /// root is a source this answer can point at; a value that names none, or names a package file,
    /// contributes nothing.
    pub(super) fn candidate_sources(
        &mut self,
        node: &LinkedNode<'_>,
    ) -> Result<Vec<CandidateSource>> {
        let root = tola_typst_syntax::syntax::root_expression(node.clone());
        let mut sources = Vec::new();
        for (value, _) in self.values(&root)? {
            sources.extend(record_source(&value));
        }
        Ok(ordered(sources))
    }

    /// The site files one records expression names, as `all-sources()` records hold them.
    ///
    /// Every observed array contributes the `file` of each record it holds, so a caller reads
    /// which content sources one call's records were taken from. A record naming a package file
    /// or no file at all contributes nothing.
    pub(super) fn record_files(&mut self, node: &LinkedNode<'_>) -> Result<HashSet<FileId>> {
        let mut files = HashSet::new();
        for (value, _) in self.values(node)? {
            let Value::Array(records) = value else {
                continue;
            };
            for record in records.iter() {
                let Value::Dict(record) = record else {
                    continue;
                };
                let Ok(Value::Dyn(dynamic)) = record.get("file") else {
                    continue;
                };
                let Some(file) = dynamic.downcast::<RootedPath>() else {
                    continue;
                };
                if matches!(file.root(), VirtualRoot::Project) {
                    files.insert(file.clone().intern());
                }
            }
        }
        Ok(files)
    }

    /// The definition target of one candidate source: the `page` binding the file declares at its
    /// own top level, or the file's first position when it declares none.
    pub(super) fn candidate_location(
        &mut self,
        candidate: &CandidateSource,
        client_root: &crate::uri::ClientRoot,
    ) -> Option<Location> {
        let uri = self.source_uri(candidate.file, client_root)?;
        let range = self
            .source_names(candidate.file)
            .and_then(|names| candidate_binding(&names))
            .and_then(|range| {
                let source = self.compilation.world().source(candidate.file).ok()?;
                position::utf16_range(source.lines(), range)
            })
            .unwrap_or_else(|| Range {
                start: lsp_types::Position::new(0, 0),
                end: lsp_types::Position::new(0, 0),
            });
        Some(Location { uri, range })
    }

    pub(super) fn location(
        &self,
        span: Span,
        client_root: &crate::uri::ClientRoot,
    ) -> Option<Location> {
        // An editor opens the real package file, so this is source mapping rather
        // than diagnostic rendering, which drops unopenable ranges.
        let world = self.compilation.world();
        let id = span.id()?;
        let source = world.source(id).ok()?;
        let range = position::utf16_range(source.lines(), world.range(span)?)?;
        Some(Location {
            uri: self.source_uri(id, client_root)?,
            range,
        })
    }

    /// The protocol location an ordinary editing answer points at.
    pub(super) fn target_location(
        &self,
        definition: editing::Definition,
        client_root: &crate::uri::ClientRoot,
    ) -> Option<Location> {
        match definition {
            editing::Definition::Span(span) => self.location(span, client_root),
            editing::Definition::File(id) => Some(Location {
                uri: self.source_uri(id, client_root)?,
                range: position::utf16_range(
                    self.compilation.world().source(id).ok()?.lines(),
                    0..0,
                )?,
            }),
        }
    }

    pub(super) fn source_uri(
        &self,
        id: tola_typst::typst::syntax::FileId,
        client_root: &crate::uri::ClientRoot,
    ) -> Option<Uri> {
        if let tola_typst::typst::syntax::VirtualRoot::Package(spec) = id.root()
            && crate::identity::embedded_source(id).is_none()
        {
            return self.compilation.file_reads().iter().find_map(|read| {
                let matches = match read.evidence().locator() {
                    tola_typst::ReadLocator::Package { package, path }
                    | tola_typst::ReadLocator::ProvidedPackage { package, path } => {
                        package == spec
                            && path == std::path::Path::new(id.vpath().get_without_slash())
                    }
                    _ => false,
                };
                if matches && let tola_typst::ReadOrigin::Disk(path) = read.origin() {
                    return crate::uri::from_file_path(path.as_path()).ok();
                }
                None
            });
        }
        crate::identity::client_uri(id, client_root)
    }

    pub(super) fn value_docs(&self, span: Span, value: Option<&Value>) -> Option<String> {
        self.source_docs(span).or_else(|| match value {
            Some(Value::Func(func)) => self.source_docs(func.span()),
            _ => None,
        })
    }

    fn function_documentation(&self, func: &Func, definition: Option<Span>) -> Option<String> {
        function_docs(func)
            .map(str::to_owned)
            .or_else(|| definition.and_then(|span| self.source_docs(span)))
            .or_else(|| self.source_docs(func.span()))
    }

    pub(super) fn source_docs(&self, span: Span) -> Option<String> {
        let source = self.compilation.world().source(span.id()?).ok()?;
        let mut node = source.find(span)?;
        if let Some(docs) = tola_typst_syntax::docs::declaration_docs(&source, node.range().start) {
            return Some(docs);
        }
        // Typst gives a closure its parameter span; a multiline initializer's docs belong to
        // the binding. Only parentheses may intervene, never another closure or a block.
        if node.kind() == SyntaxKind::Params {
            node = node.parent()?.clone();
        }
        if node.kind() != SyntaxKind::Closure {
            return None;
        }
        while let Some(parent) = node.parent() {
            match parent.kind() {
                SyntaxKind::Parenthesized => node = parent.clone(),
                SyntaxKind::LetBinding => {
                    return tola_typst_syntax::docs::declaration_docs(
                        &source,
                        parent.range().start,
                    );
                }
                _ => break,
            }
        }
        None
    }
}

/// One site source one metadata chain was observed on.
pub(super) struct CandidateSource {
    /// The file the record's own `file` field names.
    pub file: FileId,
    /// The path below the site root, as hovers and diagnostics spell it.
    pub site_path: String,
    /// The path below `build.content-dir` the record has, when it has one.
    pub content_path: Option<String>,
}

/// One source list in the order every answer names it: by site path, one entry per file.
fn ordered(mut sources: Vec<CandidateSource>) -> Vec<CandidateSource> {
    sources.sort_by(|left, right| left.site_path.cmp(&right.site_path));
    sources.dedup_by(|left, right| left.site_path == right.site_path);
    sources
}

/// The site source one `all-sources()` record names, when it has a project file.
fn record_source(value: &Value) -> Option<CandidateSource> {
    let Value::Dict(record) = value else {
        return None;
    };
    let Ok(Value::Dyn(dynamic)) = record.get(tola_build::SourceDescriptorField::File.name()) else {
        return None;
    };
    let file = dynamic.downcast::<RootedPath>()?;
    if !matches!(file.root(), VirtualRoot::Project) {
        return None;
    }
    let content_path = record
        .get(tola_build::SourceDescriptorField::Path.name())
        .ok()
        .and_then(|path| path.clone().cast::<Str>().ok())
        .map(|path| path.to_string());
    Some(CandidateSource {
        file: file.clone().intern(),
        site_path: file.vpath().get_without_slash().to_owned(),
        content_path,
    })
}

/// The byte range of the `page` binding a content file declares at its own top level.
fn candidate_binding(names: &SourceNames) -> Option<std::ops::Range<usize>> {
    names
        .scopes()
        .first()?
        .declarations
        .iter()
        .rev()
        .map(|&id| &names.declarations()[id])
        .find(|declaration| {
            declaration.name == "page" && matches!(declaration.kind, DeclarationKind::Let)
        })
        .map(|declaration| declaration.range.clone())
}

pub(super) fn function_docs(func: &Func) -> Option<&str> {
    func.docs().filter(|docs| !docs.is_empty())
}

/// The signature of one function, as every surface renders it: what Typst's own metadata
/// declares, the types and descriptions the declaration's documentation writes, and the types the
/// checked world observed where neither names one.
pub(super) fn signature(
    name: &str,
    func: &Func,
    documented: Option<&tola_typst_syntax::docs::Documentation>,
    observed: &BTreeMap<String, String>,
) -> Signature {
    let mut signature = Signature::of(name, func);
    for parameter in &mut signature.parameters {
        let documented = documented.and_then(|documentation| {
            documentation
                .parameters
                .iter()
                .find(|documented| documented.name == parameter.name)
        });
        if parameter.ty.is_none() {
            // A rest parameter's value is Typst's `arguments`, which no checked observation
            // replaces.
            let observed = match parameter.kind {
                ParameterKind::Rest => None,
                _ => observed.get(&parameter.name).cloned(),
            };
            parameter.ty = documented
                .and_then(|documented| documented.ty.clone())
                .or(observed);
        }
        if parameter.docs.is_none() {
            parameter.docs = documented
                .map(|documented| documented.description.clone())
                .filter(|description| !description.is_empty());
        }
    }
    if signature.returns.is_none() {
        signature.returns = documented.and_then(|documentation| documentation.returns.clone());
    }
    signature
}

/// One function as the editor's signature help shows it: the shared signature on one line, and
/// every parameter labelled by its name alone, which is what the client highlights inside it.
fn signature_information(
    func: &Func,
    name: &str,
    method: bool,
    docs: Option<&str>,
) -> SignatureInformation {
    let documented = docs.map(tola_typst_syntax::docs::Documentation::parse);
    let mut signature = signature(name, func, documented.as_ref(), &BTreeMap::new());
    // A method call writes its receiver outside the argument list, so the receiver's parameter is
    // never one the author fills.
    if method
        && matches!(
            signature.parameters.first().map(|parameter| parameter.kind),
            Some(ParameterKind::Positional)
        )
    {
        signature.parameters.remove(0);
    }
    let mut ordered = signature.positional();
    ordered.extend(signature.rest());
    ordered.extend(signature.named());
    let label = inline_signature(&signature);
    let parameters = ordered
        .iter()
        .map(|parameter| ParameterInformation {
            label: ParameterLabel::Simple(format!("{}:", parameter.name)),
            documentation: parameter.docs.as_deref().map(super::markdown_documentation),
        })
        .collect::<Vec<_>>();
    SignatureInformation {
        label,
        parameters: Some(parameters),
        documentation: docs.map(super::markdown_documentation),
        active_parameter: None,
    }
}

/// One function's signature on one line: `name(parameter: type, …) -> return`, the shape Tinymist
/// writes wherever a hover or completion shows a signature without its block.
pub(super) fn inline_signature(signature: &Signature) -> String {
    let mut ordered = signature.positional();
    ordered.extend(signature.rest());
    ordered.extend(signature.named());
    let mut line = format!("{}(", signature.name);
    for (index, parameter) in ordered.iter().enumerate() {
        if index > 0 {
            line.push_str(", ");
        }
        if parameter.kind == ParameterKind::Rest {
            line.push_str("..");
        }
        line.push_str(&parameter.name);
        line.push_str(": ");
        line.push_str(parameter.ty_spelling());
    }
    line.push(')');
    if let Some(returns) = &signature.returns {
        line.push_str(" -> ");
        line.push_str(returns);
    }
    line
}

/// A value's representation as much as one line holds.
///
/// A page list has every page a site serves and a document has its whole body, and neither a
/// hover nor a completion's detail can show that: an author reads the head of it and asks for the
/// rest where it lives.
pub(super) fn abbreviated(repr: &str) -> String {
    /// How much of a representation one hover has.
    ///
    /// The largest value the standard library renders is the math `arrow` symbol's variant table at
    /// 3469 characters, so this bound holds every real value whole and elides only what a hover
    /// cannot name: a page list, or a collection the site itself built.
    const SHOWN: usize = 8192;
    if repr.chars().count() <= SHOWN {
        return repr.to_owned();
    }
    let taken: String = repr.chars().take(SHOWN).collect();
    // The cut lands on an entry boundary, so the marker never interrupts an item.
    let held = match taken.rfind('\n') {
        Some(at) => taken[..at].to_owned(),
        None => taken.trim_end().to_owned(),
    };
    format!(
        "{held}\n… ({} characters omitted)",
        repr.chars().count() - held.chars().count()
    )
}

/// One value as a sampled hover line has it: its representation, as much as one line holds.
pub(super) fn sampled_value(value: &Value) -> String {
    abbreviated(&value.repr())
}

/// What one value is, as a completion item's description spells it: a function's own signature, or
/// the value's type and representation.
pub(super) fn description(value: &Value, name: &str) -> String {
    match value {
        Value::Func(func) => inline_signature(&signature(name, func, None, &BTreeMap::new())),
        _ => {
            let repr = abbreviated(&value.repr());
            // A representation that names its own constructor — `symbol("α")`, an element call,
            // `<module global>` — reads as the value, so the type does not repeat in front of it.
            if names_itself(&repr) {
                repr
            } else {
                format!("{}: {repr}", type_spelling(&value.ty()))
            }
        }
    }
}

/// Whether a value's own representation already names what the value is, so a hover need not spell
/// its type first: `symbol("α")` and `<module global>` read on their own, while `1`, `"x"`, and
/// `(1, 2)` do not.
fn names_itself(repr: &str) -> bool {
    repr.starts_with('<')
        || repr.find('(').is_some_and(|at| {
            at > 0
                && repr[..at]
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-'))
        })
}

pub(super) fn completion(name: &str, values: &[Value], edit: CompletionTextEdit) -> CompletionItem {
    let value = values.first();
    let kind = match value {
        Some(Value::Func(_)) => CompletionItemKind::FUNCTION,
        Some(Value::Module(_)) => CompletionItemKind::MODULE,
        Some(_) => CompletionItemKind::VARIABLE,
        None => CompletionItemKind::FIELD,
    };
    let documentation = match value {
        Some(Value::Func(func)) => function_docs(func).map(super::markdown_documentation),
        _ => None,
    };
    let description = match values {
        [] => None,
        [value] => Some(description(value, name)),
        values => Some(
            values
                .iter()
                .map(|value| description(value, name))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    };
    CompletionItem {
        label: name.into(),
        kind: Some(kind),
        label_details: description.map(|description| CompletionItemLabelDetails {
            detail: None,
            description: Some(description),
        }),
        text_edit: Some(edit),
        documentation,
        ..CompletionItem::default()
    }
}

/// The named parameters a call's resolved callees still accept, as the items completion offers at
/// an argument position. A parameter the call already names is left out, and a name several
/// overloads share is offered once.
pub(super) fn parameter_completions(
    semantics: &Semantic<'_>,
    functions: &[(Value, bool)],
    taken: &std::collections::BTreeSet<String>,
    range: Range,
) -> Vec<CompletionItem> {
    let mut offered = std::collections::BTreeSet::new();
    let mut items = Vec::new();
    for (value, _) in functions {
        let Value::Func(func) = value else { continue };
        let documented = semantics
            .function_documentation(func, None)
            .map(|docs| tola_typst_syntax::docs::Documentation::parse(&docs));
        let name = func.name().unwrap_or_default();
        for parameter in signature(name, func, documented.as_ref(), &BTreeMap::new()).parameters {
            if parameter.kind != ParameterKind::Named
                || taken.contains(&parameter.name)
                || !offered.insert(parameter.name.clone())
            {
                continue;
            }
            let name = parameter.name.clone();
            items.push(CompletionItem {
                label: name.clone(),
                kind: Some(CompletionItemKind::FIELD),
                label_details: Some(CompletionItemLabelDetails {
                    detail: None,
                    description: Some(parameter.ty_spelling().to_owned()),
                }),
                documentation: parameter.docs.as_deref().map(super::markdown_documentation),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: crate::completion::typst_snippet(&format!("{name}: ${{}}")),
                })),
                insert_text_format: Some(lsp_types::InsertTextFormat::SNIPPET),
                ..CompletionItem::default()
            });
        }
    }
    items
}
#[cfg(test)]
mod tests {
    use super::*;
    use tola_typst::typst::foundations::Repr;
    use tola_typst::typst::{Library, LibraryExt as _};

    /// The largest value the standard library renders reaches a hover whole.
    #[test]
    fn library_values_reach_hovers_whole() {
        let library = Library::builder().build();
        let arrow = library
            .math
            .scope()
            .get("arrow")
            .expect("the math scope has the arrow symbol")
            .read()
            .clone();
        let repr = arrow.repr();
        assert!(repr.chars().count() > 3000, "{}", repr.chars().count());
        assert_eq!(abbreviated(&repr), repr);
    }

    /// A value larger than a hover has elides at an entry boundary, with its count.
    #[test]
    fn oversized_values_elide_at_entry_boundary() {
        let entry = format!("({}, {})", "x".repeat(300), "y".repeat(300));
        let repr = std::iter::repeat_n(entry.as_str(), 40)
            .collect::<Vec<_>>()
            .join(",\n");
        let elided = abbreviated(&repr);
        assert!(elided.starts_with("(xxx"), "{elided}");
        assert!(elided.ends_with(" characters omitted)"), "{elided}");
        // The line before the marker is a whole entry: the cut never interrupts one.
        let last = elided.lines().rev().nth(1).expect("a kept line");
        assert!(last.ends_with("),"), "{last}");
    }
}
