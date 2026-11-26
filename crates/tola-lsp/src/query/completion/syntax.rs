use std::ops::Range;

use anyhow::{Context, Result};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit, InsertReplaceEdit,
    InsertTextFormat, TextEdit,
};
use tola_typst::typst::syntax::ast;
use tola_typst::typst::syntax::{LinkedNode, Side, Source, SyntaxKind};

use crate::position;
use crate::protocol::SourceReply;

pub(super) fn is_path_position(src: &Source, pos: usize) -> bool {
    if tola_typst_syntax::syntax::path_argument(src, pos).is_some() {
        return true;
    }
    tola_typst_syntax::syntax::cursor_leaves(src, pos)
        .into_iter()
        .flatten()
        .any(|leaf| {
            let mut node = leaf;
            loop {
                if matches!(
                    node.kind(),
                    SyntaxKind::ModuleImport | SyntaxKind::ModuleInclude
                ) {
                    return true;
                }
                match node.parent() {
                    Some(parent) => node = parent.clone(),
                    None => return false,
                }
            }
        })
}

pub(in crate::query) fn opaque_at(src: &Source, pos: usize) -> bool {
    let mut cur = LinkedNode::new(src.root()).leaf_at(pos, Side::Before);
    while let Some(node) = cur {
        if matches!(
            node.kind(),
            SyntaxKind::LineComment | SyntaxKind::BlockComment | SyntaxKind::Str | SyntaxKind::Raw
        ) {
            return true;
        }
        // Interpolation inside a string or raw block is code, not opaque text.
        if !matches!(
            node.kind(),
            SyntaxKind::Text | SyntaxKind::Str | SyntaxKind::Raw
        ) && node.cast::<ast::Expr>().is_some()
        {
            return false;
        }
        cur = node.parent().cloned();
    }
    false
}

pub(super) fn raw_at(src: &Source, pos: usize) -> bool {
    let mut cur = LinkedNode::new(src.root()).leaf_at(pos, Side::Before);
    while let Some(node) = cur {
        match node.kind() {
            SyntaxKind::Raw => return true,
            SyntaxKind::Str | SyntaxKind::LineComment | SyntaxKind::BlockComment => return false,
            _ => {}
        }
        cur = node.parent().cloned();
    }
    false
}

pub(in crate::query) fn comment_at(src: &Source, pos: usize) -> bool {
    LinkedNode::new(src.root())
        .leaf_at(pos, Side::Before)
        .is_some_and(|leaf| {
            matches!(
                leaf.kind(),
                SyntaxKind::LineComment | SyntaxKind::BlockComment
            )
        })
}

pub(in crate::query) fn documentation_template(src: &Source, pos: usize) -> Option<SourceReply> {
    let leaf = LinkedNode::new(src.root()).leaf_at(pos, Side::Before)?;
    if leaf.kind() != SyntaxKind::LineComment || leaf.range().end != pos {
        return None;
    }
    let space = match leaf.leaf_text().as_str() {
        "///" => " ",
        "/// " => "",
        _ => return None,
    };
    let hash = next_node(&leaf)?;
    if hash.kind() != SyntaxKind::Hash {
        return None;
    }
    if src.text()[leaf.range().end..hash.range().start]
        .chars()
        .filter(|ch| *ch == '\n')
        .count()
        > 1
    {
        return None;
    }
    let binding = next_node(&hash)?;
    let closure = match binding.cast::<ast::LetBinding>()?.init() {
        Some(ast::Expr::Closure(closure)) => closure,
        _ => return None,
    };
    let mut snippet = format!("{space}$0\n///");
    let mut next = 1;
    for param in closure.params().children() {
        let name = match param {
            ast::Param::Pos(ast::Pattern::Normal(ast::Expr::Ident(ident))) => {
                ident.get().to_string()
            }
            ast::Param::Named(named) => named.name().get().to_string(),
            ast::Param::Spread(spread) => match spread.sink_ident() {
                Some(ident) => ident.get().to_string(),
                None => continue,
            },
            _ => continue,
        };
        snippet.push_str(&format!("\n/// - {name} (${next}): ${}", next + 1));
        next += 2;
    }
    snippet.push_str(&format!("\n/// -> ${next}"));
    let range = position::utf16_range(src.lines(), pos..pos)?;
    Some(SourceReply::Completion(CompletionResponse::Array(vec![
        CompletionItem {
            label: "Document function".to_owned(),
            kind: Some(CompletionItemKind::CONSTANT),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: snippet,
            })),
            ..CompletionItem::default()
        },
    ])))
}

fn next_node<'a>(node: &LinkedNode<'a>) -> Option<LinkedNode<'a>> {
    let mut cur = node.clone();
    loop {
        if let Some(sibling) = cur.next_sibling() {
            return Some(sibling);
        }
        cur = cur.parent()?.clone();
    }
}

pub(super) fn unparenthesized(mut expr: Option<ast::Expr>) -> Option<ast::Expr> {
    while let Some(ast::Expr::Parenthesized(group)) = expr {
        expr = Some(group.expr());
    }
    expr
}

pub(super) fn typed_text<'a>(src: &'a Source, replacement: &Range<usize>, pos: usize) -> &'a str {
    &src.text()[replacement.start..pos]
}

pub(in crate::query) fn completion_edit(
    src: &Source,
    pos: usize,
    replacement: Range<usize>,
    new_text: String,
) -> Result<CompletionTextEdit> {
    let replace =
        position::utf16_range(src.lines(), replacement.clone()).context("invalid range")?;
    let insert = position::utf16_range(src.lines(), replacement.start..pos.min(replacement.end))
        .context("invalid range")?;
    Ok(if insert == replace {
        CompletionTextEdit::Edit(TextEdit {
            range: replace,
            new_text,
        })
    } else {
        CompletionTextEdit::InsertAndReplace(InsertReplaceEdit {
            insert,
            replace,
            new_text,
        })
    })
}
