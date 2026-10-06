//! The labels the site declares, and the references that point at them.
//!
//! A reference crosses documents, so it answers from the whole compiled Bundle rather than from
//! the file that has it.

use std::ops::Range;

use anyhow::Result;
use std::collections::HashMap;

use lsp_types::{
    CompletionItem, CompletionItemKind, DocumentHighlight, DocumentHighlightKind, Hover, Location,
    Range as ProtocolRange, TextEdit, Uri, WorkspaceEdit,
};
use tola_build::cancellation::BuildCancellation;
use tola_build::check::SourceCompilation;
use tola_build::config::ResolvedSiteConfig;
use tola_build::filesystem::display_path;
use tola_typst::typst::World;
use tola_typst::typst::foundations::{Content, Label, Selector};
use tola_typst::typst::syntax::{
    FileId, LinkedNode, Side, Source, Span, SyntaxKind, VirtualRoot, ast,
};
use tola_typst::typst::utils::PicoStr;

use crate::position;
use crate::protocol::markdown_hover;
use crate::routes;
use crate::sentence::listed;

/// A reference to a label of the site, as the author wrote it.
///
/// Both spellings name a label: `@name` in markup and `<name>` in code.
#[derive(Debug)]
pub(super) struct Reference {
    /// The whole reference, including its own punctuation: what a query copy drops to compile.
    pub written: Range<usize>,
    /// The label's name inside the reference.
    pub name: Range<usize>,
}

pub(super) fn referenced(source: &Source, cursor: usize) -> Option<Reference> {
    // A cursor sits between two leaves: an author who clicks the `@` of `@name` means the leaf that
    // starts there, and one whose cursor rests after `@nam` means the leaf that ends there.
    let root = LinkedNode::new(source.root());
    [Side::Before, Side::After]
        .into_iter()
        .find_map(|side| root.leaf_at(cursor, side).and_then(reference))
}

/// The reference one leaf belongs to, walking up to the marker or label that has the name.
fn reference(mut node: LinkedNode<'_>) -> Option<Reference> {
    loop {
        let range = node.range();
        match node.kind() {
            SyntaxKind::RefMarker => {
                return Some(Reference {
                    written: range.clone(),
                    name: range.start + 1..range.end,
                });
            }
            SyntaxKind::Label => {
                return Some(Reference {
                    written: range.clone(),
                    name: range.start + 1..range.end - 1,
                });
            }
            // A label the author has not closed yet is a syntax error: `<name` with no `>`.
            // Another unclosed token — a string — is no label reference, so it walks on.
            SyntaxKind::Error if node.leaf_text().starts_with('<') => {
                return Some(Reference {
                    written: range.clone(),
                    name: range.start + 1..range.end,
                });
            }
            _ => node = node.parent()?.clone(),
        }
    }
}

/// The part of a reference's name the author has typed, which is what a completion filters by.
///
/// A cursor that rests on the marker itself — the leaf `referenced` answers with when it ends
/// there — has typed none of the name.
pub(super) fn typed_name<'a>(source: &'a Source, reference: &Reference, cursor: usize) -> &'a str {
    let typed = cursor.max(reference.name.start);
    &source.text()[reference.name.start..typed]
}

/// Every spelling of one label in this source, the declaration first.
///
/// A label is declared by `<name>` and referred to by `@name`; both spellings name the one label,
/// and neither needs the site to compile.
pub(super) fn highlights(source: &Source, name: &str) -> Vec<DocumentHighlight> {
    let mut found = Vec::new();
    collect_spellings(&LinkedNode::new(source.root()), name, &mut found);
    let mut highlights: Vec<DocumentHighlight> = found
        .into_iter()
        .filter_map(|spelling| {
            Some(DocumentHighlight {
                range: position::utf16_range(source.lines(), spelling.range)?,
                kind: Some(if spelling.declares {
                    DocumentHighlightKind::WRITE
                } else {
                    DocumentHighlightKind::READ
                }),
            })
        })
        .collect();
    highlights.sort_by_key(|highlight| {
        (
            highlight.kind != Some(DocumentHighlightKind::WRITE),
            highlight.range.start,
        )
    });
    highlights
}

