//! The links a source's text declares to other sources.
//!
//! A link is a path the compiler would read, so the same resolution answers what a path names and
//! which file an editor opens for it.

use std::path::{Path, PathBuf};

use anyhow::Result;
use lsp_types::{DocumentLink, Hover};
use tola_build::cancellation::BuildCancellation;
use tola_build::check::SourceCompilation;
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::foundations::{PathOrStr, Value};
use tola_typst::typst::loading::DataSource;
use tola_typst::typst::model::CslSource;
use tola_typst::typst::syntax::ast::AstNode;
use tola_typst::typst::syntax::{LinkedNode, Source, Span, SyntaxKind, ast};
use tola_typst_syntax::names::SourceNames;

use crate::position;
use crate::sentence::listed;

/// The file the path at `cursor` names, and the range the author wrote it at.
fn target_at(source: &Source, root: &Path, cursor: usize) -> Option<(PathBuf, lsp_types::Range)> {
    let at = position::utf16_range(source.lines(), cursor..cursor)?;
    paths(source, root, &crate::uri::ClientRoot::new(root))?
        .into_iter()
        .find(|link| link.range.start <= at.start && at.start <= link.range.end)
        .and_then(|link| {
            Some((
                crate::uri::to_site_path(link.target?.as_str()).ok()?,
                link.range,
            ))
        })
        .filter(|(path, _)| path.starts_with(root))
}

/// What the path at `cursor` names, for the site that reads it.
///
/// A resolved path is described by what the site makes of the file it names — a source the site
/// builds, with the pages its documents are served at. A path no file answers says so, because the
/// build cannot read it either.
pub(super) fn hover(
    compilation: &SourceCompilation,
    config: &ResolvedSiteConfig,
    source: &Source,
    cursor: usize,
    cancellation: &BuildCancellation,
) -> Result<Option<Hover>> {
    let root = tola_build::filesystem::normalize_existing_prefix(config.get_root());
    let Some((target, range)) = target_at(source, &root, cursor) else {
        return Ok(None);
    };
    let Some(written) = written_path(source, range) else {
        return Ok(None);
    };
    if !target.exists() {
        return Ok(Some(crate::protocol::markdown_hover(
            format!("nothing in this site answers `{written}`"),
            Some(range),
        )));
    }
    let named = tola_build::filesystem::display_path(&target, &root);
    let Some(id) = crate::identity::path_id(&target, &root) else {
        return Ok(None);
    };
    let served = crate::routes::site_routes(compilation, config, id, cancellation)?;
    let mut described = String::new();
    if served.is_empty() {
        described.push_str("A file this site includes; no document is built from it.");
    } else {
        let (listed, remaining) = listed(
            served.iter().map(|route| route.route.as_str()),
            crate::routes::NAMED_PAGES,
        );
        described.push_str(&format!("Served at {listed}"));
        if remaining > 0 {
            described.push_str(&format!(" and {remaining} more"));
        }
        described.push('.');
    }
    Ok(Some(crate::protocol::markdown_hover(
        format!("```typc\n{named}\n```\n\n{described}"),
        Some(range),
    )))
}

/// The path the author wrote inside a link's range, without the quotation marks.
fn written_path(source: &Source, range: lsp_types::Range) -> Option<String> {
    let start = position::byte_offset(source.lines(), range.start).ok()?;
    let end = position::byte_offset(source.lines(), range.end).ok()?;
    Some(source.text().get(start..end)?.to_owned())
}

/// The source root resolves file identities; the client root chooses their URI spelling.
pub(super) fn paths(
    source: &Source,
    source_root: &Path,
    client_root: &crate::uri::ClientRoot,
) -> Option<Vec<DocumentLink>> {
    let mut links = Vec::new();
    let names = SourceNames::new(source.clone());
    collect_links(
        &LinkedNode::new(source.root()),
        source,
        &names,
        source_root,
        client_root,
        &mut links,
    );
    links.sort_by_key(|link| (link.range.start.line, link.range.start.character));
    (!links.is_empty()).then_some(links)
}

