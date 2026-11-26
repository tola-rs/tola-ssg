//! Site labels and their references.

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::ops::Range;

use anyhow::Result;
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, DocumentHighlight,
    DocumentHighlightKind, Hover, Location, Range as ProtocolRange, TextEdit, Uri, WorkspaceEdit,
};
use tola_build::cancellation::BuildCancellation;
use tola_build::check::SourceCompilation;
use tola_build::config::SiteConfig;
use tola_diagnostic::display_path;
use tola_typst::typst::World;
use tola_typst::typst::foundations::{Content, Label, Selector};
use tola_typst::typst::syntax::{
    FileId, LinkedNode, Side, Source, Span, SyntaxKind, VirtualRoot, ast,
};
use tola_typst::typst::utils::PicoStr;

use crate::position;
use crate::protocol::markdown_hover;
use crate::routes;

use super::hover::{shown_routes, typc_fence};

#[derive(Debug)]
pub(super) struct Reference {
    pub written: Range<usize>,
    pub name: Range<usize>,
}

impl Reference {
    pub(super) fn completion_edit(
        &self,
        src: &Source,
        pos: usize,
        name: String,
    ) -> Result<CompletionTextEdit> {
        let (range, text) = if (self.name.start..=self.name.end).contains(&pos) {
            (self.name.clone(), name)
        } else {
            let prefix = &src.text()[self.written.start..self.name.start];
            let suffix = &src.text()[self.name.end..self.written.end];
            (self.written.clone(), format!("{prefix}{name}{suffix}"))
        };
        super::completion::completion_edit(src, pos, range, text)
    }
}

pub(super) fn referenced(src: &Source, pos: usize) -> Option<Reference> {
    let root = LinkedNode::new(src.root());
    [Side::Before, Side::After]
        .into_iter()
        .find_map(|side| root.leaf_at(pos, side).and_then(reference))
}

fn reference(mut node: LinkedNode<'_>) -> Option<Reference> {
    loop {
        let range = node.range();
        let name = match node.kind() {
            SyntaxKind::RefMarker => range.start + 1..range.end,
            SyntaxKind::Label => range.start + 1..range.end - 1,
            SyntaxKind::Error if node.leaf_text().starts_with('<') => range.start + 1..range.end,
            _ => {
                node = node.parent()?.clone();
                continue;
            }
        };
        return Some(Reference {
            written: range,
            name,
        });
    }
}

pub(super) fn typed_name<'a>(src: &'a Source, ref_: &Reference, pos: usize) -> &'a str {
    let typed = pos.clamp(ref_.name.start, ref_.name.end);
    &src.text()[ref_.name.start..typed]
}

pub(super) fn highlights(src: &Source, name: &str) -> Vec<DocumentHighlight> {
    let mut highlights = Vec::new();
    visit_spellings(src, name, |range, declares| {
        let kind = if declares {
            DocumentHighlightKind::WRITE
        } else {
            DocumentHighlightKind::READ
        };
        highlights.push(DocumentHighlight {
            range,
            kind: Some(kind),
        });
    });
    highlights.sort_by_key(|h| (h.kind != Some(DocumentHighlightKind::WRITE), h.range.start));
    highlights
}

fn declared(comp: &SourceCompilation) -> Vec<String> {
    let Some(introspector) = comp.introspector() else {
        return Vec::new();
    };
    let mut labels: Vec<String> = introspector
        .query_labelled()
        .into_iter()
        .filter_map(|elem| Some(elem.label()?.into_inner().resolve().as_str().to_owned()))
        .collect();
    labels.sort();
    labels.dedup();
    labels
}

pub(super) fn completion(
    comp: &SourceCompilation,
    src: &Source,
    pos: usize,
) -> Option<Vec<CompletionItem>> {
    let ref_ = referenced(src, pos)?;
    let typed = typed_name(src, &ref_, pos);
    let items = declared(comp)
        .into_iter()
        .filter(|lbl| lbl.starts_with(typed))
        .filter_map(|lbl| {
            Some(CompletionItem {
                label: lbl.clone(),
                kind: Some(CompletionItemKind::REFERENCE),
                text_edit: Some(ref_.completion_edit(src, pos, lbl).ok()?),
                ..Default::default()
            })
        })
        .collect();
    Some(items)
}

pub(super) fn declares(comp: &SourceCompilation, name: &str) -> bool {
    declared_element(comp, name).is_some()
}

fn declared_element(comp: &SourceCompilation, name: &str) -> Option<Content> {
    let lbl = Label::new(PicoStr::intern(name))?;
    comp.introspector()?.query_first(&Selector::Label(lbl))
}

