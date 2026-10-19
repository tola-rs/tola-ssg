//! The source declaration a metadata issue points at, and the spans inside it.

use typst::diag::eco_format;
use typst::ecow::EcoString;
use typst::engine::Engine;
use typst::foundations::{Array, Value};
use typst::syntax::ast::AstNode;
use typst::syntax::{DiagSpan, LinkedNode, RootedPath, Source, ast};

pub(super) struct IssueOrigin<'a> {
    pub(super) source: &'a Source,
    pub(super) declaration: std::ops::Range<usize>,
}

impl IssueOrigin<'_> {
    /// The diagnostic span of one byte range in this source.
    pub(super) fn span(&self, range: std::ops::Range<usize>) -> DiagSpan {
        DiagSpan::from_range(self.source.id(), range)
    }

    /// The byte range of the source's `<tola-meta>` declaration.
    fn declaration_range(&self) -> std::ops::Range<usize> {
        self.declaration.clone()
    }

    /// The byte range that marks the declaration: its callee when the call is readable, and the
    /// whole declaration otherwise.
    ///
    /// Underlining a multi-line argument draws a caret block around the whole metadata dictionary,
    /// which buries the message it accompanies. The callee points at the same declaration on one
    /// line.
    pub(super) fn marker_range(&self) -> std::ops::Range<usize> {
        declaration_call(LinkedNode::new(self.source.root()), &self.declaration)
            .and_then(|call| self.source.find(call.callee().span()))
            .map_or_else(|| self.declaration_range(), |node| node.range())
    }

    /// The byte range `path` lives at, or the declaration marker when nothing literal declares it.
    pub(super) fn locate(&self, path: &[Value]) -> std::ops::Range<usize> {
        if !path.is_empty()
            && let Some(call) =
                declaration_call(LinkedNode::new(self.source.root()), &self.declaration)
            && let Some(ast::Arg::Pos(metadata)) = call.args().items().next()
            && let Some(value) = literal_field(metadata, path)
            && let Some(node) = self.source.find(value.span())
        {
            return node.range();
        }
        self.marker_range()
    }
}

fn declaration_call<'a>(
    node: LinkedNode<'a>,
    range: &std::ops::Range<usize>,
) -> Option<ast::FuncCall<'a>> {
    if node.range() == *range
        && let Some(call) = node.get().cast()
    {
        return Some(call);
    }
    node.children()
        .filter(|child| child.range().start <= range.start && child.range().end >= range.end)
        .find_map(|child| declaration_call(child, range))
}

fn literal_field<'a>(expression: ast::Expr<'a>, path: &[Value]) -> Option<ast::Expr<'a>> {
    if let ast::Expr::Parenthesized(group) = expression {
        return literal_field(group.expr(), path);
    }
    let Some((segment, remaining)) = path.split_first() else {
        return Some(expression);
    };
    match (expression, segment) {
        (ast::Expr::Dict(dictionary), Value::Str(key)) => {
            for member in dictionary.items().rev() {
                let value = match member {
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
                return literal_field(value, remaining);
            }
            None
        }
        (ast::Expr::Array(array), Value::Int(index)) => {
            for (offset, member) in array.items().enumerate() {
                let ast::ArrayItem::Pos(value) = member else {
                    return None;
                };
                if offset == *index as usize {
                    return literal_field(value, remaining);
                }
            }
            None
        }
        _ => None,
    }
}

/// Where `path` lives and declares its metadata, as the file it is addressed by and the byte range
/// of its `<tola-meta>` declaration.
///
/// A source that declares no metadata has no range, and the file's start answers for it: the
/// diagnostic still names the file and the field, and there is no declaration to point at. A path
/// no origin covers is not one of this compilation's sources, which is a programming error rather
/// than a position to guess.
pub(super) fn source_origin(
    engine: &Engine<'_>,
    path: &str,
) -> Result<(RootedPath, std::ops::Range<usize>), EcoString> {
    let Some(Value::Dict(origin)) =
        crate::library::source_origins(engine).and_then(|origins| origins.get(path).ok())
    else {
        return Err(eco_format!("`{path}` is not a source of this compilation"));
    };
    let file = origin
        .get("file")
        .map_err(|_| eco_format!("`{path}` has an origin without a file"))?
        .clone()
        .cast::<RootedPath>()
        .map_err(|_| eco_format!("`{path}` has an origin whose file is not a path"))?;
    let range = match origin.get("range") {
        Ok(Value::None) => 0..0,
        Ok(Value::Array(range)) => declaration_bounds(path, range)?,
        _ => {
            return Err(eco_format!(
                "`{path}` has an origin whose range is not an array or `none`"
            ));
        }
    };
    Ok((file, range))
}

/// The byte range of one source's `<tola-meta>` declaration, as its two integer bounds.
fn declaration_bounds(path: &str, range: &Array) -> Result<std::ops::Range<usize>, EcoString> {
    let mut bounds = range.iter();
    let bound = |value: Option<&Value>, which: &str| -> Result<i64, EcoString> {
        match value {
            Some(Value::Int(bound)) => Ok(*bound),
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
