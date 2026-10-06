//! Official value queries and source-only editor requests for one immutable source view.

mod actions;
mod citations;
mod colors;
mod completion;
pub(super) mod config;
mod context;
mod definition;
mod editing;
mod hints;
mod hover;
mod imports;
mod labels;
mod meta;
mod name_graph;
pub(super) mod package;
pub(super) mod paths;
mod postfix;
mod records;
mod repair;
mod schema;
mod semantic;
mod signature;
mod site_schema;

pub(super) use citations::BibliographyCache;
pub(in crate::query) use completion::markdown_documentation;
pub(super) use name_graph::source_reply;
pub(super) use package::package_source_definition;
pub(super) use records::origin::field_chain;
pub(super) use repair::pin_source;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use lsp_types::{
    CompletionItem, CompletionResponse, DocumentHighlight, GotoDefinitionResponse,
    PrepareRenameResponse,
};
use tola_build::AssetUrls;
use tola_build::cancellation::BuildCancellation;
use tola_build::check::{SourceCompilation, SourceDiagnosticSession};
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::World;
use tola_typst::typst::syntax::VirtualRoot;

use crate::compiler::RevisionCompilations;
use crate::packages::PackageAccess;
use crate::position;
use crate::protocol::{CheckProgress, PackageSourceText, SourceQuery, SourceReply, markdown_hover};
use crate::published::PublishedPackage;
use crate::sources::SourceView;

use completion::documentation_template;
use context::{SelectedSyntax, Selection, load_source};
use hover::says_something;
use name_graph::graph_reply;
use semantic::Semantic;

/// Answer a semantic request without writing sources, running hooks or exporting
/// output. Cancellation remains an error, distinct from an unavailable result.
/// The caller owns serialization, discards responses for superseded revisions, and holds the
/// compilations this revision already produced.
#[expect(
    clippy::too_many_arguments,
    reason = "an answer reads session state, revision inputs, the query, packages, and cancellation"
)]
pub(super) fn respond(
    session: &mut SourceDiagnosticSession,
    compilations: &mut RevisionCompilations,
    disk: &mut crate::analysis::DiskSources,
    bibliographies: &mut BibliographyCache,
    config: &Arc<ResolvedSiteConfig>,
    source_revision: u64,
    overrides: &[(PathBuf, Arc<str>)],
    view: &SourceView,
    published: &[PublishedPackage],
    package_sources: Option<&Path>,
    query: &SourceQuery,
    routes_as_hints: bool,
    check_progress: CheckProgress,
    cancellation: &BuildCancellation,
    client_root: &crate::uri::ClientRoot,
) -> Result<SourceReply> {
    cancellation.ensure_active()?;
    let (uri, position) = query.document();
    // A URI this service cannot address names no source this site holds: one outside the site, one
    // naming generated state, or a package URI with an editor's own qualifier. An editor asks
    // about every document it shows, so the answer is that there is nothing to say, never that the
    // request failed. A read the boundary itself could not perform stays a failure.
    let Ok(mut id) = crate::identity::file_id(uri, config.get_root()) else {
        return Ok(unavailable(query));
    };
    let package_access = PackageAccess::new(config, session.resources(), published);
    if matches!(id.root(), VirtualRoot::Project) {
        let path = config.get_root().join(id.vpath().get_without_slash());
        if package_access.boundary.refusal(&path)?.is_some() {
            // The mirrors `tola editor setup` publishes are how a file-only client reads this
            // site's packages, and a definition into one names the mirror rather than a
            // `tola-package:` document. A mirror of a builtin source answers as that source, which
            // is immutable and already verified against the file; anything else refused by the
            // boundary names nothing this site holds.
            let mirrored = package_sources
                .and_then(|directory| crate::identity::package_view_id(&path, directory))
                .or_else(|| crate::identity::mirrored_package_id(&path, config.get_root()));
            let Some(mirrored) = mirrored else {
                return Ok(unavailable(query));
            };
            id = mirrored;
        }
    }
    let source = match view.by_id(id) {
        Some(snapshot) => Some(snapshot.source.clone()),
        None => load_source(id, config, overrides, &package_access.boundary)?,
    };
    let Some(source) = source else {
        return Ok(unavailable(query));
    };
    // A position outside the source names nothing, and an editor may ask about any position in a
    // document it has since shortened: that answers nothing rather than failing the request.
    let Ok(cursor) = position::byte_offset(source.lines(), position) else {
        return Ok(unavailable(query));
    };
    if matches!(query, SourceQuery::Completion(_))
        && let Some(reply) = documentation_template(&source, cursor)
    {
        return Ok(reply);
    }
    if let Some(reply) = source_reply(&source, cursor, query)? {
        return Ok(reply);
    }
    // An import path is neither a name nor a label, so the package lane answers it before the
    // name graph builds over the source set.
    if let Some(reply) = package::respond(&package_access, &source, cursor, query, client_root)? {
        return Ok(reply);
    }
    if query.answered_from_name_graph()
        && labels::referenced(&source, cursor).is_none()
        && let Some(reply) = {
            graph_reply(
                session,
                config,
                &package_access,
                overrides,
                view,
                &source,
                cursor,
                query,
                disk,
                cancellation,
                client_root,
            )?
        }
    {
        return Ok(reply);
    }
    let result = {
        let selection = Selection::new(&source, cursor, query, check_progress);
        let revision = selection.realize(
            session,
            compilations,
            config,
            source_revision,
            overrides,
            cancellation,
        )?;
        // A site that does not compile still answers about the file the author is editing: that
        // source compiles on its own in the same world, and the answer says what the file alone
        // establishes. The site's diagnostics name what stopped the site itself.
        let alone = match revision.checked() {
            Some(checked) if checked.bundle().is_some() => None,
            _ => {
                // A package document is compiled where its package lives, not below the site root:
                // the alone path must load the file the compiler resolved. An embedded package
                // has no path on disk, so it keeps the no-world answer instead of failing.
                let document = match source.id().root() {
                    VirtualRoot::Package(_) => {
                        crate::identity::package_file_path(source.id(), config.package_locations())
                    }
                    VirtualRoot::Project => Some(
                        config
                            .get_root()
                            .join(source.id().vpath().get_without_slash()),
                    ),
                };
                match document {
                    Some(document) => session
                        .inspect_source(
                            document.to_string_lossy().as_ref(),
                            overrides.to_vec(),
                            cancellation,
                        )?
                        .into_checked(),
                    None => None,
                }
            }
        };
        let checked = alone.as_ref().or_else(|| revision.checked());
        match checked {
            Some(checked) => selection.compiled_reply(
                checked,
                config,
                &package_access,
                bibliographies,
                query,
                routes_as_hints,
                cancellation,
                client_root,
            )?,
            None => match selection.source_only_reply(config, query)? {
                Some(reply) => reply,
                None => unavailable(query),
            },
        }
    };
    cancellation.ensure_active()?;
    Ok(result)
}

