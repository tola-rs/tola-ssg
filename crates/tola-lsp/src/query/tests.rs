use super::*;

use lsp_types::request::Request as LspRequest;
use lsp_types::{
    CompletionTextEdit, DocumentHighlight, Hover, InlayHint, InlayHintLabel, Location, Position,
    SignatureHelp, WorkspaceEdit, request as lsp_request,
};

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

pub(crate) fn insertion_text(item: &CompletionItem) -> &str {
    match item
        .text_edit
        .as_ref()
        .expect("a completion carries its edit")
    {
        CompletionTextEdit::Edit(edit) => &edit.new_text,
        CompletionTextEdit::InsertAndReplace(edit) => &edit.new_text,
    }
}

/// Site root in the resolved spelling used by the compiler (e.g. `/private/var` on macOS).
pub(crate) fn resolved_root(site: &SiteDirectory) -> PathBuf {
    tola_input::normalize_existing_prefix(site.root())
}

pub(crate) struct QuerySession {
    pub(crate) site: SiteDirectory,
    pub(crate) config: Arc<SiteConfig>,
    pub(crate) session: tola_build::check::SourceDiagnosticSession,
    pub(crate) compilations: RevisionCompilations,
    pub(crate) bibliographies: BibliographyCache,
    pub(crate) entity_queries: EntityQueries,
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
        let cfg = load_configuration(site.root()).expect("site configuration");
        let session = tola_build::check::SourceDiagnosticSession::with_resources(
            Arc::clone(&cfg),
            tola_build::BuildResources::new()
                .with_network_access(tola_input::NetworkAccess::Denied)
                .without_system_fonts(),
        );
        Self {
            site,
            config: cfg,
            session,
            compilations: RevisionCompilations::new(),
            bibliographies: BibliographyCache::default(),
            entity_queries: EntityQueries::default(),
            revision: 0,
            capabilities: announcing_client(),
        }
    }

    pub(crate) fn reload_with(&mut self, res: tola_build::BuildResources) {
        self.config = load_configuration(self.site.root()).expect("site configuration");
        self.session = tola_build::check::SourceDiagnosticSession::with_resources(
            Arc::clone(&self.config),
            res,
        );
    }

    pub(crate) fn reload(&mut self) {
        self.reload_with(
            tola_build::BuildResources::new()
                .with_network_access(tola_input::NetworkAccess::Denied)
                .without_system_fonts(),
        );
    }

    pub(crate) fn with_published_assets() -> Self {
        let mut session = Self::with_program(ENTRY_PROGRAM);
        session.site.write("brand/logo.svg", "<svg></svg>");
        let cfg = std::fs::read_to_string(session.site.root().join("tola.toml"))
            .expect("the site's configuration");
        session.site.write(
            "tola.toml",
            &format!(
                "{cfg}\n[assets]\ntrees = [{{ source = \"brand\", url-prefix = \"/brand\" }}]\n"
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
        serde_json::from_value(self.reply(lsp_request::DocumentHighlightRequest::METHOD, marked))
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

    pub(crate) fn rename_refusal(&mut self, marked: &str, new_name: &str) -> String {
        let err = self
            .rename_error(marked, new_name)
            .expect("the rename is refused");
        err.downcast_ref::<crate::analysis::NameQueryError>()
            .expect("a rename refusal has its kind")
            .kind()
            .to_owned()
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

    pub(crate) fn rename_error(&mut self, marked: &str, new_name: &str) -> Option<anyhow::Error> {
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
        let (text, pos) = marked_cursor(marked);
        let path = self.config.build().content_dir.join("document.typ");
        let uri = crate::uri::from_file_path(&path).expect("a content path is addressable");
        self.try_reply_at(method, uri.as_str(), pos, Some((path, text)), extra)
    }

    pub(crate) fn try_reply_at(
        &mut self,
        method: &str,
        uri: &str,
        pos: Position,
        doc: Option<(PathBuf, String)>,
        extra: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let mut params = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": pos,
        });
        if let Some(extra) = extra.as_object() {
            for (key, value) in extra {
                params[key] = value.clone();
            }
        }
        let query = decode(method, params);
        let overrides: Vec<(PathBuf, Arc<str>)> = doc
            .map(|(path, text)| (path, Arc::from(text)))
            .into_iter()
            .collect();
        let reply = crate::query::respond(QueryRequest {
            session: &mut self.session,
            compilations: &mut self.compilations,
            disk: &mut crate::analysis::DiskSources::default(),
            graphs: &mut crate::analysis::GraphCache::default(),
            bibliographies: &mut self.bibliographies,
            entity_queries: &mut self.entity_queries,
            config: &self.config,
            source_revision: self.revision,
            overrides: &overrides,
            view: &crate::sources::OpenSources::new(self.config.root()).view(),
            published: &[],
            query: &query,
            routes_as_hints: crate::connection::routes_as_hints(&self.capabilities),
            check_progress: CheckProgress::Failed,
            cancellation: &BuildCancellation::new(),
            client_root: &crate::uri::ClientRoot::new(self.config.root()),
        })?;
        Ok(serde_json::to_value(reply).expect("a serializable reply"))
    }

    pub(crate) fn hover_at(&mut self, uri: &str, pos: Position) -> anyhow::Result<Option<Hover>> {
        let reply = self.try_reply_at(
            lsp_request::HoverRequest::METHOD,
            uri,
            pos,
            None,
            serde_json::json!({}),
        )?;
        Ok(serde_json::from_value(reply).expect("a hover response"))
    }
}

pub(crate) fn completion_labels(items: &[CompletionItem]) -> Vec<&str> {
    items.iter().map(|item| item.label.as_str()).collect()
}

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

#[test]
fn unaddressable_uri_returns_none() {
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

pub(crate) fn summary_program() -> String {
    parsed_program(&format!(
        "{PAGE_SCHEMA}\n  summary: describe(optional(str), \"the page summary\"),"
    ))
}

pub(crate) const PAGE_SCHEMA: &str = "  title: describe(optional(str, default: \"Untitled\"), \"the page title\"),\n  draft: describe(optional(bool, default: false), \"keeps the page out of the published site\"),";

pub(crate) const SCHEMA_PAGE: &str = r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: "Page"))
#let page = (title: "Page")
"#;