/// The labels the Bundle declares, in a deterministic order.
fn declared(compilation: &SourceCompilation) -> Vec<String> {
    // A site that did not compile declares no labels; the author reads that from the diagnostics.
    let Some(introspector) = compilation.introspector() else {
        return Vec::new();
    };
    let mut labels: Vec<String> = introspector
        .query_labelled()
        .into_iter()
        .filter_map(|element| Some(element.label()?.into_inner().resolve().as_str().to_owned()))
        .collect();
    labels.sort();
    labels.dedup();
    labels
}

/// The completion for the reference at `cursor`, whose items hold the edits they apply.
pub(super) fn completion(
    compilation: &SourceCompilation,
    source: &Source,
    cursor: usize,
) -> Option<Vec<CompletionItem>> {
    let reference = referenced(source, cursor)?;
    let typed = typed_name(source, &reference, cursor);
    let items = declared(compilation)
        .into_iter()
        .filter(|label| label.starts_with(typed))
        .filter_map(|label| {
            Some(CompletionItem {
                label: label.clone(),
                kind: Some(CompletionItemKind::REFERENCE),
                text_edit: Some(
                    super::completion::completion_edit(
                        source,
                        cursor,
                        reference.name.clone(),
                        label,
                    )
                    .ok()?,
                ),
                ..Default::default()
            })
        })
        .collect();
    Some(items)
}

/// Whether the site labels `name`, which answers a `@name` reference before any bibliography does.
pub(super) fn declares(compilation: &SourceCompilation, name: &str) -> bool {
    declared_element(compilation, name).is_some()
}

/// The element the site labels with `name`.
fn declared_element(compilation: &SourceCompilation, name: &str) -> Option<Content> {
    let label = Label::new(PicoStr::intern(name))?;
    compilation
        .introspector()?
        .query_first(&Selector::Label(label))
}

/// The label declaration this source has for `name`, if any.
///
/// A compilation without the site's own documents cannot say what the site labels, so the
/// declaration the author is looking at answers from the text in front of them.
fn declared_label<'a>(source: &'a Source, name: &str) -> Option<LinkedNode<'a>> {
    fn walk<'a>(node: LinkedNode<'a>, text: &str, name: &str) -> Option<LinkedNode<'a>> {
        let range = node.range();
        if node.kind() == SyntaxKind::Label && &text[range.start + 1..range.end - 1] == name {
            return Some(node);
        }
        node.children().find_map(|child| walk(child, text, name))
    }
    walk(LinkedNode::new(source.root()), source.text(), name)
}

/// The element a label declaration names: the call it sits inside in code, or follows in markup.
///
/// `#figure([Body]) <fig>` labels the figure; a declaration naming no call answers without an
/// element.
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

/// The call a label follows in markup, with the space the author typed, and any comment, between
/// them.
fn call_before<'b>(node: &LinkedNode<'b>) -> Option<LinkedNode<'b>> {
    let mut previous = node.prev_sibling()?;
    loop {
        match previous.kind() {
            SyntaxKind::Space
            | SyntaxKind::Parbreak
            | SyntaxKind::LineComment
            | SyntaxKind::BlockComment => previous = previous.prev_sibling()?,
            SyntaxKind::FuncCall => return Some(previous),
            _ => return None,
        }
    }
}

/// The name a call writes, whether bare or as `module.name`.
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

/// Every place the site's own files spell `name`, as a file and a range inside it.
///
/// Both spellings count: `@name` where a source references the label, and `<name>` where one
/// declares it. A label belongs to the document that has it, so the sources compiled into the
/// same documents are the ones that spell it — a partial compiled into many pages answers from all
/// of them, and a page nothing includes answers from itself.
fn spellings(
    compilation: &SourceCompilation,
    name: &str,
    cancellation: &BuildCancellation,
) -> Result<Vec<(FileId, ProtocolRange)>> {
    let Some(declared) =
        declared_element(compilation, name).and_then(|element| element.span().id())
    else {
        return Ok(Vec::new());
    };
    let world = compilation.world();
    let mut found = Vec::new();
    for id in compiled_with(compilation, declared, cancellation)? {
        let Ok(source) = world.source(id) else {
            continue;
        };
        let mut spellings = Vec::new();
        collect_spellings(&LinkedNode::new(source.root()), name, &mut spellings);
        found.extend(spellings.into_iter().filter_map(|spelling| {
            Some((id, position::utf16_range(source.lines(), spelling.range)?))
        }));
    }
    found.sort_by_key(|(id, range)| {
        (
            id.vpath().get_without_slash().to_owned(),
            range.start.line,
            range.start.character,
        )
    });
    Ok(found)
}