pub(super) fn package_source(uri: &lsp_types::Uri) -> Result<PackageSourceText> {
    let id = crate::identity::package_file_id(uri)?;
    let text = crate::identity::embedded_source(id)
        .context("source is not a builtin immutable package file")?;
    Ok(PackageSourceText { text: text.into() })
}

pub(super) fn unavailable(query: &SourceQuery) -> SourceReply {
    match query {
        SourceQuery::Completion(_) => {
            SourceReply::Completion(CompletionResponse::Array(Vec::new()))
        }
        SourceQuery::Hover(_) => SourceReply::Hover(None),
        SourceQuery::Definition(_) => SourceReply::Definition(None),
        SourceQuery::SignatureHelp(_) => SourceReply::SignatureHelp(None),
        SourceQuery::PrepareRename(_) => SourceReply::PrepareRename(None),
        SourceQuery::Rename(_) => SourceReply::Rename(None),
        SourceQuery::References(_) => SourceReply::References(None),
        SourceQuery::DocumentHighlights(_) => SourceReply::DocumentHighlights(None),
        SourceQuery::CodeActions(_) => SourceReply::CodeActions(None),
        SourceQuery::DocumentColors(_) => SourceReply::DocumentColors(None),
        SourceQuery::ColorPresentations(_) => SourceReply::ColorPresentations(None),
        SourceQuery::InlayHints(_) => SourceReply::InlayHints(Vec::new()),
    }
}

/// The completion items as one reply, each stamped with the order its lane answered in.
///
/// A client that ranks equal matches by label alone would otherwise lose the order the lanes
/// chose — the wraps, the names, and the reader's own items — so every item has its place.
fn completion_reply(mut items: Vec<CompletionItem>) -> SourceReply {
    for (index, item) in items.iter_mut().enumerate() {
        item.sort_text = Some(format!("{index:03}"));
    }
    SourceReply::Completion(CompletionResponse::Array(items))
}

