//! The compiler worker: one lane's retained session, its jobs, and their outcomes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tola_build::BuildResources;
use tola_build::cancellation::BuildCancelled;
use tola_build::check::SourceDiagnosticSession;
use tola_typst_syntax::names::SelectedInterfaces;

use super::check::CheckedSources;
use super::compilations::RevisionCompilations;
use super::jobs::{AnalysisContinuation, QueryRequest, SourceInputs, SourceJob, UnsavedSource};
use crate::protocol::{SourceQuery, SourceReply};
use crate::server::ServedWorkspace;

#[derive(Debug)]
pub(crate) enum SourceFailure {
    Cancelled,
    /// The job whose answer this would have been panicked; the request is still answered.
    Panicked,
    Failed(anyhow::Error),
}

impl SourceFailure {
    pub(super) fn of(error: anyhow::Error) -> Self {
        if error.chain().any(|cause| cause.is::<BuildCancelled>()) {
            Self::Cancelled
        } else {
            Self::Failed(error)
        }
    }

    /// The outcome of a job whose configuration load failed.
    ///
    /// Cancellation keeps its own outcome: the requester has already left. A broken configuration
    /// answers whatever `empty` builds instead of failing the job — the failure is already
    /// published as diagnostics on the configuration file, and the job must not fail on every
    /// edit.
    pub(super) fn resolve_load_failure<T>(
        error: anyhow::Error,
        empty: impl FnOnce() -> T,
    ) -> Result<T, SourceFailure> {
        match Self::of(error) {
            Self::Cancelled => Err(Self::Cancelled),
            _ => Ok(empty()),
        }
    }
}

impl From<BuildCancelled> for SourceFailure {
    fn from(_: BuildCancelled) -> Self {
        Self::Cancelled
    }
}

pub(crate) enum SourceCompilation {
    Checked {
        checked_revision: u64,
        checked: Result<CheckedSources, SourceFailure>,
    },
    Answered {
        id: lsp_server::RequestId,
        serial: u64,
        configuration: Option<Arc<tola_build::config::ResolvedSiteConfig>>,
        read_paths: Vec<PathBuf>,
        response: Result<SourceReply, SourceFailure>,
    },
    Tokens {
        id: lsp_server::RequestId,
        serial: u64,
        read: super::jobs::TokensRead,
        response: Result<lsp_types::SemanticTokens, SourceFailure>,
    },
    /// A source-only analysis whose identity proof needs the compiler lane, with the request
    /// that lane answers.
    Continued(AnalysisContinuation),
    /// The selection index one correction asked for, which no completed check could return.
    Selected {
        revision: u64,
        serial: u64,
        root: PathBuf,
        selected: Result<Arc<SelectedInterfaces>, SourceFailure>,
    },
    /// The retained compilation was let go; nothing answers to it.
    Released,
}

impl SourceCompilation {
    /// One reply to a client request, which reports no configuration or read paths of its own.
    pub(super) fn answered(
        id: lsp_server::RequestId,
        serial: u64,
        response: Result<SourceReply, SourceFailure>,
    ) -> Self {
        Self::Answered {
            id,
            serial,
            configuration: None,
            read_paths: Vec::new(),
            response,
        }
    }
}

pub(crate) struct SourceCompiler<F> {
    pub(super) load: F,
    pub(super) resources: BuildResources,
    pub(super) published: crate::published::PublishedIndex,
    pub(super) session: Option<(ServedWorkspace, SourceDiagnosticSession)>,
    pub(super) compilations: RevisionCompilations,
    /// The site's own Typst files as this lane last read them, shared by every request it
    /// answers.
    pub(super) disk: crate::analysis::DiskSources,
    /// The name graph this lane last built, reusable while its sources and import targets hold.
    pub(super) graphs: crate::analysis::GraphCache,
    /// The site's bibliography files as this lane last parsed them, shared by every request it
    /// answers.
    pub(super) bibliographies: crate::query::BibliographyCache,
    pub(super) entity_queries: super::entities::EntityQueries,
}

