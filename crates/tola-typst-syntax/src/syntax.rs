//! The expression, call, member, or import item a at sits in.
//!
//! Every selection answers with byte ranges into the exact immutable source whose tree supplied
//! it, so a caller that holds that source can quote what it selected.

use std::ops::Range;

use typst_syntax::ast::AstNode;
use typst_syntax::{LinkedNode, Side, Source, SyntaxKind, ast};

/// The import statement the at is inside.
pub struct Import {
    /// The module path string, as the author wrote it.
    pub source: Range<usize>,
    /// The range completion replaces when the at writes a new item, if it does.
    pub replacement: Option<Range<usize>>,
    /// The item the at sits in when it completes one that is already written.
    pub repair: Option<Range<usize>>,
    /// The module path's own parts, in order.
    pub path: Vec<String>,
    /// Every item the statement already imports, as its full path.
    pub imported: Vec<Vec<String>>,
}

/// Select import-item paths, not the local names introduced by `as`.
pub fn import(source: &Source, at: usize) -> Option<Import> {
    let root = LinkedNode::new(source.root());
    let mut node = root.leaf_at(at, Side::Before)?;
    while !node.is::<ast::ModuleImport>() {
        if node.kind().is_trivia()
            && let Some(previous) = node.prev_sibling()
            && previous.is::<ast::ModuleImport>()
            && source.text()[previous.range().end..at]
                .chars()
                .all(|ch| matches!(ch, ' ' | '\t'))
        {
            node = previous;
        } else {
            node = node.parent()?.clone();
        }
    }
    let import = node.cast::<ast::ModuleImport>()?;
    let colon = node
        .children()
        .find(|child| child.kind() == SyntaxKind::Colon)?;
    if at < colon.range().end
        || node
            .children()
            .any(|child| child.kind() == SyntaxKind::RightParen && at > child.range().start)
    {
        return None;
    }
    let ast::Imports::Items(items) = import.imports()? else {
        return None;
    };
    let items_node = node.find(items.span())?;
    let mut selection = Import {
        source: node.find(import.source().span())?.range(),
        replacement: None,
        repair: None,
        path: Vec::new(),
        imported: Vec::new(),
    };
    let mut active = false;
    for item in items.iter() {
        let path = items_node.find(item.path().span())?;
        let whole = match path.parent() {
            Some(parent) if parent.kind() == SyntaxKind::RenamedImportItem => parent.clone(),
            _ => path.clone(),
        };
        if whole.range().start <= at && at <= whole.range().end {
            active = true;
            if at <= path.range().end {
                for component in path.children() {
                    if component.kind() == SyntaxKind::Ident {
                        if component.range().start <= at && at <= component.range().end {
                            selection.replacement = Some(component.range());
                            break;
                        }
                        if component.range().end < at {
                            selection.path.push(component.leaf_text().to_string());
                        }
                    } else if component.kind() == SyntaxKind::Dot && component.range().end == at {
                        selection.replacement = Some(at..at);
                    }
                }
            }
            if selection.replacement.is_some() {
                let mut repair = whole.range();
                if let Some(comma) = whole
                    .next_sibling()
                    .filter(|next| next.kind() == SyntaxKind::Comma)
                {
                    repair.end = comma.range().end;
                } else if let Some(comma) = whole
                    .prev_sibling()
                    .filter(|prev| prev.kind() == SyntaxKind::Comma)
                {
                    repair.start = comma.range().start;
                }
                selection.repair = Some(repair);
            }
        } else {
            selection.imported.push(
                item.path()
                    .iter()
                    .map(|part| part.as_str().into())
                    .collect(),
            );
        }
    }
    if !active {
        let previous = items_node
            .children()
            .rfind(|child| !child.kind().is_trivia() && child.range().end <= at);
        if previous.is_none_or(|previous| previous.kind() == SyntaxKind::Comma) {
            selection.replacement = Some(at..at);
        }
    }
    Some(selection)
}

/// The member access the at is inside.
pub struct Member {
    /// The expression the dot reads from.
    pub target: Range<usize>,
    /// The dot and everything the access reaches through.
    pub suffix: Range<usize>,
    /// The text the at is writing, which completion replaces.
    pub replacement: Range<usize>,
}

/// One import item's own path, with the module it is imported from.
///
/// An item's own name is not a binding in this file, so the path is what identifies the export
/// whether the item renames it or not.
pub struct ImportedName {
    /// The range of the import's path string, which names the module.
    pub source: Range<usize>,
    /// The item's path, in order.
    pub path: Vec<String>,
}

