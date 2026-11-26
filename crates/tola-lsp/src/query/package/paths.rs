use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use lsp_types::{CompletionItem, CompletionItemKind, CompletionTextEdit, TextEdit};
use tola_build::cancellation::BuildCancellation;
use tola_build::config::SiteConfig;
use tola_typst::typst::foundations::Repr;
use tola_typst::typst::syntax::{LinkedNode, Side, Source, SyntaxKind, VirtualRoot};
use tola_typst_syntax::names::SourceNames;

use crate::packages::PackageAccess;
use crate::position;

pub(super) struct ImportPath<'a> {
    pub(super) node: LinkedNode<'a>,
    pub(super) path: std::ops::Range<usize>,
    pub(super) unterminated: bool,
}

struct WrittenPath {
    range: std::ops::Range<usize>,
    unterminated: bool,
    extension: Option<&'static str>,
}

pub(super) fn import_path(src: &Source, pos: usize) -> Option<ImportPath<'_>> {
    let node = LinkedNode::new(src.root()).leaf_at(pos, Side::Before)?;
    let unclosed = node.kind() == SyntaxKind::Error
        && node.leaf_text().starts_with('"')
        && matches!(
            node.parent_kind(),
            Some(SyntaxKind::ModuleImport | SyntaxKind::ModuleInclude)
        );
    if !tola_typst_syntax::syntax::is_import_path(&node) && !unclosed {
        return None;
    }
    let start = node.range().start + 1;
    let line_end = string_line_end(src.text(), start, node.range().end);
    let unterminated = unclosed || line_end < node.range().end;
    let end = if unterminated {
        line_end
    } else {
        node.range().end - 1
    };
    (pos >= start && pos <= end).then_some(ImportPath {
        path: start..end,
        unterminated,
        node,
    })
}

/// Unfinished path strings stop at the line break rather than spanning lines.
fn string_line_end(text: &str, start: usize, token_end: usize) -> usize {
    text[start..token_end]
        .find(['\r', '\n'])
        .map_or(token_end, |offset| start + offset)
}

pub(super) fn with_closing_quote(mut content: String, unterminated: bool) -> String {
    if unterminated {
        content.push('"');
    }
    content
}

pub(super) fn import_completions(
    cfg: &SiteConfig,
    pkgs: &PackageAccess<'_>,
    src: &Source,
    pos: usize,
    import: &ImportPath<'_>,
    cancel: &BuildCancellation,
    overrides: &[(PathBuf, Arc<str>)],
) -> Result<Option<Vec<CompletionItem>>> {
    let prefix = &src.text()[import.path.start..pos];
    if prefix.starts_with('@') && prefix.contains(':') {
        let ver_start = import.path.start + prefix.rfind(':').expect("a version separator") + 1;
        return Ok(Some(super::version_completions(
            pkgs,
            src,
            prefix,
            ver_start,
            import.path.end,
            import.unterminated,
        )));
    }
    if prefix.starts_with('@') {
        return Ok(Some(super::spec_completions(
            pkgs,
            src,
            import.path.clone(),
            prefix,
            import.unterminated,
        )?));
    }
    let written = WrittenPath {
        range: import.path.clone(),
        unterminated: import.unterminated,
        extension: Some("typ"),
    };
    site_path_completion(cfg, src, pos, &written, cancel, overrides, &pkgs.boundary)
}

pub(super) fn reader_path_completions(
    cfg: &SiteConfig,
    src: &Source,
    pos: usize,
    names: Option<&SourceNames>,
    cancel: &BuildCancellation,
    overrides: &[(PathBuf, Arc<str>)],
    boundary: &tola_typst::SourceBoundary,
) -> Result<Option<Vec<CompletionItem>>> {
    let Some(written) = reader_path(src, pos, names) else {
        return Ok(None);
    };
    site_path_completion(cfg, src, pos, &written, cancel, overrides, boundary)
}

