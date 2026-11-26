//! Source selections and literal module dependencies.
//!
//! Every selection answers with byte ranges into the exact immutable source whose tree supplied it.

use std::ops::Range;
use std::path::Path;

use typst_syntax::ast::AstNode;
use typst_syntax::{LinkedNode, Side, Source, SyntaxKind, SyntaxNode, ast};

/// One literal path a source imports or includes.
pub struct ModulePath {
    /// Only `ModuleImport` and `ModuleInclude` produce module paths.
    pub kind: SyntaxKind,
    /// Decoded path, as the compiler resolves it.
    pub text: String,
    /// The string literal's bytes, including its quotes.
    pub range: Range<usize>,
}

/// Literal module dependencies, in source order; expressions require evaluation instead.
pub fn module_paths(src: &Source) -> Vec<ModulePath> {
    let mut paths = Vec::new();
    let mut stack = vec![LinkedNode::new(src.root())];
    while let Some(node) = stack.pop() {
        let lit = match node.kind() {
            SyntaxKind::ModuleInclude => node
                .get()
                .children()
                .find(|child| child.kind() == SyntaxKind::Str)
                .and_then(|child| child.cast::<ast::Str>()),
            SyntaxKind::ModuleImport => {
                node.get()
                    .cast::<ast::ModuleImport>()
                    .and_then(|import| match import.source() {
                        ast::Expr::Str(text) => Some(text),
                        _ => None,
                    })
            }
            _ => {
                stack.extend(node.children());
                continue;
            }
        };
        if let Some(text) = lit
            && let Some(written) = node.find(text.span())
        {
            let range = written.range();
            // Unclosed string literal has no closing quote for `ast::Str::get`.
            if range.end >= range.start + 2 {
                paths.push(ModulePath {
                    kind: node.kind(),
                    text: text.get().to_string(),
                    range,
                });
            }
        }
    }
    paths.sort_by_key(|path| path.range.start);
    paths
}

/// The import statement the cursor is inside.
pub struct Import {
    /// The module path string, as the author wrote it.
    pub source: Range<usize>,
    /// The range completion replaces when writing a new item, if any.
    pub replacement: Option<Range<usize>>,
    /// The item the cursor sits in when completing an existing item.
    pub repair: Option<Range<usize>>,
    /// The module path's own parts, in order.
    pub path: Vec<String>,
    /// Every item the statement already imports, as its full path.
    pub imported: Vec<Vec<String>>,
}

fn item_whole<'a>(path: &LinkedNode<'a>) -> LinkedNode<'a> {
    match path.parent() {
        Some(parent) if parent.kind() == SyntaxKind::RenamedImportItem => parent.clone(),
        _ => path.clone(),
    }
}

/// Select import-item paths, not the local names introduced by `as`.
pub fn import(src: &Source, at: usize) -> Option<Import> {
    let root = LinkedNode::new(src.root());
    let mut node = root.leaf_at(at, Side::Before)?;
    while !node.is::<ast::ModuleImport>() {
        if node.kind().is_trivia()
            && let Some(prev) = node.prev_sibling()
            && prev.is::<ast::ModuleImport>()
            && src.text()[prev.range().end..at]
                .chars()
                .all(|ch| matches!(ch, ' ' | '\t'))
        {
            node = prev;
        } else {
            node = node.parent()?.clone();
        }
    }
    let import = node.cast::<ast::ModuleImport>()?;
    let colon = node
        .children()
        .find(|child| child.kind() == SyntaxKind::Colon)?;
    if at < colon.range().start
        || node
            .children()
            .any(|child| child.kind() == SyntaxKind::RightParen && at > child.range().start)
    {
        return None;
    }
    let items = match import.imports() {
        Some(ast::Imports::Items(items)) => Some(items),
        Some(ast::Imports::Wildcard) => return None,
        None => None,
    };
    let items_node = items.and_then(|items| node.find(items.span()));
    let mut sel = Import {
        source: node.find(import.source().span())?.range(),
        replacement: None,
        repair: None,
        path: Vec::new(),
        imported: Vec::new(),
    };
    let mut active = false;
    if let (Some(items), Some(items_node)) = (items, items_node.as_ref()) {
        for item in items.iter() {
            let path = items_node.find(item.path().span())?;
            let whole = item_whole(&path);
            if whole.range().start <= at && at <= whole.range().end {
                active = true;
                select_import_item(&path, &whole, at, &mut sel);
            } else {
                sel.imported.push(
                    item.path()
                        .iter()
                        .map(|part| part.as_str().into())
                        .collect(),
                );
            }
        }
    }
    if !active && at_import_gap(src, &colon, items_node.as_ref(), at) {
        sel.replacement = Some(at..at);
    }
    Some(sel)
}

