use std::collections::HashSet;

use anyhow::Result;
use tola_build::cancellation::BuildCancellation;
use tola_typst::GLOBAL_LIBRARY;
use tola_typst::typst::Library;
use tola_typst::typst::foundations::{Styles, Value};
use tola_typst::typst::syntax::ast::AstNode;
use tola_typst::typst::syntax::{FileId, LinkedNode, Source, Span, SyntaxKind, ast};
use tola_typst_syntax::imports::{BindingId, ModuleExport, NameDefinition, NameGraph};
use tola_typst_syntax::names::{DeclarationKind, Initializer, SourceNames};

use super::{ExpressionResolver, Inference};

pub(in crate::query) const INITIALIZER_DEPTH: usize = 8;

#[derive(Clone)]
pub(in crate::query) struct ExpressionBinding {
    pub span: Span,
    pub value: Option<Value>,
    pub parameter: bool,
    pub initializer: Option<Span>,
}

impl ExpressionBinding {
    pub(in crate::query) fn unknown(span: Span) -> Self {
        Self {
            span,
            value: None,
            parameter: false,
            initializer: None,
        }
    }
}

/// An assignment or unproven method call invalidates the initializer unless the caller proves
/// the method preserves it.
pub(in crate::query) fn binding_is_written(
    names: &SourceNames,
    decl: usize,
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
            let target = write_target(&node, names, decl, &preserves_method);
            if let Some(target) = target {
                let root = tola_typst_syntax::syntax::root_expression(target);
                if names.declared_at(root.range().start) == Some(decl) {
                    return Ok(true);
                }
            }
            if let Some(assign) = node.cast::<ast::DestructAssignment>()
                && assign.pattern().bindings().into_iter().any(|name| {
                    names
                        .source()
                        .find(name.span())
                        .is_some_and(|n| names.declared_at(n.range().start) == Some(decl))
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

fn write_target<'a>(
    node: &LinkedNode<'a>,
    names: &SourceNames,
    decl: usize,
    preserves_method: &impl Fn(&str) -> bool,
) -> Option<LinkedNode<'a>> {
    if let Some(bin) = node.cast::<ast::Binary>()
        && matches!(
            bin.op(),
            ast::BinOp::Assign
                | ast::BinOp::AddAssign
                | ast::BinOp::SubAssign
                | ast::BinOp::MulAssign
                | ast::BinOp::DivAssign
        )
    {
        return node.find(bin.lhs().span());
    }
    let call = node.cast::<ast::FuncCall>()?;
    let callee = node.find(call.callee().span())?;
    let access = callee.cast::<ast::FieldAccess>()?;
    if preserves_method(access.field().as_str())
        || matches!(
            names.declarations()[decl].kind,
            DeclarationKind::Import { .. }
        )
    {
        return None;
    }
    callee.find(access.target().span())
}

pub(in crate::query) struct SourceResolver<'a> {
    names: &'a SourceNames,
    graph: Option<&'a NameGraph>,
    cancel: &'a BuildCancellation,
    inference: Inference,
}