fn collect_links(
    node: &LinkedNode<'_>,
    source: &Source,
    names: &SourceNames,
    source_root: &Path,
    client_root: &crate::uri::ClientRoot,
    links: &mut Vec<DocumentLink>,
) {
    let written: Vec<(Span, String)> = match node.kind() {
        SyntaxKind::ModuleImport => node
            .cast::<ast::ModuleImport>()
            .and_then(|import| string_literal(import.source()))
            .into_iter()
            .collect(),
        SyntaxKind::ModuleInclude => node
            .cast::<ast::ModuleInclude>()
            .and_then(|include| string_literal(include.source()))
            .into_iter()
            .collect(),
        SyntaxKind::FuncCall => reader_paths(node, names),
        _ => Vec::new(),
    };
    for (span, path) in written {
        push_link(node, span, &path, source, source_root, client_root, links);
    }
    for child in node.children() {
        collect_links(&child, source, names, source_root, client_root, links);
    }
}

/// The paths one reader call names: every positional source, and the named ones that name files.
fn reader_paths(node: &LinkedNode<'_>, names: &SourceNames) -> Vec<(Span, String)> {
    let Some(call) = node.cast::<ast::FuncCall>() else {
        return Vec::new();
    };
    let ast::Expr::Ident(name) = call.callee() else {
        return Vec::new();
    };
    if node
        .find(name.span())
        .is_some_and(|callee| names.declared_at(callee.range().start).is_some())
    {
        return Vec::new();
    }
    // Only a file the site already holds is a link; an output the site creates is not.
    if tola_typst_syntax::syntax::path_base(name.get().as_str())
        != Some(tola_typst_syntax::syntax::PathBase::File)
    {
        return Vec::new();
    }
    call.args()
        .items()
        .filter_map(|item| match item {
            ast::Arg::Pos(expression) => string_literal(expression),
            ast::Arg::Named(named) if named.name().get().as_str() == "path" => {
                string_literal(named.expr())
            }
            ast::Arg::Named(named)
                if name.get().as_str() == "bibliography"
                    && named.name().get().as_str() == "style" =>
            {
                let (span, text) = string_literal(named.expr())?;
                matches!(
                    Value::Str(text.as_str().into()).cast::<CslSource>().ok(),
                    Some(CslSource::Normal(DataSource::Path(_)))
                )
                .then_some((span, text))
            }
            _ => None,
        })
        .collect()
}

/// The string literal one path expression writes, with its own span.
fn string_literal(expression: ast::Expr<'_>) -> Option<(Span, String)> {
    match expression {
        ast::Expr::Str(text) => Some((text.span(), text.get().to_string())),
        _ => None,
    }
}