fn declared_label<'a>(src: &'a Source, name: &str) -> Option<LinkedNode<'a>> {
    fn walk<'a>(node: LinkedNode<'a>, text: &str, name: &str) -> Option<LinkedNode<'a>> {
        let range = node.range();
        if node.kind() == SyntaxKind::Label && &text[range.start + 1..range.end - 1] == name {
            return Some(node);
        }
        node.children().find_map(|child| walk(child, text, name))
    }
    walk(LinkedNode::new(src.root()), src.text(), name)
}

fn labelled_element(label: &LinkedNode<'_>) -> Option<String> {
    let mut node = label.clone();
    loop {
        let call = if node.kind() == SyntaxKind::FuncCall {
            Some(node.clone())
        } else {
            call_before(&node)
        };
        if let Some(call) = call
            && let Some(name) = called_name(&call)
        {
            return Some(name);
        }
        node = node.parent()?.clone();
    }
}

fn call_before<'b>(node: &LinkedNode<'b>) -> Option<LinkedNode<'b>> {
    let mut prev = node.prev_sibling()?;
    loop {
        match prev.kind() {
            SyntaxKind::Space
            | SyntaxKind::Parbreak
            | SyntaxKind::LineComment
            | SyntaxKind::BlockComment => prev = prev.prev_sibling()?,
            SyntaxKind::FuncCall => return Some(prev),
            _ => return None,
        }
    }
}

fn called_name(call: &LinkedNode<'_>) -> Option<String> {
    let call = call.cast::<ast::FuncCall>()?;
    match call.callee() {
        ast::Expr::Ident(ident) => Some(ident.get().to_string()),
        ast::Expr::FieldAccess(access) => match access.target() {
            ast::Expr::Ident(_) => Some(access.field().get().to_string()),
            _ => None,
        },
        _ => None,
    }
}

fn compare_paths(l: &FileId, r: &FileId) -> std::cmp::Ordering {
    l.vpath()
        .get_without_slash()
        .cmp(r.vpath().get_without_slash())
}

fn spellings(
    comp: &SourceCompilation,
    name: &str,
    include_declaration: bool,
    cancel: &BuildCancellation,
) -> Result<Vec<(FileId, ProtocolRange)>> {
    let Some(declared) = declared_element(comp, name).and_then(|elem| elem.span().id()) else {
        return Ok(Vec::new());
    };
    let world = comp.world();
    let mut found = Vec::new();
    for id in compiled_with(comp, declared, cancel)? {
        let Ok(src) = world.source(id) else {
            continue;
        };
        visit_spellings(&src, name, |range, declares| {
            if include_declaration || !declares {
                found.push((id, range));
            }
        });
    }
    found.sort_by(|(l, l_range), (r, r_range)| {
        compare_paths(l, r)
            .then_with(|| l_range.start.line.cmp(&r_range.start.line))
            .then_with(|| l_range.start.character.cmp(&r_range.start.character))
    });
    Ok(found)
}

fn compiled_with(
    comp: &SourceCompilation,
    id: FileId,
    cancel: &BuildCancellation,
) -> Result<Vec<FileId>> {
    let mut srcs: Vec<FileId> = comp
        .realized_documents(cancel)?
        .into_iter()
        .filter(|doc| doc.sources.contains(&id))
        .flat_map(|doc| doc.sources)
        .filter(|id| id.vpath().extension() == Some("typ"))
        .collect();
    srcs.push(id);
    srcs.sort_by(compare_paths);
    srcs.dedup();
    Ok(srcs)
}

pub(super) fn visit_spellings(
    src: &Source,
    name: &str,
    mut visit: impl FnMut(ProtocolRange, bool),
) {
    fn walk(node: &LinkedNode<'_>, name: &str, visit: &mut impl FnMut(Range<usize>, bool)) {
        if let Some((range, declares)) = token_spelling(node, name) {
            visit(range, declares);
        }
        for child in node.children() {
            walk(&child, name, visit);
        }
    }

    let lines = src.lines();
    walk(
        &LinkedNode::new(src.root()),
        name,
        &mut |range, declares| {
            if let Some(range) = position::utf16_range(lines, range) {
                visit(range, declares);
            }
        },
    );
}