impl<'a> SourceResolver<'a> {
    pub(in crate::query) fn new(
        names: &'a SourceNames,
        graph: Option<&'a NameGraph>,
        cancel: &'a BuildCancellation,
    ) -> Self {
        Self {
            names,
            graph,
            cancel,
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

    fn file_names(&self, node: &LinkedNode<'_>) -> Option<(FileId, &SourceNames)> {
        let file = node.span().id()?;
        Some((file, self.names(file)?))
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
        let Some((file, names)) = self.file_names(node) else {
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
                        let d = &names.declarations()[id];
                        Some((d.scope, d.visible_from))
                    }
                    None => {
                        let import = &names.imports()[bound.wildcard?];
                        Some((import.scope, import.range.end))
                    }
                }
            })
            .or_else(|| {
                names.declared_at(node.range().start).map(|id| {
                    let d = &names.declarations()[id];
                    (d.scope, d.visible_from)
                })
            });
        let at = node.range().start;
        let mut scope = Some(names.scope_at(at));
        while let Some(id) = scope {
            for (stmt, import) in names.imports().iter().enumerate() {
                self.ensure_active()?;
                if import.wildcard
                    && import.scope == id
                    && import.range.end <= at
                    && candidate.is_none_or(|(s, from)| s != id || import.range.end > from)
                    && self
                        .graph
                        .is_none_or(|graph| !graph.interface_established(file, stmt))
                {
                    return Ok(true);
                }
            }
            if candidate.is_some_and(|(s, _)| s == id) {
                return Ok(false);
            }
            scope = names.scopes()[id].parent;
        }
        Ok(false)
    }

    fn declared(
        &self,
        file: FileId,
        decl: usize,
        followed: &mut HashSet<(FileId, usize)>,
        usage: (FileId, usize),
    ) -> Result<Option<ExpressionBinding>> {
        self.ensure_active()?;
        let Some(names) = self.names(file) else {
            return Ok(None);
        };
        let declared = &names.declarations()[decl];
        let Some(node) = tola_typst_syntax::syntax::node_at_range(names.source(), &declared.range)
        else {
            return Ok(None);
        };
        let mut binding = ExpressionBinding {
            span: node.span(),
            value: None,
            parameter: matches!(declared.kind, DeclarationKind::Parameter),
            initializer: None,
        };
        if !followed.insert((file, decl)) {
            return Ok(Some(binding));
        }
        let before = if usage.0 == file {
            usage.1
        } else {
            names.source().text().len()
        };
        let root_checked = usage.0 == file && names.declared_at(usage.1) == Some(decl);
        if !root_checked
            && binding_is_written(names, decl, before, |_| false, || self.ensure_active())?
        {
            binding.parameter = false;
            return Ok(Some(binding));
        }
        if let Initializer::Import { import, path } = &declared.initializer {
            if !path.is_empty()
                && let Some(def) = self
                    .graph
                    .and_then(|graph| graph.import_definition(file, *import, path))
            {
                return self.defined(def, followed, usage);
            }
            return Ok(Some(binding));
        }
        if matches!(declared.kind, DeclarationKind::Let) {
            binding.initializer = enclosing_let_binding(node, &|| self.ensure_active())?
                .and_then(|b| b.init().map(|init| init.span()));
        }
        Ok(Some(binding))
    }

    fn defined(
        &self,
        def: NameDefinition,
        followed: &mut HashSet<(FileId, usize)>,
        usage: (FileId, usize),
    ) -> Result<Option<ExpressionBinding>> {
        match def {
            NameDefinition::Source { file, declaration } => {
                self.declared(file, declaration, followed, usage)
            }
            NameDefinition::Builtin(val) => Ok(Some(ExpressionBinding {
                span: Span::detached(),
                value: Some(val),
                parameter: false,
                initializer: None,
            })),
        }
    }

    fn root_is_written(&self, node: &LinkedNode<'_>) -> Result<bool> {
        self.ensure_active()?;
        let Some((_, names)) = self.file_names(node) else {
            return Ok(false);
        };
        let root = tola_typst_syntax::syntax::root_expression(node.clone());
        let Some(decl) = names.declared_at(root.range().start) else {
            return Ok(false);
        };
        binding_is_written(
            names,
            decl,
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
        let Some(graph) = self.graph else {
            return Ok(None);
        };
        let Some(binding) = self.selected(node) else {
            return Ok(None);
        };
        if self.root_is_written(node)? || self.ambiguous_wildcard(node, Some(binding))? {
            return Ok(None);
        }
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
        let Some(file) = node.span().id() else {
            return Ok(None);
        };
        let selected = self.selected(node);
        if self.root_is_written(node)? || self.ambiguous_wildcard(node, selected)? {
            return Ok(Some(ExpressionBinding::unknown(node.span())));
        }
        if let Some(binding) = selected
            && let Some(def) = self
                .graph
                .and_then(|graph| graph.binding_definition(binding))
        {
            return self.defined(def, &mut HashSet::new(), (file, node.range().start));
        }
        if !matches!(node.kind(), SyntaxKind::Ident | SyntaxKind::MathIdent) {
            return Ok(None);
        }
        let Some(decl) = self
            .names(file)
            .and_then(|names| names.declared_at(node.range().start))
        else {
            return Ok(None);
        };
        self.declared(file, decl, &mut HashSet::new(), (file, node.range().start))
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
        self.cancel.ensure_active()?;
        Ok(())
    }
}

pub(in crate::query) fn expression_name(node: &LinkedNode<'_>) -> Option<String> {
    match node.cast::<ast::Expr>()? {
        ast::Expr::Ident(ident) => Some(ident.get().to_string()),
        ast::Expr::MathIdent(ident) => Some(ident.get().to_string()),
        _ => None,
    }
}

/// Whether `node` reads a name rather than declaring a binding, parameter, or dict key.
pub(in crate::query) fn is_name_reference(node: &LinkedNode<'_>) -> bool {
    if !matches!(
        node.cast::<ast::Expr>(),
        Some(ast::Expr::Ident(_) | ast::Expr::MathIdent(_))
    ) {
        return true;
    }
    match node.parent_kind() {
        Some(SyntaxKind::LetBinding) => node.prev_sibling_kind() == Some(SyntaxKind::Eq),
        Some(SyntaxKind::Named) => node.prev_sibling_kind() == Some(SyntaxKind::Colon),
        Some(SyntaxKind::Params | SyntaxKind::Destructuring | SyntaxKind::Closure) => false,
        Some(SyntaxKind::ForLoop) => node.prev_sibling_kind() == Some(SyntaxKind::In),
        Some(SyntaxKind::ImportItemPath | SyntaxKind::RenamedImportItem) => false,
        Some(SyntaxKind::FieldAccess | SyntaxKind::MathFieldAccess) => {
            node.prev_sibling_kind() != Some(SyntaxKind::Dot)
        }
        _ => true,
    }
}

pub(super) fn enclosing_let_binding<'a>(
    node: LinkedNode<'a>,
    ensure_active: &impl Fn() -> Result<()>,
) -> Result<Option<ast::LetBinding<'a>>> {
    let mut cur = Some(node);
    while let Some(node) = cur {
        ensure_active()?;
        if let Some(binding) = node.get().cast::<ast::LetBinding<'a>>() {
            return Ok(Some(binding));
        }
        cur = node.parent().cloned();
    }
    Ok(None)
}