fn select_import_item(path: &LinkedNode<'_>, whole: &LinkedNode<'_>, at: usize, sel: &mut Import) {
    if at <= path.range().end {
        for part in path.children() {
            if part.kind() == SyntaxKind::Ident {
                if part.range().start <= at && at <= part.range().end {
                    sel.replacement = Some(part.range());
                    break;
                }
                if part.range().end < at {
                    sel.path.push(part.leaf_text().to_string());
                }
            } else if part.kind() == SyntaxKind::Dot && part.range().end == at {
                sel.replacement = Some(at..at);
            }
        }
    }
    if sel.replacement.is_some() {
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
        sel.repair = Some(repair);
    }
}

fn at_import_gap(
    src: &Source,
    colon: &LinkedNode<'_>,
    items_node: Option<&LinkedNode<'_>>,
    at: usize,
) -> bool {
    let prev = items_node.and_then(|node| {
        node.children()
            .rfind(|child| !child.kind().is_trivia() && child.range().end <= at)
    });
    let before_items = items_node.is_none_or(|node| {
        node.children()
            .find(|child| !child.kind().is_trivia())
            .is_none_or(|first| at <= first.range().start)
    });
    let blank = prev.is_some_and(|prev| {
        src.text()[prev.range().end..at]
            .chars()
            .all(|ch| matches!(ch, ' ' | '\t'))
    });
    at <= colon.range().end || before_items || blank
}

fn enclosing<'a>(
    leaf: LinkedNode<'a>,
    wanted: impl Fn(&LinkedNode<'a>) -> bool,
) -> Option<LinkedNode<'a>> {
    std::iter::successors(Some(leaf), |node| node.parent().cloned()).find(|node| wanted(node))
}

/// The member access the cursor is inside.
pub struct Member {
    /// The expression the dot reads from.
    pub target: Range<usize>,
    /// The dot and everything the access reaches through.
    pub suffix: Range<usize>,
    /// The text the cursor is writing, which completion replaces.
    pub replacement: Range<usize>,
}

/// One import item's own path, with the module it is imported from.
///
/// An item's own name is not a binding in this file, so the path is what identifies the export
/// whether the item renames it or not.
pub struct ImportedName {
    /// The range of the import's path string, which names the module.
    pub source: Range<usize>,
    /// The selected identifier in the item's path.
    pub name: Range<usize>,
    /// The item's path through the selected identifier, in order.
    pub path: Vec<String>,
}

/// Select the import item whose path the cursor is inside.
pub fn imported_name(src: &Source, at: usize) -> Option<ImportedName> {
    let node = cursor_leaves(src, at)
        .into_iter()
        .flatten()
        .find_map(|leaf| enclosing(leaf, |node| node.cast::<ast::ModuleImport>().is_some()))?;
    let import = node.cast::<ast::ModuleImport>()?;
    let ast::Imports::Items(items) = import.imports()? else {
        return None;
    };
    let list = node.find(items.span())?;
    let src_range = node.find(import.source().span())?.range();
    for item in items.iter() {
        let path = list.find(item.path().span())?;
        let whole = item_whole(&path);
        if whole.range().start <= at && at <= whole.range().end {
            if path.range().start > at || at > path.range().end {
                return None;
            }
            let mut parts = Vec::new();
            for part in path
                .children()
                .filter(|node| node.kind() == SyntaxKind::Ident)
            {
                parts.push(part.leaf_text().to_string());
                let range = part.range();
                if range.start <= at && at <= range.end {
                    return Some(ImportedName {
                        source: src_range,
                        name: range,
                        path: parts,
                    });
                }
            }
            return None;
        }
    }
    None
}