fn token_spelling(node: &LinkedNode<'_>, name: &str) -> Option<(Range<usize>, bool)> {
    let (inner, declares) = match node.kind() {
        SyntaxKind::RefMarker => (node.leaf_text().strip_prefix('@')?, false),
        SyntaxKind::Label => (
            node.leaf_text().strip_prefix('<')?.strip_suffix('>')?,
            node.parent_kind() == Some(SyntaxKind::Markup),
        ),
        _ => return None,
    };
    if inner != name {
        return None;
    }
    let start = node.range().start + 1;
    Some((start..start + inner.len(), declares))
}

pub(super) fn prepared(
    comp: &SourceCompilation,
    src: &Source,
    pos: usize,
) -> Option<(ProtocolRange, String)> {
    let ref_ = referenced(src, pos)?;
    let name = &src.text()[ref_.name.clone()];
    declared_element(comp, name)?;
    Some((
        position::utf16_range(src.lines(), ref_.name)?,
        name.to_owned(),
    ))
}

pub(super) fn definition(comp: &SourceCompilation, name: &str) -> Option<Span> {
    Some(declared_element(comp, name)?.span())
}

pub(super) fn locations(
    comp: &SourceCompilation,
    name: &str,
    include_declaration: bool,
    cancel: &BuildCancellation,
    client_root: &crate::uri::ClientRoot,
) -> Result<Vec<Location>> {
    let mut found = Vec::new();
    for (id, range) in spellings(comp, name, include_declaration, cancel)? {
        if let Ok(uri) = crate::identity::client_uri(id, comp.world().root(), client_root) {
            found.push(Location { uri, range });
        }
    }
    Ok(found)
}

pub(super) fn rename(
    comp: &SourceCompilation,
    name: &str,
    new_name: &str,
    cancel: &BuildCancellation,
    client_root: &crate::uri::ClientRoot,
) -> Result<Option<WorkspaceEdit>> {
    if !literal_name(new_name) {
        return Ok(None);
    }
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for spelled in locations(comp, name, true, cancel, client_root)? {
        changes.entry(spelled.uri).or_default().push(TextEdit {
            range: spelled.range,
            new_text: new_name.to_owned(),
        });
    }
    Ok((!changes.is_empty()).then(|| WorkspaceEdit {
        changes: Some(changes),
        ..WorkspaceEdit::default()
    }))
}

fn literal_name(name: &str) -> bool {
    [format!("<{name}>"), format!("@{name}")]
        .into_iter()
        .all(|text| {
            let src = Source::detached(text);
            referenced(&src, 1).is_some_and(|ref_| {
                ref_.written == (0..src.text().len()) && &src.text()[ref_.name] == name
            })
        })
}

pub(super) fn hover(
    comp: &SourceCompilation,
    cfg: &SiteConfig,
    src: &Source,
    pos: usize,
    cancel: &BuildCancellation,
) -> Result<Option<Hover>> {
    let Some(ref_) = referenced(src, pos) else {
        return Ok(None);
    };
    let range = position::utf16_range(src.lines(), ref_.written.clone());
    let name = &src.text()[ref_.name.clone()];
    let written = &src.text()[ref_.written.clone()];
    let Some(elem) = declared_element(comp, name) else {
        return Ok(Some(undeclared_hover(comp, src, name, written, range)));
    };
    let mut desc = format!("Labels a `{}` element", elem.func().name());
    if let Some(id) = elem.span().id() {
        desc.push_str(&format!(" declared in `{}`", source_path(comp, id)));
        let served = routes::site_routes(comp, cfg, id, cancel)?;
        let listed = shown_routes(served.iter().map(|r| r.route.as_str()));
        if !listed.is_empty() {
            desc.push_str(&format!(", served at {listed}"));
        }
    }
    desc.push('.');
    Ok(Some(markdown_hover(
        format!("{}\n\n{desc}", typc_fence(written)),
        range,
    )))
}

fn undeclared_hover(
    comp: &SourceCompilation,
    src: &Source,
    name: &str,
    written: &str,
    range: Option<ProtocolRange>,
) -> Hover {
    if comp.bundle().is_none()
        && let Some(label) = declared_label(src, name)
    {
        let desc = match labelled_element(&label) {
            Some(elem) => format!("Labels a `{elem}` element declared in this file."),
            None => format!("`{written}` is declared in this file."),
        };
        return markdown_hover(format!("{}\n\n{desc}", typc_fence(written)), range);
    }
    markdown_hover(format!("no source declares the label `{name}`"), range)
}

pub(super) fn source_path(comp: &SourceCompilation, id: FileId) -> String {
    let world = comp.world();
    match id.root() {
        VirtualRoot::Package(spec) => spec.to_string(),
        VirtualRoot::Project => display_path(
            &world.root().join(id.vpath().get_without_slash()),
            world.root(),
        ),
    }
}