impl<F> SourceCompiler<F>
where
    F: FnMut(&Path, &[UnsavedSource]) -> Result<ServedWorkspace>,
{
    pub(crate) fn new(load: F, resources: BuildResources) -> Self {
        Self {
            load,
            resources,
            published: crate::published::PublishedIndex::default(),
            session: None,
            compilations: RevisionCompilations::new(),
            disk: crate::analysis::DiskSources::default(),
            graphs: crate::analysis::GraphCache::default(),
            bibliographies: crate::query::BibliographyCache::default(),
            entity_queries: super::entities::EntityQueries::default(),
        }
    }

    pub(crate) fn compile(&mut self, job: SourceJob) -> SourceCompilation {
        match job {
            SourceJob::Check(request) => SourceCompilation::Checked {
                checked_revision: request.checked_revision,
                checked: self.check(&request.sources, &request.view),
            },
            SourceJob::Query(request) => {
                let response = self.query_reply(&request);
                let configuration = response.as_ref().ok().and_then(|_| {
                    self.session
                        .as_ref()
                        .map(|(served, _)| Arc::clone(served.configuration()))
                });
                let read_paths = self.session.as_ref().map_or_else(Vec::new, |(served, _)| {
                    self.entity_queries.observed_paths(
                        served.configuration(),
                        request.sources.source_revision,
                        &request.sources.overrides,
                    )
                });
                SourceCompilation::Answered {
                    response,
                    configuration,
                    read_paths,
                    id: request.id,
                    serial: request.serial,
                }
            }
            SourceJob::Route(request) => {
                let response = self.route(&request);
                SourceCompilation::answered(request.id, request.serial, response)
            }
            SourceJob::RouteIndex(request) => {
                let response = self.route_index(&request);
                SourceCompilation::answered(request.id, request.serial, response)
            }
            SourceJob::Lenses(request) => {
                let response = self.lenses(&request);
                SourceCompilation::answered(request.id, request.serial, response)
            }
            SourceJob::Symbols(request) => {
                let response = self.workspace_symbols(&request);
                SourceCompilation::answered(request.id, request.serial, response)
            }
            SourceJob::Rename(request) => {
                let response = self.rename(&request);
                SourceCompilation::answered(request.id, request.serial, response)
            }
            SourceJob::PackageSource(request) => {
                let response = self.package_source(&request);
                SourceCompilation::answered(request.id, request.serial, response)
            }
            SourceJob::IncomingCalls(request) => {
                let response = self.incoming_calls(&request);
                SourceCompilation::answered(request.id, request.serial, response)
            }
            SourceJob::Selection(request) => {
                let selected = crate::analysis::site_selected_interfaces(
                    &request.root,
                    &request.view,
                    &request.boundary,
                    &mut self.disk,
                    &request.cancellation,
                );
                SourceCompilation::Selected {
                    revision: request.revision,
                    serial: request.serial,
                    root: request.root,
                    selected: selected.map_err(SourceFailure::of),
                }
            }
            SourceJob::ReleaseIdle => {
                self.compilations.release();
                self.entity_queries.release();
                SourceCompilation::Released
            }
        }
    }

    /// Resolve the workspace one job runs against, replacing the session when it changed.
    pub(super) fn prepare(&mut self, sources: &SourceInputs) -> Result<()> {
        sources.cancellation.ensure_active()?;
        let served = (self.load)(&sources.root, &sources.overrides);
        sources.cancellation.ensure_active()?;
        let served = served?;
        if self.session.as_ref().is_none_or(|(active, _)| {
            active.compiles_site() != served.compiles_site()
                || !Arc::ptr_eq(active.configuration(), served.configuration())
        }) {
            let configuration = Arc::clone(served.configuration());
            let session = match &served {
                ServedWorkspace::Site(_) => {
                    SourceDiagnosticSession::with_resources(configuration, self.resources.clone())
                }
                ServedWorkspace::Documents(_) => {
                    SourceDiagnosticSession::for_documents(configuration, self.resources.clone())
                }
            };
            self.compilations.release();
            self.entity_queries.release();
            self.session = Some((served, session));
        }
        Ok(())
    }

    /// The session of the configuration `prepare` resolved, without borrowing the whole compiler.
    ///
    /// Jobs call `prepare` for its side effect and then read the session through this function:
    /// the borrow `prepare` returned would keep the whole compiler borrowed while the job reads
    /// other fields as well.
    pub(super) fn prepared_session(
        session: &mut Option<(ServedWorkspace, SourceDiagnosticSession)>,
    ) -> (&ServedWorkspace, &mut SourceDiagnosticSession) {
        let (served, session) = session
            .as_mut()
            .expect("prepared configuration owns source session");
        (served, session)
    }

    pub(super) fn query_reply(
        &mut self,
        request: &QueryRequest,
    ) -> Result<SourceReply, SourceFailure> {
        let response = (|| -> Result<SourceReply, SourceFailure> {
            if let Err(error) = self.prepare(&request.sources) {
                return SourceFailure::resolve_load_failure(error, || {
                    crate::query::unavailable(&request.query)
                });
            }
            let (served, session) = Self::prepared_session(&mut self.session);
            let configuration = served.configuration();
            let client_root = crate::uri::ClientRoot::new(&request.sources.root);
            let published = self.published.published_packages(
                &self.resources,
                configuration.package_locations(),
                matches!(
                    request.query,
                    SourceQuery::Completion(_) | SourceQuery::InlayHints(_)
                ),
            );
            let mut response = crate::query::respond(crate::query::QueryRequest {
                session,
                compilations: &mut self.compilations,
                disk: &mut self.disk,
                graphs: &mut self.graphs,
                bibliographies: &mut self.bibliographies,
                entity_queries: &mut self.entity_queries,
                config: configuration,
                source_revision: request.sources.source_revision,
                overrides: &request.sources.overrides,
                view: &request.view,
                published,
                query: &request.query,
                routes_as_hints: request.routes_as_hints,
                check_progress: request.check_progress,
                cancellation: &request.sources.cancellation,
                client_root: &client_root,
            })
            .map_err(SourceFailure::of)?;
            if let SourceReply::Definition(Some(definition)) = &mut response
                && let Some(directory) = request.view.package_sources()
            {
                crate::query::package_source_definition(
                    definition,
                    directory,
                    &client_root,
                    &request.sources.cancellation,
                )
                .map_err(SourceFailure::of)?;
            }
            Ok(response)
        })();
        request
            .sources
            .cancellation
            .ensure_active()
            .map_err(SourceFailure::from)?;
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::tests::{assert_cancelled, inputs, site_configuration};
    use crate::compiler::{CheckRequest, SourceJob};
    use crate::sources::SourceView;
    use tola_build::BuildResources;
    use tola_build::cancellation::BuildCancellation;
    use tola_diagnostic::{Diagnostic, DiagnosticError, Severity};

    fn package_hover(sources: SourceInputs) -> SourceJob {
        let uri = crate::uri::from_file_path(&sources.root.join("site.typ")).unwrap();
        let marked = "#import \"@tola/docu|ment:0.0.0\"";
        let cursor = marked.find('|').unwrap();
        let source = tola_typst::typst::syntax::Source::detached(marked.replace('|', ""));
        let position = crate::position::utf16_range(source.lines(), cursor..cursor)
            .unwrap()
            .start;
        SourceJob::Query(QueryRequest {
            id: 7.into(),
            serial: 11,
            sources,
            view: SourceView::default(),
            routes_as_hints: false,
            check_progress: crate::protocol::CheckProgress::NotChecked,
            query: crate::protocol::SourceQuery::Hover(lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier::new(uri),
                position,
            }),
        })
    }

    #[test]
    fn cancellation_overrides_config_errors() {
        let root = std::env::current_dir().unwrap();
        for query in [false, true] {
            for cancel_before_load in [false, true] {
                let canceller = tola_build::cancellation::BuildCanceller::new();
                if cancel_before_load {
                    canceller.cancel();
                }
                let mut compiler = SourceCompiler::new(
                    |_, _| {
                        assert!(
                            !cancel_before_load,
                            "cancelled jobs must not load configuration"
                        );
                        canceller.cancel();
                        anyhow::bail!("configuration failed while cancellation arrived")
                    },
                    BuildResources::default(),
                );
                let sources = inputs(&root, canceller.token());
                let job = if query {
                    package_hover(sources)
                } else {
                    SourceJob::Check(CheckRequest {
                        checked_revision: 3,
                        sources,
                        view: SourceView::default(),
                    })
                };
                assert_cancelled(compiler.compile(job));
            }
        }
    }
    #[test]
    fn configuration_failure_retains_workspace() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tola.toml");
        std::fs::write(
            directory.path().join("site.typ"),
            "#import \"@tola/document:0.0.0\": current-document\n#document(\"index.html\")[Body]",
        )
        .unwrap();
        let configuration = site_configuration(directory.path(), "");
        let diagnostics = vec![
            Diagnostic::at_path(
                tola_diagnostic::codes::config::INVALID,
                Severity::Error,
                path.to_string_lossy(),
                "invalid configuration",
            )
            .with_note("retained explanation"),
            Diagnostic::at_path(
                tola_diagnostic::codes::config::WARNING,
                Severity::Warning,
                path.to_string_lossy(),
                "configuration warning",
            ),
        ];
        let mut first = true;
        let mut compiler = SourceCompiler::new(
            |_, _| {
                if std::mem::take(&mut first) {
                    Ok(ServedWorkspace::Site(Arc::clone(&configuration)))
                } else {
                    Err(anyhow::Error::new(DiagnosticError::new(
                        "configuration rejected",
                        diagnostics.clone(),
                    ))
                    .context("reload editor configuration"))
                }
            },
            BuildResources::default(),
        );
        let SourceCompilation::Answered { response, .. } = compiler.compile(package_hover(inputs(
            directory.path(),
            BuildCancellation::new(),
        ))) else {
            panic!("expected query completion")
        };
        let SourceReply::Hover(Some(hover)) = response.unwrap() else {
            panic!("expected package hover")
        };
        assert!(
            matches!(hover.contents, lsp_types::HoverContents::Markup(contents) if contents.value.contains("@tola/document"))
        );
        let SourceCompilation::Checked { checked, .. } =
            compiler.compile(SourceJob::Check(CheckRequest {
                checked_revision: 2,
                sources: inputs(directory.path(), BuildCancellation::new()),
                view: SourceView::default(),
            }))
        else {
            panic!("expected check completion")
        };
        let checked = checked.unwrap();
        assert!(Arc::ptr_eq(
            checked.configuration.as_ref().unwrap(),
            &configuration
        ));
        assert_eq!(checked.diagnostics, diagnostics);
        let SourceCompilation::Answered { response, .. } = compiler.compile(package_hover(inputs(
            directory.path(),
            BuildCancellation::new(),
        ))) else {
            panic!("expected query completion")
        };
        assert!(matches!(
            response.expect("configurations failures do not fail queries"),
            SourceReply::Hover(None)
        ));
    }
}
