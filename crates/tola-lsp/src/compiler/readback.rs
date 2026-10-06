//! The replies one checked revision produces for the read-back jobs (route, lenses, rename,
//! symbols, package source, incoming calls).

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use lsp_types::{SymbolKind, WorkspaceSymbol, WorkspaceSymbolResponse};

use super::jobs::{
    IncomingCallsRead, LensesRead, PackageSourceRead, RenameRead, RouteIndexRead, RouteRead,
    SymbolsRead, UnsavedSource,
};
use super::worker::{SourceCompiler, SourceFailure};
use crate::protocol::{RouteReply, SourceReply};
use crate::routes::RouteIndexReply;
use crate::server::ServedWorkspace;

impl<F> SourceCompiler<F>
where
    F: FnMut(&Path, &[UnsavedSource]) -> Result<ServedWorkspace>,
{
    pub(super) fn workspace_symbols(
        &mut self,
        request: &SymbolsRead,
    ) -> Result<SourceReply, SourceFailure> {
        request
            .sources
            .cancellation
            .ensure_active()
            .map_err(SourceFailure::from)?;
        if let Err(error) = self.prepare(&request.sources).map(|_| ()) {
            return SourceFailure::resolve_load_failure(error, || {
                SourceReply::WorkspaceSymbols(WorkspaceSymbolResponse::Nested(Vec::new()))
            });
        }
        let (served, session) = Self::prepared_session(&mut self.session);
        let configuration = served.configuration();
        let root = configuration.get_root().to_path_buf();
        let client_root = crate::uri::ClientRoot::new(&request.sources.root);
        let boundary = session.resources().source_boundary(configuration);
        let mut symbols = Vec::new();
        self.disk.begin();
        for id in crate::files::site_files(configuration) {
            request
                .sources
                .cancellation
                .ensure_active()
                .map_err(SourceFailure::from)?;
            let path = root.join(id.vpath().get_without_slash());
            if boundary.check(&path).is_err() {
                continue;
            }
            // The author's unsaved text is what a search answers about; a file no editor holds is
            // read through this lane's own parsed copy.
            let names = match request
                .sources
                .overrides
                .iter()
                .find(|(known, _)| *known == path)
            {
                Some((_, text)) => Arc::new(tola_typst_syntax::names::SourceNames::new(
                    tola_typst::typst::syntax::Source::new(id, text.to_string()),
                )),
                None => match self.disk.names(&path, id) {
                    Some(names) => names,
                    None => continue,
                },
            };
            let source = names.source();
            let container = id.vpath().get_without_slash().to_owned();
            let Ok(uri) = client_root.address(&path) else {
                continue;
            };
            for (name, kind, range) in crate::symbols::matching(source, &request.query) {
                symbols.push(WorkspaceSymbol {
                    name,
                    kind,
                    tags: None,
                    container_name: Some(container.clone()),
                    location: lsp_types::OneOf::Left(lsp_types::Location {
                        uri: uri.clone(),
                        range,
                    }),
                    data: None,
                });
            }
            // A search finds a label by the name the author gave it, whatever element has it.
            for (name, range) in crate::symbols::labels(source, &request.query) {
                symbols.push(WorkspaceSymbol {
                    name,
                    kind: SymbolKind::CONSTANT,
                    tags: None,
                    container_name: Some(container.clone()),
                    location: lsp_types::OneOf::Left(lsp_types::Location {
                        uri: uri.clone(),
                        range,
                    }),
                    data: None,
                });
            }
        }
        // Closes the pass; a cancellation above left it open, and the next completed pass drops
        // what this one did not see.
        self.disk.retain_pass();
        // A search finds the site's pages and its declarations: a page answers to the route
        // an author knows it by, which no source declares.
        let revision = self
            .compilations
            .inspect(
                session,
                configuration,
                request.sources.source_revision,
                request.sources.overrides.to_vec(),
                &request.sources.cancellation,
            )
            .map_err(SourceFailure::from)?;
        if let Some(compilation) = revision.checked() {
            let realized =
                crate::routes::pages(compilation, configuration, &request.sources.cancellation)
                    .map_err(SourceFailure::of)?;
            symbols.extend(crate::symbols::pages(
                &realized,
                &client_root,
                &request.query,
            ));
        }
        Ok(SourceReply::WorkspaceSymbols(
            WorkspaceSymbolResponse::Nested(symbols),
        ))
    }

    pub(super) fn rename(&mut self, request: &RenameRead) -> Result<SourceReply, SourceFailure> {
        request
            .sources
            .cancellation
            .ensure_active()
            .map_err(SourceFailure::from)?;
        let (served, session) = match self.prepare(&request.sources) {
            Ok(prepared) => prepared,
            Err(error) => {
                return SourceFailure::resolve_load_failure(error, || SourceReply::Rename(None));
            }
        };
        let configuration = served.configuration();
        crate::renames::imports(
            configuration,
            &crate::uri::ClientRoot::new(&request.sources.root),
            &request.sources.overrides,
            &request.files,
            &session.resources().source_boundary(configuration),
        )
        .map(SourceReply::Rename)
        .map_err(SourceFailure::of)
    }

    pub(super) fn package_source(
        &self,
        request: &PackageSourceRead,
    ) -> Result<SourceReply, SourceFailure> {
        request
            .cancellation
            .ensure_active()
            .map_err(SourceFailure::from)?;
        let source = crate::query::package_source(&request.uri);
        request
            .cancellation
            .ensure_active()
            .map_err(SourceFailure::from)?;
        source
            .map(SourceReply::PackageSource)
            .map_err(SourceFailure::of)
    }

    /// The sources that reach the item's file, with the ranges inside each that write its path.
    ///
    /// A site that does not compile answers with nothing rather than failing, as the route and
    /// lens answers do: the author fixes the source first.
    pub(super) fn incoming_calls(
        &mut self,
        request: &IncomingCallsRead,
    ) -> Result<SourceReply, SourceFailure> {
        request
            .sources
            .cancellation
            .ensure_active()
            .map_err(SourceFailure::from)?;
        if let Err(error) = self.prepare(&request.sources).map(|_| ()) {
            return SourceFailure::resolve_load_failure(error, || SourceReply::IncomingCalls(None));
        }
        let (served, session) = Self::prepared_session(&mut self.session);
        let configuration = served.configuration();
        let boundary = session.resources().source_boundary(configuration);
        let locations = self.resources.package_locations(configuration);
        let revision = self
            .compilations
            .inspect(
                session,
                configuration,
                request.sources.source_revision,
                request.sources.overrides.to_vec(),
                &request.sources.cancellation,
            )
            .map_err(SourceFailure::from)?;
        let Some(compilation) = revision.checked() else {
            return Ok(SourceReply::IncomingCalls(None));
        };
        crate::call_hierarchy::incoming(
            &mut self.disk,
            &request.sources.root,
            &request.view,
            Some(&locations),
            &boundary,
            configuration,
            compilation,
            &request.source,
            &request.sources.cancellation,
        )
        .map(|calls| SourceReply::IncomingCalls(Some(calls)))
        .map_err(SourceFailure::of)
    }

    /// The routes the site serves the requested source's documents at.
    ///
    /// A source the site no longer holds — or a site that does not compile — answers with no
    /// routes rather than a failure: the author fixes the source first.
    pub(super) fn route(&mut self, request: &RouteRead) -> Result<SourceReply, SourceFailure> {
        request
            .sources
            .cancellation
            .ensure_active()
            .map_err(SourceFailure::from)?;
        if let Err(error) = self.prepare(&request.sources).map(|_| ()) {
            return SourceFailure::resolve_load_failure(error, || {
                SourceReply::Routes(RouteReply::default())
            });
        }
        let (served, session) = Self::prepared_session(&mut self.session);
        let configuration = served.configuration();
        let Some(source) = crate::sources::load(
            &request.uri,
            configuration.get_root(),
            &request.sources.overrides,
            &session.resources().source_boundary(configuration),
        ) else {
            return Ok(SourceReply::Routes(RouteReply::default()));
        };
        crate::routes::respond(
            session,
            &mut self.compilations,
            configuration,
            request.sources.source_revision,
            &request.sources.overrides,
            &source,
            &request.sources.cancellation,
        )
        .map(SourceReply::Routes)
        .map_err(SourceFailure::of)
    }

    pub(super) fn route_index(
        &mut self,
        request: &RouteIndexRead,
    ) -> Result<SourceReply, SourceFailure> {
        request
            .sources
            .cancellation
            .ensure_active()
            .map_err(SourceFailure::from)?;
        if let Err(error) = self.prepare(&request.sources).map(|_| ()) {
            return SourceFailure::resolve_load_failure(error, || {
                SourceReply::RouteIndex(RouteIndexReply::default())
            });
        }
        let (served, session) = Self::prepared_session(&mut self.session);
        let configuration = served.configuration();
        let revision = self
            .compilations
            .inspect(
                session,
                configuration,
                request.sources.source_revision,
                request.sources.overrides.to_vec(),
                &request.sources.cancellation,
            )
            .map_err(SourceFailure::from)?;
        let Some(compilation) = revision.checked() else {
            return Ok(SourceReply::RouteIndex(RouteIndexReply::default()));
        };
        crate::routes::index(compilation, configuration, &request.sources.cancellation)
            .map(SourceReply::RouteIndex)
            .map_err(SourceFailure::of)
    }

    pub(super) fn lenses(&mut self, request: &LensesRead) -> Result<SourceReply, SourceFailure> {
        request
            .sources
            .cancellation
            .ensure_active()
            .map_err(SourceFailure::from)?;
        if let Err(error) = self.prepare(&request.sources).map(|_| ()) {
            return SourceFailure::resolve_load_failure(error, || SourceReply::CodeLenses(None));
        }
        let (served, session) = Self::prepared_session(&mut self.session);
        let configuration = served.configuration();
        let Some(source) = crate::sources::load(
            &request.uri,
            configuration.get_root(),
            &request.sources.overrides,
            &session.resources().source_boundary(configuration),
        ) else {
            return Ok(SourceReply::CodeLenses(None));
        };
        crate::lenses::respond(
            session,
            &mut self.compilations,
            configuration,
            request.sources.source_revision,
            &request.sources.root,
            &request.sources.overrides,
            &source,
            &request.sources.cancellation,
        )
        .map(SourceReply::CodeLenses)
        .map_err(SourceFailure::of)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::tests::{
        assert_cancelled, compiler, inputs, inputs_at, site_configuration,
    };
    use crate::compiler::{
        IncomingCallsRead, PackageSourceRead, SourceCompilation, SourceCompiler, SourceInputs,
        SourceJob,
    };
    use crate::sources::SourceView;
    use lsp_server::RequestId;
    use tola_build::BuildResources;
    use tola_typst::typst::syntax::Source;

    fn package_query(sources: SourceInputs) -> SourceJob {
        SourceJob::PackageSource(PackageSourceRead {
            id: 7.into(),
            serial: 11,
            uri: "tola-package:/tola/document/0.0.0/lib.typ".parse().unwrap(),
            cancellation: sources.cancellation,
        })
    }

    /// A failed revision's world still answers which sources reach a file.
    ///
    /// The pages a compilation realized are absent from a failed realization, but the sources
    /// that write a file's path in an `#include` are syntax, not evaluation: the call hierarchy
    /// reads them from the revision's own world rather than answering nothing.
    #[test]
    fn failed_revision_answers_incoming_calls() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("content")).unwrap();
        // The caller writes the callee's own path, which is the range the reply names.
        std::fs::write(root.join("content/page.typ"), "#include \"lib.typ\"\n").unwrap();
        let library = root.join("content/lib.typ");
        std::fs::write(&library, "#let greet = [hi]\n").unwrap();
        std::fs::write(
            root.join("site.typ"),
            "#document(\"index.html\", format: \"html\")[#include \"content/page.typ\"\n\
             #unknown_in_the_program]\n",
        )
        .unwrap();
        let configuration = site_configuration(root, "[build]\nentry = \"site.typ\"\n");
        let mut compiler = compiler(&configuration);
        // The client may name the site through a symlinked root; identity is the resolved spelling.
        let resolved = tola_build::filesystem::normalize_existing_prefix(root);
        let id = crate::identity::path_id(&resolved.join("content/lib.typ"), &resolved)
            .expect("a site source");
        let source = Source::new(id, std::fs::read_to_string(&library).unwrap());

        let SourceCompilation::Answered { response, .. } =
            compiler.compile(SourceJob::IncomingCalls(IncomingCallsRead {
                id: RequestId::from(5),
                serial: 7,
                sources: inputs_at(root, 1, Arc::default()),
                view: SourceView::default(),
                source,
            }))
        else {
            panic!("expected an incoming-calls completion")
        };
        let SourceReply::IncomingCalls(Some(calls)) = response.expect("a call hierarchy reply")
        else {
            panic!("a failed revision still names the sources that reach the file");
        };
        assert!(
            calls.iter().any(|call| {
                call.from.name.ends_with("content/page.typ") && !call.from_ranges.is_empty()
            }),
            "{calls:?}"
        );
    }

    #[test]
    fn cancelled_package_read_is_never_answered() {
        let mut compiler = SourceCompiler::new(
            |_, _| anyhow::bail!("invalid configuration"),
            BuildResources::default(),
        );
        let canceller = tola_build::cancellation::BuildCanceller::new();
        canceller.cancel();
        assert_cancelled(compiler.compile(package_query(inputs(
            Path::new("/invalid-site"),
            canceller.token(),
        ))));
    }

    /// A declaration and the page one file writes answer at one address: a symbol list spells every
    /// document the way the client named the root.
    ///
    /// The alias is required, not constructed: `Url` erases `..` and separator differences from the
    /// address, and a root that does not exist comes back from normalization unchanged, so only a
    /// real filesystem alias gives the two spellings distinct URIs.
    #[test]
    #[cfg(unix)]
    fn declaration_and_page_addresses_agree() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let alias = directory.path().join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        std::fs::create_dir_all(root.join("content")).unwrap();
        std::fs::write(
            root.join("content/document.typ"),
            "#let greeting = [hello]\n#document(\"index.html\")[Body]",
        )
        .unwrap();
        std::fs::write(root.join("site.typ"), "#include \"content/document.typ\"").unwrap();
        let configuration = site_configuration(&root, "");
        let mut compiler = compiler(&configuration);
        // The client named the root through the alias; the site is read through the form the
        // filesystem resolves that to.
        let SourceCompilation::Answered { response, .. } =
            compiler.compile(SourceJob::Symbols(SymbolsRead {
                id: RequestId::from(3),
                serial: 5,
                sources: inputs_at(&alias, 0, crate::compiler::SourceOverrides::default()),
                query: String::new(),
            }))
        else {
            panic!("expected a symbols answer");
        };
        let SourceReply::WorkspaceSymbols(WorkspaceSymbolResponse::Nested(symbols)) =
            response.expect("a symbols reply")
        else {
            panic!("expected workspace symbols");
        };
        let uri = |symbol: &WorkspaceSymbol| match &symbol.location {
            lsp_types::OneOf::Left(location) => location.uri.clone(),
            lsp_types::OneOf::Right(_) => panic!("a symbol opens a document"),
        };
        let declaration = symbols
            .iter()
            .find(|symbol| symbol.name == "greeting")
            .expect("the declaration");
        let page = symbols
            .iter()
            .find(|symbol| symbol.kind == lsp_types::SymbolKind::FILE)
            .expect("the page");
        let address = crate::uri::from_file_path(&alias.join("content/document.typ")).unwrap();
        assert_eq!(uri(declaration), address);
        assert_eq!(uri(page), address);
    }
}