fn reader_path(src: &Source, pos: usize, indexed: Option<&SourceNames>) -> Option<WrittenPath> {
    let text = src.text();
    let arg = tola_typst_syntax::syntax::path_argument(src, pos)?;
    if arg.base != tola_typst_syntax::syntax::PathBase::File {
        return None;
    }
    if let Some(callee) = &arg.callee {
        let local;
        let names = match indexed {
            Some(names) => names,
            None => {
                local = SourceNames::new(src.clone());
                &local
            }
        };
        if names.declared_at(callee.start).is_some() {
            return None;
        }
    }
    let line_end = string_line_end(text, arg.range.start, arg.range.end);
    (pos <= line_end).then_some(WrittenPath {
        range: arg.range.start..line_end,
        unterminated: text.as_bytes().get(line_end) != Some(&b'"'),
        extension: arg.extension,
    })
}

fn site_path_completion(
    cfg: &SiteConfig,
    src: &Source,
    pos: usize,
    written: &WrittenPath,
    cancel: &BuildCancellation,
    overrides: &[(PathBuf, Arc<str>)],
    boundary: &tola_typst::SourceBoundary,
) -> Result<Option<Vec<CompletionItem>>> {
    if src.id().root() != &VirtualRoot::Project {
        return Ok(None);
    }
    let raw_prefix = &src.text()[written.range.start..pos];
    let prefix = tola_typst_syntax::syntax::string_prefix(raw_prefix);
    let rooted = prefix.starts_with('/');
    let Some(dir) = src.id().vpath().parent() else {
        return Ok(Some(Vec::new()));
    };
    let mut dirs = std::collections::BTreeSet::new();
    let mut files = Vec::new();
    for id in crate::files::completable_file_ids(cfg, overrides, boundary, cancel)? {
        cancel.ensure_active()?;
        if id == src.id()
            || written
                .extension
                .is_some_and(|ext| id.vpath().extension() != Some(ext))
        {
            continue;
        }
        let label = if rooted {
            id.vpath().get_with_slash().to_owned()
        } else {
            id.vpath().relative_from(&dir).to_string()
        };
        for (idx, _) in label.match_indices('/') {
            let sub = &label[..=idx];
            if sub != "/" && sub != prefix.as_str() {
                dirs.insert(sub.to_owned());
            }
        }
        files.push(label);
    }
    files.sort();
    let mut replacement =
        super::super::completion::completion_edit(src, pos, written.range.clone(), String::new())?;
    if written.unterminated
        && let CompletionTextEdit::InsertAndReplace(edit) = &replacement
    {
        replacement = CompletionTextEdit::Edit(TextEdit {
            range: edit.replace,
            new_text: String::new(),
        });
    }
    let mut items = Vec::new();
    let labels = dirs
        .into_iter()
        .map(|label| (label, CompletionItemKind::FOLDER))
        .chain(
            files
                .into_iter()
                .map(|label| (label, CompletionItemKind::FILE)),
        );
    for (label, kind) in labels {
        let quoted = label.as_str().repr();
        let content = &quoted[1..quoted.len() - 1];
        let new_text = if written.unterminated {
            quoted[1..].to_owned()
        } else {
            content.to_owned()
        };
        let filter = match label.strip_prefix(prefix.as_str()) {
            Some(suffix) => {
                let suffix = suffix.repr();
                format!("{raw_prefix}{}", &suffix[1..suffix.len() - 1])
            }
            None => content.to_owned(),
        };
        let filter_text = (filter != label).then_some(filter);
        let mut edit = replacement.clone();
        crate::completion::set_edit_text(&mut edit, new_text);
        items.push(CompletionItem {
            label,
            kind: Some(kind),
            filter_text,
            text_edit: Some(edit),
            ..CompletionItem::default()
        });
    }
    crate::completion::preserve_order(&mut items);
    Ok(Some(items))
}

pub(super) fn import_position(src: &Source) -> lsp_types::Range {
    let lines = src.lines();
    let mut at = 0;
    let mut hashed = false;
    for child in LinkedNode::new(src.root()).children() {
        match child.kind() {
            SyntaxKind::Hash => hashed = true,
            SyntaxKind::ModuleImport if hashed => {
                let next = position::utf16_range(lines, child.range().end..child.range().end)
                    .and_then(|end| {
                        position::byte_offset(
                            lines,
                            lsp_types::Position::new(end.start.line + 1, 0),
                        )
                        .ok()
                    });
                at = next.unwrap_or(child.range().end);
                hashed = false;
            }
            SyntaxKind::Space | SyntaxKind::Parbreak => {}
            _ => break,
        }
    }
    position::utf16_range(lines, at..at).unwrap_or_default()
}
