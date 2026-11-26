//! Source expression inference shared by checked observations and immutable source bindings.

mod pattern;
mod source;
#[cfg(test)]
mod tests;

pub(in crate::query) use source::{
    ExpressionBinding, INITIALIZER_DEPTH, SourceResolver, binding_is_written, expression_name,
    is_name_reference,
};

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use tola_typst::typst::Library;
use tola_typst::typst::foundations::{Arg, Args, Array, Dict, Scope, Str, Styles, Type, Value};
use tola_typst::typst::syntax::ast::AstNode;
use tola_typst::typst::syntax::{
    FileId, LinkedNode, Source, Span, Spanned, SyntaxKind, SyntaxMode, ast,
};

use super::expression::{ExpressionType, MemberCall, members, native_returns, single_value};
use pattern::{binary_type, bind_pattern, has_control_flow, unary_type};
use source::enclosing_let_binding;

const EXPRESSION_DEPTH: usize = 64;

#[derive(Default)]
pub(in crate::query) struct Inference {
    inferring: HashSet<Span>,
    pub(super) parameters: Vec<HashMap<Span, ExpressionType>>,
    depth: usize,
}

impl Inference {
    pub(in crate::query) fn has_call_frame(&self) -> bool {
        !self.parameters.is_empty()
    }
}