/// The completion edit and the replaced expression are separate ranges.
pub fn member(src: &Source, at: usize) -> Option<Member> {
    let cursor = LinkedNode::new(src.root()).leaf_at(at, Side::Before)?;
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

/// The unfinished name or label at the cursor, which completion drops from its query copy.
pub fn name(src: &Source, at: usize) -> Option<Range<usize>> {
    let node = LinkedNode::new(src.root()).leaf_at(at, Side::Before)?;
    match node.kind() {
        SyntaxKind::Ident | SyntaxKind::Hash => Some(node.range()),
        SyntaxKind::Error => Some(dropped(&expression(src, at)?)),
        _ => None,
    }
}

/// The `include` statement at the cursor, whose path the site may not have yet.
pub fn include_path(src: &Source, at: usize) -> Option<Range<usize>> {
    let leaf = LinkedNode::new(src.root()).leaf_at(at, Side::Before)?;
    let node = enclosing(leaf, |node| node.kind() == SyntaxKind::ModuleInclude)?;
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
pub fn path_base(name: &str) -> Option<PathBase> {
    match name {
        "read" | "json" | "yaml" | "toml" | "csv" | "xml" | "cbor" | "image" | "bibliography" => {
            Some(PathBase::File)
        }
        "asset" => Some(PathBase::Site),
        _ => None,
    }
}

/// Decode string content with Typst's escape rules, including unfinished prefixes.
pub fn string_prefix(raw: &str) -> String {
    let lit = SyntaxNode::leaf(SyntaxKind::Str, format!("\"{raw}\""));
    lit.cast::<ast::Str>()
        .expect("a string leaf casts to a string literal")
        .get()
        .to_string()
}

/// A path argument the offset sits inside, with the base its call site resolves it from.
pub struct PathArgument {
    /// Content bytes, ending at the token's end when its closing quote is absent.
    pub range: Range<usize>,
    /// What the call site resolves it from.
    pub base: PathBase,
    /// The owning call's callee; an include has no callee.
    pub callee: Option<Range<usize>>,
    /// The file format the parameter accepts, when it narrows suggestions.
    pub extension: Option<&'static str>,
}

/// The path argument the offset sits inside: a call's string, or an include's path.
pub fn path_argument(src: &Source, at: usize) -> Option<PathArgument> {
    let lit = LinkedNode::new(src.root()).leaf_at(at, Side::Before)?;
    let closed = lit.kind() == SyntaxKind::Str;
    if !closed && !(lit.kind() == SyntaxKind::Error && lit.leaf_text().starts_with('"')) {
        return None;
    }
    let range = lit.range().start + 1..lit.range().end - usize::from(closed);
    if at < range.start || at > range.end {
        return None;
    }
    let mut arg = lit;
    let mut in_array = false;
    loop {
        let parent = arg.parent()?;
        match parent.kind() {
            SyntaxKind::ModuleInclude => {
                return Some(PathArgument {
                    range,
                    base: PathBase::File,
                    callee: None,
                    extension: None,
                });
            }
            SyntaxKind::Args => {
                let call_node = parent.parent()?;
                let target = call_node
                    .cast::<ast::FuncCall>()
                    .map(|call| call.callee())
                    .or_else(|| call_node.cast::<ast::SetRule>().map(|rule| rule.target()))?;
                let name = callee_name(target)?;
                let base = if name == "raw" {
                    PathBase::File
                } else {
                    path_base(&name)?
                };
                let extension = path_arg_extension(src, &range, parent, &arg, &name, in_array)?;
                return Some(PathArgument {
                    range,
                    base,
                    callee: Some(call_node.find(target.span())?.range()),
                    extension,
                });
            }
            SyntaxKind::Array | SyntaxKind::Parenthesized | SyntaxKind::Named => {
                in_array |= parent.kind() == SyntaxKind::Array;
                arg = parent.clone();
            }
            _ => return None,
        }
    }
}

fn path_arg_extension(
    src: &Source,
    range: &Range<usize>,
    args: &LinkedNode<'_>,
    arg: &LinkedNode<'_>,
    callee: &str,
    in_array: bool,
) -> Option<Option<&'static str>> {
    if let Some(named) = arg.cast::<ast::Named>() {
        return match (callee, named.name().as_str()) {
            ("raw", "theme") if !in_array => Some(Some("tmTheme")),
            ("raw", "syntaxes") => Some(Some("sublime-syntax")),
            ("bibliography", "style") => {
                let val = string_prefix(&src.text()[range.clone()]);
                if !val.contains('/') && Path::new(val.as_str()).extension().is_none() {
                    return None;
                }
                Some(Some("csl"))
            }
            ("bibliography" | "raw", _) => None,
            (_, "path") => Some(None),
            _ => None,
        };
    }
    if callee == "raw" || (arg.kind() == SyntaxKind::Array && callee != "bibliography") {
        return None;
    }
    let has_prior_pos = args.children().take(arg.index()).any(|prev| {
        matches!(
            prev.cast::<ast::Arg>(),
            Some(ast::Arg::Pos(_) | ast::Arg::Spread(_))
        )
    });
    (!has_prior_pos).then_some(None)
}

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
pub fn cursor_leaves(src: &Source, at: usize) -> [Option<LinkedNode<'_>>; 2] {
    let root = LinkedNode::new(src.root());
    [
        root.leaf_at(at, Side::After),
        root.leaf_at(at, Side::Before),
    ]
}

/// The leaf the cursor follows, when the author has not written an expression yet.
pub fn before(src: &Source, at: usize) -> Option<LinkedNode<'_>> {
    LinkedNode::new(src.root()).leaf_at(at, Side::Before)
}