/// The sources compiled into the documents that hold `id`, in a deterministic order.
///
/// A label belongs to the document that has it, so the spellings that can name it are the
/// ones in the sources compiled beside it. A source the site publishes no document for still
/// declares its own labels.
fn compiled_with(
    compilation: &SourceCompilation,
    id: FileId,
    cancellation: &BuildCancellation,
) -> Result<Vec<FileId>> {
    let mut sources: Vec<FileId> = compilation
        .realized_documents(cancellation)?
        .into_iter()
        .filter(|document| document.sources.contains(&id))
        .flat_map(|document| document.sources)
        .filter(|id| id.vpath().extension() == Some("typ"))
        .collect();
    sources.push(id);
    sources.sort_by_key(|id| id.vpath().get_without_slash().to_owned());
    sources.dedup();
    Ok(sources)
}

/// One place one source spells a name.
pub(super) struct Spelling {
    /// The name's own bytes, inside the reference's or label's punctuation.
    pub range: Range<usize>,
    /// Whether the spelling declares the name (`<name>`) rather than references it (`@name`).
    pub declares: bool,
}

/// Every place one source spells `name`, in source order.
pub(super) fn collect_spellings(node: &LinkedNode<'_>, name: &str, found: &mut Vec<Spelling>) {
    if let Some(spelling) = token_spelling(node, name) {
        found.push(spelling);
    }
    for child in node.children() {
        collect_spellings(&child, name, found);
    }
}

/// The spelling one token has, when it spells `name`.
fn token_spelling(node: &LinkedNode<'_>, name: &str) -> Option<Spelling> {
    let text = node.leaf_text();
    let (inner, declares) = match node.kind() {
        SyntaxKind::RefMarker => (text.strip_prefix('@')?, false),
        SyntaxKind::Label => (text.strip_prefix('<')?.strip_suffix('>')?, true),
        _ => return None,
    };
    if inner != name {
        return None;
    }
    let start = node.range().start + 1;
    Some(Spelling {
        range: start..start + inner.len(),
        declares,
    })
}

/// The name a rename at the cursor would replace, and the spelling the author selected.
pub(super) fn prepared(
    compilation: &SourceCompilation,
    source: &Source,
    cursor: usize,
) -> Option<(ProtocolRange, String)> {
    let reference = referenced(source, cursor)?;
    let name = &source.text()[reference.name.clone()];
    // Renaming what the site never declares would rename a typo rather than a label.
    declared_element(compilation, name)?;
    Some((
        position::utf16_range(source.lines(), reference.name)?,
        name.to_owned(),
    ))
}

/// Where the site declares `name`.
pub(super) fn definition(compilation: &SourceCompilation, name: &str) -> Option<Span> {
    Some(declared_element(compilation, name)?.span())
}

/// Every document that names the label `name`, wherever in the site it is written.
///
/// A file is addressed in the spelling the client named the site root by, never the resolved form
/// the compiler reads it through.
pub(super) fn locations(
    compilation: &SourceCompilation,
    name: &str,
    cancellation: &BuildCancellation,
    client_root: &crate::uri::ClientRoot,
) -> Result<Vec<Location>> {
    let mut found = Vec::new();
    for (id, range) in spellings(compilation, name, cancellation)? {
        if let Some(uri) = crate::identity::client_uri(id, client_root) {
            found.push(Location { uri, range });
        }
    }
    Ok(found)
}

