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
use tola_typst::typst::syntax::{FileId, LinkedNode, Source, Span, SyntaxKind, VirtualRoot, ast};
use tola_typst_syntax::names::SourceNames;

use crate::position;
use crate::sentence::listed;

/// One path a source's text declares: the file it names and the text that writes it.
pub(super) struct Link {
    /// The file the written path resolves to, under the source's own root.
    target: FileId,
    /// The string literal that writes the path, quotation marks included.
    pub(super) literal: std::ops::Range<usize>,
    /// The path between the quotation marks, as an editor addresses it.
    pub(super) range: lsp_types::Range,
}

impl Link {
    /// The site path this link names, in the spelling every site path is compared with.
    ///
    /// `root` is the site root as the filesystem resolves it. A package file names no site path,
    /// and neither does a spelling that resolves outside the site.
    pub(super) fn site_path(&self, root: &Path) -> Option<PathBuf> {
        if !matches!(self.target.root(), VirtualRoot::Project) {
            return None;
        }
        let path = tola_build::filesystem::normalize_existing_prefix(
            &root.join(self.target.vpath().get_without_slash()),
        );
        path.starts_with(root).then_some(path)
    }
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
    let links = declared(source);
    let Some(link) = holding(&links, cursor) else {
        return Ok(None);
    };
    let Some(target) = link.site_path(&root) else {
        return Ok(None);
    };
    let Some(written) = written_path(source, link.range) else {
        return Ok(None);
    };
    if !target.exists() {
        return Ok(Some(crate::protocol::markdown_hover(
            format!("nothing in this site answers `{written}`"),
            Some(link.range),
        )));
    }
    let named = tola_build::filesystem::display_path(&target, &root);
    let Some(id) = crate::identity::path_id(&target, &root) else {
        return Ok(None);
    };
    let served = crate::routes::site_routes(compilation, config, id, cancellation)?;
    let described = if served.is_empty() {
        "A file this site includes; no document is built from it.".to_owned()
    } else {
        let (listed, remaining) = listed(
            served.iter().map(|route| route.route.as_str()),
            crate::routes::NAMED_PAGES,
        );
        let more = match remaining {
            0 => String::new(),
            remaining => format!(" and {remaining} more"),
        };
        format!("Served at {listed}{more}.")
    };
    Ok(Some(crate::protocol::markdown_hover(
        format!("```typc\n{named}\n```\n\n{described}"),
        Some(link.range),
    )))
}

/// The path the author wrote inside a link's range, without the quotation marks.
fn written_path(source: &Source, range: lsp_types::Range) -> Option<String> {
    let bytes = position::byte_range(source.lines(), range)?;
    Some(source.text().get(bytes)?.to_owned())
}

/// The link among `links` whose path the author's cursor at byte `cursor` stands in.
///
/// A cursor on either quotation mark names the literal, not the path between them.
pub(super) fn holding(links: &[Link], cursor: usize) -> Option<&Link> {
    links
        .iter()
        .find(|link| link.literal.start < cursor && cursor < link.literal.end)
}

/// The source root resolves file identities; the client root chooses their URI spelling.
pub(super) fn paths(
    source: &Source,
    source_root: &Path,
    client_root: &crate::uri::ClientRoot,
) -> Option<Vec<DocumentLink>> {
    let links: Vec<DocumentLink> = declared(source)
        .iter()
        .filter_map(|link| {
            Some(DocumentLink {
                range: link.range,
                target: Some(
                    crate::identity::client_uri(link.target, source_root, client_root).ok()?,
                ),
                tooltip: None,
                data: None,
            })
        })
        .collect();
    (!links.is_empty()).then_some(links)
}

/// The links this source's text declares, in the order it writes them.
pub(super) fn declared(source: &Source) -> Vec<Link> {
    let names = SourceNames::new(source.clone());
    let mut links = collect_links(&LinkedNode::new(source.root()), source, &names);
    links.sort_by_key(|link| (link.range.start.line, link.range.start.character));
    links
}

fn collect_links(node: &LinkedNode<'_>, source: &Source, names: &SourceNames) -> Vec<Link> {
    let mut written: Vec<(Span, String)> = Vec::new();
    match node.kind() {
        SyntaxKind::ModuleImport => written.extend(
            node.cast::<ast::ModuleImport>()
                .and_then(|import| string_literal(import.source())),
        ),
        SyntaxKind::ModuleInclude => written.extend(
            node.cast::<ast::ModuleInclude>()
                .and_then(|include| string_literal(include.source())),
        ),
        SyntaxKind::FuncCall | SyntaxKind::SetRule => written = reader_paths(node, source, names),
        _ => {}
    }
    let mut links: Vec<Link> = written
        .into_iter()
        .filter_map(|(span, path)| link_at(node, span, &path, source))
        .collect();
    for child in node.children() {
        links.extend(collect_links(&child, source, names));
    }
    links
}