/// The expression the cursor is inside, climbing from the leaf it sits in.
pub fn expression(src: &Source, at: usize) -> Option<LinkedNode<'_>> {
    let mut node = cursor_leaves(src, at)
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

/// Walks down field accesses, calls, parenthesized groups, and `context` to the root expression.
pub fn root_expression(node: LinkedNode<'_>) -> LinkedNode<'_> {
    let mut cur = node;
    loop {
        let next = if let Some(access) = cur.cast::<ast::FieldAccess>() {
            cur.find(access.target().span())
        } else if let Some(call) = cur.cast::<ast::FuncCall>() {
            cur.find(call.callee().span())
        } else if let Some(grouped) = cur.cast::<ast::Parenthesized>() {
            cur.find(grouped.expr().span())
        } else if cur.kind() == SyntaxKind::Contextual {
            cur.children().next_back()
        } else {
            None
        };
        let Some(next) = next else {
            return cur;
        };
        cur = next;
    }
}

/// The innermost expression node that covers exactly this byte range.
pub fn node_at_range<'a>(src: &'a Source, range: &Range<usize>) -> Option<LinkedNode<'a>> {
    let root = LinkedNode::new(src.root());
    let mut node = root.leaf_at(range.end, Side::Before)?;
    loop {
        if node.range() == *range && node.is::<ast::Expr>() {
            return Some(node);
        }
        node = node.parent()?.clone();
    }
}

