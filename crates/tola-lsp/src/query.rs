//! Official value queries and source-only editor requests for one immutable source view.

mod actions;
mod citations;
mod colors;
mod completion;
pub(super) mod config;
mod context;
mod definition;
mod editing;
mod entities;
mod entity_reply;
mod expression;
mod hints;
mod hover;
mod imports;
mod inference;
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
pub(crate) use completion::{
    source_completion_reply, source_import_completion, source_member_completion,
    source_member_graph_needed, source_name_completion, source_name_graph_needed,
    source_name_position,
};
pub(super) use name_graph::source_reply;
pub(super) use package::package_source_definition;
pub(super) use records::origin::field_chain;
pub(super) use repair::pin_source;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use lsp_types::{
    CodeActionParams, ColorPresentationParams, CompletionItem, CompletionResponse,
    DocumentHighlight, GotoDefinitionResponse, InlayHintParams, PrepareRenameResponse,
    ReferenceParams, RenameParams,
};
use tola_build::AssetUrls;
use tola_build::cancellation::BuildCancellation;
use tola_build::check::{SourceCompilation, SourceDiagnosticSession};
use tola_build::config::ResolvedSiteConfig;
use tola_typst::typst::World;
use tola_typst::typst::syntax::{Source, VirtualRoot};
use tola_typst_syntax::names::SourceNames;

use crate::compiler::{EntityQueries, RevisionCompilations};
use crate::packages::PackageAccess;
use crate::position;
use crate::protocol::{CheckProgress, PackageSourceText, SourceQuery, SourceReply, markdown_hover};
use crate::published::PublishedPackage;
use crate::sources::SourceView;

use context::{SelectedSyntax, Selection, load_source};
use hover::says_something;
use name_graph::graph_reply;
use semantic::Semantic;

/// The site one source query answers against: its resolved configuration, the packages the
/// editor may reach, the cancellation that ends the answer, and the client's spelling of the root.
struct QuerySite<'a> {
    configuration: &'a Arc<ResolvedSiteConfig>,
    packages: &'a PackageAccess<'a>,
    cancellation: &'a BuildCancellation,
    client_root: &'a crate::uri::ClientRoot,
}

/// One compiled selection's inputs: the compilation that proves it, its site, and the caches and
/// client view the answer reuses.
struct CompiledAnswer<'a> {
    site: &'a QuerySite<'a>,
    compilation: &'a SourceCompilation,
    bibliographies: &'a mut BibliographyCache,
    query: &'a SourceQuery,
    /// Whether the client renders a page's route as an inlay hint.
    routes_as_hints: bool,
    /// The editor's unsaved text, by path.
    overrides: &'a [(PathBuf, Arc<str>)],
}

/// The source one query addresses, the cursor in it, and the name index the connection built for
/// that source while its text was the one the index read.
struct AddressedSource {
    source: Source,
    cursor: usize,
    indexed_names: Option<Arc<SourceNames>>,
}

/// The source `query` addresses and the cursor in it; `None` when the site holds no such file or
/// the URI names nothing this service serves.
fn addressed_source(
    query: &SourceQuery,
    view: &SourceView,
    config: &ResolvedSiteConfig,
    overrides: &[(PathBuf, Arc<str>)],
    package_access: &PackageAccess<'_>,
) -> Result<Option<AddressedSource>> {
    let (uri, position) = query.document();
    // An unaddressable URI answers nothing; a read the boundary itself could not perform fails.
    let Some(id) = crate::identity::source_id_with_packages(
        uri,
        config.get_root(),
        view.package_sources(),
        &package_access.boundary,
        &package_access.local_packages,
        Some((
            &package_access.project_content,
            &package_access.project_files,
        )),
    )?
    else {
        return Ok(None);
    };
    let (source, indexed_names) = match view.by_id(id) {
        Some(parsed) => (parsed.source.clone(), Some(parsed.names())),
        None => match load_source(id, config, overrides, package_access)? {
            Some(source) => (source, None),
            None => return Ok(None),
        },
    };
    // A position outside the source names nothing, and an editor may ask about any position in a
    let Ok(cursor) = position::byte_offset(source.lines(), position) else {
        return Ok(None);
    };
    Ok(Some(AddressedSource {
        source,
        cursor,
        indexed_names,
    }))
}

