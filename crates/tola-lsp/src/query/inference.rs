//! Source expression inference shared by checked observations and immutable source bindings.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use tola_build::cancellation::BuildCancellation;
use tola_typst::GLOBAL_LIBRARY;
use tola_typst::typst::Library;
use tola_typst::typst::foundations::{Arg, Args, Array, Dict, Scope, Str, Styles, Type, Value};
use tola_typst::typst::syntax::ast::AstNode;
use tola_typst::typst::syntax::{
    FileId, LinkedNode, Source, Span, Spanned, SyntaxKind, SyntaxMode, ast,
};
use tola_typst_syntax::imports::{BindingId, ModuleExport, NameDefinition, NameGraph};
use tola_typst_syntax::names::{DeclarationKind, Initializer, SourceNames};

use super::expression::{ExpressionType, MemberCall, members, native_returns, single_value};

pub(in crate::query) const INITIALIZER_DEPTH: usize = 8;
// An alias or call can re-enter value lookup before its initializer identity is selected.
const EXPRESSION_DEPTH: usize = 64;

#[derive(Clone)]
pub(in crate::query) struct ExpressionBinding {
    pub span: Span,
    pub value: Option<Value>,
    pub parameter: bool,
    pub initializer: Option<Span>,
}

/// Owned by one request, so arguments and recursive initializers never escape their source parse.
#[derive(Default)]
pub(in crate::query) struct Inference {
    inferring: HashSet<Span>,
    parameters: Vec<HashMap<Span, ExpressionType>>,
    depth: usize,
}

impl Inference {
    pub(in crate::query) fn has_call_frame(&self) -> bool {
        !self.parameters.is_empty()
    }
}

/// A completed assignment or an unproven method call can invalidate the initializer. Callers
/// with a separate proof for a method may retain it; spelling alone is not a purity guarantee.
pub(in crate::query) fn binding_is_written(
    names: &SourceNames,
    declaration: usize,
    before: usize,
    preserves_method: impl Fn(&str) -> bool,
    ensure_active: impl Fn() -> Result<()>,
) -> Result<bool> {
    let mut stack = vec![LinkedNode::new(names.source().root())];
    while let Some(node) = stack.pop() {
        ensure_active()?;
        if node.range().start >= before {
            continue;
        }
        if node.range().end <= before {
            let target = if let Some(binary) = node.cast::<ast::Binary>()
                && matches!(
                    binary.op(),
                    ast::BinOp::Assign
                        | ast::BinOp::AddAssign
                        | ast::BinOp::SubAssign
                        | ast::BinOp::MulAssign
                        | ast::BinOp::DivAssign
                ) {
                node.find(binary.lhs().span())
            } else if let Some(call) = node.cast::<ast::FuncCall>()
                && let Some(callee) = node.find(call.callee().span())
                && let Some(access) = callee.cast::<ast::FieldAccess>()
                && !preserves_method(access.field().as_str())
                && !matches!(
                    names.declarations()[declaration].kind,
                    DeclarationKind::Import { .. }
                )
            {
                callee.find(access.target().span())
            } else {
                None
            };
            if let Some(target) = target {
                let root = tola_typst_syntax::syntax::root_expression(target);
                if names.declared_at(root.range().start) == Some(declaration) {
                    return Ok(true);
                }
            }
            if let Some(assignment) = node.cast::<ast::DestructAssignment>()
                && assignment.pattern().bindings().into_iter().any(|name| {
                    names.source().find(name.span()).is_some_and(|node| {
                        names.declared_at(node.range().start) == Some(declaration)
                    })
                })
            {
                return Ok(true);
            }
        }
        for child in node.children() {
            ensure_active()?;
            stack.push(child);
        }
    }
    Ok(false)
}

pub(in crate::query) struct SourceResolver<'a> {
    names: &'a SourceNames,
    graph: Option<&'a NameGraph>,
    cancellation: &'a BuildCancellation,
    inference: Inference,
}

impl<'a> SourceResolver<'a> {
    pub(in crate::query) fn new(
        names: &'a SourceNames,
        graph: Option<&'a NameGraph>,
        cancellation: &'a BuildCancellation,
    ) -> Self {
        Self {
            names,
            graph,
            cancellation,
            inference: Inference::default(),
        }
    }

    fn names(&self, file: FileId) -> Option<&SourceNames> {
        if file == self.names.source().id() {
            Some(self.names)
        } else {
            self.graph?.source(file)
        }
    }

    fn selected(&self, node: &LinkedNode<'_>) -> Option<BindingId> {
        let range = match node.cast::<ast::Expr>()? {
            ast::Expr::Ident(_) | ast::Expr::MathIdent(_) => node.range(),
            ast::Expr::FieldAccess(access) => node.find(access.field().span())?.range(),
            ast::Expr::MathFieldAccess(access) => node.find(access.field().span())?.range(),
            _ => return None,
        };
        self.graph?
            .selected(node.span().id()?, range.start)?
            .binding
    }

    fn ambiguous_wildcard(
        &self,
        node: &LinkedNode<'_>,
        binding: Option<BindingId>,
    ) -> Result<bool> {
        let Some(file) = node.span().id() else {
            return Ok(false);
        };
        let Some(names) = self.names(file) else {
            return Ok(false);
        };
        let candidate = binding
            .and_then(|id| {
                let bound = self.graph?.binding(id);
                if bound.file != file {
                    return None;
                }
                match bound.declaration {
                    Some(id) => {
                        let declaration = &names.declarations()[id];
                        Some((declaration.scope, declaration.visible_from))
                    }
                    None => {
                        let import = &names.imports()[bound.wildcard?];
                        Some((import.scope, import.range.end))
                    }
                }
            })
            .or_else(|| {
                names.declared_at(node.range().start).map(|id| {
                    let declaration = &names.declarations()[id];
                    (declaration.scope, declaration.visible_from)
                })
            });
        let at = node.range().start;
        let mut scope = Some(names.scope_at(at));
        while let Some(id) = scope {
            for (statement, import) in names.imports().iter().enumerate() {
                self.ensure_active()?;
                if import.wildcard
                    && import.scope == id
                    && import.range.end <= at
                    && candidate.is_none_or(|(scope, from)| scope != id || import.range.end > from)
                    && self
                        .graph
                        .is_none_or(|graph| !graph.interface_established(file, statement))
                {
                    return Ok(true);
                }
            }
            if candidate.is_some_and(|(scope, _)| scope == id) {
                return Ok(false);
            }
            scope = names.scopes()[id].parent;
        }
        Ok(false)
    }

