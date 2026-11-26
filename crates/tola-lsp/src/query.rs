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
#[cfg(test)]
mod tests;

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
    CompletionItem, CompletionResponse, DocumentHighlight, GotoDefinitionResponse, InlayHintParams,
    PrepareRenameResponse, ReferenceParams, RenameParams,
};
use tola_build::AssetUrls;
use tola_build::cancellation::BuildCancellation;
use tola_build::check::{SourceCompilation, SourceDiagnosticSession};
use tola_build::config::SiteConfig;
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

struct QuerySite<'a> {
    cfg: &'a Arc<SiteConfig>,
    pkgs: &'a PackageAccess<'a>,
    cancel: &'a BuildCancellation,
    client_root: &'a crate::uri::ClientRoot,
}

struct CompiledAnswer<'a> {
    site: &'a QuerySite<'a>,
    comp: &'a SourceCompilation,
    biblios: &'a mut BibliographyCache,
    query: &'a SourceQuery,
    routes_as_hints: bool,
    overrides: &'a [(PathBuf, Arc<str>)],
}

struct AddressedSource {
    src: Source,
    pos: usize,
    names: Option<Arc<SourceNames>>,
}

fn addressed_source(
    query: &SourceQuery,
    view: &SourceView,
    cfg: &SiteConfig,
    overrides: &[(PathBuf, Arc<str>)],
    pkgs: &PackageAccess<'_>,
) -> Result<Option<AddressedSource>> {
    let (uri, pos) = query.document();
    // Unaddressable URI answers nothing; boundary/read failure still errors.
    let Some(id) = crate::identity::source_id_with_packages(
        uri,
        cfg.root(),
        view.package_sources(),
        &pkgs.boundary,
        &pkgs.local_packages,
        Some((&pkgs.project_content, &pkgs.project_files)),
    )?
    else {
        return Ok(None);
    };
    let (src, names) = match view.by_id(id) {
        Some(parsed) => (parsed.source.clone(), Some(parsed.names())),
        None => match load_source(id, cfg, overrides, pkgs)? {
            Some(src) => (src, None),
            None => return Ok(None),
        },
    };
    let Ok(pos) = position::byte_offset(src.lines(), pos) else {
        return Ok(None);
    };
    Ok(Some(AddressedSource { src, pos, names }))
}

/// One semantic answer's session state, revision inputs, query, packages, and cancellation.
pub(super) struct QueryRequest<'a> {
    pub(super) session: &'a mut SourceDiagnosticSession,
    pub(super) compilations: &'a mut RevisionCompilations,
    pub(super) disk: &'a mut crate::analysis::DiskSources,
    pub(super) graphs: &'a mut crate::analysis::GraphCache,
    pub(super) bibliographies: &'a mut BibliographyCache,
    pub(super) entity_queries: &'a mut EntityQueries,
    pub(super) config: &'a Arc<SiteConfig>,
    pub(super) source_revision: u64,
    pub(super) overrides: &'a [(PathBuf, Arc<str>)],
    pub(super) view: &'a SourceView,
    pub(super) published: &'a [PublishedPackage],
    pub(super) query: &'a SourceQuery,
    pub(super) routes_as_hints: bool,
    pub(super) check_progress: CheckProgress,
    pub(super) cancellation: &'a BuildCancellation,
    pub(super) client_root: &'a crate::uri::ClientRoot,
}