/// Select the import item whose path the at is inside.
pub fn imported_name(source: &Source, at: usize) -> Option<ImportedName> {
    let node = cursor_leaves(source, at)
        .into_iter()
        .flatten()
        .find_map(|node| {
            let mut node = node;
            while node.cast::<ast::ModuleImport>().is_none() {
                node = node.parent()?.clone();
            }
            Some(node)
        })?;
    let import = node.cast::<ast::ModuleImport>()?;
    let ast::Imports::Items(items) = import.imports()? else {
        return None;
    };
    let list = node.find(items.span())?;
    let source = node.find(import.source().span())?.range();
    for item in items.iter() {
        let path = list.find(item.path().span())?;
        let whole = match path.parent() {
            Some(parent) if parent.kind() == SyntaxKind::RenamedImportItem => parent,
            _ => &path,
        };
        if whole.range().start <= at && at <= whole.range().end {
            // `as` and the new name are the author's own words, not the export's.
            if path.range().start > at || at > path.range().end {
                return None;
            }
            return Some(ImportedName {
                source,
                path: path_parts(&path),
            });
        }
    }
    None
}

/// The identifier parts of one import path, in order.
fn path_parts(path: &LinkedNode<'_>) -> Vec<String> {
    path.children()
        .filter(|component| component.kind() == SyntaxKind::Ident)
        .map(|component| component.leaf_text().to_string())
        .collect()
}

/// The completion edit and the replaced expression are separate ranges.
pub fn member(source: &Source, at: usize) -> Option<Member> {
    // A member is completed after its dot or inside its name, so the token ending at the cursor
    // is the one that decides it.
    let cursor = LinkedNode::new(source.root()).leaf_at(at, Side::Before)?;
    let (dot, replacement) = if matches!(cursor.kind(), SyntaxKind::Ident | SyntaxKind::MathIdent) {
        (cursor.prev_sibling()?, cursor.range())
    } else {
        (cursor, at..at)
    };
    let textual_dot =
        matches!(dot.kind(), SyntaxKind::Text | SyntaxKind::MathText) && dot.leaf_text() == ".";
    if dot.kind() != SyntaxKind::Dot && !textual_dot {
        return None;
    }
    let target = dot.prev_sibling()?;
    if !target.is::<ast::Expr>() || (textual_dot && target.range().end != dot.range().start) {
        return None;
    }
    if target.parent_kind() == Some(SyntaxKind::Markup)
        && target.prev_sibling_kind() != Some(SyntaxKind::Hash)
    {
        return None;
    }
    let mut suffix_end = replacement.end.max(dot.range().end);
    if let Some(access) = dot.parent()
        && let Some(parent) = access.parent()
        && let Some(call) = parent.cast::<ast::FuncCall>()
        && call.callee().span() == access.span()
    {
        suffix_end = parent.range().end;
    }
    Some(Member {
        target: target.range(),
        suffix: dot.range().start..suffix_end,
        replacement,
    })
}

/// The unfinished name or label at the at, which completion drops from its query copy.
pub fn name(source: &Source, at: usize) -> Option<Range<usize>> {
    let node = LinkedNode::new(source.root()).leaf_at(at, Side::Before)?;
    match node.kind() {
        SyntaxKind::Ident => Some(node.range()),
        // The author's own hash counts as unfinished: `#` alone is not yet a name.
        SyntaxKind::Hash => Some(node.range()),
        // An unfinished label is a syntax error inside the expression that carries it, so the
        // query copy drops that expression.
        SyntaxKind::Error => Some(dropped(&expression(source, at)?)),
        _ => None,
    }
}

/// The `include` statement at the at, whose path the site may not have yet.
pub fn include_path(source: &Source, at: usize) -> Option<Range<usize>> {
    let mut node = LinkedNode::new(source.root()).leaf_at(at, Side::Before)?;
    while node.kind() != SyntaxKind::ModuleInclude {
        node = node.parent()?.clone();
    }
    Some(dropped(&node))
}

/// What a path argument resolves against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathBase {
    /// The directory of the file that writes it.
    File,
    /// The site root.
    Site,
}

/// The base a call's path arguments resolve from, or `None` when the call takes no path.
///
/// A reader names a file the site already holds, from the file that writes it. An asset names the
/// output the site creates, from the site root. Every other call's strings name nothing.
pub fn path_base(name: &str) -> Option<PathBase> {
    match name {
        "read" | "json" | "yaml" | "toml" | "csv" | "xml" | "cbor" | "image" | "bibliography" => {
            Some(PathBase::File)
        }
        "asset" => Some(PathBase::Site),
        _ => None,
    }
}