/// The edits that rename every spelling of `name` in the site's own files.
pub(super) fn rename(
    compilation: &SourceCompilation,
    name: &str,
    new_name: &str,
    cancellation: &BuildCancellation,
    client_root: &crate::uri::ClientRoot,
) -> Result<Option<WorkspaceEdit>> {
    if Label::new(PicoStr::intern(new_name)).is_none() {
        return Ok(None);
    }
    // The protocol's own edit map is keyed by URI, which clippy reads as interior-mutable.
    #[expect(clippy::mutable_key_type, reason = "the protocol keys edits by URI")]
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for (id, range) in spellings(compilation, name, cancellation)? {
        let Some(uri) = crate::identity::client_uri(id, client_root) else {
            continue;
        };
        changes.entry(uri).or_default().push(TextEdit {
            range,
            new_text: new_name.to_owned(),
        });
    }
    Ok((!changes.is_empty()).then(|| WorkspaceEdit {
        changes: Some(changes),
        ..WorkspaceEdit::default()
    }))
}

/// What the site says about the label the reference at `cursor` names: the element that has
/// it, the file that declares it, and the pages the site serves it on.
pub(super) fn hover(
    compilation: &SourceCompilation,
    config: &ResolvedSiteConfig,
    source: &Source,
    cursor: usize,
    cancellation: &BuildCancellation,
) -> Result<Option<Hover>> {
    let Some(reference) = referenced(source, cursor) else {
        return Ok(None);
    };
    let range = position::utf16_range(source.lines(), reference.written.clone());
    let name = &source.text()[reference.name.clone()];
    // The hover shows the spelling the author wrote, brackets and all.
    let written = &source.text()[reference.written.clone()];
    let Some(element) = declared_element(compilation, name) else {
        // The compiled label set is unknown without the site's own documents, so a declaration
        // this file has answers from the file instead of as a missing label.
        if compilation.bundle().is_none()
            && let Some(label) = declared_label(source, name)
        {
            let described = match labelled_element(&label) {
                Some(element) => format!("Labels a `{element}` element declared in this file."),
                None => format!("`{written}` is declared in this file."),
            };
            return Ok(Some(markdown_hover(
                format!("```typc\n{written}\n```\n\n{described}"),
                range,
            )));
        }
        return Ok(Some(markdown_hover(
            format!("no source declares the label `{name}`"),
            range,
        )));
    };
    let mut described = format!("Labels a `{}` element", element.func().name());
    if let Some(id) = element.span().id() {
        described.push_str(&format!(" declared in `{}`", declared_in(compilation, id)));
        let served = routes::site_routes(compilation, config, id, cancellation)?;
        let (listed, remaining) = listed(
            served.iter().map(|route| route.route.as_str()),
            crate::routes::NAMED_PAGES,
        );
        if !listed.is_empty() {
            described.push_str(&format!(", served at {listed}"));
            if remaining > 0 {
                described.push_str(&format!(" and {remaining} more"));
            }
        }
    }
    described.push('.');
    Ok(Some(markdown_hover(
        format!("```typc\n{written}\n```\n\n{described}"),
        range,
    )))
}