    fn declared(
        &self,
        file: FileId,
        declaration: usize,
        followed: &mut HashSet<(FileId, usize)>,
        usage: (FileId, usize),
    ) -> Result<Option<ExpressionBinding>> {
        self.ensure_active()?;
        let Some(names) = self.names(file) else {
            return Ok(None);
        };
        let declaration_id = declaration;
        let declaration = &names.declarations()[declaration];
        let Some(node) =
            tola_typst_syntax::syntax::node_at_range(names.source(), &declaration.range)
        else {
            return Ok(None);
        };
        let mut binding = ExpressionBinding {
            span: node.span(),
            value: None,
            parameter: matches!(declaration.kind, DeclarationKind::Parameter),
            initializer: None,
        };
        if !followed.insert((file, declaration_id)) {
            return Ok(Some(binding));
        }
        let before = if usage.0 == file {
            usage.1
        } else {
            names.source().text().len()
        };
        let root_checked = usage.0 == file && names.declared_at(usage.1) == Some(declaration_id);
        if !root_checked
            && binding_is_written(
                names,
                declaration_id,
                before,
                |_| false,
                || self.ensure_active(),
            )?
        {
            binding.parameter = false;
            return Ok(Some(binding));
        }
        if let Initializer::Import { import, path } = &declaration.initializer {
            if !path.is_empty()
                && let Some(definition) = self
                    .graph
                    .and_then(|graph| graph.import_definition(file, *import, path))
            {
                return self.defined(definition, followed, usage);
            }
            return Ok(Some(binding));
        }
        if matches!(declaration.kind, DeclarationKind::Let) {
            let mut current = Some(node);
            while let Some(node) = current {
                self.ensure_active()?;
                if let Some(declared) = node.cast::<ast::LetBinding>() {
                    binding.initializer = declared.init().map(|init| init.span());
                    break;
                }
                current = node.parent().cloned();
            }
        }
        Ok(Some(binding))
    }

    fn defined(
        &self,
        definition: NameDefinition,
        followed: &mut HashSet<(FileId, usize)>,
        usage: (FileId, usize),
    ) -> Result<Option<ExpressionBinding>> {
        match definition {
            NameDefinition::Source { file, declaration } => {
                self.declared(file, declaration, followed, usage)
            }
            NameDefinition::Builtin(value) => Ok(Some(ExpressionBinding {
                span: Span::detached(),
                value: Some(value),
                parameter: false,
                initializer: None,
            })),
        }
    }

    fn root_is_written(&self, node: &LinkedNode<'_>) -> Result<bool> {
        self.ensure_active()?;
        let Some(file) = node.span().id() else {
            return Ok(false);
        };
        let Some(names) = self.names(file) else {
            return Ok(false);
        };
        let root = tola_typst_syntax::syntax::root_expression(node.clone());
        let Some(declaration) = names.declared_at(root.range().start) else {
            return Ok(false);
        };
        binding_is_written(
            names,
            declaration,
            node.range().start,
            |_| false,
            || self.ensure_active(),
        )
    }

    pub(in crate::query) fn module_members(
        &self,
        node: &LinkedNode<'_>,
        prefix: &str,
    ) -> Result<Option<Vec<ModuleExport>>> {
        self.ensure_active()?;
        let Some(binding) = self.selected(node) else {
            return Ok(None);
        };
        if self.root_is_written(node)? || self.ambiguous_wildcard(node, Some(binding))? {
            return Ok(None);
        }
        let Some(graph) = self.graph else {
            return Ok(None);
        };
        graph.binding_exports(binding, prefix, || self.ensure_active())
    }
}

impl ExpressionResolver for SourceResolver<'_> {
    fn source(&mut self, file: FileId) -> Option<Source> {
        Some(self.names(file)?.source().clone())
    }

    fn binding(&mut self, node: &LinkedNode<'_>) -> Result<Option<ExpressionBinding>> {
        if !matches!(
            node.kind(),
            SyntaxKind::Ident
                | SyntaxKind::MathIdent
                | SyntaxKind::FieldAccess
                | SyntaxKind::MathFieldAccess
        ) {
            return Ok(None);
        }
        let selected = self.selected(node);
        if self.root_is_written(node)? || self.ambiguous_wildcard(node, selected)? {
            return Ok(Some(ExpressionBinding {
                span: node.span(),
                value: None,
                parameter: false,
                initializer: None,
            }));
        }
        if let Some(binding) = selected
            && let Some(definition) = self
                .graph
                .and_then(|graph| graph.binding_definition(binding))
        {
            return self.defined(
                definition,
                &mut HashSet::new(),
                (node.span().id().unwrap(), node.range().start),
            );
        }
        let Some(file) = node.span().id() else {
            return Ok(None);
        };
        let declared = matches!(node.kind(), SyntaxKind::Ident | SyntaxKind::MathIdent)
            .then(|| {
                self.names(file)
                    .and_then(|names| names.declared_at(node.range().start))
            })
            .flatten();
        let Some(declaration) = declared else {
            return Ok(None);
        };
        self.declared(
            file,
            declaration,
            &mut HashSet::new(),
            (file, node.range().start),
        )
    }

    fn observed(&mut self, _: Span) -> Result<Vec<(Value, Option<Styles>)>> {
        Ok(Vec::new())
    }
    fn library(&self) -> &Library {
        &GLOBAL_LIBRARY
    }
    fn inference(&mut self) -> &mut Inference {
        &mut self.inference
    }
    fn ensure_active(&self) -> Result<()> {
        self.cancellation.ensure_active()?;
        Ok(())
    }
}