/// Answer a semantic request read-only; cancellation stays distinct from an unavailable result.
#[expect(
    clippy::too_many_arguments,
    reason = "an answer reads session state, revision inputs, the query, packages, and cancellation"
)]
pub(super) fn respond(
    session: &mut SourceDiagnosticSession,
    compilations: &mut RevisionCompilations,
    disk: &mut crate::analysis::DiskSources,
    graphs: &mut crate::analysis::GraphCache,
    bibliographies: &mut BibliographyCache,
    entity_queries: &mut EntityQueries,
    config: &Arc<ResolvedSiteConfig>,
    source_revision: u64,
    overrides: &[(PathBuf, Arc<str>)],
    view: &SourceView,
    published: &[PublishedPackage],
    query: &SourceQuery,
    routes_as_hints: bool,
    check_progress: CheckProgress,
    cancellation: &BuildCancellation,
    client_root: &crate::uri::ClientRoot,
) -> Result<SourceReply> {
    cancellation.ensure_active()?;
    let package_access = PackageAccess::prepared(config, session.resources(), published)?;
    let site = QuerySite {
        configuration: config,
        packages: &package_access,
        cancellation,
        client_root,
    };
    let Some(addressed) = addressed_source(query, view, config, overrides, &package_access)? else {
        return Ok(unavailable(query));
    };
    let source = addressed.source;
    let cursor = addressed.cursor;
    let indexed_names = addressed.indexed_names;
    // A comment, a raw block, and a `///` template answer before the world is read.
    if let SourceQuery::Completion(_) = query
        && let Some(reply) = completion::source_completion_reply(&source, cursor)
    {
        return Ok(reply);
    }
    if let Some(reply) = source_reply(&source, query)? {
        return Ok(reply);
    }
    // An import path is neither a name nor a label, so the package lane answers it before the
    // name graph builds over the source set.
    if let Some(reply) = package::respond(
        config,
        &package_access,
        &source,
        cursor,
        query,
        client_root,
        cancellation,
        overrides,
        indexed_names.as_deref(),
    )? {
        return Ok(reply);
    }
    if let Some(reply) = entity_reply::respond(entity_reply::EntityQuery {
        site: &site,
        source: &source,
        cursor,
        query,
        cache: entity_queries,
        resources: session.resources(),
        revision: source_revision,
        overlays: overrides,
        view,
        disk: &mut *disk,
    })? {
        return Ok(reply);
    }
    if query.answered_from_name_graph()
        && !completion::opaque_at(&source, cursor)
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
                graphs,
                source_revision,
                cancellation,
                client_root,
            )?
        }
    {
        return Ok(reply);
    }
    let selection = Selection::new(&source, cursor, query, check_progress, indexed_names);
    let revision = selection.realize(
        session,
        compilations,
        config,
        &package_access,
        source_revision,
        overrides,
        cancellation,
    )?;
    // A site that does not compile still answers from the file alone, in the same world.
    let alone = match revision.checked() {
        Some(checked) if checked.bundle().is_some() => None,
        // Preserve the package identity when a mutable library is checked alone. Embedded sources
        // retain the world of the site check.
        _ if repair::editable_source_path(&source, config, &package_access).is_some() => session
            .inspect_source_file(source.id(), overrides.to_vec(), cancellation)?
            .into_checked(),
        _ => None,
    };
    let checked = alone.as_ref().or_else(|| revision.checked());
    if checked.is_some_and(|compiled| {
        compiled.world().local_packages().as_ref() != package_access.local_packages.as_ref()
    }) {
        return Err(tola_build::cancellation::BuildCancelled.into());
    }
    let reply = match checked {
        Some(checked) => selection.compiled_reply(&mut CompiledAnswer {
            site: &site,
            compilation: checked,
            bibliographies: &mut *bibliographies,
            query,
            routes_as_hints,
            overrides,
        })?,
        None => selection
            .source_only_reply(config, query)?
            .unwrap_or_else(|| unavailable(query)),
    };
    cancellation.ensure_active()?;
    Ok(reply)
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

fn completion_reply(mut items: Vec<CompletionItem>) -> SourceReply {
    crate::completion::preserve_order(&mut items);
    SourceReply::Completion(CompletionResponse::Array(items))
}