/// Answer a semantic request read-only; cancellation stays distinct from an unavailable result.
pub(super) fn respond(req: QueryRequest<'_>) -> Result<SourceReply> {
    let QueryRequest {
        session,
        compilations,
        disk,
        graphs,
        bibliographies: biblios,
        entity_queries,
        config: cfg,
        source_revision,
        overrides,
        view,
        published,
        query,
        routes_as_hints,
        check_progress,
        cancellation: cancel,
        client_root,
    } = req;
    cancel.ensure_active()?;
    let pkgs = PackageAccess::prepared(cfg, session.resources(), published)?;
    let site = QuerySite {
        cfg,
        pkgs: &pkgs,
        cancel,
        client_root,
    };
    let Some(addressed) = addressed_source(query, view, cfg, overrides, &pkgs)? else {
        return Ok(unavailable(query));
    };
    let (src, pos, names) = (addressed.src, addressed.pos, addressed.names);
    if let SourceQuery::Completion(_) = query
        && let Some(reply) = completion::source_completion_reply(&src, pos)
    {
        return Ok(reply);
    }
    if let Some(reply) = source_reply(&src, query)? {
        return Ok(reply);
    }
    if let Some(reply) = package::respond(package::PackageQuery {
        config: cfg,
        package_access: &pkgs,
        source: &src,
        cursor: pos,
        query,
        client_root,
        cancellation: cancel,
        overrides,
        indexed_names: names.as_deref(),
    })? {
        return Ok(reply);
    }
    if let Some(reply) = entity_reply::respond(entity_reply::EntityQuery {
        site: &site,
        source: &src,
        cursor: pos,
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
        && !completion::opaque_at(&src, pos)
        && labels::referenced(&src, pos).is_none()
        && let Some(reply) = graph_reply(name_graph::GraphReplyRequest {
            session: &mut *session,
            config: cfg,
            package_access: &pkgs,
            overrides,
            view,
            source: &src,
            cursor: pos,
            query,
            disk,
            graphs,
            revision: source_revision,
            cancellation: cancel,
            client_root,
        })?
    {
        return Ok(reply);
    }
    let selection = Selection::new(&src, pos, query, check_progress, names);
    let revision = selection.realize(context::Realize {
        session: &mut *session,
        compilations,
        config: cfg,
        package_access: &pkgs,
        source_revision,
        overrides,
        cancellation: cancel,
    })?;
    // Fall back to single-file check when site does not compile; embedded sources retain site world.
    let alone = match revision.checked() {
        Some(checked) if checked.bundle().is_some() => None,
        _ if repair::editable_source_path(&src, cfg, &pkgs).is_some() => session
            .inspect_source_file(src.id(), overrides.to_vec(), cancel)?
            .into_checked(),
        _ => None,
    };
    let checked = alone.as_ref().or_else(|| revision.checked());
    if checked
        .is_some_and(|comp| comp.world().local_packages().as_ref() != pkgs.local_packages.as_ref())
    {
        return Err(tola_build::cancellation::BuildCancelled.into());
    }
    let reply = match checked {
        Some(comp) => selection.compiled_reply(&mut CompiledAnswer {
            site: &site,
            comp,
            biblios: &mut *biblios,
            query,
            routes_as_hints,
            overrides,
        })?,
        None => selection
            .source_only_reply(cfg, query)?
            .unwrap_or_else(|| unavailable(query)),
    };
    cancel.ensure_active()?;
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
    fn referenced_name(&self) -> Option<&str> {
        let SelectedSyntax::Reference(ref_) = &self.syntax else {
            return None;
        };
        Some(&self.source.text()[ref_.name.clone()])
    }

    fn compiled_reply(&self, ans: &mut CompiledAnswer<'_>) -> Result<SourceReply> {
        let prep = ans
            .comp
            .world()
            .source(self.source.id())
            .context("prepared source is unavailable")?;
        let mut sem = Semantic::new(ans.comp, ans.site.cancel);
        Ok(match ans.query {
            SourceQuery::Completion(_) => self.compiled_completion(ans, &prep, &mut sem)?,
            SourceQuery::Hover(_) => self.compiled_hover(ans, &prep, &mut sem)?,
            SourceQuery::Definition(_) => self.compiled_definition(ans, &prep, &mut sem)?,
            SourceQuery::SignatureHelp(_) => {
                SourceReply::SignatureHelp(self.signature_help(&prep, &mut sem)?)
            }
            SourceQuery::PrepareRename(_) => self.compiled_prepare_rename(ans.comp),
            SourceQuery::Rename(params) => self.compiled_rename(ans, params)?,
            SourceQuery::References(params) => self.compiled_references(ans, params)?,
            SourceQuery::DocumentHighlights(_) => self.compiled_highlights(),
            SourceQuery::CodeActions(params) => SourceReply::CodeActions(self.narrowing(
                &params.text_document.uri,
                &prep,
                &mut sem,
            )?),
            SourceQuery::DocumentColors(_) => {
                SourceReply::DocumentColors(Some(colors::document(self.source)?))
            }
            SourceQuery::ColorPresentations(params) => SourceReply::ColorPresentations(Some(
                colors::presentations(self.source, params.color, params.range),
            )),
            SourceQuery::InlayHints(params) => self.compiled_inlay_hints(ans, params)?,
        })
    }

    fn compiled_definition(
        &self,
        ans: &mut CompiledAnswer<'_>,
        prep: &Source,
        sem: &mut Semantic<'_>,
    ) -> Result<SourceReply> {
        let (comp, cfg, cancel, root) = (
            ans.comp,
            ans.site.cfg,
            ans.site.cancel,
            ans.site.client_root,
        );
        if let Some((_, bytes)) = self.asset_url_argument(sem)? {
            let urls = AssetUrls::for_check_project(cfg, &ans.site.pkgs.boundary, cancel)?;
            if let Some(def) = crate::assets::written_definition(&urls, root, self.source, bytes) {
                return Ok(SourceReply::Definition(Some(def)));
            }
        }
        let referenced = match self.referenced_name() {
            Some(name) => {
                match labels::definition(comp, name).and_then(|s| sem.location(s, root)) {
                    Some(loc) => Some(GotoDefinitionResponse::Scalar(loc)),
                    None if !labels::declares(comp, name) => citations::definition(
                        comp,
                        cfg,
                        self.source.id(),
                        name,
                        ans.biblios,
                        cancel,
                        root,
                    )?
                    .map(GotoDefinitionResponse::Scalar),
                    None => None,
                }
            }
            None => None,
        };
        let name_def = self.definition(prep, sem, root)?;
        let def = match referenced.or(name_def) {
            Some(def) => Some(def),
            // Typst's own reader reads the repaired copy; skip repaired stand-ins.
            None if self.cursor_keeps_source_text(prep) => {
                editing::definition(comp.world(), prep, self.cursor)
                    .and_then(|target| sem.target_location(target, root))
                    .map(GotoDefinitionResponse::Scalar)
            }
            None => None,
        };
        Ok(SourceReply::Definition(def))
    }

    fn compiled_prepare_rename(&self, comp: &SourceCompilation) -> SourceReply {
        let prepared = labels::prepared(comp, self.source, self.cursor);
        SourceReply::PrepareRename(prepared.map(|(range, placeholder)| {
            PrepareRenameResponse::RangeWithPlaceholder { range, placeholder }
        }))
    }

    fn compiled_references(
        &self,
        ans: &mut CompiledAnswer<'_>,
        params: &ReferenceParams,
    ) -> Result<SourceReply> {
        let (comp, cfg, cancel, root) = (
            ans.comp,
            ans.site.cfg,
            ans.site.cancel,
            ans.site.client_root,
        );
        let refs = match self.referenced_name() {
            Some(name) => {
                let found = if labels::declares(comp, name) {
                    labels::locations(comp, name, params.context.include_declaration, cancel, root)?
                } else {
                    citations::references(
                        comp,
                        cfg,
                        &citations::CitationRequest {
                            open: self.source.id(),
                            key: name,
                            include_declaration: params.context.include_declaration,
                        },
                        ans.biblios,
                        cancel,
                        root,
                    )?
                };
                (!found.is_empty()).then_some(found)
            }
            None => None,
        };
        Ok(SourceReply::References(refs))
    }

    fn compiled_highlights(&self) -> SourceReply {
        let highlights = match self.referenced_name() {
            Some(name) => Some(labels::highlights(self.source, name)),
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

    fn compiled_inlay_hints(
        &self,
        ans: &CompiledAnswer<'_>,
        params: &InlayHintParams,
    ) -> Result<SourceReply> {
        let (comp, cancel) = (ans.comp, ans.site.cancel);
        let mut hints = hints::hints(
            comp,
            ans.site.pkgs.published,
            self.source,
            params.range,
            cancel,
        )?;
        if ans.routes_as_hints {
            hints.extend(hints::route_hints(
                comp,
                ans.site.cfg,
                self.source,
                params.range,
                cancel,
            )?);
        }
        Ok(SourceReply::InlayHints(hints))
    }

    fn compiled_rename(
        &self,
        ans: &CompiledAnswer<'_>,
        params: &RenameParams,
    ) -> Result<SourceReply> {
        let rename = match self.referenced_name() {
            Some(name) => labels::rename(
                ans.comp,
                name,
                &params.new_name,
                ans.site.cancel,
                ans.site.client_root,
            )?,
            None => None,
        };
        Ok(SourceReply::Rename(rename))
    }

    fn compiled_hover(
        &self,
        ans: &mut CompiledAnswer<'_>,
        prep: &Source,
        sem: &mut Semantic<'_>,
    ) -> Result<SourceReply> {
        let (comp, cfg, cancel) = (ans.comp, ans.site.cfg, ans.site.cancel);
        let mut hover = self.hover(comp, cfg, cancel, prep, sem)?;
        if hover.is_none() {
            hover = self.citation(comp, cfg, ans.biblios, cancel)?;
        }
        if hover.is_none() {
            hover = labels::hover(comp, cfg, self.source, self.cursor, cancel)?;
        }
        if hover.is_none() {
            hover = crate::assets::hover(cfg, self.source, self.cursor);
        }
        if hover.is_none() {
            hover = self.asset_url_hover(cfg, &ans.site.pkgs.boundary, sem, cancel)?;
        }
        if hover.is_none() {
            hover = crate::links::hover(comp, cfg, self.source, self.cursor, cancel)?;
        }
        let hover = hover.or_else(|| {
            self.cursor_keeps_source_text(prep)
                .then(|| self.builtin_hover(comp.world(), prep, self.cursor))
                .flatten()
        });
        Ok(SourceReply::Hover(hover.filter(says_something)))
    }

    fn compiled_completion(
        &self,
        ans: &mut CompiledAnswer<'_>,
        prep: &Source,
        sem: &mut Semantic<'_>,
    ) -> Result<SourceReply> {
        let (comp, cfg, cancel) = (ans.comp, ans.site.cfg, ans.site.cancel);
        if let Some(items) = self.meta_completion(comp, cfg, sem)? {
            return Ok(completion_reply(items));
        }
        if let Some((typed, bytes)) = self.asset_url_argument(sem)? {
            let urls = AssetUrls::for_check_project(cfg, &ans.site.pkgs.boundary, cancel)?;
            return Ok(SourceReply::Completion(crate::assets::url_completions(
                &urls,
                cfg.root(),
                self.source,
                &typed,
                bytes,
                self.cursor,
            )));
        }
        if completion::opaque_at(self.source, self.cursor) {
            return Ok(completion_reply(Vec::new()));
        }
        // Postfix wraps come before existing fields so matching prefixes surface wraps first.
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
        add_unseen(self.completion(prep, sem)?);
        if let Some(referenced) = labels::completion(comp, self.source, self.cursor) {
            add_unseen(referenced);
            add_unseen(citations::completions(
                comp,
                cfg,
                self.source,
                self.cursor,
                ans.biblios,
                cancel,
            )?);
        }
        if !self.at_code_member()
            && !matches!(self.syntax, SelectedSyntax::Import(_))
            && let Some((_, ordinary)) = editing::completion(
                comp.world(),
                cfg,
                ans.site.pkgs,
                self.source,
                self.cursor,
                cancel,
                ans.overrides,
            )?
        {
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

    fn source_only_reply(
        &self,
        cfg: &SiteConfig,
        query: &SourceQuery,
    ) -> Result<Option<SourceReply>> {
        Ok(Some(match query {
            SourceQuery::Hover(_) => SourceReply::Hover(
                crate::assets::hover(cfg, self.source, self.cursor).or_else(|| {
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