pub(in crate::query) trait ExpressionResolver: Sized {
    fn source(&mut self, file: FileId) -> Option<Source>;
    fn binding(&mut self, node: &LinkedNode<'_>) -> Result<Option<ExpressionBinding>>;
    fn observed(&mut self, span: Span) -> Result<Vec<(Value, Option<Styles>)>>;
    fn library(&self) -> &Library;
    fn inference(&mut self) -> &mut Inference;
    fn ensure_active(&self) -> Result<()>;

    fn types(&mut self, node: &LinkedNode<'_>) -> Result<Vec<(ExpressionType, Option<Styles>)>> {
        self.ensure_active()?;
        if self.inference().depth >= EXPRESSION_DEPTH {
            return Ok(self
                .observed(node.span())?
                .into_iter()
                .map(|(value, styles)| (ExpressionType::Known(value), styles))
                .collect());
        }
        self.inference().depth += 1;
        let values = self.expression_types(node);
        self.inference().depth -= 1;
        self.ensure_active()?;
        values.map(|values| {
            if values
                .iter()
                .any(|(ty, _)| matches!(ty, ExpressionType::Unknown))
            {
                Vec::new()
            } else {
                values
            }
        })
    }

    fn known_values(&mut self, node: &LinkedNode<'_>) -> Result<Vec<(Value, Option<Styles>)>> {
        let mut values = Vec::new();
        for (ty, styles) in self.types(node)? {
            self.ensure_active()?;
            let Some(value) = ty.into_value() else {
                return Ok(Vec::new());
            };
            values.push((value, styles));
        }
        Ok(values)
    }

    fn expression_types(
        &mut self,
        node: &LinkedNode<'_>,
    ) -> Result<Vec<(ExpressionType, Option<Styles>)>> {
        if node.kind() == SyntaxKind::Contextual
            && let Some(body) = node.children().next_back()
        {
            return self.types(&body);
        }
        let values = self.observed(node.span())?;
        if !values.is_empty() {
            return Ok(values
                .into_iter()
                .map(|(value, styles)| (ExpressionType::Known(value), styles))
                .collect());
        }
        if (expression_name(node).is_some()
            || matches!(
                node.kind(),
                SyntaxKind::FieldAccess | SyntaxKind::MathFieldAccess
            ))
            && let Some(binding) = self.binding(node)?
        {
            if let Some(value) = binding.value {
                return Ok(vec![(ExpressionType::Known(value), None)]);
            }
            if binding.parameter
                && let Some(value) = self.parameter_type(binding.span)
            {
                return Ok(vec![(value, None)]);
            }
            let traced = self.observed(binding.span)?;
            if !traced.is_empty() {
                return Ok(traced
                    .into_iter()
                    .map(|(value, styles)| (ExpressionType::Known(value), styles))
                    .collect());
            }
            if let Some(initializer) = binding.initializer {
                if let Some(value) = self.constant_span(initializer, 0)? {
                    return Ok(self
                        .project_binding_types(binding.span, vec![ExpressionType::Known(value)])?
                        .into_iter()
                        .map(|value| (value, None))
                        .collect());
                }
                let inferred = self.inferred_span(initializer)?;
                return Ok(self
                    .project_binding_types(binding.span, inferred)?
                    .into_iter()
                    .map(|value| (value, None))
                    .collect());
            }
            // A declaration shadows every library name, even when its own value never evaluated.
            return Ok(Vec::new());
        }
        if is_name_reference(node) {
            if let Some(value) = self.library_value(node) {
                return Ok(vec![(ExpressionType::Known(value), None)]);
            }
            if let Some(value) = self.constant(node, 0)? {
                return Ok(vec![(ExpressionType::Known(value), None)]);
            }
            let inferred = self.inferred(node)?;
            if !inferred.is_empty() {
                return Ok(inferred.into_iter().map(|value| (value, None)).collect());
            }
        }
        Ok(Vec::new())
    }

    /// The library and the scope one syntax mode resolves names in.
    fn library_scope(&self, mode: SyntaxMode) -> (&Library, &Scope) {
        let library = self.library();
        let scope = match mode {
            SyntaxMode::Math => library.math.scope(),
            _ => library.global.scope(),
        };
        (library, scope)
    }

    fn parameter_type(&mut self, declaration: Span) -> Option<ExpressionType> {
        self.inference()
            .parameters
            .iter()
            .rev()
            .find_map(|scope| scope.get(&declaration).cloned())
    }

    /// The initializer a declaration span names, for a function that lives in another source.
    fn declaration_initializer(&mut self, span: Span) -> Result<Option<Span>> {
        let Some(source) = span.id().and_then(|id| self.source(id)) else {
            return Ok(None);
        };
        let root = LinkedNode::new(source.root());
        let Some(node) = root.find(span) else {
            return Ok(None);
        };
        if matches!(node.cast::<ast::Expr>(), Some(ast::Expr::Closure(_))) {
            return Ok(Some(span));
        }
        let mut current = Some(node);
        while let Some(node) = current {
            self.ensure_active()?;
            if let Some(binding) = node.cast::<ast::LetBinding>() {
                return Ok(binding.init().map(|init| init.span()));
            }
            current = node.parent().cloned();
        }
        Ok(None)
    }

    /// The value a library name holds in the mode the expression reads it in.
    ///
    /// A site that failed before reaching this expression traces no value, and a library name is
    /// no declaration the file holds; the standard library still answers, as Typst's own IDE does.
    fn library_value(&self, node: &LinkedNode<'_>) -> Option<Value> {
        let (name, mode) = match node.cast::<ast::Expr>()? {
            ast::Expr::Ident(ident) => (ident.get(), SyntaxMode::Code),
            ast::Expr::MathIdent(ident) => (ident.get(), SyntaxMode::Math),
            _ => return None,
        };
        let (library, scope) = self.library_scope(mode);
        scope
            .get(name.as_str())
            .map(|binding| binding.read().clone())
            .or_else(|| (name.as_str() == "std").then(|| library.std.read().clone()))
    }

    fn inferred_span(&mut self, span: Span) -> Result<Vec<ExpressionType>> {
        let Some(id) = span.id() else {
            return Ok(Vec::new());
        };
        let Some(source) = self.source(id) else {
            return Ok(Vec::new());
        };
        let root = LinkedNode::new(source.root());
        let Some(node) = root.find(span) else {
            return Ok(Vec::new());
        };
        self.inferred(&node)
    }

    fn project_binding_types(
        &mut self,
        span: Span,
        values: Vec<ExpressionType>,
    ) -> Result<Vec<ExpressionType>> {
        let Some(source) = span.id().and_then(|file| self.source(file)) else {
            return Ok(Vec::new());
        };
        let root = LinkedNode::new(source.root());
        let mut current = root.find(span);
        while let Some(node) = current {
            self.ensure_active()?;
            if let Some(binding) = node.cast::<ast::LetBinding>() {
                let ast::LetBindingKind::Normal(pattern) = binding.kind() else {
                    return Ok(values);
                };
                if !pattern.bindings().iter().any(|ident| ident.span() == span) {
                    return Ok(values);
                }
                let mut projected = Vec::new();
                for value in values {
                    self.ensure_active()?;
                    let mut bindings = HashMap::new();
                    if !bind_pattern(pattern, value, &mut bindings, &|| self.ensure_active())? {
                        return Ok(Vec::new());
                    }
                    let Some(value) = bindings.remove(&span) else {
                        return Ok(Vec::new());
                    };
                    projected.push(value);
                }
                return Ok(projected);
            }
            current = node.parent().cloned();
        }
        Ok(values)
    }

    fn constant_span(&mut self, span: Span, depth: usize) -> Result<Option<Value>> {
        let Some(id) = span.id() else {
            return Ok(None);
        };
        let Some(source) = self.source(id) else {
            return Ok(None);
        };
        let root = LinkedNode::new(source.root());
        let Some(node) = root.find(span) else {
            return Ok(None);
        };
        self.constant(&node, depth)
    }

    /// The value an expression spells without the site's evaluation.
    ///
    /// Completion answers while the site is mid-edit, so a literal, array, dictionary, or a field
    /// selected from one still names its members, and a name resolves through the file's bindings
    /// or the library. Anything the evaluator owns — calls, operators, control flow — has no value
    /// here.
    fn constant(&mut self, node: &LinkedNode<'_>, depth: usize) -> Result<Option<Value>> {
        self.ensure_active()?;
        if depth > INITIALIZER_DEPTH {
            return Ok(None);
        }
        let Some(expr) = node.cast::<ast::Expr>() else {
            return Ok(None);
        };
        Ok(Some(match expr {
            ast::Expr::None(_) => Value::None,
            ast::Expr::Auto(_) => Value::Auto,
            ast::Expr::Bool(value) => Value::Bool(value.get()),
            ast::Expr::Int(value) => Value::Int(value.get()),
            ast::Expr::Float(value) => Value::Float(value.get()),
            ast::Expr::Numeric(value) => Value::numeric(value.get()),
            ast::Expr::Str(value) => Value::Str(value.get().into()),
            ast::Expr::Parenthesized(value) => {
                let Some(inner) = node.find(value.expr().span()) else {
                    return Ok(None);
                };
                return self.constant(&inner, depth + 1);
            }
            ast::Expr::FieldAccess(access) => {
                let Some(target) = node.find(access.target().span()) else {
                    return Ok(None);
                };
                let Some(value) = self.constant(&target, depth + 1)? else {
                    return Ok(None);
                };
                let Some(field) = value.field(access.field().as_str(), ()).ok() else {
                    return Ok(None);
                };
                field
            }
            ast::Expr::Array(array) => {
                let mut values = Array::new();
                for item in array.items() {
                    self.ensure_active()?;
                    let ast::ArrayItem::Pos(item) = item else {
                        return Ok(None);
                    };
                    let Some(item) = node.find(item.span()) else {
                        return Ok(None);
                    };
                    let Some(value) = self.constant(&item, depth + 1)? else {
                        return Ok(None);
                    };
                    values.push(value);
                }
                Value::Array(values)
            }
            ast::Expr::Ident(_) | ast::Expr::MathIdent(_) => {
                if !is_name_reference(node) {
                    return Ok(None);
                }
                if let Some(binding) = self.binding(node)? {
                    if let Some(value) = binding.value {
                        return Ok(Some(value));
                    }
                    if binding.parameter
                        && let Some(value) = self
                            .parameter_type(binding.span)
                            .and_then(ExpressionType::into_value)
                    {
                        return Ok(Some(value));
                    }
                    if let Some(initializer) = binding.initializer {
                        let values = self
                            .constant_span(initializer, depth + 1)?
                            .into_iter()
                            .map(ExpressionType::Known)
                            .collect();
                        return Ok(
                            single_value(self.project_binding_types(binding.span, values)?)
                                .and_then(ExpressionType::into_value),
                        );
                    }
                    return Ok(None);
                }
                return Ok(self.library_value(node));
            }
            ast::Expr::Dict(dict) => {
                let mut values = Dict::new();
                for item in dict.items() {
                    self.ensure_active()?;
                    let (key, value) = match item {
                        ast::DictItem::Named(pair) => {
                            (Str::from(pair.name().get().as_str()), pair.expr())
                        }
                        ast::DictItem::Keyed(pair) => {
                            let Some(key_node) = node.find(pair.key().span()) else {
                                return Ok(None);
                            };
                            let Some(Value::Str(key)) = self.constant(&key_node, depth + 1)? else {
                                return Ok(None);
                            };
                            (key, pair.expr())
                        }
                        ast::DictItem::Spread(_) => return Ok(None),
                    };
                    let Some(value_node) = node.find(value.span()) else {
                        return Ok(None);
                    };
                    let Some(value) = self.constant(&value_node, depth + 1)? else {
                        return Ok(None);
                    };
                    values.insert(key, value);
                }
                Value::Dict(values)
            }
            _ => return Ok(None),
        }))
    }

    fn field_functions(
        &mut self,
        receiver: &LinkedNode<'_>,
        field: &str,
    ) -> Result<Vec<(Value, bool)>> {
        let mut functions = Vec::new();
        for (receiver, styles) in self.types(receiver)? {
            self.ensure_active()?;
            let Some(member) =
                members(&receiver, styles.as_ref(), || self.ensure_active())?.remove(field)
            else {
                return Ok(Vec::new());
            };
            let (Some(call), Some(value)) = (member.call, member.ty.into_value()) else {
                return Ok(Vec::new());
            };
            functions.push((value, matches!(call, MemberCall::Receiver)));
        }
        Ok(functions)
    }

    /// The values an expression can have, inferred from its syntax.
    ///
    /// This runs only after the compiler's trace, the file's bindings, and literal evaluation
    /// found nothing; a successful evaluation always wins.
    fn inferred(&mut self, node: &LinkedNode<'_>) -> Result<Vec<ExpressionType>> {
        self.ensure_active()?;
        let Some(expr) = node.cast::<ast::Expr>() else {
            return Ok(Vec::new());
        };
        match expr {
            ast::Expr::Parenthesized(paren) => self.infer_child(node, paren.expr().span()),
            ast::Expr::Binary(binary) => {
                let Some(lhs) = node.find(binary.lhs().span()) else {
                    return Ok(Vec::new());
                };
                let Some(rhs) = node.find(binary.rhs().span()) else {
                    return Ok(Vec::new());
                };
                let lhs = single_value(plain(self.types(&lhs)?));
                let rhs = single_value(plain(self.types(&rhs)?));
                Ok(binary_type(binary.op(), lhs.as_ref(), rhs.as_ref())
                    .into_iter()
                    .collect())
            }
            ast::Expr::Unary(unary) => {
                let Some(inner) = node.find(unary.expr().span()) else {
                    return Ok(Vec::new());
                };
                let value = single_value(plain(self.types(&inner)?));
                Ok(unary_type(unary.op(), value.as_ref()).into_iter().collect())
            }
            ast::Expr::Conditional(conditional) => {
                let mut values = Vec::new();
                for body in [Some(conditional.if_body()), conditional.else_body()] {
                    self.ensure_active()?;
                    let branch = match body {
                        Some(body) => {
                            let Some(body) = node.find(body.span()) else {
                                return Ok(Vec::new());
                            };
                            plain(self.types(&body)?)
                        }
                        None => vec![ExpressionType::Known(Value::None)],
                    };
                    if branch.is_empty() {
                        return Ok(Vec::new());
                    }
                    values.extend(branch);
                }
                Ok(values)
            }
            ast::Expr::FieldAccess(access) => {
                let Some(target) = node.find(access.target().span()) else {
                    return Ok(Vec::new());
                };
                let field = access.field().as_str();
                let receivers = plain(self.types(&target)?);
                let mut fields = Vec::new();
                for receiver in receivers {
                    self.ensure_active()?;
                    let Some(value) = receiver.field(field, || self.ensure_active())? else {
                        return Ok(Vec::new());
                    };
                    fields.push(value);
                }
                Ok(fields)
            }
            ast::Expr::Contextual(_) => match node.children().next_back() {
                Some(body) => Ok(plain(self.types(&body)?)),
                None => Ok(Vec::new()),
            },
            ast::Expr::CodeBlock(block) => {
                if has_control_flow(node, &|| self.ensure_active())? {
                    return Ok(Vec::new());
                }
                let mut expressions = block.body().exprs().peekable();
                while let Some(expression) = expressions.next() {
                    self.ensure_active()?;
                    if expressions.peek().is_none() {
                        return self.infer_child(node, expression.span());
                    }
                    if matches!(
                        expression,
                        ast::Expr::LetBinding(_) | ast::Expr::ModuleImport(_)
                    ) {
                        continue;
                    }
                    // Typst joins every expression: only a prefix producing `none` preserves
                    // the tail's type, and an unknown prefix proves nothing about that join.
                    let preceding = self.infer_child(node, expression.span())?;
                    if preceding.is_empty()
                        || preceding.iter().any(|value| {
                            value.ty()
                                != Some(Type::of::<tola_typst::typst::foundations::NoneValue>())
                        })
                    {
                        return Ok(Vec::new());
                    }
                }
                Ok(vec![ExpressionType::Known(Value::None)])
            }
            ast::Expr::Dict(dict) => {
                if has_control_flow(node, &|| self.ensure_active())? {
                    return Ok(Vec::new());
                }
                let mut fields = BTreeMap::new();
                for item in dict.items() {
                    self.ensure_active()?;
                    let (key, expr) = match item {
                        ast::DictItem::Named(pair) => (pair.name().get().to_string(), pair.expr()),
                        ast::DictItem::Keyed(pair) => {
                            let Some(key) = node.find(pair.key().span()) else {
                                return Ok(Vec::new());
                            };
                            let Some(Value::Str(key)) = self.constant(&key, 0)? else {
                                return Ok(Vec::new());
                            };
                            (key.to_string(), pair.expr())
                        }
                        ast::DictItem::Spread(_) => return Ok(Vec::new()),
                    };
                    let ty = match node.find(expr.span()) {
                        Some(child) => single_value(plain(self.types(&child)?))
                            .unwrap_or(ExpressionType::Unknown),
                        None => ExpressionType::Unknown,
                    };
                    fields.insert(key, ty);
                }
                Ok(vec![ExpressionType::Dictionary(fields)])
            }
            ast::Expr::Array(array) => {
                if has_control_flow(node, &|| self.ensure_active())? {
                    return Ok(Vec::new());
                }
                let mut items = Vec::new();
                for item in array.items() {
                    self.ensure_active()?;
                    let ast::ArrayItem::Pos(expr) = item else {
                        return Ok(Vec::new());
                    };
                    let ty = match node.find(expr.span()) {
                        Some(child) => single_value(plain(self.types(&child)?))
                            .unwrap_or(ExpressionType::Unknown),
                        None => ExpressionType::Unknown,
                    };
                    items.push(ty);
                }
                Ok(vec![ExpressionType::tuple(items, || self.ensure_active())?])
            }
            ast::Expr::FuncCall(call) => self.inferred_call(node, call),
            _ => Ok(Vec::new()),
        }
    }

    fn infer_child(&mut self, root: &LinkedNode<'_>, span: Span) -> Result<Vec<ExpressionType>> {
        let Some(child) = root.find(span) else {
            return Ok(Vec::new());
        };
        Ok(plain(self.types(&child)?))
    }

    /// Source calls bind the current argument frame; native calls expose only official return metadata.
    fn inferred_call(
        &mut self,
        node: &LinkedNode<'_>,
        call: ast::FuncCall<'_>,
    ) -> Result<Vec<ExpressionType>> {
        let Some(callee) = node.find(call.callee().span()) else {
            return Ok(Vec::new());
        };
        if let Some(binding) = self.binding(&callee)? {
            let initializer = if binding.parameter {
                None
            } else {
                match binding.initializer {
                    Some(initializer) => Some(initializer),
                    None => self.declaration_initializer(binding.span)?,
                }
            };
            if let Some(initializer) = initializer {
                if !self.inference().inferring.insert(initializer) {
                    return Ok(Vec::new());
                }
                let inferred = self.infer_function(initializer, Some((node, call)));
                self.inference().inferring.remove(&initializer);
                return inferred;
            }
            return Ok(binding
                .value
                .as_ref()
                .map(native_returns)
                .unwrap_or_default());
        }
        if let Some(ast::Expr::FieldAccess(access)) = callee.cast::<ast::Expr>() {
            let Some(target) = callee.find(access.target().span()) else {
                return Ok(Vec::new());
            };
            if self.types(&target)?.len() != 1 {
                return Ok(Vec::new());
            }
            return Ok(self
                .field_functions(&target, access.field().as_str())?
                .into_iter()
                .flat_map(|(function, _)| native_returns(&function))
                .collect());
        }
        let callable = single_value(
            self.known_values(&callee)?
                .into_iter()
                .map(|(value, _)| value)
                .collect(),
        );
        Ok(callable.as_ref().map(native_returns).unwrap_or_default())
    }

    fn infer_function(
        &mut self,
        span: Span,
        call: Option<(&LinkedNode<'_>, ast::FuncCall<'_>)>,
    ) -> Result<Vec<ExpressionType>> {
        let Some(id) = span.id() else {
            return Ok(Vec::new());
        };
        let Some(source) = self.source(id) else {
            return Ok(Vec::new());
        };
        let root = LinkedNode::new(source.root());
        let Some(mut node) = root.find(span) else {
            return Ok(Vec::new());
        };
        while let Some(ast::Expr::Parenthesized(group)) = node.cast::<ast::Expr>() {
            self.ensure_active()?;
            let Some(inner) = node.find(group.expr().span()) else {
                return Ok(Vec::new());
            };
            node = inner;
        }
        let Some(ast::Expr::Closure(closure)) = node.cast::<ast::Expr>() else {
            let Some(binding) = self.binding(&node)? else {
                return Ok(single_value(plain(self.types(&node)?))
                    .and_then(ExpressionType::into_value)
                    .as_ref()
                    .map(native_returns)
                    .unwrap_or_default());
            };
            let Some(initializer) = binding.initializer else {
                return Ok(binding
                    .value
                    .as_ref()
                    .map(native_returns)
                    .unwrap_or_default());
            };
            if !self.inference().inferring.insert(initializer) {
                return Ok(Vec::new());
            }
            let values = self.infer_function(initializer, call);
            self.inference().inferring.remove(&initializer);
            return values;
        };
        let scope = match call {
            Some((call_node, call)) => self.call_arguments(&node, closure, call_node, call)?,
            None => HashMap::new(),
        };
        self.inference().parameters.push(scope);
        let inferred = self.infer_child(&node, closure.body().span());
        self.inference().parameters.pop();
        inferred
    }

    /// Argument values are read before pushing parameters; declaration spans distinguish
    /// parameters with the same spelling in nested calls. An unresolved spread leaves both
    /// positional and named arguments unknown.
    fn call_arguments(
        &mut self,
        closure_node: &LinkedNode<'_>,
        closure: ast::Closure<'_>,
        call_node: &LinkedNode<'_>,
        call: ast::FuncCall<'_>,
    ) -> Result<HashMap<Span, ExpressionType>> {
        let mut args = Vec::new();
        for argument in call.args().items() {
            self.ensure_active()?;
            if matches!(argument, ast::Arg::Spread(_)) {
                return Ok(HashMap::new());
            }
            args.push(argument);
        }
        let mut positional = args.iter().filter_map(|arg| match arg {
            ast::Arg::Pos(expr) => Some(*expr),
            _ => None,
        });
        let mut named_parameters = HashSet::new();
        for parameter in closure.params().children() {
            self.ensure_active()?;
            if let ast::Param::Named(named) = parameter {
                named_parameters.insert(named.name().get().to_string());
            }
        }
        let mut scope = HashMap::new();
        for param in closure.params().children() {
            self.ensure_active()?;
            match param {
                ast::Param::Pos(pattern) => {
                    let Some(expr) = positional.next() else {
                        continue;
                    };
                    let Some(argument) = call_node.find(expr.span()) else {
                        continue;
                    };
                    let Some(value) = single_value(plain(self.types(&argument)?)) else {
                        continue;
                    };
                    let mut bindings = HashMap::new();
                    if bind_pattern(pattern, value, &mut bindings, &|| self.ensure_active())? {
                        scope.extend(bindings);
                    }
                }
                ast::Param::Named(named) => {
                    let name = named.name().get().to_string();
                    let explicit = args.iter().find_map(|arg| match arg {
                        ast::Arg::Named(arg) if arg.name().get().as_str() == name => {
                            Some(arg.expr())
                        }
                        _ => None,
                    });
                    let value = match explicit {
                        Some(expr) => {
                            let Some(argument) = call_node.find(expr.span()) else {
                                continue;
                            };
                            single_value(plain(self.types(&argument)?))
                        }
                        None => {
                            let Some(default) = closure_node.find(named.expr().span()) else {
                                continue;
                            };
                            // A default reads the scope the closure is written in; Typst keeps the
                            // closure's own parameters out of it.
                            single_value(plain(self.types(&default)?))
                        }
                    };
                    let Some(value) = value else {
                        continue;
                    };
                    scope.insert(named.name().span(), value);
                }
                ast::Param::Spread(spread) => {
                    let Some(sink) = spread.sink_ident() else {
                        continue;
                    };
                    let mut values = Array::new();
                    let mut known = true;
                    for expr in positional.by_ref() {
                        self.ensure_active()?;
                        let Some(argument) = call_node.find(expr.span()) else {
                            continue;
                        };
                        let Some(value) = single_value(plain(self.types(&argument)?)) else {
                            known = false;
                            continue;
                        };
                        if let Some(value) = value.into_value() {
                            values.push(value);
                        } else {
                            known = false;
                        }
                    }
                    let mut captured = Args::new(call.span(), values);
                    for argument in &args {
                        self.ensure_active()?;
                        let ast::Arg::Named(named) = argument else {
                            continue;
                        };
                        if named_parameters.contains(named.name().get().as_str()) {
                            continue;
                        }
                        let Some(argument) = call_node.find(named.expr().span()) else {
                            continue;
                        };
                        let Some(value) = single_value(plain(self.types(&argument)?)) else {
                            known = false;
                            continue;
                        };
                        if let Some(value) = value.into_value() {
                            captured.items.push(Arg {
                                span: argument.span(),
                                name: Some(named.name().get().clone().into()),
                                value: Spanned::new(value, argument.span()),
                            });
                        } else {
                            known = false;
                        }
                    }
                    scope.insert(
                        sink.span(),
                        if known {
                            ExpressionType::Known(Value::Args(captured))
                        } else {
                            ExpressionType::Unknown
                        },
                    );
                }
            }
        }
        Ok(scope)
    }
}

/// The name an expression reads, whether it is written in code or math.
pub(in crate::query) fn expression_name(node: &LinkedNode<'_>) -> Option<String> {
    match node.cast::<ast::Expr>()? {
        ast::Expr::Ident(ident) => Some(ident.get().to_string()),
        ast::Expr::MathIdent(ident) => Some(ident.get().to_string()),
        _ => None,
    }
}

/// Whether `node` reads a name, not a declaration, parameter, or dict key that spells one.
pub(in crate::query) fn is_name_reference(node: &LinkedNode<'_>) -> bool {
    if !matches!(
        node.cast::<ast::Expr>(),
        Some(ast::Expr::Ident(_) | ast::Expr::MathIdent(_))
    ) {
        return true;
    }
    match node.parent_kind() {
        // The initializer follows `=`, the bound name precedes it.
        Some(SyntaxKind::LetBinding) => node.prev_sibling_kind() == Some(SyntaxKind::Eq),
        // A dict key or parameter name precedes its colon; the value follows it.
        Some(SyntaxKind::Named) => node.prev_sibling_kind() == Some(SyntaxKind::Colon),
        Some(SyntaxKind::Params | SyntaxKind::Destructuring | SyntaxKind::Closure) => false,
        // The iterable follows `in`; the loop pattern precedes it.
        Some(SyntaxKind::ForLoop) => node.prev_sibling_kind() == Some(SyntaxKind::In),
        Some(SyntaxKind::ImportItemPath | SyntaxKind::RenamedImportItem) => false,
        Some(SyntaxKind::FieldAccess | SyntaxKind::MathFieldAccess) => {
            node.prev_sibling_kind() != Some(SyntaxKind::Dot)
        }
        _ => true,
    }
}

fn has_control_flow(
    root: &LinkedNode<'_>,
    ensure_active: &impl Fn() -> Result<()>,
) -> Result<bool> {
    let mut stack = vec![root.clone()];
    while let Some(node) = stack.pop() {
        ensure_active()?;
        if matches!(
            node.kind(),
            SyntaxKind::Return | SyntaxKind::Break | SyntaxKind::Continue
        ) {
            return Ok(true);
        }
        if let Some(closure) = node.cast::<ast::Closure>() {
            // Defaults run while the closure is created; its body runs in its own call frame.
            if let Some(parameters) = node.find(closure.params().span()) {
                stack.push(parameters);
            }
        } else {
            for child in node.children() {
                ensure_active()?;
                stack.push(child);
            }
        }
    }
    Ok(false)
}

fn bind_pattern(
    pattern: ast::Pattern<'_>,
    value: ExpressionType,
    scope: &mut HashMap<Span, ExpressionType>,
    ensure_active: &impl Fn() -> Result<()>,
) -> Result<bool> {
    ensure_active()?;
    match pattern {
        ast::Pattern::Normal(ast::Expr::Ident(ident)) => {
            scope.insert(ident.span(), value);
            Ok(true)
        }
        ast::Pattern::Placeholder(_) => Ok(true),
        ast::Pattern::Parenthesized(group) => {
            bind_pattern(group.pattern(), value, scope, ensure_active)
        }
        ast::Pattern::Destructuring(destructuring) => {
            bind_destructuring(destructuring, value, scope, ensure_active)
        }
        _ => Ok(false),
    }
}

fn bind_destructuring(
    destructuring: ast::Destructuring<'_>,
    value: ExpressionType,
    scope: &mut HashMap<Span, ExpressionType>,
    ensure_active: &impl Fn() -> Result<()>,
) -> Result<bool> {
    let mut items = Vec::new();
    for item in destructuring.items() {
        ensure_active()?;
        items.push(item);
    }
    match &value {
        ExpressionType::Known(Value::Dict(_)) | ExpressionType::Dictionary(_) => {
            let mut used = HashSet::new();
            let mut sink = None;
            for item in items {
                ensure_active()?;
                let (name, pattern) = match item {
                    ast::DestructuringItem::Pattern(
                        pattern @ ast::Pattern::Normal(ast::Expr::Ident(ident)),
                    ) => (ident.get(), pattern),
                    ast::DestructuringItem::Named(named) => (named.name().get(), named.pattern()),
                    ast::DestructuringItem::Spread(spread) => {
                        if sink.is_some() {
                            return Ok(false);
                        }
                        sink = spread.sink_expr();
                        continue;
                    }
                    _ => return Ok(false),
                };
                let Some(field) = value.field(name.as_str(), ensure_active)? else {
                    return Ok(false);
                };
                if !bind_pattern(pattern, field, scope, ensure_active)? {
                    return Ok(false);
                }
                used.insert(name.as_str());
            }
            if let Some(sink) = sink {
                let ast::Expr::Ident(ident) = sink else {
                    return Ok(false);
                };
                let remaining = match &value {
                    ExpressionType::Known(Value::Dict(dict)) => {
                        let mut remaining = Dict::new();
                        for (name, value) in dict.iter() {
                            ensure_active()?;
                            if !used.contains(name.as_str()) {
                                remaining.insert(name.clone(), value.clone());
                            }
                        }
                        ExpressionType::Known(Value::Dict(remaining))
                    }
                    ExpressionType::Dictionary(fields) => {
                        let mut remaining = BTreeMap::new();
                        for (name, ty) in fields {
                            ensure_active()?;
                            if !used.contains(name.as_str()) {
                                remaining.insert(name.clone(), ty.clone());
                            }
                        }
                        ExpressionType::Dictionary(remaining)
                    }
                    _ => unreachable!(),
                };
                scope.insert(ident.span(), remaining);
            }
        }
        ExpressionType::Known(Value::Array(_)) | ExpressionType::Tuple(_) => {
            let len = match &value {
                ExpressionType::Known(Value::Array(array)) => array.len(),
                ExpressionType::Tuple(items) => items.len(),
                _ => unreachable!(),
            };
            let item_at = |index: usize| match &value {
                ExpressionType::Known(Value::Array(array)) => {
                    ExpressionType::Known(array.as_slice()[index].clone())
                }
                ExpressionType::Tuple(items) => items[index].clone(),
                _ => unreachable!(),
            };
            let mut spreads = 0;
            for item in &items {
                ensure_active()?;
                match item {
                    ast::DestructuringItem::Named(_) => return Ok(false),
                    ast::DestructuringItem::Spread(_) => spreads += 1,
                    ast::DestructuringItem::Pattern(_) => {}
                }
            }
            let required = items.len() - spreads;
            let Some(sink_size) = len.checked_sub(required) else {
                return Ok(false);
            };
            if spreads > 1 || (spreads == 0 && sink_size != 0) {
                return Ok(false);
            }
            let mut offset = 0;
            for item in items {
                ensure_active()?;
                match item {
                    ast::DestructuringItem::Pattern(pattern) => {
                        if !bind_pattern(pattern, item_at(offset), scope, ensure_active)? {
                            return Ok(false);
                        }
                        offset += 1;
                    }
                    ast::DestructuringItem::Spread(spread) => {
                        if let Some(sink) = spread.sink_expr() {
                            let ast::Expr::Ident(ident) = sink else {
                                return Ok(false);
                            };
                            let mut captured = Vec::new();
                            for index in offset..offset + sink_size {
                                ensure_active()?;
                                captured.push(item_at(index));
                            }
                            scope.insert(
                                ident.span(),
                                ExpressionType::tuple(captured, ensure_active)?,
                            );
                        }
                        offset += sink_size;
                    }
                    ast::DestructuringItem::Named(_) => unreachable!(),
                }
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn plain(values: Vec<(ExpressionType, Option<Styles>)>) -> Vec<ExpressionType> {
    values.into_iter().map(|(value, _)| value).collect()
}

fn binary_type(
    op: ast::BinOp,
    lhs: Option<&ExpressionType>,
    rhs: Option<&ExpressionType>,
) -> Option<ExpressionType> {
    use ast::BinOp;
    let (lhs, rhs) = (lhs?.ty()?, rhs?.ty()?);
    let int = Type::of::<i64>();
    let float = Type::of::<f64>();
    let numeric = |ty| ty == int || ty == float;
    let ty = match op {
        BinOp::Add if lhs == Type::of::<Str>() && rhs == lhs => lhs,
        BinOp::Add if lhs == Type::of::<Array>() && rhs == lhs => lhs,
        BinOp::Add | BinOp::Sub | BinOp::Mul if numeric(lhs) && numeric(rhs) => {
            if lhs == int && rhs == int { int } else { float }
        }
        BinOp::Div if numeric(lhs) && numeric(rhs) => float,
        BinOp::And
        | BinOp::Or
        | BinOp::Eq
        | BinOp::Neq
        | BinOp::Lt
        | BinOp::Leq
        | BinOp::Gt
        | BinOp::Geq
        | BinOp::In
        | BinOp::NotIn => Type::of::<bool>(),
        _ => return None,
    };
    Some(ExpressionType::Instance(ty))
}

fn unary_type(op: ast::UnOp, value: Option<&ExpressionType>) -> Option<ExpressionType> {
    let ty = match op {
        ast::UnOp::Not => Type::of::<bool>(),
        ast::UnOp::Pos | ast::UnOp::Neg => {
            let ty = value?.ty()?;
            if ty != Type::of::<i64>() && ty != Type::of::<f64>() {
                return None;
            }
            ty
        }
    };
    Some(ExpressionType::Instance(ty))
}

#[cfg(test)]
mod tests {
    use crate::query::tests::*;

    #[test]
    fn known_dictionaries_preserve_key_order() {
        use super::*;

        for text in [
            "#let value = (z: 1, a: 2)\n#value",
            "#let (ignored, ..value) = (ignored: 0, z: 1, a: 2)\n#value",
        ] {
            let source = Source::detached(text);
            let names = SourceNames::new(source.clone());
            let node = tola_typst_syntax::syntax::expression(&source, text.len()).unwrap();
            let cancellation = BuildCancellation::new();
            let values = SourceResolver::new(&names, None, &cancellation)
                .known_values(&node)
                .unwrap();
            let Value::Dict(dict) = &values[0].0 else {
                panic!("dictionary value: {values:?}");
            };
            assert_eq!(
                dict.keys(),
                [Value::Str("z".into()), Value::Str("a".into())]
                    .into_iter()
                    .collect::<Array>(),
                "{text}"
            );
        }
    }

    #[test]
    fn destructured_names_keep_projected_values() {
        use super::*;
        use crate::query::source_member_completion;

        for marked in [
            "#let (value,) = ((current: 1),)\n#value.|",
            "#let (ignored, value) = ((wrong: 1), (current: 1))\n#value.|",
            "#let (..rest, value) = (0, (wrong: 1), (current: 1))\n#value.|",
            "#let (ignored, ..rest) = (0, (current: 1), (extra: 1))\n#let (value, ..) = rest\n#value.|",
            "#let ((value,),) = (((current: 1),),)\n#value.|",
            "#let (value,) = (value: (current: 1))\n#value.|",
            "#let (slot: (value,),) = (slot: ((current: 1),))\n#value.|",
            "#let (ignored, ..rest) = (ignored: 0, current: 1)\n#rest.|",
            "#let make() = ((current: 1),)\n#let (value,) = make()\n#value.|",
        ] {
            let cursor = marked.find('|').unwrap();
            let source = Source::detached(marked.replace('|', ""));
            let names = SourceNames::new(source.clone());
            let cold =
                source_member_completion(None, &names, &source, cursor, &BuildCancellation::new())
                    .unwrap()
                    .unwrap_or_default();
            for items in [
                cold,
                QuerySession::with_program(FAILED_PROGRAM)
                    .completion(&format!("#panic(\"page\")\n{marked}")),
            ] {
                let labels = completion_labels(&items);
                assert!(labels.contains(&"current"), "{marked}: {labels:?}");
                for absent in ["wrong", "flatten", "slot", "ignored"] {
                    assert!(!labels.contains(&absent), "{marked}: {labels:?}");
                }
            }
        }
    }

    /// A binding whose value has a type answers a hover that names that type.
    #[test]
    fn typed_binding_answers_its_type() {
        let mut site = QuerySession::with_program(FAILED_PROGRAM);
        let hover = site
            .hover_text("#panic(\"page\")\n#let value = str(123)\n#val|ue")
            .expect("typed binding hover");
        assert!(hover.contains("let value = str;"), "{hover}");
    }

    /// A method call on a typed field names the parameter the cursor fills, never the receiver.
    #[test]
    fn method_call_marks_its_parameter() {
        let mut site = QuerySession::with_program(FAILED_PROGRAM);
        let signature = site
            .signature(
                "#panic(\"page\")\n#let f(x) = (current: x.len())\n#f(\"test\").current.bit-lshift(|)",
            )
            .expect("typed member signature");
        assert!(
            signature.signatures[0]
                .label
                .starts_with("f(\"test\").current.bit-lshift("),
            "{signature:?}"
        );
        assert!(
            !signature.signatures[0].label.contains("self"),
            "{signature:?}"
        );
        let parameters = signature.signatures[0]
            .parameters
            .as_ref()
            .expect("method parameters");
        assert_eq!(parameters.len(), 1, "{signature:?}");
        assert_eq!(
            parameters[0].label,
            lsp_types::ParameterLabel::Simple("shift:".into()),
            "{signature:?}"
        );
        assert_eq!(
            signature.signatures[0].active_parameter,
            Some(0),
            "{signature:?}"
        );
    }

    #[test]
    fn cancelled_mutation_scans_preserve_errors() {
        use super::*;
        use std::cell::Cell;
        use tola_build::cancellation::{BuildCancelled, BuildCanceller};

        let source =
            Source::detached("#let value = (old: 1)\n#let unrelated = 1\n#value = (current: 1)");
        let names = SourceNames::new(source);
        let canceller = BuildCanceller::new();
        let cancellation = canceller.token();
        let visits = Cell::new(0);
        let error = binding_is_written(
            &names,
            0,
            names.source().text().len(),
            |_| false,
            || {
                visits.set(visits.get() + 1);
                if visits.get() == 3 {
                    canceller.cancel();
                }
                cancellation.ensure_active()?;
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error.is::<BuildCancelled>());
    }

    #[test]
    fn unproven_patterns_keep_values_unknown() {
        use super::*;
        use crate::query::source_member_completion;

        for marked in [
            "#let (value,) = unknown\n#value.|",
            "#let (value, missing) = ((current: 1),)\n#value.|",
            "#let (value, missing) = (value: (current: 1))\n#value.|",
        ] {
            let cursor = marked.find('|').unwrap();
            let source = Source::detached(marked.replace('|', ""));
            let names = SourceNames::new(source.clone());
            assert!(
                source_member_completion(None, &names, &source, cursor, &BuildCancellation::new())
                    .unwrap()
                    .is_none(),
                "{marked}"
            );
            let items = QuerySession::with_program(FAILED_PROGRAM)
                .completion(&format!("#panic(\"page\")\n{marked}"));
            let labels = completion_labels(&items);
            assert!(!labels.contains(&"current"), "{marked}: {labels:?}");
        }
    }

    #[test]
    fn rest_parameters_hold_arguments() {
        let mut site = QuerySession::with_program(FAILED_PROGRAM);
        let completions = site.completion(
            "#panic(\"page\")\n#let capture(used: 0, ..args) = args\n#capture(1, used: 2, extra: 3).|"
        );
        let labels = completion_labels(&completions);
        for expected in ["pos", "named", "extra", "map", "filter"] {
            assert!(labels.contains(&expected), "{labels:?}");
        }
        for absent in ["join", "used"] {
            assert!(!labels.contains(&absent), "{labels:?}");
        }
    }
}