/// Record the link one path expression declares, when it names an addressable document.
fn push_link(
    node: &LinkedNode<'_>,
    span: Span,
    path: &str,
    source: &Source,
    source_root: &Path,
    client_root: &crate::uri::ClientRoot,
    links: &mut Vec<DocumentLink>,
) {
    // A package specification and a URL both name something a source file cannot address.
    if path.is_empty() || path.starts_with('@') || path.contains("://") {
        return;
    }
    let Ok(resolved) = PathOrStr::Str(path.into()).resolve(source.id()) else {
        return;
    };
    let Some(target) = crate::identity::client_uri(resolved.intern(), source_root, client_root)
    else {
        return;
    };
    let Some(written) = node.find(span) else {
        return;
    };
    // The link covers the path the author wrote, not the quotes around it.
    let written = written.range();
    let Some(range) = position::utf16_range(source.lines(), written.start + 1..written.end - 1)
    else {
        return;
    };
    links.push(DocumentLink {
        range,
        target: Some(target),
        tooltip: None,
        data: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tola_typst::typst::syntax::{RootedPath, VirtualPath, VirtualRoot};

    /// The site path a written link resolves to, from the file that writes it.
    fn resolved_target(root: &Path, writing: &str, text: &str) -> Option<String> {
        let id = RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new(writing).expect("a site path"),
        )
        .intern();
        let source = Source::new(id, text.to_owned());
        let link = paths(&source, root, &crate::uri::ClientRoot::new(root))?
            .into_iter()
            .next()?;
        let target = crate::uri::to_site_path(link.target.as_ref()?.as_str()).ok()?;
        let inside = target.strip_prefix(root).ok()?;
        Some(inside.to_string_lossy().replace('\\', "/"))
    }

    /// A link target resolves against the file that writes it, and stays inside the site.
    #[test]
    fn link_targets_resolve_from_the_writing_file() {
        let directory = tempfile::tempdir().unwrap();
        // The site root as the paths resolve it: a temporary directory is reachable through a
        // symlink on some systems, and every resolved path takes the target's spelling.
        let root = directory.path().canonicalize().unwrap();
        for (writing, text, expected) in [
            (
                "content/document.typ",
                "#include \"absent.typ\"\n",
                Some("content/absent.typ"),
            ),
            (
                "content/post.typ",
                "#import \"../templates/page.typ\": page\n",
                Some("templates/page.typ"),
            ),
            (
                "content/document.typ",
                "#include \"../../outside.typ\"\n",
                None,
            ),
        ] {
            assert_eq!(
                resolved_target(&root, writing, text).as_deref(),
                expected,
                "{writing} {text}"
            );
        }
    }

    /// The root these tests address: absolute on any host, and no file has to exist under it.
    fn site_root() -> PathBuf {
        std::env::current_dir()
            .expect("the tests run in the crate's directory")
            .join("site")
    }

    /// One rendered link: the text the source writes, and the address it resolves to.
    fn link_line(written: &str, resolved: &str) -> String {
        let address = crate::uri::from_file_path(&site_root().join(resolved)).unwrap();
        format!("{written} -> {}", address.as_str())
    }

    fn rendered(relative: &str, text: &str) -> Vec<String> {
        let id = RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new(relative).expect("a site path"),
        )
        .intern();
        let source = Source::new(id, text.to_owned());
        let root = site_root();
        paths(&source, &root, &crate::uri::ClientRoot::new(&root))
            .unwrap_or_default()
            .into_iter()
            .map(|link| {
                let lines = source.lines();
                let start = position::byte_offset(lines, link.range.start).expect("a link start");
                let end = position::byte_offset(lines, link.range.end).expect("a link end");
                format!(
                    "{} -> {}",
                    &source.text()[start..end],
                    link.target.expect("a link target").as_str()
                )
            })
            .collect()
    }
    #[test]
    fn reader_calls_link_their_file_arguments() {
        assert_eq!(
            rendered(
                "site.typ",
                "#let data = json(\"data/site.json\")\n#let notes = read(path: \"notes.md\", encoding: \"utf-8\")\n"
            ),
            [
                link_line("data/site.json", "data/site.json"),
                link_line("notes.md", "notes.md"),
            ]
        );
    }

    /// A bibliography names its sources and its citation style, and each is a file of the site.
    #[test]
    fn bibliography_paths_declare_their_links() {
        assert_eq!(
            rendered(
                "content/post.typ",
                "#bibliography(\"../assets/refs.bib\", \"../assets/more.bib\", style: \"../assets/style.csl\")\n"
            ),
            [
                link_line("../assets/refs.bib", "assets/refs.bib"),
                link_line("../assets/more.bib", "assets/more.bib"),
                link_line("../assets/style.csl", "assets/style.csl"),
            ]
        );
    }
    #[test]
    fn link_target_follows_the_caller_root_spelling() {
        let directory = tempfile::tempdir().unwrap();
        let spelled = directory.path().join("client-site");
        let id = RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new("content/document.typ").expect("a site path"),
        )
        .intern();
        let source = Source::new(id, "#include \"page.typ\"\n".to_owned());
        let target = paths(
            &source,
            directory.path(),
            &crate::uri::ClientRoot::with_resolved(&spelled, directory.path()),
        )
        .unwrap_or_default()
        .into_iter()
        .next()
        .and_then(|link| link.target)
        .expect("a link target");
        assert_eq!(
            target,
            crate::uri::from_file_path(&spelled.join("content/page.typ")).unwrap()
        );
    }
}