/// A path argument the offset sits inside, with the base its call site resolves it from.
pub struct PathArgument {
    /// The string's own bytes, without the quotation marks.
    pub range: Range<usize>,
    /// What the call site resolves it from.
    pub base: PathBase,
}

/// The path argument the offset sits inside: a call's string, or an include's path.
pub fn path_argument(source: &Source, at: usize) -> Option<PathArgument> {
    let mut node = LinkedNode::new(source.root()).leaf_at(at, Side::Before)?;
    loop {
        match node.kind() {
            // An include statement names a file from the file that writes it.
            SyntaxKind::ModuleInclude => {
                let path = node
                    .children()
                    .find(|child| child.kind() == SyntaxKind::Str)?;
                return Some(PathArgument {
                    range: inner(path.range())?,
                    base: PathBase::File,
                });
            }
            SyntaxKind::FuncCall => {
                let call = node.cast::<ast::FuncCall>()?;
                let base = path_base(callee_name(call.callee())?.as_str())?;
                let text = call.args().items().find_map(argument_string)?;
                let range = inner(node.find(text.span())?.range())?;
                return (range.start <= at && at <= range.end)
                    .then_some(PathArgument { range, base });
            }
            _ => node = node.parent()?.clone(),
        }
    }
}

/// The bytes a string literal holds, without the quotation marks that enclose them.
fn inner(range: Range<usize>) -> Option<Range<usize>> {
    (range.end >= range.start + 2).then(|| range.start + 1..range.end - 1)
}