/// The site-relative path that declares a label, or the package that has it.
fn declared_in(compilation: &SourceCompilation, id: FileId) -> String {
    let world = compilation.world();
    match id.root() {
        VirtualRoot::Package(spec) => spec.to_string(),
        VirtualRoot::Project => display_path(
            &world.root().join(id.vpath().get_without_slash()),
            world.root(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference yields the span it writes and the name inside it, whether it is markup or a
    /// label argument.
    #[test]
    fn references_yield_their_name() {
        for (text, cursor, written, name) in [
            ("See @introduction\n", 8, 4..17, 5..17),
            ("#ref(<introduction>)\n", 8, 5..19, 6..18),
        ] {
            let source = Source::detached(text);
            let reference = referenced(&source, cursor).expect("a reference");
            assert_eq!(reference.written, written, "{text:?}");
            assert_eq!(reference.name, name, "{text:?}");
        }
    }

    #[test]
    fn text_that_names_no_label_yields_none() {
        for (text, cursor) in [("See the introduction\n", 12), ("#import \"abc\n", 10)] {
            let source = Source::detached(text);
            assert!(referenced(&source, cursor).is_none(), "{text:?}");
        }
    }

    /// A completion's prefix stops at the name's start: a cursor resting on the `@` has typed
    /// nothing, and one inside the name has typed up to it.
    #[test]
    fn marker_cursor_yields_empty_prefix() {
        let source = Source::detached("See @name\n");
        let reference = referenced(&source, 4).expect("the marker's reference");
        assert_eq!(typed_name(&source, &reference, 4), "");
        assert_eq!(typed_name(&source, &reference, 8), "nam");
    }

    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use tola_build::BuildResources;
    use tola_build::check::SourceDiagnosticSession;
    use tola_build::config::loading::{BuildOverrides, load_site_config};

    /// A site program that compiles: every content file becomes one page.
    const SITE_PROGRAM: &str =
        "#for source in (\"content/page.typ\",) {\n  document(\"page.html\")[#include source]\n}\n";

    /// One site on disk, with the configuration its own `tola.toml` declares.
    struct Site {
        _directory: tempfile::TempDir,
        root: PathBuf,
    }

    impl Site {
        fn new(program: &str) -> Self {
            let directory = tempfile::tempdir().unwrap();
            // The world resolves the site root through the filesystem, so the site's paths are
            // the ones it resolves.
            let root = tola_build::filesystem::normalize_existing_prefix(directory.path());
            let site = Self {
                _directory: directory,
                root,
            };
            site.write("tola.toml", "");
            site.write("site.typ", program);
            site
        }

        fn root(&self) -> &Path {
            &self.root
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.root().join(relative)
        }

        fn write(&self, relative: &str, text: &str) {
            let path = self.path(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    fn configuration(root: &Path) -> Arc<ResolvedSiteConfig> {
        Arc::new(
            load_site_config(
                Some(&root.join("tola.toml")),
                tola_typst::PackageLocations::from_absolute_roots(None, None).unwrap(),
                &BuildOverrides::default(),
            )
            .unwrap()
            .into_config(),
        )
    }

    /// The label hover over `marked` in `relative`, from the compilation the query ladder answers
    /// with: the site's own when it compiled, and the file's alone when it did not.
    fn label_hover(site: &Site, relative: &str, marked: &str) -> String {
        let path = site.path(relative);
        let text = marked.replace('|', "");
        let cursor = marked.find('|').expect("a cursor marker");
        site.write(relative, &text);
        let config = configuration(site.root());
        let cancellation = BuildCancellation::new();
        let overrides = vec![(path.clone(), Arc::<str>::from(text.as_str()))];
        let mut session = SourceDiagnosticSession::with_resources(
            Arc::clone(&config),
            BuildResources::new()
                .with_network_access(tola_build::NetworkAccess::Denied)
                .without_system_fonts(),
        );
        let revision = session
            .inspect(overrides.clone(), &cancellation)
            .expect("a completed check");
        let alone;
        let compilation = match revision.checked() {
            Some(checked) if checked.bundle().is_some() => checked,
            _ => {
                alone = session
                    .inspect_source(relative, overrides, &cancellation)
                    .expect("a prepared world")
                    .into_checked();
                alone.as_ref().expect("a compilation for the file")
            }
        };
        let uri = crate::uri::from_file_path(&path).expect("the file's uri");
        let id = crate::identity::file_id(&uri, site.root()).expect("the file's id");
        let source = compilation.world().source(id).expect("the file's source");
        let hover = hover(compilation, &config, &source, cursor, &cancellation)
            .expect("a label hover")
            .expect("a label answer");
        match hover.contents {
            lsp_types::HoverContents::Markup(markup) => markup.value,
            _ => panic!("a label hover is markdown"),
        }
    }

    #[test]
    fn file_labels_answer_while_the_check_fails() {
        // The site's own program does not compile, and neither does the page on its own, so the
        // compiled label set is unknown while the page still has the label the author reads.
        let site = Site::new("#let broken = )\n");
        let answer = label_hover(
            &site,
            "content/page.typ",
            "#figure([Body]) <int|ro>\n\nSee @intro\n\n#undefined-fn()\n",
        );
        assert!(answer.contains("`figure`"), "{answer}");
        assert!(answer.contains("declared in this file"), "{answer}");
        assert!(!answer.contains("no source declares the label"), "{answer}");
    }

    #[test]
    fn compiled_site_still_denies_unknown_labels() {
        // A site that compiled knows every label it has, so one it does not hold is absent
        // rather than unknown.
        let site = Site::new(SITE_PROGRAM);
        let answer = label_hover(&site, "content/page.typ", "See @missing|\n");
        assert!(answer.contains("no source declares the label"), "{answer}");
    }

    /// A label definition, reference and rename edit name the file the client spelled, never the
    /// resolved root the compiler reads through.
    #[test]
    fn label_answers_keep_the_client_spelling() {
        // The same shape `QuerySession` compiles with: `all-sources()` reads every content unit,
        // and `document(…)` realizes one page for it.
        let site = Site::new(
            r#"#import "@tola/source:0.0.0": all-sources
#for source in all-sources() {
  document("page.html")[#include source.file]
}
"#,
        );
        site.write(
            "content/page.typ",
            "#figure([Body]) <intro>\n\nSee @intro\n",
        );
        let config = configuration(site.root());
        let cancellation = BuildCancellation::new();
        let mut session = SourceDiagnosticSession::with_resources(
            Arc::clone(&config),
            BuildResources::new()
                .with_network_access(tola_build::NetworkAccess::Denied)
                .without_system_fonts(),
        );
        let revision = session
            .inspect(Vec::new(), &cancellation)
            .expect("a completed check");
        let compilation = revision.checked().expect("the site compiles");
        let spelled = site.root().join("spelled");
        let client_root = crate::uri::ClientRoot::with_resolved(&spelled, site.root());
        let expected = crate::uri::from_file_path(&spelled.join("content/page.typ")).unwrap();

        let found =
            locations(compilation, "intro", &cancellation, &client_root).expect("locations");
        assert!(!found.is_empty(), "the label's spellings");
        assert!(
            found.iter().all(|location| location.uri == expected),
            "{found:?}"
        );

        let rename = rename(compilation, "intro", "outro", &cancellation, &client_root)
            .expect("a rename")
            .expect("edits");
        // The protocol's own edit map is keyed by URI, which clippy reads as interior-mutable.
        #[expect(clippy::mutable_key_type, reason = "the protocol keys edits by URI")]
        let changes = rename.changes.expect("edits by URI");
        assert_eq!(changes.keys().collect::<Vec<_>>(), [&expected]);
    }
    /// A label hover names the page the labelled element is served at.
    #[test]
    fn label_hover_names_the_serving_page() {
        let site = Site::new(
            r#"#for source in ("content/page.typ",) {
  document("page.html")[#include source]
}
"#,
        );
        let answer = label_hover(
            &site,
            "content/page.typ",
            "#figure([Body]) <introduction>\n\nSee @intro|duction\n",
        );
        assert_eq!(
            answer,
            "```typc\n@introduction\n```\n\nLabels a `figure` element declared in `content/page.typ`, served at /page.html."
        );
    }

    use crate::query::tests::*;
    use lsp_types::{GotoDefinitionResponse, PrepareRenameResponse};
    /// A cursor resting on a reference's own marker — where a click or a motion lands — means that
    /// reference, not the text beside it.
    #[test]
    fn reference_answers_on_its_marker() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("= Title <intro>\n\nSee |@intro.\n")
            .expect("a hover on the marker itself");
        assert!(hover.contains("@intro"), "{hover}");
    }

    /// A cursor on a loop's control flow answers the loop it belongs to.
    #[test]
    fn loop_control_flow_highlights_its_loop() {
        let mut site = QuerySession::new();
        let highlights = site
            .highlights("#for x in (1, 2) {\n  if x == 1 { br|eak }\n  continue\n}\n")
            .expect("highlights for the loop");
        assert_eq!(highlights.len(), 3, "{highlights:?}");
        assert_eq!(
            (
                highlights[0].range.start.line,
                highlights[0].range.start.character
            ),
            (0, 0)
        );
        assert_eq!(
            (
                highlights[1].range.start.line,
                highlights[1].range.start.character
            ),
            (1, 14)
        );
        assert!(
            highlights.iter().all(|highlight| highlight.kind.is_none()),
            "{highlights:?}"
        );
    }
    /// A reference describes the element its label sits on, the file that declares it, and the
    /// page the site serves it on.
    #[test]
    fn references_describe_their_element() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("#metadata((kind: \"note\")) <introduction>\nSee @intro|duction")
            .expect("a hover for the reference");
        assert!(hover.contains("metadata"), "{hover}");
        assert!(hover.contains("content/document.typ"), "{hover}");
        assert!(hover.contains("/document/"), "{hover}");
    }

    /// A label answers every place the site spells it, wherever that is written.
    #[test]
    fn references_answer_every_spelling() {
        let mut site = QuerySession::new();
        let references = site
            .references(
                "#figure([Body]) <introduction>\nSee @intro|duction and @introduction\n",
                true,
            )
            .expect("the spellings of the label");
        assert_eq!(references.len(), 3, "{references:?}");
        assert!(
            references
                .iter()
                .all(|location| location.uri.as_str().ends_with("document.typ")),
            "{references:?}"
        );
    }

    /// Renaming a label edits every spelling of it in the site's own files.
    #[test]
    fn label_rename_edits_every_spelling() {
        let mut site = QuerySession::new();
        let edit = site
            .rename(
                "#figure([Body]) <introduction>\nSee @intro|duction and @introduction\n",
                "overview",
            )
            .expect("edits for the rename");
        #[expect(clippy::mutable_key_type, reason = "the protocol keys edits by URI")]
        let changes = edit.changes.expect("a changes map");
        assert_eq!(changes.len(), 1, "{changes:?}");
        let edits = changes.values().flatten().collect::<Vec<_>>();
        assert_eq!(edits.len(), 3, "{changes:?}");
        assert!(
            edits.iter().all(|change| change.new_text == "overview"),
            "{changes:?}"
        );
    }

    /// The names a file binds answer references, with or without their declaration.
    #[test]
    fn bound_name_answers_references() {
        let mut site = QuerySession::new();
        let text = "#let factor = 2\n#let twice = factor * fa|ctor\n";
        let uses = site
            .references(text, false)
            .expect("the spellings of the name");
        assert_eq!(uses.len(), 2, "{uses:?}");
        let all = site
            .references(text, true)
            .expect("the spellings of the name");
        assert_eq!(all.len(), 3, "{all:?}");
        assert_eq!(
            (all[0].range.start.line, all[0].range.start.character),
            (0, 5),
            "the declaration comes first: {all:?}"
        );
    }

    /// A rename replaces every spelling of a name the file binds, and refuses a name that is not
    /// one identifier.
    #[test]
    fn bound_name_rename_edits_every_spelling() {
        let mut site = QuerySession::new();
        let text = "#let factor = 2\n#f|actor\n";
        let Some(PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. }) =
            site.prepare_rename(text)
        else {
            panic!("no rename preparation for a bound name");
        };
        assert_eq!(placeholder, "factor");
        let edit = site.rename(text, "scale").expect("the rename edits");
        let mut files = edit.changes.expect("edits by file").into_values();
        let edits = files.next().expect("one edited file");
        assert!(files.next().is_none(), "the rename edits one file");
        assert_eq!(edits.len(), 2, "{edits:?}");
        assert!(
            edits.iter().all(|edit| edit.new_text == "scale"),
            "{edits:?}"
        );
        assert_eq!(
            site.rename_refusal(text, "2scale"),
            "invalidIdentifier",
            "a name that is not one identifier is refused"
        );
    }

    /// A bound name answers the highlight request the way a label does.
    #[test]
    fn bound_name_answers_highlights() {
        let mut site = QuerySession::new();
        let highlights = site
            .highlights("#let factor = 2\n#f|actor\n")
            .expect("the spellings of the name");
        assert_eq!(highlights.len(), 2, "{highlights:?}");
        assert_eq!(highlights[0].kind, Some(DocumentHighlightKind::WRITE));
        assert_eq!(highlights[1].kind, Some(DocumentHighlightKind::READ));
        assert_eq!(
            (
                highlights[1].range.start.line,
                highlights[1].range.start.character
            ),
            (1, 1),
            "{highlights:?}"
        );
    }

    /// A rename replaces the parameter's own spellings, never the outer binding of that name.
    #[test]
    fn rename_touches_only_the_parameter() {
        let mut site = QuerySession::new();
        let edit = site
            .rename(
                "#let scale = 3\n#let render(scale) = sca|le\n#render(2)\n",
                "factor",
            )
            .expect("the rename edits");
        let mut files = edit.changes.expect("edits by file").into_values();
        let edits = files.next().expect("one edited file");
        assert_eq!(edits.len(), 2, "{edits:?}");
        assert!(
            edits.iter().all(|edit| edit.range.start.line == 1),
            "only the parameter's own spellings: {edits:?}"
        );
    }
    /// Every spelling of a label answers the highlight request, the declaration first.
    #[test]
    fn highlights_answer_every_spelling() {
        let mut site = QuerySession::new();
        let highlights = site
            .highlights("#figure([Body]) <introduction>\nSee @intro|duction and @introduction\n")
            .expect("the spellings of the label");
        assert_eq!(highlights.len(), 3, "{highlights:?}");
        assert_eq!(
            (
                highlights[0].kind,
                highlights[0].range.start.line,
                highlights[0].range.start.character,
                highlights[0].range.end.line,
                highlights[0].range.end.character,
            ),
            (Some(DocumentHighlightKind::WRITE), 0, 17, 0, 29),
            "{highlights:?}"
        );
        assert!(
            highlights[1..]
                .iter()
                .all(|highlight| highlight.kind == Some(DocumentHighlightKind::READ)),
            "{highlights:?}"
        );
        assert_eq!(
            (
                highlights[1].range.start.line,
                highlights[1].range.start.character,
                highlights[1].range.end.line,
                highlights[1].range.end.character,
            ),
            (1, 5, 1, 17),
            "{highlights:?}"
        );
    }

    #[test]
    fn prepare_rename_answers_the_name() {
        let mut site = QuerySession::new();
        let Some(PrepareRenameResponse::RangeWithPlaceholder { placeholder, .. }) =
            site.prepare_rename("#metadata((kind: \"note\")) <introduction>\nSee @intro|duction\n")
        else {
            panic!("no rename preparation for a declared label");
        };
        assert_eq!(placeholder, "introduction");
    }

    /// Renaming what the site never declares would rename a typo, so nothing is offered.
    #[test]
    fn undeclared_labels_offer_no_rename() {
        let mut site = QuerySession::new();
        assert!(site.prepare_rename("See @intro|duction\n").is_none());
        assert!(site.rename("See @intro|duction\n", "overview").is_none());
    }

    /// A code label describes the same element its markup reference names, spelling included.
    #[test]
    fn code_labels_describe_their_element() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("#figure([A]) <intro|duction>\n")
            .expect("a hover for the label");
        assert!(hover.contains("<introduction>"), "{hover}");
        assert!(hover.contains("figure"), "{hover}");
        assert!(hover.contains("content/document.typ"), "{hover}");
    }

    /// A label nothing declares says so, rather than describing another site's element.
    #[test]
    fn undeclared_labels_say_so() {
        let mut site = QuerySession::new();
        let hover = site
            .hover_text("See @intro|duction\n")
            .expect("a hover for the reference");
        assert!(
            hover.contains("no source declares the label `introduction`"),
            "{hover}"
        );
    }

    /// A reference points at the element its label sits on, wherever in the site that is.
    #[test]
    fn references_resolve_their_element() {
        let mut site = QuerySession::new();
        let Some(GotoDefinitionResponse::Scalar(location)) =
            site.definition("#metadata((kind: \"note\")) <introduction>\nSee @intro|duction")
        else {
            panic!("expected the labelled element");
        };
        assert_eq!(location.range.start.line, 0);
    }
}