/// The call the cursor is inside.
pub struct Call {
    /// The expression being called.
    pub callee: Range<usize>,
    /// The callee and everything the call reaches through.
    pub suffix: Range<usize>,
    /// The index of the argument the cursor sits in.
    pub active_argument: usize,
    /// The name the argument the cursor sits in writes, when the argument is a named one.
    pub argument_name: Option<Range<usize>>,
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

/// The call the cursor is inside, with the argument it sits in.
pub fn call(src: &Source, at: usize) -> Option<Call> {
    let leaf = LinkedNode::new(src.root()).leaf_at(at, Side::Before)?;
    let node = enclosing(leaf, |node| {
        node.cast::<ast::FuncCall>()
            .and_then(|call| node.find(call.args().span()))
            .is_some_and(|args| args.range().start <= at)
    })?;
    let call = node.cast::<ast::FuncCall>()?;
    let args = node.find(call.args().span())?;
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
    let active_argument = args
        .children()
        .filter(|node| node.kind() == SyntaxKind::Comma && node.range().end <= at)
        .count();
    let argument_name = args
        .cast::<ast::Args>()
        .and_then(|arguments| arguments.items().nth(active_argument))
        .and_then(|arg| match arg {
            ast::Arg::Named(named) => Some(named.name().span()),
            ast::Arg::Pos(_) | ast::Arg::Spread(_) => None,
        })
        .and_then(|span| args.find(span))
        .map(|name| name.range());
    Some(Call {
        suffix: suffix_start..node.range().end,
        callee: callee.range(),
        target,
        active_argument,
        argument_name,
    })
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

    fn argument(marked: &str) -> Option<(String, PathBase)> {
        let at = marked.find('|').expect("an offset marker");
        let text = marked.replace('|', "");
        let src = Source::detached(text.clone());
        path_argument(&src, at).map(|arg| (text[arg.range].to_owned(), arg.base))
    }

    #[test]
    fn path_argument_roles() {
        for (marked, expected) in [
            ("#read(\"da|ta.csv\")", Some("data.csv")),
            ("#read(path: \"da|ta.csv\")", Some("data.csv")),
            ("#read(path: \"da|", Some("da")),
            ("#set raw(theme: \"my|.tmTheme\")", Some("my.tmTheme")),
            ("#raw(\"text\", theme: \"my|.tmTheme\")", Some("my.tmTheme")),
            (
                "#set raw(syntaxes: (\"a.sublime-syntax\", \"b|.sublime-syntax\"))",
                Some("b.sublime-syntax"),
            ),
            ("#bibliography((\"a.bib\", \"b|.bib\"))", Some("b.bib")),
            ("#bibliography((\"a.bib\", \"b|", Some("b")),
            (
                "#bibliography(\"a.bib\", style: \"custom.c|sl\")",
                Some("custom.csl"),
            ),
            (
                "#bibliography(\"a.bib\", style: \"styles/|\")",
                Some("styles/"),
            ),
            ("#read(\"data.csv\", encoding: \"utf|8\")", None),
            ("#read(encoding: \"utf|", None),
            ("#image(alt: \"al|t\")", None),
            ("#image(alt: \"al|", None),
            ("#image(style: \"custom.c|sl\")", None),
            ("#bibliography(\"a.bib\", style: \"ap|a\")", None),
            ("#bibliography(\"a.bib\", \"b|.bib\")", None),
            ("#read((\"a.csv\", \"b|.csv\"))", None),
            ("#raw(\"my|.tmTheme\")", None),
        ] {
            assert_eq!(
                argument(marked),
                expected.map(|path| (path.to_owned(), PathBase::File)),
                "{marked}"
            );
        }
    }

    #[test]
    fn path_argument_base() {
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
        );
        assert_eq!(argument("#image(\"brand.svg\")|"), None);
    }

    #[test]
    fn call_active_named_arg() {
        let marked = "#strong(delta: |300)";
        let at = marked.find('|').expect("an offset marker");
        let text = marked.replace('|', "");
        let src = Source::detached(text.clone());
        let named = call(&src, at).expect("the call");
        assert_eq!(
            named.argument_name.map(|name| text[name].to_owned()),
            Some("delta".to_owned())
        );

        let marked = "#strong(|300)";
        let at = marked.find('|').expect("an offset marker");
        let src = Source::detached(marked.replace('|', ""));
        assert_eq!(call(&src, at).expect("the call").argument_name, None);
    }
}