pub(crate) const DECLARED_PAGE: &str = r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: "Page", draft: false))
#let page = (title: "Page")
"#;

pub(crate) fn marked_before(text: &str, at: &str) -> String {
    text.replacen(at, &format!("|{at}"), 1)
}

pub(crate) fn site_reply(
    site: &mut QuerySession,
    method: &str,
    marked: &str,
    extra: serde_json::Value,
) -> serde_json::Value {
    let (text, pos) = marked_cursor(marked);
    let path = site.site.path("site.typ");
    let uri = crate::uri::from_file_path(&path).expect("the site program is addressable");
    site.try_reply_at(method, uri.as_str(), pos, Some((path, text)), extra)
        .expect("a completed query")
}

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

pub(crate) fn site_actions(
    site: &mut QuerySession,
    marked: &str,
    only: &[&str],
) -> Vec<serde_json::Value> {
    let (_, pos) = marked_cursor(marked);
    let reply = site_reply(
        site,
        lsp_request::CodeActionRequest::METHOD,
        marked,
        serde_json::json!({
            "range": { "start": pos, "end": pos },
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

    fn ask(
        site: &mut QuerySession,
        sources: &OpenSources,
        uri: &Uri,
        pos: Position,
        method: &str,
    ) -> Value {
        let query = decode(
            method,
            json!({"textDocument": {"uri": uri}, "position": pos}),
        );
        let reply = crate::query::respond(crate::query::QueryRequest {
            session: &mut site.session,
            compilations: &mut site.compilations,
            disk: &mut crate::analysis::DiskSources::default(),
            graphs: &mut crate::analysis::GraphCache::default(),
            bibliographies: &mut site.bibliographies,
            entity_queries: &mut crate::compiler::EntityQueries::default(),
            config: &site.config,
            source_revision: sources.revision(),
            overrides: sources.overrides(),
            view: &sources.view(),
            published: &[],
            query: &query,
            routes_as_hints: false,
            check_progress: CheckProgress::Failed,
            cancellation: &BuildCancellation::default(),
            client_root: &crate::uri::ClientRoot::new(site.config.root()),
        })
        .unwrap();
        serde_json::to_value(reply).unwrap()
    }

    fn definition(value: Value) -> (Uri, Range) {
        let response: Option<GotoDefinitionResponse> = serde_json::from_value(value).unwrap();
        match response.expect("the definition is available") {
            GotoDefinitionResponse::Scalar(loc) => (loc.uri, loc.range),
            GotoDefinitionResponse::Array(locs) => {
                let loc = locs.into_iter().next().unwrap();
                (loc.uri, loc.range)
            }
            GotoDefinitionResponse::Link(locs) => {
                let loc = locs.into_iter().next().unwrap();
                (loc.target_uri, loc.target_selection_range)
            }
        }
    }

    fn install(mut site: QuerySession, module: &Path) -> QuerySession {
        fs::write(
            module.join("tola-module.toml"),
            "[typst]\nentry='lib.typ'\n",
        )
        .unwrap();
        let old = fs::read_to_string(site.site.root().join("tola.toml")).unwrap();
        let path = serde_json::to_string(&module.to_string_lossy()).unwrap();
        site.site.write(
            "tola.toml",
            &format!("{old}\n[modules.notes]\npath={path}\n"),
        );
        site.reload_with(
            tola_build::BuildResources::new().with_input_scope(tola_input::InputScope::Offline),
        );
        site
    }

    #[test]
    fn module_library_root_identity() {
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
        let access =
            crate::packages::PackageAccess::prepared(&site.config, site.session.resources(), &[])
                .unwrap();
        let id = |uri| {
            crate::identity::source_id_with_packages(
                uri,
                site.config.root(),
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
        let mut sources = OpenSources::new(site.config.root());
        open(&mut sources, &library_uri, &library);
        open(&mut sources, &project_uri, &project);
        let cancel = BuildCancellation::default();
        let checked = site
            .session
            .inspect(sources.overrides().to_vec(), &cancel)
            .unwrap();
        assert!(checked.compiled(), "{:?}", checked.diagnostics());
        assert!(
            checked.diagnostics().is_empty(),
            "{:?}",
            checked.diagnostics()
        );
        let alone = site
            .session
            .inspect_source_file(library_id, sources.overrides().to_vec(), &cancel)
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
}