impl<'a> Selection<'a> {
    /// The name the selected reference writes, when the cursor stands on one.
    fn referenced_name(&self) -> Option<&str> {
        let SelectedSyntax::Reference(reference) = &self.syntax else {
            return None;
        };
        Some(&self.source.text()[reference.name.clone()])
    }

    /// The one answer this selection's compiled world gives, one query category at a time.
    fn compiled_reply(&self, answer: &mut CompiledAnswer<'_>) -> Result<SourceReply> {
        let prepared = answer
            .compilation
            .world()
            .source(self.source.id())
            .context("prepared source is unavailable")?;
        let compilation = answer.compilation;
        let mut semantics = Semantic::new(compilation, answer.site.cancellation);
        Ok(match answer.query {
            SourceQuery::Completion(_) => {
                self.compiled_completion(answer, &prepared, &mut semantics)?
            }
            SourceQuery::Hover(_) => self.compiled_hover(answer, &prepared, &mut semantics)?,
            SourceQuery::Definition(_) => {
                self.compiled_definition(answer, &prepared, &mut semantics)?
            }
            SourceQuery::SignatureHelp(_) => {
                self.compiled_signature_help(&prepared, &mut semantics)?
            }
            SourceQuery::PrepareRename(_) => self.compiled_prepare_rename(compilation),
            SourceQuery::Rename(params) => self.compiled_rename(answer, params)?,
            SourceQuery::References(params) => self.compiled_references(answer, params)?,
            SourceQuery::DocumentHighlights(_) => self.compiled_highlights(),
            SourceQuery::CodeActions(params) => {
                self.compiled_code_actions(params, &prepared, &mut semantics)?
            }
            SourceQuery::DocumentColors(_) => self.compiled_document_colors()?,
            SourceQuery::ColorPresentations(params) => self.compiled_color_presentations(params),
            SourceQuery::InlayHints(params) => self.compiled_inlay_hints(answer, params)?,
        })
    }

    fn compiled_definition(
        &self,
        answer: &mut CompiledAnswer<'_>,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
    ) -> Result<SourceReply> {
        let compilation = answer.compilation;
        let configuration = answer.site.configuration;
        let cancellation = answer.site.cancellation;
        let client_root = answer.site.client_root;
        // A written asset URL resolves to the file its declaration names, before the ordinary
        if let Some((_, bytes)) = self.asset_url_argument(semantics)? {
            let urls = AssetUrls::for_check(configuration, cancellation)?;
            if let Some(definition) =
                crate::assets::written_definition(&urls, client_root, self.source, bytes)
            {
                return Ok(SourceReply::Definition(Some(definition)));
            }
        }
        let referenced = match self.referenced_name() {
            Some(name) => match labels::definition(compilation, name)
                .and_then(|span| semantics.location(span, client_root))
            {
                Some(location) => Some(GotoDefinitionResponse::Scalar(location)),
                // A name the site does not label is the entry a bibliography declares.
                None if !labels::declares(compilation, name) => citations::definition(
                    compilation,
                    configuration,
                    self.source.id(),
                    name,
                    answer.bibliographies,
                    cancellation,
                    client_root,
                )?
                .map(GotoDefinitionResponse::Scalar),
                None => None,
            },
            None => None,
        };
        let name_definition = self.definition(prepared, semantics, client_root)?;
        let definition = match referenced.or(name_definition) {
            Some(definition) => Some(definition),
            // The reader's own search reads the compiled copy: a construct the repair rewrote has
            None if self.cursor_keeps_source_text(prepared) => {
                editing::definition(compilation.world(), prepared, self.cursor)
                    .and_then(|target| semantics.target_location(target, client_root))
                    .map(GotoDefinitionResponse::Scalar)
            }
            None => None,
        };
        Ok(SourceReply::Definition(definition))
    }

    fn compiled_signature_help(
        &self,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
    ) -> Result<SourceReply> {
        Ok(SourceReply::SignatureHelp(
            self.signature_help(prepared, semantics)?,
        ))
    }