impl<'a> Selection<'a> {
    #[expect(
        clippy::too_many_arguments,
        reason = "an answer reads its compilation, config, package access, bibliographies, query, and cancellation"
    )]
    fn compiled_reply(
        &self,
        compilation: &SourceCompilation,
        config: &ResolvedSiteConfig,
        package_access: &PackageAccess<'_>,
        bibliographies: &mut BibliographyCache,
        query: &SourceQuery,
        routes_as_hints: bool,
        cancellation: &BuildCancellation,
        client_root: &crate::uri::ClientRoot,
    ) -> Result<SourceReply> {
        let prepared = compilation
            .world()
            .source(self.source.id())
            .context("prepared source is unavailable")?;
        let mut semantics = Semantic::new(compilation, cancellation);
        Ok(match query {
            SourceQuery::Completion(_) => {
                // A `#tola-meta` dictionary completes the keys the site's schemas declare there.
                if let Some(items) = self.meta_completion(compilation, config, &mut semantics)? {
                    return Ok(completion_reply(items));
                }
                // An argument the site's own declarations constrain to published asset URLs
                // completes with those URLs alone: a name this file holds names no URL.
                if let Some((typed, bytes)) = self.asset_url_argument(&mut semantics)? {
                    let urls = AssetUrls::for_check(config, cancellation)?;
                    return Ok(SourceReply::Completion(crate::assets::url_completions(
                        &urls,
                        config.get_root(),
                        self.source,
                        &typed,
                        bytes,
                        self.cursor,
                    )));
                }
                // A value's own wraps answer before the fields it already has, so a name both
                // spellings could complete reaches the wrap first.
                let mut items = postfix::completions(self.source, self.cursor);
                let mut seen: std::collections::BTreeSet<String> =
                    items.iter().map(|item| item.label.clone()).collect();
                let mut add_unseen = |candidates: Vec<CompletionItem>| {
                    items.extend(
                        candidates
                            .into_iter()
                            .filter(|item| seen.insert(item.label.clone())),
                    );
                };
                add_unseen(self.completion(&prepared, &mut semantics)?);
                if let Some(referenced) = labels::completion(compilation, self.source, self.cursor)
                {
                    add_unseen(referenced);
                    add_unseen(citations::completions(
                        compilation,
                        config,
                        self.source,
                        self.cursor,
                        bibliographies,
                        cancellation,
                    )?);
                }
                // A member position answers the receiver's own records — its members and wraps —
                // never the general names the reader falls back to for an unresolved receiver.
                // Every other position reads the author's own text, whether or not the query
                // copy could compile it.
                if !self.at_code_member()
                    && let Some((_, ordinary)) = editing::completion(
                        compilation.world(),
                        config,
                        package_access,
                        self.source,
                        self.cursor,
                    )
                {
                    // A value's own representation is the editor's line to draw, and a page list
                    // would draw a screenful of it.
                    add_unseen(
                        ordinary
                            .into_iter()
                            .map(|mut item| {
                                item.detail = item.detail.as_deref().map(semantic::abbreviated);
                                item
                            })
                            .collect(),
                    );
                }
                completion_reply(items)
            }
            SourceQuery::Hover(_) => {
                // The first answer that knows the cursor wins: Tola's own names, then the entry a
                // citation reaches, then the site's labels, an asset argument, a reader path, and
                // finally what Typst itself reads at the cursor.
                let hover = self
                    .hover(compilation, config, cancellation, &prepared, &mut semantics)?
                    .or(self.citation(compilation, config, bibliographies, cancellation)?)
                    .or(labels::hover(
                        compilation,
                        config,
                        self.source,
                        self.cursor,
                        cancellation,
                    )?)
                    .or(crate::assets::hover(config, self.source, self.cursor))
                    .or(self.asset_url_hover(config, &mut semantics, cancellation)?)
                    .or(crate::links::hover(
                        compilation,
                        config,
                        self.source,
                        self.cursor,
                        cancellation,
                    )?)
                    .or_else(|| {
                        // The reader's own answer reads the compiled copy: a construct the repair
                        // rewrote has no answer in the author's own text, and the lanes above had their say.
                        self.cursor_keeps_source_text(&prepared)
                            .then(|| {
                                self.builtin_hover(compilation.world(), &prepared, self.cursor)
                            })
                            .flatten()
                    });
                // A box an editor opens to show nothing is worse than no answer: a hover that says
                // nothing at all is no hover.
                SourceReply::Hover(hover.filter(says_something))
            }
            SourceQuery::Definition(_) => {
                // A written asset URL resolves to the file its declaration names, before the
                // ordinary definition search reads the name under the cursor.
                if let Some((_, bytes)) = self.asset_url_argument(&mut semantics)? {
                    let urls = AssetUrls::for_check(config, cancellation)?;
                    if let Some(definition) =
                        crate::assets::written_definition(&urls, client_root, self.source, bytes)
                    {
                        return Ok(SourceReply::Definition(Some(definition)));
                    }
                }
                let referenced = match &self.syntax {
                    SelectedSyntax::Reference(reference) => {
                        let name = &self.source.text()[reference.name.clone()];
                        match labels::definition(compilation, name)
                            .and_then(|span| semantics.location(span, client_root))
                        {
                            Some(location) => Some(GotoDefinitionResponse::Scalar(location)),
                            // A name the site does not label is the entry a bibliography
                            // declares; a label the site cannot place answers nothing here.
                            None if !labels::declares(compilation, name) => citations::definition(
                                compilation,
                                config,
                                self.source.id(),
                                name,
                                bibliographies,
                                cancellation,
                                client_root,
                            )?
                            .map(GotoDefinitionResponse::Scalar),
                            None => None,
                        }
                    }
                    _ => None,
                };
                let name_definition = self.definition(&prepared, &mut semantics, client_root)?;
                let definition = match referenced.or(name_definition) {
                    Some(definition) => Some(definition),
                    // The reader's own search reads the compiled copy: a construct the repair
                    // rewrote has no definition in the author's own text.
                    None if self.cursor_keeps_source_text(&prepared) => {
                        editing::definition(compilation.world(), &prepared, self.cursor)
                            .and_then(|target| semantics.target_location(target, client_root))
                            .map(GotoDefinitionResponse::Scalar)
                    }
                    None => None,
                };
                SourceReply::Definition(definition)
            }
            SourceQuery::SignatureHelp(_) => {
                SourceReply::SignatureHelp(self.signature_help(&prepared, &mut semantics)?)
            }
            SourceQuery::PrepareRename(_) => {
                let prepared = labels::prepared(compilation, self.source, self.cursor);
                SourceReply::PrepareRename(prepared.map(|(range, placeholder)| {
                    PrepareRenameResponse::RangeWithPlaceholder { range, placeholder }
                }))
            }
            SourceQuery::DocumentHighlights(_) => {
                let highlights = match &self.syntax {
                    SelectedSyntax::Reference(reference) => {
                        let name = &self.source.text()[reference.name.clone()];
                        Some(labels::highlights(self.source, name))
                    }
                    // A cursor on a loop's control flow answers the loop it belongs to: its own
                    // keyword and every `break` or `continue` inside, which no compilation is
                    // needed for.
                    _ => {
                        let keywords = tola_typst_syntax::loops::keywords(self.source, self.cursor);
                        (!keywords.is_empty()).then(|| {
                            keywords
                                .into_iter()
                                .filter_map(|range| {
                                    Some(DocumentHighlight {
                                        range: position::utf16_range(self.source.lines(), range)?,
                                        kind: None,
                                    })
                                })
                                .collect()
                        })
                    }
                };
                SourceReply::DocumentHighlights(highlights)
            }
            SourceQuery::CodeActions(params) => SourceReply::CodeActions(self.narrowing(
                &params.text_document.uri,
                &prepared,
                &mut semantics,
            )?),
            SourceQuery::DocumentColors(_) => {
                SourceReply::DocumentColors(Some(colors::document(self.source)?))
            }
            SourceQuery::ColorPresentations(params) => SourceReply::ColorPresentations(Some(
                colors::presentations(params.color, params.range),
            )),
            SourceQuery::InlayHints(params) => {
                let mut hints = hints::hints(
                    compilation,
                    package_access.published,
                    self.source,
                    params.range,
                    cancellation,
                )?;
                if routes_as_hints {
                    hints.extend(hints::route_hints(
                        compilation,
                        config,
                        self.source,
                        params.range,
                        cancellation,
                    )?);
                }
                SourceReply::InlayHints(hints)
            }
            SourceQuery::References(params) => {
                let references = match &self.syntax {
                    SelectedSyntax::Reference(reference) => {
                        let name = &self.source.text()[reference.name.clone()];
                        // A label's spellings are the ones its own documents hold; a citation
                        // key's are the ones any source the site holds spells, since a
                        // bibliography can be cited from any page.
                        let found = if labels::declares(compilation, name) {
                            labels::locations(compilation, name, cancellation, client_root)?
                        } else {
                            citations::references(
                                compilation,
                                config,
                                &citations::CitationRequest {
                                    open: self.source.id(),
                                    key: name,
                                    include_declaration: params.context.include_declaration,
                                },
                                bibliographies,
                                cancellation,
                                client_root,
                            )?
                        };
                        (!found.is_empty()).then_some(found)
                    }
                    _ => None,
                };
                SourceReply::References(references)
            }
            SourceQuery::Rename(params) => {
                let rename = match &self.syntax {
                    SelectedSyntax::Reference(reference) => {
                        let name = &self.source.text()[reference.name.clone()];
                        labels::rename(
                            compilation,
                            name,
                            &params.new_name,
                            cancellation,
                            client_root,
                        )?
                    }
                    _ => None,
                };
                SourceReply::Rename(rename)
            }
        })
    }

    /// The answers a source decides on its own, which a site that does not compile still has.
    ///
    /// Completion answers only its wraps: every name it could offer comes from the compilation.
    fn source_only_reply(
        &self,
        config: &ResolvedSiteConfig,
        query: &SourceQuery,
    ) -> Result<Option<SourceReply>> {
        Ok(Some(match query {
            // An asset argument names an output the site creates, so the answer needs the
            // configuration and the source, not a compilation. Only this site's own file can say
            // why the site has none: a package document belongs to its package, and blaming the
            // site for it would be false.
            SourceQuery::Hover(_) => SourceReply::Hover(
                crate::assets::hover(config, self.source, self.cursor).or_else(|| {
                    matches!(self.source.id().root(), VirtualRoot::Project)
                        .then(|| markdown_hover(self.check_progress.absent().to_owned(), None))
                }),
            ),
            SourceQuery::DocumentColors(_) => {
                SourceReply::DocumentColors(Some(colors::document(self.source)?))
            }
            SourceQuery::ColorPresentations(params) => SourceReply::ColorPresentations(Some(
                colors::presentations(params.color, params.range),
            )),
            SourceQuery::Completion(_) => {
                completion_reply(postfix::completions(self.source, self.cursor))
            }
            _ => return Ok(None),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::request::Request as LspRequest;
    use lsp_types::{
        DocumentHighlight, InlayHint, Location, WorkspaceEdit, request as lsp_request,
    };
    use lsp_types::{Hover, InlayHintLabel, Position, SignatureHelp};
    use std::path::Path;
    use tola_build::config::loading::{BuildOverrides, load_site_config};
    use tola_typst::typst::syntax::Source;
    pub(crate) const ENTRY_PROGRAM: &str = r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/site:0.0.0": site
#import "@tola/address:0.0.0": decode-url-path, route, route-to-output, slugify
#for source in all-sources() {
  let declared = if source.meta == none { none } else { source.meta.at("permalink", default: none) }
  let route = if declared == none {
    route(source.route-segments.map(segment => slugify(segment, language: site.language.lang)))
  } else {
    decode-url-path(declared)
  }
  document(route-to-output(route))[#include source.file]
}
"#;
    pub(crate) const TWO_DOCUMENT_PROGRAM: &str = r#"#import "@tola/source:0.0.0": all-sources
#for source in all-sources() {
  document("a.html")[#include source.file]
  document("b.html")[#include source.file]
}
"#;

    pub(crate) struct SiteDirectory(tempfile::TempDir);
    impl SiteDirectory {
        pub(crate) fn new() -> Self {
            let site = Self(tempfile::tempdir().unwrap());
            site.write("tola.toml", "");
            std::fs::create_dir_all(site.path("content")).unwrap();
            site
        }
        pub(crate) fn root(&self) -> &Path {
            self.0.path()
        }
        pub(crate) fn path(&self, relative: &str) -> PathBuf {
            self.root().join(relative)
        }
        pub(crate) fn write(&self, relative: &str, text: &str) {
            let path = self.path(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    pub(crate) fn load_configuration(root: &Path) -> Result<Arc<ResolvedSiteConfig>> {
        Ok(Arc::new(
            load_site_config(
                Some(&root.join("tola.toml")),
                tola_typst::PackageLocations::from_absolute_roots(None, None)?,
                &BuildOverrides::default(),
            )?
            .into_config(),
        ))
    }

    pub(crate) fn marked_cursor(marked: &str) -> (String, Position) {
        let at = marked.find('|').unwrap();
        let text = marked.replacen('|', "", 1);
        let source = Source::detached(text.clone());
        (
            text,
            position::utf16_range(source.lines(), at..at).unwrap().start,
        )
    }

    pub(crate) fn announcing_client() -> lsp_types::ClientCapabilities {
        lsp_types::ClientCapabilities {
            text_document: Some(lsp_types::TextDocumentClientCapabilities {
                code_lens: Some(lsp_types::CodeLensClientCapabilities::default()),
                inlay_hint: Some(lsp_types::InlayHintClientCapabilities::default()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    pub(crate) struct QuerySession {
        pub(crate) site: SiteDirectory,
        pub(crate) config: Arc<ResolvedSiteConfig>,
        pub(crate) session: tola_build::check::SourceDiagnosticSession,
        pub(crate) compilations: RevisionCompilations,
        /// The site's parsed bibliography files, as one lane reuses them.
        pub(crate) bibliographies: BibliographyCache,
        /// The source revision this session's requests belong to, which a test advances when the
        /// site's files change on disk.
        pub(crate) revision: u64,
        pub(crate) capabilities: lsp_types::ClientCapabilities,
        /// The directory a file-only client reads this site's packages from, when one does.
        pub(crate) package_sources: Option<PathBuf>,
    }

    impl QuerySession {
        pub(crate) fn new() -> Self {
            Self::with_program(ENTRY_PROGRAM)
        }

        pub(crate) fn with_program(program: &str) -> Self {
            let site = SiteDirectory::new();
            site.write("site.typ", program);
            let config = load_configuration(site.root()).expect("site configuration");
            let session = tola_build::check::SourceDiagnosticSession::with_resources(
                Arc::clone(&config),
                tola_build::BuildResources::new()
                    .with_network_access(tola_build::NetworkAccess::Denied)
                    .without_system_fonts(),
            );
            Self {
                site,
                config,
                session,
                compilations: RevisionCompilations::new(),
                bibliographies: BibliographyCache::default(),
                revision: 0,
                capabilities: announcing_client(),
                package_sources: None,
            }
        }

        /// A session whose site declares a published asset URL, so an `asset-url` argument has one
        /// to answer with.
        pub(crate) fn with_published_assets() -> Self {
            let mut session = Self::with_program(ENTRY_PROGRAM);
            session.site.write("brand/logo.svg", "<svg></svg>");
            let configuration = std::fs::read_to_string(session.site.root().join("tola.toml"))
                .expect("the site's configuration");
            session.site.write(
                "tola.toml",
                &format!(
                    "{configuration}\n[assets]\ntrees = [{{ source = \"brand\", url-prefix = \"/brand\" }}]\n"
                ),
            );
            session.config = load_configuration(session.site.root()).expect("site configuration");
            session.session = tola_build::check::SourceDiagnosticSession::with_resources(
                Arc::clone(&session.config),
                tola_build::BuildResources::new()
                    .with_network_access(tola_build::NetworkAccess::Denied)
                    .without_system_fonts(),
            );
            session
        }

        pub(crate) fn two_documents() -> Self {
            Self::with_program(TWO_DOCUMENT_PROGRAM)
        }

        pub(crate) fn hover_text(&mut self, marked: &str) -> Option<String> {
            match self.hover(marked)?.contents {
                lsp_types::HoverContents::Markup(markup) => Some(markup.value),
                lsp_types::HoverContents::Scalar(marked) => match marked {
                    lsp_types::MarkedString::String(text) => Some(text),
                    lsp_types::MarkedString::LanguageString(code) => Some(code.value),
                },
                lsp_types::HoverContents::Array(parts) => Some(
                    parts
                        .into_iter()
                        .map(|part| match part {
                            lsp_types::MarkedString::String(text) => text,
                            lsp_types::MarkedString::LanguageString(code) => code.value,
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            }
        }

        pub(crate) fn completion(&mut self, marked: &str) -> Vec<lsp_types::CompletionItem> {
            match serde_json::from_value::<Option<CompletionResponse>>(
                self.reply(lsp_request::Completion::METHOD, marked),
            )
            .expect("a completion response")
            {
                Some(CompletionResponse::Array(items)) => items,
                Some(CompletionResponse::List(list)) => list.items,
                None => Vec::new(),
            }
        }

        pub(crate) fn hover(&mut self, marked: &str) -> Option<Hover> {
            serde_json::from_value(self.reply(lsp_request::HoverRequest::METHOD, marked))
                .expect("a hover response")
        }

        pub(crate) fn definition(&mut self, marked: &str) -> Option<GotoDefinitionResponse> {
            serde_json::from_value(self.reply(lsp_request::GotoDefinition::METHOD, marked))
                .expect("a definition response")
        }

        pub(crate) fn signature(&mut self, marked: &str) -> Option<SignatureHelp> {
            serde_json::from_value(self.reply(lsp_request::SignatureHelpRequest::METHOD, marked))
                .expect("a signature help response")
        }

        pub(crate) fn inlay_hints(&mut self, marked: &str) -> Vec<InlayHint> {
            let reply = self.reply_with(
                lsp_request::InlayHintRequest::METHOD,
                marked,
                serde_json::json!({
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": u32::MAX, "character": 0 },
                    },
                }),
            );
            serde_json::from_value(reply).expect("an inlay hint reply")
        }

        pub(crate) fn references(
            &mut self,
            marked: &str,
            include_declaration: bool,
        ) -> Option<Vec<Location>> {
            let reply = self.reply_with(
                lsp_request::References::METHOD,
                marked,
                serde_json::json!({ "context": { "includeDeclaration": include_declaration } }),
            );
            serde_json::from_value(reply).expect("a references reply")
        }

        pub(crate) fn highlights(&mut self, marked: &str) -> Option<Vec<DocumentHighlight>> {
            serde_json::from_value(
                self.reply(lsp_request::DocumentHighlightRequest::METHOD, marked),
            )
            .expect("a document highlight reply")
        }

        pub(crate) fn prepare_rename(&mut self, marked: &str) -> Option<PrepareRenameResponse> {
            serde_json::from_value(self.reply(lsp_request::PrepareRenameRequest::METHOD, marked))
                .expect("a prepare rename reply")
        }

        pub(crate) fn rename(&mut self, marked: &str, new_name: &str) -> Option<WorkspaceEdit> {
            let reply = self.reply_with(
                lsp_request::Rename::METHOD,
                marked,
                serde_json::json!({ "newName": new_name }),
            );
            serde_json::from_value(reply).expect("a rename reply")
        }

        /// The rename refusal one name receives, as the protocol kind it has.
        ///
        /// A name that is not one identifier is refused as a failed request, not as an empty
        /// edit, so the caller reads the kind the connection layer projects.
        pub(crate) fn rename_refusal(&mut self, marked: &str, new_name: &str) -> String {
            let error = self
                .rename_error(marked, new_name)
                .expect("the rename is refused");
            let error = error
                .downcast_ref::<crate::analysis::RenameError>()
                .expect("a rename refusal has its kind");
            error.kind().to_owned()
        }

        pub(crate) fn reply(&mut self, method: &str, marked: &str) -> serde_json::Value {
            self.reply_with(method, marked, serde_json::json!({}))
        }

        pub(crate) fn reply_with(
            &mut self,
            method: &str,
            marked: &str,
            extra: serde_json::Value,
        ) -> serde_json::Value {
            self.try_reply_with(method, marked, extra)
                .expect("a completed query")
        }

        /// The failure one request produced, left as the error the connection layer projects.
        pub(crate) fn rename_error(
            &mut self,
            marked: &str,
            new_name: &str,
        ) -> Option<anyhow::Error> {
            self.try_reply_with(
                lsp_request::Rename::METHOD,
                marked,
                serde_json::json!({ "newName": new_name }),
            )
            .err()
        }

        pub(crate) fn try_reply_with(
            &mut self,
            method: &str,
            marked: &str,
            extra: serde_json::Value,
        ) -> anyhow::Result<serde_json::Value> {
            let (text, position) = marked_cursor(marked);
            let path = self.config.build().content_dir.join("document.typ");
            let uri = crate::uri::from_file_path(&path).expect("a content path is addressable");
            self.try_reply_at(method, uri.as_str(), position, Some((path, text)), extra)
        }

        /// The answer to one request about `uri`, with the editor's own text for the source it
        /// names when the caller supplies it.
        pub(crate) fn try_reply_at(
            &mut self,
            method: &str,
            uri: &str,
            position: Position,
            document: Option<(PathBuf, String)>,
            extra: serde_json::Value,
        ) -> anyhow::Result<serde_json::Value> {
            let mut params = serde_json::json!({
                "textDocument": { "uri": uri },
                "position": position,
            });
            if let Some(extra) = extra.as_object() {
                for (key, value) in extra {
                    params[key] = value.clone();
                }
            }
            let query = crate::protocol::SourceQuery::decode(lsp_server::Request {
                id: 1.into(),
                method: method.to_owned(),
                params,
            })
            .expect("a supported source query")
            .1;
            let overrides: Vec<(PathBuf, Arc<str>)> = document
                .map(|(path, text)| (path, Arc::from(text)))
                .into_iter()
                .collect();
            let reply = crate::query::respond(
                &mut self.session,
                &mut self.compilations,
                &mut crate::analysis::DiskSources::default(),
                &mut self.bibliographies,
                &self.config,
                self.revision,
                &overrides,
                &SourceView::default(),
                &[],
                self.package_sources.as_deref(),
                &query,
                crate::connection::routes_as_hints(&self.capabilities),
                CheckProgress::Failed,
                &BuildCancellation::new(),
                &crate::uri::ClientRoot::new(self.config.get_root()),
            )?;
            Ok(serde_json::to_value(reply).expect("a serializable reply"))
        }

        /// The hover answer for one URI, however the document it names is spelled.
        pub(crate) fn hover_at(
            &mut self,
            uri: &str,
            position: Position,
        ) -> anyhow::Result<Option<Hover>> {
            let reply = self.try_reply_at(
                lsp_request::HoverRequest::METHOD,
                uri,
                position,
                None,
                serde_json::json!({}),
            )?;
            Ok(serde_json::from_value(reply).expect("a hover response"))
        }
    }

    pub(crate) fn completion_labels(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.label.as_str()).collect()
    }

    /// The label of every inlay hint, as the text an editor renders.
    pub(crate) fn hint_labels(hints: &[InlayHint]) -> Vec<String> {
        hints
            .iter()
            .map(|hint| match &hint.label {
                InlayHintLabel::String(label) => label.clone(),
                label => format!("{label:?}"),
            })
            .collect()
    }

    pub(crate) fn item_labeled<'a>(items: &'a [CompletionItem], label: &str) -> &'a CompletionItem {
        items
            .iter()
            .find(|item| item.label == label)
            .unwrap_or_else(|| panic!("`{label}` is missing from {:?}", completion_labels(items)))
    }

    pub(crate) fn decode(method: &str, params: serde_json::Value) -> SourceQuery {
        SourceQuery::decode(lsp_server::Request {
            id: 1.into(),
            method: method.into(),
            params,
        })
        .unwrap()
        .1
    }

    /// A URI `respond` cannot address names no source this site holds, and the editor is asking
    /// about a document it shows: the answer is nothing rather than a failed request, so no editor
    /// reports an error for a file outside the site or a package URI with the editor's own
    /// qualifier.
    #[test]
    fn unaddressable_uri_answers_nothing() {
        let mut site = QuerySession::new();
        let sibling = site.site.root().parent().unwrap().join("outside.typ");
        site.site.write(
            ".tola/builtin-packages/tola/site/0.0.0/lib.typ",
            "#let value = 1\n",
        );
        let uris = [
            crate::uri::from_file_path(&sibling).unwrap(),
            "tola-package:/tola/site/0.0.0/lib.typ?site=file%3A%2F%2F%2Fsite"
                .parse()
                .unwrap(),
        ];
        for uri in &uris {
            let answer = site
                .hover_at(uri.as_str(), Position::new(0, 5))
                .expect("an answerable request");
            assert!(answer.is_none(), "{uri:?}");
        }
    }

    /// The scaffold-shaped site program: records parsed against a schema, then filtered.
    pub(crate) fn parsed_program(schema: &str) -> String {
        format!(
            r#"#import "@tola/source:0.0.0": all-sources, parse-sources
#import "@tola/site:0.0.0": site
#import "@tola/address:0.0.0": route, route-to-output, slugify
#import "@tola/schema:0.0.0": describe, optional, schema
#let page-schema = schema((
{schema}
))
#let declared = parse-sources(all-sources(), page-schema)
#let kept = declared.filter(source => not source.meta.draft)
#for source in kept {{
  document(route-to-output(route(source.route-segments.map(segment => slugify(segment, language: site.language.lang)))))[#include source.file]
}}
"#
        )
    }

    /// The scaffold program with a third declared field, so a `#tola-meta` key has a declaration
    /// whose line and description an answer has.
    pub(crate) fn summary_program() -> String {
        parsed_program(&format!(
            "{PAGE_SCHEMA}\n  summary: describe(optional(str), \"the page summary\"),"
        ))
    }

    /// The schema the scaffold writes for the two fields a metadata chain reads.
    pub(crate) const PAGE_SCHEMA: &str = "  title: describe(optional(str, default: \"Untitled\"), \"the page title\"),\n  draft: describe(optional(bool, default: false), \"keeps the page out of the published site\"),";

    /// One content page whose metadata the scaffold's schema resolves, with the binding a chain
    /// points at.
    pub(crate) const SCHEMA_PAGE: &str = r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: "Page"))
#let page = (title: "Page")
"#;

    /// One content page whose own metadata declares every field the chain reads.
    pub(crate) const DECLARED_PAGE: &str = r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: "Page", draft: false))
#let page = (title: "Page")
"#;

    /// A site program whose records flow through a project file's own function: the scaffold's
    /// shape, with the parse and the map in `site/selection.typ` and one entry parameter rendered.
    pub(crate) const MAPPED_PROGRAM: &str = r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/site:0.0.0": site
#import "site/selection.typ": select-pages
#import "site/page.typ": page-template

#let pages = select-pages(all-sources())
#for entry in pages { page-template(entry) }
"#;

    /// The project file the mapped program parses and maps its records in, with the schema and
    /// the record literal an answer derives its fields from.
    pub(crate) fn mapped_pages() -> String {
        format!(
            r#"#import "@tola/address:0.0.0": route, route-to-output, slugify
#import "@tola/schema:0.0.0": describe, optional, schema
#import "@tola/source:0.0.0": parse-sources

#let page-schema = schema((
{PAGE_SCHEMA}
))

#let entry-title(record, language) = {{
  if record.meta.draft {{ "" }} else {{ record.meta.title }}
}}

#let select-pages(sources) = {{
  let declared = parse-sources(sources, page-schema)
  let published = declared.filter(record => not record.meta.draft)
  published.map(record => (
    source: record,
    output: route-to-output(route(record.route-segments.map(segment => slugify(segment, language: "en")))),
    title: entry-title(record, "en"),
  ))
}}
"#
        )
    }

    /// The project file the mapped program renders each entry in.
    pub(crate) const MAPPED_PAGE: &str = r#"#let page-template(page) = document(page.output, format: "html")[#page.source.path]
