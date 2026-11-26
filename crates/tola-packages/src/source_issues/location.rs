//! Source metadata declaration spans for diagnostic attribution.

use std::ops::Range;

use typst::diag::eco_format;
use typst::ecow::EcoString;
use typst::engine::Engine;
use typst::foundations::{Array, Dict, Value};
use typst::syntax::ast::AstNode;
use typst::syntax::{DiagSpan, LinkedNode, RootedPath, Source, ast};

pub(super) struct IssueOrigin<'a> {
    pub(super) source: &'a Source,
    pub(super) declaration: Range<usize>,
}

impl IssueOrigin<'_> {
    pub(super) fn span(&self, range: Range<usize>) -> DiagSpan {
        DiagSpan::from_range(self.source.id(), range)
    }

    pub(super) fn marker_range(&self) -> Range<usize> {
        declaration_call(LinkedNode::new(self.source.root()), &self.declaration)
            .and_then(|call| self.source.find(call.callee().span()))
            .map_or_else(|| self.declaration.clone(), |node| node.range())
    }

    pub(super) fn locate(&self, path: &[Value]) -> Range<usize> {
        if !path.is_empty()
            && let Some(call) =
                declaration_call(LinkedNode::new(self.source.root()), &self.declaration)
            && let Some(ast::Arg::Pos(meta)) = call.args().items().next()
            && let Some(val) = literal_field(meta, path)
            && let Some(node) = self.source.find(val.span())
        {
            return node.range();
        }
        self.marker_range()
    }
}

fn declaration_call<'a>(node: LinkedNode<'a>, range: &Range<usize>) -> Option<ast::FuncCall<'a>> {
    if node.range() == *range
        && let Some(call) = node.get().cast()
    {
        return Some(call);
    }
    node.children()
        .filter(|c| c.range().start <= range.start && c.range().end >= range.end)
        .find_map(|c| declaration_call(c, range))
}

fn literal_field<'a>(expr: ast::Expr<'a>, path: &[Value]) -> Option<ast::Expr<'a>> {
    if let ast::Expr::Parenthesized(group) = expr {
        return literal_field(group.expr(), path);
    }
    let Some((seg, rest)) = path.split_first() else {
        return Some(expr);
    };
    match (expr, seg) {
        (ast::Expr::Dict(dict), Value::Str(key)) => {
            for member in dict.items().rev() {
                let val = match member {
                    ast::DictItem::Named(named) if named.name().as_str() == key.as_str() => {
                        named.expr()
                    }
                    ast::DictItem::Keyed(keyed) => match keyed.key() {
                        ast::Expr::Str(name) if name.get().as_str() == key.as_str() => keyed.expr(),
                        ast::Expr::Str(_) => continue,
                        _ => return None,
                    },
                    ast::DictItem::Named(_) => continue,
                    ast::DictItem::Spread(_) => return None,
                };
                return literal_field(val, rest);
            }
            None
        }
        (ast::Expr::Array(arr), Value::Int(idx)) => {
            for (offset, member) in arr.items().enumerate() {
                let ast::ArrayItem::Pos(val) = member else {
                    return None;
                };
                if offset == *idx as usize {
                    return literal_field(val, rest);
                }
            }
            None
        }
        _ => None,
    }
}

pub(super) struct SourceOrigin {
    pub(super) owner: RootedPath,
    pub(super) file: RootedPath,
    pub(super) range: Range<usize>,
}

pub(super) fn source_origin(engine: &Engine<'_>, path: &str) -> Result<SourceOrigin, EcoString> {
    let Some(origin) =
        crate::library::source_origins(engine).and_then(|origins| crate::dict_dict(origins, path))
    else {
        return Err(eco_format!("`{path}` is not a source of this compilation"));
    };
    let owner = origin_path_field(origin, path, "source")?;
    let file = origin_path_field(origin, path, "file")?;
    let range = match origin.get("range") {
        Ok(Value::None) => 0..0,
        Ok(Value::Array(range)) => declaration_bounds(path, range)?,
        _ => {
            return Err(eco_format!(
                "`{path}` has an origin whose range is not an array or `none`"
            ));
        }
    };
    Ok(SourceOrigin { owner, file, range })
}

fn origin_path_field(origin: &Dict, path: &str, key: &str) -> Result<RootedPath, EcoString> {
    origin
        .get(key)
        .map_err(|_| eco_format!("`{path}` has an origin without a {key}"))?
        .clone()
        .cast::<RootedPath>()
        .map_err(|_| eco_format!("`{path}` has an origin whose {key} is not a path"))
}

fn declaration_bounds(path: &str, range: &Array) -> Result<Range<usize>, EcoString> {
    let mut bounds = range.iter();
    let bound = |val: Option<&Value>, which: &str| -> Result<i64, EcoString> {
        match val {
            Some(Value::Int(b)) => Ok(*b),
            _ => Err(eco_format!(
                "`{path}` declares metadata at a range without a {which}"
            )),
        }
    };
    let start = bound(bounds.next(), "start")?;
    let end = bound(bounds.next(), "end")?;
    if start < 0 || end < start {
        return Err(eco_format!(
            "`{path}` declares metadata at a range that runs backwards: {start}..{end}"
        ));
    }
    Ok(start as usize..end as usize)
}