/// The string one argument writes, when it writes one: a positional path, or `path:`/`style:`.
fn argument_string(item: ast::Arg<'_>) -> Option<ast::Str<'_>> {
    match item {
        ast::Arg::Pos(ast::Expr::Str(text)) => Some(text),
        ast::Arg::Named(named) if matches!(named.name().get().as_str(), "path" | "style") => {
            match named.expr() {
                ast::Expr::Str(text) => Some(text),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The name a callee writes, whether bare or as `module.<name>`.
fn callee_name(callee: ast::Expr<'_>) -> Option<String> {
    match callee {
        ast::Expr::Ident(ident) => Some(ident.get().to_string()),
        ast::Expr::FieldAccess(access) => match access.target() {
            ast::Expr::Ident(_) => Some(access.field().get().to_string()),
            _ => None,
        },
        _ => None,
    }
}

/// The range of one statement, with the hash that opens it: an empty statement cannot compile.
pub fn dropped(node: &LinkedNode<'_>) -> Range<usize> {
    let range = node.range();
    match node.prev_sibling_kind() {
        Some(SyntaxKind::Hash) => node
            .prev_sibling()
            .map_or(range.clone(), |hash| hash.range().start..range.end),
        _ => range,
    }
}

/// The leaves a cursor addresses, the token it opens first.
///
/// An editor's cursor sits between characters, so the one the author points at is the character
/// after it: a byte inside a name names that name, and a byte at its first character sits in the
/// space before it. A cursor at a name's end points past the name, so the token that ends there is
/// the second answer rather than none.
pub fn cursor_leaves(source: &Source, at: usize) -> [Option<LinkedNode<'_>>; 2] {
    let root = LinkedNode::new(source.root());
    [
        root.leaf_at(at, Side::After),
        root.leaf_at(at, Side::Before),
    ]
}

/// The leaf the at follows, when the author has not written an expression yet.
pub fn before(source: &Source, at: usize) -> Option<LinkedNode<'_>> {
    LinkedNode::new(source.root()).leaf_at(at, Side::Before)
}

/// The expression the at is inside, climbing from the leaf it sits in.
pub fn expression(source: &Source, at: usize) -> Option<LinkedNode<'_>> {
    // A cursor that opens a token addresses one whose own expression is the widest one enclosing
    // it, so the expression that covers least is the one the cursor means.
    let mut node = cursor_leaves(source, at)
        .into_iter()
        .flatten()
        .filter_map(|node| {
            let mut node = node;
            while !node.is::<ast::Expr>() {
                node = node.parent()?.clone();
            }
            Some(node)
        })
        .min_by_key(|node| node.range().len())?;
    if let Some(parent) = node.parent()
        && matches!(
            parent.kind(),
            SyntaxKind::FieldAccess | SyntaxKind::MathFieldAccess
        )
        && node.index() > 0
    {
        node = parent.clone();
    }
    Some(node)
}

/// Whether Tola supplies a package. Tola answers its own semantics only; the standard library,
/// configured packages, and site files belong to the native Typst language server.
pub fn root_expression(node: LinkedNode<'_>) -> LinkedNode<'_> {
    let mut current = node;
    loop {
        let next = if let Some(access) = current.cast::<ast::FieldAccess>() {
            current.find(access.target().span())
        } else if let Some(call) = current.cast::<ast::FuncCall>() {
            current.find(call.callee().span())
        } else if let Some(grouped) = current.cast::<ast::Parenthesized>() {
            current.find(grouped.expr().span())
        } else if current.kind() == SyntaxKind::Contextual {
            current.children().next_back()
        } else {
            None
        };
        let Some(next) = next else {
            return current;
        };
        current = next;
    }
}

/// The innermost node that covers exactly this byte range.
pub fn node_at_range<'a>(source: &'a Source, range: &Range<usize>) -> Option<LinkedNode<'a>> {
    let root = LinkedNode::new(source.root());
    let mut node = root.leaf_at(range.end, Side::Before)?;
    loop {
        if node.range() == *range && node.is::<ast::Expr>() {
            return Some(node);
        }
        node = node.parent()?.clone();
    }
}

/// The call the at is inside.
pub struct Call {
    /// The expression being called.
    pub callee: Range<usize>,
    /// The callee and everything the call reaches through.
    pub suffix: Range<usize>,
    /// The index of the argument the at sits in.
    pub active_argument: usize,
    /// What the call calls.
    pub target: CallTarget,
}

/// What a call calls.
pub enum CallTarget {
    /// A name or an expression that is not a member read.
    Function,
    /// A member of a receiver value, as in `list.len()`.
    Field {
        /// The expression the field is read from.
        receiver: Range<usize>,
        /// The field's own name.
        field: Range<usize>,
    },
}

/// The call the at is inside, with the argument it sits in.
pub fn call(source: &Source, at: usize) -> Option<Call> {
    let root = LinkedNode::new(source.root());
    let mut node = root.leaf_at(at, Side::Before)?;
    loop {
        if let Some(call) = node.cast::<ast::FuncCall>()
            && let Some(args) = node.find(call.args().span())
            && args.range().start <= at
        {
            let callee = node.find(call.callee().span())?;
            let target = match callee.cast::<ast::FieldAccess>() {
                Some(access) => CallTarget::Field {
                    receiver: callee.find(access.target().span())?.range(),
                    field: callee.find(access.field().span())?.range(),
                },
                None => CallTarget::Function,
            };
            let suffix_start = match &target {
                CallTarget::Function => callee.range().end,
                CallTarget::Field { receiver, .. } => receiver.end,
            };
            return Some(Call {
                suffix: suffix_start..node.range().end,
                callee: callee.range(),
                target,
                active_argument: args
                    .children()
                    .filter(|node| node.kind() == SyntaxKind::Comma && node.range().end <= at)
                    .count(),
            });
        }
        node = node.parent()?.clone();
    }
}

/// Whether this node is the module path of an import statement.
pub fn is_import_path(node: &LinkedNode<'_>) -> bool {
    node.kind() == SyntaxKind::Str
        && matches!(
            node.parent_kind(),
            Some(SyntaxKind::ModuleImport | SyntaxKind::ModuleInclude)
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path argument at the offset the text marks, as its text and the base it uses.
    fn argument(marked: &str) -> Option<(String, PathBase)> {
        let at = marked.find('|').expect("an offset marker");
        let text = marked.replace('|', "");
        let source = Source::detached(text.clone());
        path_argument(&source, at).map(|argument| (text[argument.range].to_owned(), argument.base))
    }

    #[test]
    fn call_site_declares_its_path_base() {
        assert_eq!(
            argument("#asset(\"down|loads/file.pdf\", read(\"file.pdf\"))"),
            Some(("downloads/file.pdf".to_owned(), PathBase::Site))
        );
        assert_eq!(
            argument("#read(\"data/si|te.json\")"),
            Some(("data/site.json".to_owned(), PathBase::File))
        );
        assert_eq!(
            argument("#include \"content/po|st.typ\""),
            Some(("content/post.typ".to_owned(), PathBase::File))
        );
        assert_eq!(
            argument("#asset(\"downloads/file.pdf\", read(\"fi|le.pdf\"))"),
            Some(("file.pdf".to_owned(), PathBase::File)),
            "inside the reader call, the reader's own argument is the path"
        );
        assert_eq!(
            argument("#image(\"brand.svg\")|"),
            None,
            "a call with no path argument names none"
        );
    }
}