"#;

    /// One piece of source with the cursor marker placed directly before `at`.
    pub(crate) fn marked_before(text: &str, at: &str) -> String {
        text.replacen(at, &format!("|{at}"), 1)
    }

    /// The reply to one request about the site's own entry program.
    pub(crate) fn site_reply(
        site: &mut QuerySession,
        method: &str,
        marked: &str,
        extra: serde_json::Value,
    ) -> serde_json::Value {
        let (text, position) = marked_cursor(marked);
        let path = site.site.path("site.typ");
        let uri = crate::uri::from_file_path(&path).expect("the site program is addressable");
        site.try_reply_at(method, uri.as_str(), position, Some((path, text)), extra)
            .expect("a completed query")
    }

    /// The hover text for one cursor in the site's own entry program.
    pub(crate) fn site_hover(site: &mut QuerySession, marked: &str) -> Option<String> {
        let hover: Option<Hover> = serde_json::from_value(site_reply(
            site,
            lsp_request::HoverRequest::METHOD,
            marked,
            serde_json::json!({}),
        ))
        .expect("a hover response");
        let lsp_types::HoverContents::Markup(markup) = hover?.contents else {
            panic!("a markdown hover");
        };
        Some(markup.value)
    }

    /// The hover text for one cursor in a site file outside the content root, whose records no
    /// `parse-sources` call covers.
    pub(crate) fn template_hover(
        site: &mut QuerySession,
        relative: &str,
        marked: &str,
    ) -> Option<String> {
        let (text, position) = marked_cursor(marked);
        let path = site.site.path(relative);
        let uri = crate::uri::from_file_path(&path).expect("a site path is addressable");
        let reply = site
            .try_reply_at(
                lsp_request::HoverRequest::METHOD,
                uri.as_str(),
                position,
                Some((path, text)),
                serde_json::json!({}),
            )
            .expect("a completed query");
        let hover: Option<Hover> = serde_json::from_value(reply).expect("a hover response");
        let lsp_types::HoverContents::Markup(markup) = hover?.contents else {
            panic!("a markdown hover");
        };
        Some(markup.value)
    }

    /// The definition for one cursor in the site's own entry program.
    pub(crate) fn site_definition(
        site: &mut QuerySession,
        marked: &str,
    ) -> Option<GotoDefinitionResponse> {
        serde_json::from_value(site_reply(
            site,
            lsp_request::GotoDefinition::METHOD,
            marked,
            serde_json::json!({}),
        ))
        .expect("a definition response")
    }

    /// The code actions for one cursor in the site's own entry program.
    pub(crate) fn site_actions(
        site: &mut QuerySession,
        marked: &str,
        only: &[&str],
    ) -> Vec<serde_json::Value> {
        let (_, position) = marked_cursor(marked);
        let reply = site_reply(
            site,
            lsp_request::CodeActionRequest::METHOD,
            marked,
            serde_json::json!({
                "range": { "start": position, "end": position },
                "context": { "only": only, "diagnostics": [] },
            }),
        );
        reply.as_array().cloned().unwrap_or_default()
    }
}
