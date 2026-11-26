use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use tola_typst::typst::foundations::{Array, Dict, Str, Type, Value};
use tola_typst::typst::syntax::ast::AstNode;
use tola_typst::typst::syntax::{LinkedNode, Span, SyntaxKind, ast};

use super::super::expression::ExpressionType;

pub(super) fn has_control_flow(
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
            if let Some(params) = node.find(closure.params().span()) {
                stack.push(params);
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

pub(super) fn bind_pattern(
    pattern: ast::Pattern<'_>,
    val: ExpressionType,
    scope: &mut HashMap<Span, ExpressionType>,
    ensure_active: &impl Fn() -> Result<()>,
) -> Result<bool> {
    ensure_active()?;
    match pattern {
        ast::Pattern::Normal(ast::Expr::Ident(ident)) => {
            scope.insert(ident.span(), val);
            Ok(true)
        }
        ast::Pattern::Placeholder(_) => Ok(true),
        ast::Pattern::Parenthesized(group) => {
            bind_pattern(group.pattern(), val, scope, ensure_active)
        }
        ast::Pattern::Destructuring(destr) => bind_destructuring(destr, val, scope, ensure_active),
        _ => Ok(false),
    }
}

fn bind_destructuring(
    destr: ast::Destructuring<'_>,
    val: ExpressionType,
    scope: &mut HashMap<Span, ExpressionType>,
    ensure_active: &impl Fn() -> Result<()>,
) -> Result<bool> {
    let mut items = Vec::new();
    for item in destr.items() {
        ensure_active()?;
        items.push(item);
    }
    match &val {
        ExpressionType::Known(Value::Dict(dict)) => bind_fields(
            items,
            &val,
            |used| {
                let mut rem = Dict::new();
                for (name, val) in dict.iter() {
                    ensure_active()?;
                    if !used.contains(name.as_str()) {
                        rem.insert(name.clone(), val.clone());
                    }
                }
                Ok(ExpressionType::Known(Value::Dict(rem)))
            },
            scope,
            ensure_active,
        ),
        ExpressionType::Dictionary(fields) => bind_fields(
            items,
            &val,
            |used| {
                let mut rem = BTreeMap::new();
                for (name, ty) in fields {
                    ensure_active()?;
                    if !used.contains(name.as_str()) {
                        rem.insert(name.clone(), ty.clone());
                    }
                }
                Ok(ExpressionType::Dictionary(rem))
            },
            scope,
            ensure_active,
        ),
        ExpressionType::Known(Value::Array(arr)) => bind_sequence(
            items,
            &arr.iter()
                .cloned()
                .map(ExpressionType::Known)
                .collect::<Vec<_>>(),
            scope,
            ensure_active,
        ),
        ExpressionType::Tuple(fields) => bind_sequence(items, fields, scope, ensure_active),
        _ => Ok(false),
    }
}

fn bind_fields(
    items: Vec<ast::DestructuringItem<'_>>,
    val: &ExpressionType,
    remainder: impl FnOnce(&HashSet<&str>) -> Result<ExpressionType>,
    scope: &mut HashMap<Span, ExpressionType>,
    ensure_active: &impl Fn() -> Result<()>,
) -> Result<bool> {
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
        let Some(field) = val.field(name.as_str(), ensure_active)? else {
            return Ok(false);
        };
        if !bind_pattern(pattern, field, scope, ensure_active)? {
            return Ok(false);
        }
        used.insert(name.as_str());
    }
    let Some(sink) = sink else {
        return Ok(true);
    };
    let ast::Expr::Ident(ident) = sink else {
        return Ok(false);
    };
    scope.insert(ident.span(), remainder(&used)?);
    Ok(true)
}

enum Slot<'a> {
    Pattern(ast::Pattern<'a>),
    Spread(Option<ast::Expr<'a>>),
}

fn bind_sequence(
    items: Vec<ast::DestructuringItem<'_>>,
    vals: &[ExpressionType],
    scope: &mut HashMap<Span, ExpressionType>,
    ensure_active: &impl Fn() -> Result<()>,
) -> Result<bool> {
    let mut slots = Vec::with_capacity(items.len());
    let mut spreads = 0;
    for item in items {
        ensure_active()?;
        match item {
            ast::DestructuringItem::Pattern(pattern) => slots.push(Slot::Pattern(pattern)),
            ast::DestructuringItem::Spread(spread) => {
                spreads += 1;
                slots.push(Slot::Spread(spread.sink_expr()));
            }
            ast::DestructuringItem::Named(_) => return Ok(false),
        }
    }
    let Some(sink_size) = vals.len().checked_sub(slots.len() - spreads) else {
        return Ok(false);
    };
    if spreads > 1 || (spreads == 0 && sink_size != 0) {
        return Ok(false);
    }
    let mut offset = 0;
    for slot in slots {
        ensure_active()?;
        match slot {
            Slot::Pattern(pattern) => {
                if !bind_pattern(pattern, vals[offset].clone(), scope, ensure_active)? {
                    return Ok(false);
                }
                offset += 1;
            }
            Slot::Spread(sink) => {
                if let Some(sink) = sink {
                    let ast::Expr::Ident(ident) = sink else {
                        return Ok(false);
                    };
                    let mut captured = Vec::new();
                    for val in &vals[offset..offset + sink_size] {
                        ensure_active()?;
                        captured.push(val.clone());
                    }
                    scope.insert(
                        ident.span(),
                        ExpressionType::tuple(captured, ensure_active)?,
                    );
                }
                offset += sink_size;
            }
        }
    }
    Ok(true)
}

pub(super) fn binary_type(
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

pub(super) fn unary_type(op: ast::UnOp, val: Option<&ExpressionType>) -> Option<ExpressionType> {
    let ty = match op {
        ast::UnOp::Not => Type::of::<bool>(),
        ast::UnOp::Pos | ast::UnOp::Neg => {
            let ty = val?.ty()?;
            if ty != Type::of::<i64>() && ty != Type::of::<f64>() {
                return None;
            }
            ty
        }
    };
    Some(ExpressionType::Instance(ty))
}