    fn compiled_prepare_rename(&self, compilation: &SourceCompilation) -> SourceReply {
        let range_with_placeholder = labels::prepared(compilation, self.source, self.cursor);
        SourceReply::PrepareRename(range_with_placeholder.map(|(range, placeholder)| {
            PrepareRenameResponse::RangeWithPlaceholder { range, placeholder }
        }))
    }

    fn compiled_references(
        &self,
        answer: &mut CompiledAnswer<'_>,
        params: &ReferenceParams,
    ) -> Result<SourceReply> {
        let compilation = answer.compilation;
        let configuration = answer.site.configuration;
        let cancellation = answer.site.cancellation;
        let client_root = answer.site.client_root;
        let references = match self.referenced_name() {
            // A label's spellings stay in its own documents; a citation key's span sources.
            Some(name) => {
                let found = if labels::declares(compilation, name) {
                    labels::locations(
                        compilation,
                        name,
                        params.context.include_declaration,
                        cancellation,
                        client_root,
                    )?
                } else {
                    citations::references(
                        compilation,
                        configuration,
                        &citations::CitationRequest {
                            open: self.source.id(),
                            key: name,
                            include_declaration: params.context.include_declaration,
                        },
                        answer.bibliographies,
                        cancellation,
                        client_root,
                    )?
                };
                (!found.is_empty()).then_some(found)
            }
            None => None,
        };
        Ok(SourceReply::References(references))
    }