pub(in crate::query) trait ExpressionResolver: Sized {
    fn source(&mut self, file: FileId) -> Option<Source>;
    fn binding(&mut self, node: &LinkedNode<'_>) -> Result<Option<ExpressionBinding>>;
    fn observed(&mut self, span: Span) -> Result<Vec<(Value, Option<Styles>)>>;
    fn library(&self) -> &Library;
    fn inference(&mut self) -> &mut Inference;
    fn ensure_active(&self) -> Result<()>;

    fn owning_source(&mut self, span: Span) -> Option<Source> {
        self.source(span.id()?)
    }

    fn types(&mut self, node: &LinkedNode<'_>) -> Result<Vec<(ExpressionType, Option<Styles>)>> {
        self.ensure_active()?;
        if self.inference().depth >= EXPRESSION_DEPTH {
            return Ok(observed_types(self.observed(node.span())?));
        }
        self.inference().depth += 1;
        let vals = self.expression_types(node);
        self.inference().depth -= 1;
        self.ensure_active()?;
        vals.map(|vals| {
            if vals
                .iter()
                .any(|(ty, _)| matches!(ty, ExpressionType::Unknown))
            {
                Vec::new()
            } else {
                vals
            }
        })
    }

    fn known_values(&mut self, node: &LinkedNode<'_>) -> Result<Vec<(Value, Option<Styles>)>> {
        let mut vals = Vec::new();
        for (ty, styles) in self.types(node)? {
            self.ensure_active()?;
            let Some(val) = ty.into_value() else {
                return Ok(Vec::new());
            };
            vals.push((val, styles));
        }
        Ok(vals)
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
        let vals = self.observed(node.span())?;
        if !vals.is_empty() {
            return Ok(observed_types(vals));
        }
        if let Some(binding) = self.binding(node)? {
            if let Some(val) = binding.value {
                return Ok(vec![(ExpressionType::Known(val), None)]);
            }
            if binding.parameter
                && let Some(val) = self.parameter_type(binding.span)
            {
                return Ok(vec![(val, None)]);
            }
            let traced = self.observed(binding.span)?;
            if !traced.is_empty() {
                return Ok(observed_types(traced));
            }
            if let Some(init) = binding.initializer {
                let init_types = match self.constant_span(init, 0)? {
                    Some(val) => vec![ExpressionType::Known(val)],
                    None => self.inferred_span(init)?,
                };
                return Ok(self
                    .project_binding_types(binding.span, init_types)?
                    .into_iter()
                    .map(|val| (val, None))
                    .collect());
            }
            // A local declaration shadows any library name even without an evaluated value.
            return Ok(Vec::new());
        }
        if is_name_reference(node) {
            if let Some(val) = self.library_value(node) {
                return Ok(vec![(ExpressionType::Known(val), None)]);
            }
            if let Some(val) = self.constant(node, 0)? {
                return Ok(vec![(ExpressionType::Known(val), None)]);
            }
            let inferred = self.inferred(node)?;
            if !inferred.is_empty() {
                return Ok(inferred.into_iter().map(|val| (val, None)).collect());
            }
        }
        Ok(Vec::new())
    }

    fn library_scope(&self, mode: SyntaxMode) -> (&Library, &Scope) {
        let lib = self.library();
        let scope = match mode {
            SyntaxMode::Math => lib.math.scope(),
            _ => lib.global.scope(),
        };
        (lib, scope)
    }

    fn parameter_type(&mut self, decl: Span) -> Option<ExpressionType> {
        self.inference()
            .parameters
            .iter()
            .rev()
            .find_map(|scope| scope.get(&decl).cloned())
    }

    fn declaration_initializer(&mut self, span: Span) -> Result<Option<Span>> {
        let Some(src) = self.owning_source(span) else {
            return Ok(None);
        };
        let Some(node) = src.find(span) else {
            return Ok(None);
        };
        if matches!(node.cast::<ast::Expr>(), Some(ast::Expr::Closure(_))) {
            return Ok(Some(span));
        }
        Ok(enclosing_let_binding(node, &|| self.ensure_active())?
            .and_then(|binding| binding.init().map(|init| init.span())))
    }

    fn library_value(&self, node: &LinkedNode<'_>) -> Option<Value> {
        let (name, mode) = match node.cast::<ast::Expr>()? {
            ast::Expr::Ident(ident) => (ident.get(), SyntaxMode::Code),
            ast::Expr::MathIdent(ident) => (ident.get(), SyntaxMode::Math),
            _ => return None,
        };
        let (lib, scope) = self.library_scope(mode);
        scope
            .get(name.as_str())
            .map(|binding| binding.read().clone())
            .or_else(|| (name.as_str() == "std").then(|| lib.std.read().clone()))
    }

    fn inferred_span(&mut self, span: Span) -> Result<Vec<ExpressionType>> {
        let Some(src) = self.owning_source(span) else {
            return Ok(Vec::new());
        };
        let Some(node) = src.find(span) else {
            return Ok(Vec::new());
        };
        self.inferred(&node)
    }

    fn project_binding_types(
        &mut self,
        span: Span,
        vals: Vec<ExpressionType>,
    ) -> Result<Vec<ExpressionType>> {
        let Some(src) = self.owning_source(span) else {
            return Ok(Vec::new());
        };
        let Some(node) = src.find(span) else {
            return Ok(vals);
        };
        let Some(binding) = enclosing_let_binding(node, &|| self.ensure_active())? else {
            return Ok(vals);
        };
        let ast::LetBindingKind::Normal(pattern) = binding.kind() else {
            return Ok(vals);
        };
        if !pattern.bindings().iter().any(|ident| ident.span() == span) {
            return Ok(vals);
        }
        let mut projected = Vec::new();
        for val in vals {
            self.ensure_active()?;
            let mut bindings = HashMap::new();
            if !bind_pattern(pattern, val, &mut bindings, &|| self.ensure_active())? {
                return Ok(Vec::new());
            }
            let Some(val) = bindings.remove(&span) else {
                return Ok(Vec::new());
            };
            projected.push(val);
        }
        Ok(projected)
    }

    fn constant_span(&mut self, span: Span, depth: usize) -> Result<Option<Value>> {
        let Some(src) = self.owning_source(span) else {
            return Ok(None);
        };
        let Some(node) = src.find(span) else {
            return Ok(None);
        };
        self.constant(&node, depth)
    }

    fn dict_item_entry<'ast>(
        &mut self,
        node: &LinkedNode<'_>,
        item: ast::DictItem<'ast>,
        depth: usize,
    ) -> Result<Option<(Str, ast::Expr<'ast>)>> {
        match item {
            ast::DictItem::Named(pair) => Ok(Some((pair.name().get().clone().into(), pair.expr()))),
            ast::DictItem::Keyed(pair) => {
                let Some(key_node) = node.find(pair.key().span()) else {
                    return Ok(None);
                };
                let Some(Value::Str(key)) = self.constant(&key_node, depth)? else {
                    return Ok(None);
                };
                Ok(Some((key, pair.expr())))
            }
            ast::DictItem::Spread(_) => Ok(None),
        }
    }

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
            ast::Expr::Bool(v) => Value::Bool(v.get()),
            ast::Expr::Int(v) => Value::Int(v.get()),
            ast::Expr::Float(v) => Value::Float(v.get()),
            ast::Expr::Numeric(v) => Value::numeric(v.get()),
            ast::Expr::Str(v) => Value::Str(v.get().into()),
            ast::Expr::Parenthesized(v) => {
                let Some(inner) = node.find(v.expr().span()) else {
                    return Ok(None);
                };
                return self.constant(&inner, depth + 1);
            }
            ast::Expr::FieldAccess(access) => {
                let Some(target) = node.find(access.target().span()) else {
                    return Ok(None);
                };
                let Some(val) = self.constant(&target, depth + 1)? else {
                    return Ok(None);
                };
                let Some(field) = val.field(access.field().as_str(), ()).ok() else {
                    return Ok(None);
                };
                field
            }
            ast::Expr::Array(array) => {
                let mut vals = Array::new();
                for item in array.items() {
                    self.ensure_active()?;
                    let ast::ArrayItem::Pos(item) = item else {
                        return Ok(None);
                    };
                    let Some(item) = node.find(item.span()) else {
                        return Ok(None);
                    };
                    let Some(val) = self.constant(&item, depth + 1)? else {
                        return Ok(None);
                    };
                    vals.push(val);
                }
                Value::Array(vals)
            }
            ast::Expr::Ident(_) | ast::Expr::MathIdent(_) => {
                if !is_name_reference(node) {
                    return Ok(None);
                }
                let Some(binding) = self.binding(node)? else {
                    return Ok(self.library_value(node));
                };
                if let Some(val) = binding.value {
                    return Ok(Some(val));
                }
                if binding.parameter
                    && let Some(val) = self
                        .parameter_type(binding.span)
                        .and_then(ExpressionType::into_value)
                {
                    return Ok(Some(val));
                }
                if let Some(init) = binding.initializer {
                    let vals = self
                        .constant_span(init, depth + 1)?
                        .into_iter()
                        .map(ExpressionType::Known)
                        .collect();
                    return Ok(
                        single_value(self.project_binding_types(binding.span, vals)?)
                            .and_then(ExpressionType::into_value),
                    );
                }
                return Ok(None);
            }
            ast::Expr::Dict(dict) => {
                let mut vals = Dict::new();
                for item in dict.items() {
                    self.ensure_active()?;
                    let Some((key, expr)) = self.dict_item_entry(node, item, depth + 1)? else {
                        return Ok(None);
                    };
                    let Some(val_node) = node.find(expr.span()) else {
                        return Ok(None);
                    };
                    let Some(val) = self.constant(&val_node, depth + 1)? else {
                        return Ok(None);
                    };
                    vals.insert(key, val);
                }
                Value::Dict(vals)
            }
            _ => return Ok(None),
        }))
    }

    fn field_functions(
        &mut self,
        receiver: &LinkedNode<'_>,
        field: &str,
    ) -> Result<Vec<(Value, bool)>> {
        let mut funcs = Vec::new();
        for (receiver, styles) in self.types(receiver)? {
            self.ensure_active()?;
            let Some(member) =
                members(&receiver, styles.as_ref(), || self.ensure_active())?.remove(field)
            else {
                return Ok(Vec::new());
            };
            let (Some(call), Some(val)) = (member.call, member.ty.into_value()) else {
                return Ok(Vec::new());
            };
            funcs.push((val, matches!(call, MemberCall::Receiver)));
        }
        Ok(funcs)
    }

    fn inferred(&mut self, node: &LinkedNode<'_>) -> Result<Vec<ExpressionType>> {
        self.ensure_active()?;
        let Some(expr) = node.cast::<ast::Expr>() else {
            return Ok(Vec::new());
        };
        match expr {
            ast::Expr::Parenthesized(paren) => self.infer_child(node, paren.expr().span()),
            ast::Expr::Binary(binary) => {
                let lhs = self.operand_type(node, binary.lhs())?;
                let rhs = self.operand_type(node, binary.rhs())?;
                Ok(binary_type(binary.op(), lhs.as_ref(), rhs.as_ref())
                    .into_iter()
                    .collect())
            }
            ast::Expr::Unary(unary) => {
                let val = self.operand_type(node, unary.expr())?;
                Ok(unary_type(unary.op(), val.as_ref()).into_iter().collect())
            }
            ast::Expr::Conditional(cond) => self.infer_conditional(node, cond),
            ast::Expr::FieldAccess(access) => {
                let Some(target) = node.find(access.target().span()) else {
                    return Ok(Vec::new());
                };
                let field = access.field().as_str();
                let mut fields = Vec::new();
                for receiver in plain(self.types(&target)?) {
                    self.ensure_active()?;
                    let Some(val) = receiver.field(field, || self.ensure_active())? else {
                        return Ok(Vec::new());
                    };
                    fields.push(val);
                }
                Ok(fields)
            }
            ast::Expr::Contextual(_) => match node.children().next_back() {
                Some(body) => Ok(plain(self.types(&body)?)),
                None => Ok(Vec::new()),
            },
            ast::Expr::CodeBlock(block) => self.infer_code_block(node, block),
            ast::Expr::Dict(dict) => {
                if has_control_flow(node, &|| self.ensure_active())? {
                    return Ok(Vec::new());
                }
                let mut fields = BTreeMap::new();
                for item in dict.items() {
                    self.ensure_active()?;
                    let Some((key, expr)) = self.dict_item_entry(node, item, 0)? else {
                        return Ok(Vec::new());
                    };
                    let ty = self.child_type(node, expr.span())?;
                    fields.insert(key.to_string(), ty);
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
                    items.push(self.child_type(node, expr.span())?);
                }
                Ok(vec![ExpressionType::tuple(items, || self.ensure_active())?])
            }
            ast::Expr::FuncCall(call) => self.inferred_call(node, call),
            _ => Ok(Vec::new()),
        }
    }

    fn infer_code_block(
        &mut self,
        node: &LinkedNode<'_>,
        block: ast::CodeBlock<'_>,
    ) -> Result<Vec<ExpressionType>> {
        if has_control_flow(node, &|| self.ensure_active())? {
            return Ok(Vec::new());
        }
        let mut exprs = block.body().exprs().peekable();
        while let Some(expr) = exprs.next() {
            self.ensure_active()?;
            if exprs.peek().is_none() {
                return self.infer_child(node, expr.span());
            }
            if matches!(expr, ast::Expr::LetBinding(_) | ast::Expr::ModuleImport(_)) {
                continue;
            }
            // Only a prefix producing `none` preserves the tail expression's type.
            let prev = self.infer_child(node, expr.span())?;
            if prev.is_empty()
                || prev.iter().any(|val| {
                    val.ty() != Some(Type::of::<tola_typst::typst::foundations::NoneValue>())
                })
            {
                return Ok(Vec::new());
            }
        }
        Ok(vec![ExpressionType::Known(Value::None)])
    }

    fn infer_conditional(
        &mut self,
        node: &LinkedNode<'_>,
        cond: ast::Conditional<'_>,
    ) -> Result<Vec<ExpressionType>> {
        let mut vals = Vec::new();
        for body in [Some(cond.if_body()), cond.else_body()] {
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
            vals.extend(branch);
        }
        Ok(vals)
    }

    fn infer_child(&mut self, root: &LinkedNode<'_>, span: Span) -> Result<Vec<ExpressionType>> {
        let Some(child) = root.find(span) else {
            return Ok(Vec::new());
        };
        Ok(plain(self.types(&child)?))
    }

    fn child_type(&mut self, node: &LinkedNode<'_>, span: Span) -> Result<ExpressionType> {
        let Some(child) = node.find(span) else {
            return Ok(ExpressionType::Unknown);
        };
        Ok(single_value(plain(self.types(&child)?)).unwrap_or(ExpressionType::Unknown))
    }

    fn operand_type(
        &mut self,
        node: &LinkedNode<'_>,
        operand: ast::Expr<'_>,
    ) -> Result<Option<ExpressionType>> {
        let Some(operand) = node.find(operand.span()) else {
            return Ok(None);
        };
        Ok(single_value(plain(self.types(&operand)?)))
    }

    fn inferred_call(
        &mut self,
        node: &LinkedNode<'_>,
        call: ast::FuncCall<'_>,
    ) -> Result<Vec<ExpressionType>> {
        let Some(callee) = node.find(call.callee().span()) else {
            return Ok(Vec::new());
        };
        if let Some(binding) = self.binding(&callee)? {
            let init = if binding.parameter {
                None
            } else {
                match binding.initializer {
                    Some(init) => Some(init),
                    None => self.declaration_initializer(binding.span)?,
                }
            };
            if let Some(init) = init {
                return self.infer_once(init, node, call);
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
                .flat_map(|(func, _)| native_returns(&func))
                .collect());
        }
        let callable = single_value(
            self.known_values(&callee)?
                .into_iter()
                .map(|(val, _)| val)
                .collect(),
        );
        Ok(callable.as_ref().map(native_returns).unwrap_or_default())
    }

    fn infer_once(
        &mut self,
        init: Span,
        call_node: &LinkedNode<'_>,
        call: ast::FuncCall<'_>,
    ) -> Result<Vec<ExpressionType>> {
        if !self.inference().inferring.insert(init) {
            return Ok(Vec::new());
        }
        let inferred = self.infer_function(init, call_node, call);
        self.inference().inferring.remove(&init);
        inferred
    }

    fn infer_function(
        &mut self,
        span: Span,
        call_node: &LinkedNode<'_>,
        call: ast::FuncCall<'_>,
    ) -> Result<Vec<ExpressionType>> {
        let Some(src) = self.owning_source(span) else {
            return Ok(Vec::new());
        };
        let Some(mut node) = src.find(span) else {
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
            let Some(init) = binding.initializer else {
                return Ok(binding
                    .value
                    .as_ref()
                    .map(native_returns)
                    .unwrap_or_default());
            };
            return self.infer_once(init, call_node, call);
        };
        let scope = self.call_arguments(&node, closure, call_node, call)?;
        self.inference().parameters.push(scope);
        let inferred = self.infer_child(&node, closure.body().span());
        self.inference().parameters.pop();
        inferred
    }

    fn argument_type(
        &mut self,
        call_node: &LinkedNode<'_>,
        expr: ast::Expr<'_>,
    ) -> Result<Option<(ExpressionType, Span)>> {
        let Some(arg) = call_node.find(expr.span()) else {
            return Ok(None);
        };
        Ok(single_value(plain(self.types(&arg)?)).map(|ty| (ty, arg.span())))
    }

    /// Arguments are read before parameters are pushed; an unresolved spread leaves the call frame empty.
    fn call_arguments(
        &mut self,
        closure_node: &LinkedNode<'_>,
        closure: ast::Closure<'_>,
        call_node: &LinkedNode<'_>,
        call: ast::FuncCall<'_>,
    ) -> Result<HashMap<Span, ExpressionType>> {
        let mut args = Vec::new();
        for arg in call.args().items() {
            self.ensure_active()?;
            if matches!(arg, ast::Arg::Spread(_)) {
                return Ok(HashMap::new());
            }
            args.push(arg);
        }
        let mut positional = args.iter().filter_map(|arg| match arg {
            ast::Arg::Pos(expr) => Some(*expr),
            _ => None,
        });
        let mut named_params = HashSet::new();
        for param in closure.params().children() {
            self.ensure_active()?;
            if let ast::Param::Named(named) = param {
                named_params.insert(named.name().get().to_string());
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
                    let Some((val, _)) = self.argument_type(call_node, expr)? else {
                        continue;
                    };
                    let mut bindings = HashMap::new();
                    if bind_pattern(pattern, val, &mut bindings, &|| self.ensure_active())? {
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
                    let val = match explicit {
                        Some(expr) => self.argument_type(call_node, expr)?.map(|(ty, _)| ty),
                        None => {
                            let Some(default) = closure_node.find(named.expr().span()) else {
                                continue;
                            };
                            single_value(plain(self.types(&default)?))
                        }
                    };
                    if let Some(val) = val {
                        scope.insert(named.name().span(), val);
                    }
                }
                ast::Param::Spread(spread) => {
                    let Some(sink) = spread.sink_ident() else {
                        continue;
                    };
                    let val = self.rest_value(
                        call_node,
                        call.span(),
                        &args,
                        &named_params,
                        &mut positional,
                    )?;
                    scope.insert(sink.span(), val);
                }
            }
        }
        Ok(scope)
    }

    fn rest_value<'ast>(
        &mut self,
        call_node: &LinkedNode<'_>,
        call_span: Span,
        args: &[ast::Arg<'ast>],
        named_params: &HashSet<String>,
        positional: &mut impl Iterator<Item = ast::Expr<'ast>>,
    ) -> Result<ExpressionType> {
        let mut vals = Array::new();
        let mut known = true;
        for expr in positional {
            self.ensure_active()?;
            let Some((val, _)) = self.argument_type(call_node, expr)? else {
                known = false;
                continue;
            };
            if let Some(val) = val.into_value() {
                vals.push(val);
            } else {
                known = false;
            }
        }
        let mut captured = Args::new(call_span, vals);
        for arg in args {
            self.ensure_active()?;
            let ast::Arg::Named(named) = arg else {
                continue;
            };
            if named_params.contains(named.name().get().as_str()) {
                continue;
            }
            let Some((val, span)) = self.argument_type(call_node, named.expr())? else {
                known = false;
                continue;
            };
            if let Some(val) = val.into_value() {
                captured.items.push(Arg {
                    span,
                    name: Some(named.name().get().clone().into()),
                    value: Spanned::new(val, span),
                });
            } else {
                known = false;
            }
        }
        Ok(if known {
            ExpressionType::Known(Value::Args(captured))
        } else {
            ExpressionType::Unknown
        })
    }
}

fn plain(vals: Vec<(ExpressionType, Option<Styles>)>) -> Vec<ExpressionType> {
    vals.into_iter().map(|(val, _)| val).collect()
}

fn observed_types(vals: Vec<(Value, Option<Styles>)>) -> Vec<(ExpressionType, Option<Styles>)> {
    vals.into_iter()
        .map(|(val, styles)| (ExpressionType::Known(val), styles))
        .collect()
}