/// The paths one reader call names: every positional source, and the named ones that name files.
fn reader_paths(
    node: &LinkedNode<'_>,
    source: &Source,
    names: &SourceNames,
) -> Vec<(Span, String)> {
    let (target, args) = if let Some(call) = node.cast::<ast::FuncCall>() {
        (call.callee(), call.args())
    } else if let Some(rule) = node.cast::<ast::SetRule>() {
        (rule.target(), rule.args())
    } else {
        return Vec::new();
    };
    let ast::Expr::Ident(name) = target else {
        return Vec::new();
    };
    let Some(callee) = node.find(name.span()) else {
        return Vec::new();
    };
    if names.declared_at(callee.range().start).is_some() {
        return Vec::new();
    }
    let reader = name.as_str();
    if reader == "raw" {
        return raw_paths(node, source, &callee.range());
    }
    // Only a file the site already holds is a link; an output the site creates is not.
    if tola_typst_syntax::syntax::path_base(reader)
        != Some(tola_typst_syntax::syntax::PathBase::File)
    {
        return Vec::new();
    }
    // A bibliography takes a list of sources, and names its citation style beside them.
    let bibliography = reader == "bibliography";
    let mut positional = false;
    let mut paths = Vec::new();
    for item in args.items() {
        match item {
            ast::Arg::Pos(expression) if !positional => {
                positional = true;
                paths.extend(source_strings(expression, bibliography));
            }
            ast::Arg::Spread(_) => positional = true,
            ast::Arg::Named(named) if bibliography => {
                if named.name().as_str() == "style"
                    && let Some((span, text)) = string_literal(named.expr())
                    && matches!(
                        Value::Str(text.as_str().into()).cast::<CslSource>().ok(),
                        Some(CslSource::Normal(DataSource::Path(_)))
                    )
                {
                    paths.push((span, text));
                }
            }
            ast::Arg::Named(named) if named.name().as_str() == "path" => {
                paths.extend(string_literal(named.expr()));
            }
            _ => {}
        }
    }
    paths
}

/// The paths one `raw` call names: the text it renders and the theme and syntax files it reads.
fn raw_paths(
    node: &LinkedNode<'_>,
    source: &Source,
    callee: &std::ops::Range<usize>,
) -> Vec<(Span, String)> {
    if let Some(text) = node.cast::<ast::Str>() {
        return tola_typst_syntax::syntax::path_argument(source, node.range().start + 1)
            .is_some_and(|argument| argument.callee.as_ref() == Some(callee))
            .then(|| (text.span(), text.get().to_string()))
            .into_iter()
            .collect();
    }
    node.children()
        .flat_map(|child| raw_paths(&child, source, callee))
        .collect()
}

/// The string literals one positional argument writes.
fn source_strings(expression: ast::Expr<'_>, accepts_list: bool) -> Vec<(Span, String)> {
    let mut paths = Vec::new();
    let mut values = vec![expression];
    while let Some(value) = values.pop() {
        match value {
            ast::Expr::Str(text) => paths.push((text.span(), text.get().to_string())),
            ast::Expr::Parenthesized(group) => values.push(group.expr()),
            ast::Expr::Array(items) if accepts_list => {
                values.extend(items.items().filter_map(|item| match item {
                    ast::ArrayItem::Pos(expression) => Some(expression),
                    _ => None,
                }));
            }
            _ => {}
        }
    }
    paths
}

/// The string literal one path expression writes, with its own span.
fn string_literal(expression: ast::Expr<'_>) -> Option<(Span, String)> {
    match expression {
        ast::Expr::Str(text) => Some((text.span(), text.get().to_string())),
        _ => None,
    }
}

/// The link one path expression declares, when it names an addressable document.
fn link_at(node: &LinkedNode<'_>, span: Span, path: &str, source: &Source) -> Option<Link> {
    // A package specification and a URL both name something a source file cannot address.
    if path.is_empty() || path.starts_with('@') || path.contains("://") {
        return None;
    }
    let target = PathOrStr::Str(path.into())
        .resolve(source.id())
        .ok()?
        .intern();
    let literal = node.find(span)?.range();
    // The link covers the path the author wrote, not the quotes around it.
    let range = position::utf16_range(source.lines(), literal.start + 1..literal.end - 1)?;
    Some(Link {
        target,
        literal,
        range,
    })
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
        let target = declared(&source).into_iter().next()?.site_path(root)?;
        let inside = target.strip_prefix(root).ok()?;
        Some(inside.to_string_lossy().replace('\\', "/"))
    }

    /// A link target resolves against the file that writes it, and stays inside the site.
    #[test]
    fn link_targets_resolve_writing_file() {
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
                "#bibliography((\"../assets/refs.bib\", \"../assets/more.bib\"), style: \"../assets/style.csl\")\n"
            ),
            [
                link_line("../assets/refs.bib", "assets/refs.bib"),
                link_line("../assets/more.bib", "assets/more.bib"),
                link_line("../assets/style.csl", "assets/style.csl"),
            ]
        );
    }

    #[test]
    fn raw_theme_and_syntax_links() {
        assert_eq!(
            rendered(
                "content/post.typ",
                "#set raw(theme: \"../assets/my.tmTheme\", syntaxes: (\"../assets/a.sublime-syntax\", \"../assets/b.sublime-syntax\"))\n#raw(\"text\", theme: \"../assets/my.tmTheme\")\n"
            ),
            [
                link_line("../assets/my.tmTheme", "assets/my.tmTheme"),
                link_line("../assets/a.sublime-syntax", "assets/a.sublime-syntax"),
                link_line("../assets/b.sublime-syntax", "assets/b.sublime-syntax"),
                link_line("../assets/my.tmTheme", "assets/my.tmTheme"),
            ]
        );
        assert!(
            rendered(
                "content/post.typ",
                "#let raw(theme) = theme\n#raw(theme: \"../assets/my.tmTheme\")\n"
            )
            .is_empty()
        );
    }
    #[test]
    fn link_follows_caller_root_spelling() {
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