    fn compiled_highlights(&self) -> SourceReply {
        let highlights = match self.referenced_name() {
            Some(name) => Some(labels::highlights(self.source, name)),
            // A cursor on a loop's control flow answers its own keywords, without a world.
            None => {
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

    fn compiled_code_actions(
        &self,
        params: &CodeActionParams,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
    ) -> Result<SourceReply> {
        Ok(SourceReply::CodeActions(self.narrowing(
            &params.text_document.uri,
            prepared,
            semantics,
        )?))
    }

    fn compiled_document_colors(&self) -> Result<SourceReply> {
        Ok(SourceReply::DocumentColors(Some(colors::document(
            self.source,
        )?)))
    }

    fn compiled_color_presentations(&self, params: &ColorPresentationParams) -> SourceReply {
        SourceReply::ColorPresentations(Some(colors::presentations(
            self.source,
            params.color,
            params.range,
        )))
    }

    fn compiled_inlay_hints(
        &self,
        answer: &CompiledAnswer<'_>,
        params: &InlayHintParams,
    ) -> Result<SourceReply> {
        let compilation = answer.compilation;
        let cancellation = answer.site.cancellation;
        let mut hints = hints::hints(
            compilation,
            answer.site.packages.published,
            self.source,
            params.range,
            cancellation,
        )?;
        if answer.routes_as_hints {
            hints.extend(hints::route_hints(
                compilation,
                answer.site.configuration,
                self.source,
                params.range,
                cancellation,
            )?);
        }
        Ok(SourceReply::InlayHints(hints))
    }

    fn compiled_rename(
        &self,
        answer: &CompiledAnswer<'_>,
        params: &RenameParams,
    ) -> Result<SourceReply> {
        let rename = match self.referenced_name() {
            Some(name) => labels::rename(
                answer.compilation,
                name,
                &params.new_name,
                answer.site.cancellation,
                answer.site.client_root,
            )?,
            None => None,
        };
        Ok(SourceReply::Rename(rename))
    }

    /// The first lane that knows the cursor wins; Typst's own read comes last.
    fn compiled_hover(
        &self,
        answer: &mut CompiledAnswer<'_>,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
    ) -> Result<SourceReply> {
        let compilation = answer.compilation;
        let configuration = answer.site.configuration;
        let cancellation = answer.site.cancellation;
        let mut hover = self.hover(
            compilation,
            configuration,
            cancellation,
            prepared,
            semantics,
        )?;
        if hover.is_none() {
            hover = self.citation(
                compilation,
                configuration,
                answer.bibliographies,
                cancellation,
            )?;
        }
        if hover.is_none() {
            hover = labels::hover(
                compilation,
                configuration,
                self.source,
                self.cursor,
                cancellation,
            )?;
        }
        if hover.is_none() {
            hover = crate::assets::hover(configuration, self.source, self.cursor);
        }
        if hover.is_none() {
            hover = self.asset_url_hover(configuration, semantics, cancellation)?;
        }
        if hover.is_none() {
            hover = crate::links::hover(
                compilation,
                configuration,
                self.source,
                self.cursor,
                cancellation,
            )?;
        }
        // The reader's own answer reads the compiled copy: a repair-rewritten construct has none.
        let hover = hover.or_else(|| {
            self.cursor_keeps_source_text(prepared)
                .then(|| self.builtin_hover(compilation.world(), prepared, self.cursor))
                .flatten()
        });
        Ok(SourceReply::Hover(hover.filter(says_something)))
    }

    /// The completion one compiled selection answers: the site's own keys and asset URLs first,
    /// then the names and wraps the authored text can hold.
    fn compiled_completion(
        &self,
        answer: &mut CompiledAnswer<'_>,
        prepared: &Source,
        semantics: &mut Semantic<'_>,
    ) -> Result<SourceReply> {
        let compilation = answer.compilation;
        let configuration = answer.site.configuration;
        let cancellation = answer.site.cancellation;
        // A `#tola-meta` dictionary completes the keys the site's schemas declare there.
        if let Some(items) = self.meta_completion(compilation, configuration, semantics)? {
            return Ok(completion_reply(items));
        }
        // An argument the site's own declarations constrain to published asset URLs
        // completes with those URLs alone: a name this file holds names no URL.
        if let Some((typed, bytes)) = self.asset_url_argument(semantics)? {
            let urls = AssetUrls::for_check(configuration, cancellation)?;
            return Ok(SourceReply::Completion(crate::assets::url_completions(
                &urls,
                configuration.get_root(),
                self.source,
                &typed,
                bytes,
                self.cursor,
            )));
        }
        // A plain string or raw block has no names; only the lanes above (paths, asset
        // URLs, metadata keys) answer there.
        if completion::opaque_at(self.source, self.cursor) {
            return Ok(completion_reply(Vec::new()));
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
        add_unseen(self.completion(prepared, semantics)?);
        if let Some(referenced) = labels::completion(compilation, self.source, self.cursor) {
            add_unseen(referenced);
            add_unseen(citations::completions(
                compilation,
                configuration,
                self.source,
                self.cursor,
                answer.bibliographies,
                cancellation,
            )?);
        }
        // A member position answers only the receiver's records, an import list only its exports.
        if !self.at_code_member()
            && !matches!(self.syntax, SelectedSyntax::Import(_))
            && let Some((_, ordinary)) = editing::completion(
                compilation.world(),
                configuration,
                answer.site.packages,
                self.source,
                self.cursor,
                cancellation,
                answer.overrides,
            )?
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
        Ok(completion_reply(items))
    }

    /// The answers a source decides alone: completion offers only wraps here.
    fn source_only_reply(
        &self,
        config: &ResolvedSiteConfig,
        query: &SourceQuery,
    ) -> Result<Option<SourceReply>> {
        Ok(Some(match query {
            // An asset argument needs configuration, not a compilation; only a site file blames.
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
                colors::presentations(self.source, params.color, params.range),
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

    #[test]
    fn large_completions_keep_rank() {
        let candidates = (0..=1000)
            .map(|index| CompletionItem {
                label: format!("candidate-{}", 1000 - index),
                ..CompletionItem::default()
            })
            .collect();
        let SourceReply::Completion(CompletionResponse::Array(mut candidates)) =
            completion_reply(candidates)
        else {
            panic!("completion list");
        };
        candidates.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        assert_eq!(candidates.last().unwrap().label, "candidate-0");
    }
    use lsp_types::request::Request as LspRequest;
    use lsp_types::{CompletionTextEdit, Hover, InlayHintLabel, Position, SignatureHelp};
    use lsp_types::{
        DocumentHighlight, InlayHint, Location, WorkspaceEdit, request as lsp_request,
    };
    /// A site whose evaluation stops before it includes the page being edited.
    pub(crate) const FAILED_PROGRAM: &str = r#"#panic("early")
#document("index.html", format: "html")[#include "content/document.typ"]
"#;

    pub(crate) use crate::server::tests::fixtures::ENTRY_PROGRAM;
    pub(crate) const TWO_DOCUMENT_PROGRAM: &str = r#"#import "@tola/source:0.0.0": all-sources
#for source in all-sources() {
  document("a.html")[#include source.file]
  document("b.html")[#include source.file]
}
"#;

    pub(crate) use crate::server::tests::fixtures::{
        SiteDirectory, announcing_client, marked_cursor, site_configuration as load_configuration,
    };

    /// The text a completion inserts, whichever edit shape it carries.
    pub(crate) fn insertion_text(completion: &CompletionItem) -> &str {
        match completion
            .text_edit
            .as_ref()
            .expect("a completion carries its edit")
        {
            CompletionTextEdit::Edit(edit) => &edit.new_text,
            CompletionTextEdit::InsertAndReplace(edit) => &edit.new_text,
        }
    }

    /// A site root in the spelling the compiler resolves it to.
    ///
    /// The world locates a site's files through the filesystem, so a lane that turns a path into a
    /// file id must read the root this way: macOS reports `/var` for `/private/var`, and an answer
    /// keyed by the temporary directory's own spelling misses.
    pub(crate) fn resolved_root(site: &SiteDirectory) -> PathBuf {
        tola_build::filesystem::normalize_existing_prefix(site.root())
    }

    pub(crate) struct QuerySession {
        pub(crate) site: SiteDirectory,
        pub(crate) config: Arc<ResolvedSiteConfig>,
        pub(crate) session: tola_build::check::SourceDiagnosticSession,
        pub(crate) compilations: RevisionCompilations,
        /// The site's parsed bibliography files, as one lane reuses them.
        pub(crate) bibliographies: BibliographyCache,
        pub(crate) entity_queries: EntityQueries,
        /// The source revision this session's requests belong to, which a test advances when the
        /// site's files change on disk.
        pub(crate) revision: u64,
        pub(crate) capabilities: lsp_types::ClientCapabilities,
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
                entity_queries: EntityQueries::default(),
                revision: 0,
                capabilities: announcing_client(),
            }
        }

        pub(crate) fn reload_with(&mut self, resources: tola_build::BuildResources) {
            self.config = load_configuration(self.site.root()).expect("site configuration");
            self.session = tola_build::check::SourceDiagnosticSession::with_resources(
                Arc::clone(&self.config),
                resources,
            );
        }

        pub(crate) fn reload(&mut self) {
            self.reload_with(
                tola_build::BuildResources::new()
                    .with_network_access(tola_build::NetworkAccess::Denied)
                    .without_system_fonts(),
            );
        }

        /// A session whose site declares a published asset URL, so a `asset-url` argument has one
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
            session.reload();
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
                .downcast_ref::<crate::analysis::NameQueryError>()
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
                &mut crate::analysis::GraphCache::default(),
                &mut self.bibliographies,
                &mut self.entity_queries,
                &self.config,
                self.revision,
                &overrides,
                &crate::sources::OpenSources::new(self.config.get_root()).view(),
                &[],
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

    pub(crate) fn published(name: &str, version: &str) -> PublishedPackage {
        PublishedPackage {
            name: name.to_owned(),
            version: version.to_owned(),
            description: None,
            homepage: None,
            repository: None,
        }
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
            ".tola/mirrors/builtin-packages/tola/site/0.0.0/lib.typ",
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
#import "@tola/address:0.0.0": join-route, output-of, slugify
#import "@tola/schema:0.0.0": describe, optional, schema
#let page-schema = schema((
{schema}
))
#let declared = parse-sources(all-sources(), page-schema)
#let kept = declared.filter(source => not source.meta.draft)
#for source in kept {{
  document(output-of(join-route(source.route-segments.map(segment => slugify(segment, language: site.language.lang)))))[#include source.file]
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

    mod local_packages {
        use std::{fs, path::Path};

        use lsp_types::{GotoDefinitionResponse, Position, Range, Uri};
        use serde_json::{Value, json};
        use tola_build::cancellation::BuildCancellation;
        use tola_typst::typst::syntax::VirtualRoot;

        use super::{QuerySession, decode, marked_cursor};
        use crate::protocol::CheckProgress;
        use crate::sources::OpenSources;

        fn open(sources: &mut OpenSources, uri: &Uri, text: &str) {
            sources
                .apply(lsp_server::Notification::new(
                    "textDocument/didOpen".into(),
                    json!({
                        "textDocument": {"uri": uri, "languageId": "typst", "version": 1, "text": text},
                    }),
                ))
                .unwrap();
        }

        fn change(sources: &mut OpenSources, uri: &Uri, version: i32, text: &str) {
            sources
                .apply(lsp_server::Notification::new(
                    "textDocument/didChange".into(),
                    json!({
                        "textDocument": {"uri": uri, "version": version},
                        "contentChanges": [{"text": text}],
                    }),
                ))
                .unwrap();
        }

        fn ask(
            site: &mut QuerySession,
            sources: &OpenSources,
            uri: &Uri,
            position: Position,
            method: &str,
        ) -> Value {
            let query = decode(
                method,
                json!({"textDocument": {"uri": uri}, "position": position}),
            );
            let reply = crate::query::respond(
                &mut site.session,
                &mut site.compilations,
                &mut crate::analysis::DiskSources::default(),
                &mut crate::analysis::GraphCache::default(),
                &mut site.bibliographies,
                &mut crate::compiler::EntityQueries::default(),
                &site.config,
                sources.revision(),
                sources.overrides(),
                &sources.view(),
                &[],
                &query,
                false,
                CheckProgress::Failed,
                &BuildCancellation::default(),
                &crate::uri::ClientRoot::new(site.config.get_root()),
            )
            .unwrap();
            serde_json::to_value(reply).unwrap()
        }

        fn definition(value: Value) -> (Uri, Range) {
            let response: Option<GotoDefinitionResponse> = serde_json::from_value(value).unwrap();
            match response.expect("the definition is available") {
                GotoDefinitionResponse::Scalar(location) => (location.uri, location.range),
                GotoDefinitionResponse::Array(locations) => {
                    let location = locations.into_iter().next().unwrap();
                    (location.uri, location.range)
                }
                GotoDefinitionResponse::Link(locations) => {
                    let location = locations.into_iter().next().unwrap();
                    (location.target_uri, location.target_selection_range)
                }
            }
        }

        fn configured(module: &Path) -> QuerySession {
            install(QuerySession::new(), module)
        }

        fn install(mut site: QuerySession, module: &Path) -> QuerySession {
            fs::write(
                module.join("tola-module.toml"),
                "schema-version=1\nname='notes'\nversion='1.0.0'\n[typst]\nentry='lib.typ'\n",
            )
            .unwrap();
            let old = fs::read_to_string(site.site.root().join("tola.toml")).unwrap();
            let path = serde_json::to_string(&module.to_string_lossy()).unwrap();
            site.site.write(
                "tola.toml",
                &format!("{old}\n[modules.notes]\npath={path}\n"),
            );
            site.reload_with(
                tola_build::BuildResources::new().with_input_scope(tola_build::InputScope::Offline),
            );
            site
        }

        #[test]
        fn module_library_keeps_root() {
            let site = QuerySession::new();
            let module = site.site.root().join("theme");
            fs::create_dir(&module).unwrap();
            let (library, _) = marked_cursor(
                "#import \"@tola/module:0.0.0\": get-module\n#let context-name = get-module().name\n#assert.eq(context-name, \"notes\")\n#let greet() = context-name\n#context-na|me",
            );
            fs::write(module.join("lib.typ"), &library).unwrap();
            let mut site = install(site, &module);
            let project = "#import \"@tola/source:0.0.0\": current-source\n#assert.eq(current-source().path, \"document.typ\")\n#import \"@tola-modules/notes:0.0.0\": greet\n#gre|et()";
            let (project, project_cursor) = marked_cursor(project);
            site.site.write("content/document.typ", &project);
            let library_uri = crate::uri::from_file_path(&module.join("lib.typ")).unwrap();
            let project_uri =
                crate::uri::from_file_path(&site.config.build().content_dir.join("document.typ"))
                    .unwrap();
            let access = crate::packages::PackageAccess::prepared(
                &site.config,
                site.session.resources(),
                &[],
            )
            .unwrap();
            let id = |uri| {
                crate::identity::source_id_with_packages(
                    uri,
                    site.config.get_root(),
                    None,
                    &access.boundary,
                    &access.local_packages,
                    Some((&access.project_content, &access.project_files)),
                )
                .unwrap()
                .unwrap()
            };
            let library_id = id(&library_uri);
            assert!(matches!(library_id.root(), VirtualRoot::Package(_)));
            assert_eq!(id(&project_uri).root(), &VirtualRoot::Project);
            let mut sources = OpenSources::new(site.config.get_root());
            open(&mut sources, &library_uri, &library);
            open(&mut sources, &project_uri, &project);
            let cancellation = BuildCancellation::default();
            let checked = site
                .session
                .inspect(sources.overrides().to_vec(), &cancellation)
                .unwrap();
            assert!(checked.compiled(), "{:?}", checked.diagnostics());
            assert!(
                checked.diagnostics().is_empty(),
                "{:?}",
                checked.diagnostics()
            );
            let alone = site
                .session
                .inspect_source_file(library_id, sources.overrides().to_vec(), &cancellation)
                .unwrap();
            assert!(alone.compiled(), "{:?}", alone.diagnostics());
            assert!(alone.diagnostics().is_empty(), "{:?}", alone.diagnostics());
            let (target, _) = definition(ask(
                &mut site,
                &sources,
                &project_uri,
                project_cursor,
                "textDocument/definition",
            ));
            assert_eq!(
                crate::uri::to_site_path(target.as_str()).unwrap(),
                module.canonicalize().unwrap().join("lib.typ")
            );
        }

        #[test]
        fn module_queries_follow_unsaved_buffers() {
            let module = tempfile::tempdir().unwrap();
            let disk = "#let greeting = \"saved\"\n#let greet() = greeting\n";
            fs::write(module.path().join("lib.typ"), disk).unwrap();
            let mut site = configured(module.path());
            let (project_text, project_cursor) =
                marked_cursor("#import \"@tola-modules/notes:0.0.0\": greet\n#gr|eet()");
            site.site.write("content/document.typ", &project_text);
            let project_uri =
                crate::uri::from_file_path(&site.config.build().content_dir.join("document.typ"))
                    .unwrap();
            let mut sources = OpenSources::new(site.config.get_root());
            open(&mut sources, &project_uri, &project_text);
            let (library_uri, saved_range) = definition(ask(
                &mut site,
                &sources,
                &project_uri,
                project_cursor,
                "textDocument/definition",
            ));
            assert_eq!(
                crate::uri::to_site_path(library_uri.as_str()).unwrap(),
                module.path().canonicalize().unwrap().join("lib.typ")
            );
            assert_eq!(saved_range.start.line, 1);
            open(&mut sources, &library_uri, disk);

            let unsaved = "// editor buffer\n#let greeting = \"unsaved\"\n#let greet() = greeting\n#let buffer-only = 42\n";
            change(&mut sources, &library_uri, 2, unsaved);
            let (target, updated_range) = definition(ask(
                &mut site,
                &sources,
                &project_uri,
                project_cursor,
                "textDocument/definition",
            ));
            assert_eq!(target, library_uri);
            assert_eq!(
                updated_range.start.line, 2,
                "navigation must use the editor's source lines"
            );

            let (editing, cursor) = marked_cursor(&format!("{unsaved}#buff|"));
            change(&mut sources, &library_uri, 3, &editing);
            let completed = ask(
                &mut site,
                &sources,
                &library_uri,
                cursor,
                "textDocument/completion",
            );
            let completion: Option<lsp_types::CompletionResponse> =
                serde_json::from_value(completed).unwrap();
            let items = match completion.unwrap() {
                lsp_types::CompletionResponse::Array(items) => items,
                lsp_types::CompletionResponse::List(list) => list.items,
            };
            assert!(
                items.iter().any(|item| item.label == "buffer-only"),
                "{items:?}"
            );

            let (editing, cursor) = marked_cursor(&format!("{unsaved}#gr|eet()"));
            change(&mut sources, &library_uri, 4, &editing);
            let (target, range) = definition(ask(
                &mut site,
                &sources,
                &library_uri,
                cursor,
                "textDocument/definition",
            ));
            assert_eq!(target, library_uri);
            assert_eq!(range.start.line, 2);
            assert_eq!(
                fs::read_to_string(module.path().join("lib.typ")).unwrap(),
                disk
            );
        }
    }
}
