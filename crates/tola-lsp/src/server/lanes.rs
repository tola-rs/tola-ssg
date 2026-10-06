//! The two worker lanes: what each job owes, panic containment, and job execution.

use std::path::PathBuf;

use crossbeam_channel::{Receiver, Sender};
use lsp_server::RequestId;
use tola_build::cancellation::BuildCancellation;

use super::events::{Event, send_event};
use crate::compiler::{AnalysisRequest, SourceCompilation, SourceFailure, SourceJob};

/// The failure of one lane job whose own work panicked.
///
/// A panic is not a refusal of the job's request: the client is answered with an internal failure,
/// and the lane keeps taking jobs.
#[derive(Debug)]
pub(crate) struct PanickedJob;

impl std::fmt::Display for PanickedJob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a lane job panicked")
    }
}

impl std::error::Error for PanickedJob {}

/// The failure one panicked lane job answers with, with the panic's own message for the log.
fn panicked_job(payload: &(dyn std::any::Any + Send)) -> anyhow::Error {
    let detail = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("no message");
    anyhow::Error::new(PanickedJob).context(format!("a lane job panicked: {detail}"))
}

/// What one lane job owes an answer, read before the job runs so a job that panicked can still be
/// answered.
pub(super) enum JobOwner {
    /// A source check, answered with its own revision's outcome.
    Check(u64),
    /// One selection index a revision's corrections wait for, answered with the revision and site
    /// it was asked about.
    Selection(u64, PathBuf),
    /// A client request, answered under the id and serial it was admitted with.
    Request(RequestId, u64),
    /// A job nothing waits on.
    None,
}

impl JobOwner {
    /// The owner of one compiler-lane job.
    pub(super) fn of(job: &SourceJob) -> Self {
        match job {
            SourceJob::Check(request) => Self::Check(request.checked_revision),
            SourceJob::Query(request) => Self::Request(request.id.clone(), request.serial),
            SourceJob::Route(request) => Self::Request(request.id.clone(), request.serial),
            SourceJob::Lenses(request) => Self::Request(request.id.clone(), request.serial),
            SourceJob::Rename(request) => Self::Request(request.id.clone(), request.serial),
            SourceJob::Symbols(request) => Self::Request(request.id.clone(), request.serial),
            SourceJob::PackageSource(request) => Self::Request(request.id.clone(), request.serial),
            SourceJob::IncomingCalls(request) => Self::Request(request.id.clone(), request.serial),
            SourceJob::RouteIndex(request) => Self::Request(request.id.clone(), request.serial),
            SourceJob::Selection(request) => {
                Self::Selection(request.revision, request.root.clone())
            }
            SourceJob::ReleaseIdle => Self::None,
        }
    }

    /// The owner of one analysis-lane job.
    pub(super) fn of_analysis(request: &AnalysisRequest) -> Self {
        Self::Request(request.id.clone(), request.serial)
    }

    /// The completion one job answers with when its work panicked.
    fn panicked(self, error: anyhow::Error) -> SourceCompilation {
        match self {
            Self::Check(checked_revision) => SourceCompilation::Checked {
                checked_revision,
                checked: Err(SourceFailure::Failed(error)),
            },
            Self::Request(id, serial) => SourceCompilation::Answered {
                id,
                serial,
                response: Err(SourceFailure::Failed(error)),
            },
            Self::Selection(revision, root) => SourceCompilation::Selected {
                revision,
                root,
                selected: Err(SourceFailure::Failed(error)),
            },
            Self::None => SourceCompilation::Released,
        }
    }
}

/// Run one lane job, turning a panic into the completion its owner is answered with.
///
/// The lane's caches are keyed and revalidated, so whatever a panicking job left half-written is
/// re-derived by the next one; only the panic's own message reaches the log. This containment needs
/// a profile that unwinds: `panic = 'abort'` ends the process before this call can return.
fn contain_panic<J>(
    job: J,
    owner: JobOwner,
    work: impl FnOnce(J) -> SourceCompilation,
) -> SourceCompilation {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || work(job))) {
        Ok(completed) => completed,
        Err(payload) => {
            let error = panicked_job(&*payload);
            tracing::error!(error = ?error, "a lane job panicked");
            owner.panicked(error)
        }
    }
}

/// Run one lane's jobs until its channel closes or the connection is cancelled.
pub(super) fn run<J>(
    jobs: Receiver<J>,
    events: &Sender<Event>,
    cancellation: &BuildCancellation,
    owner_of: impl Fn(&J) -> JobOwner,
    mut work: impl FnMut(J) -> SourceCompilation,
    event_of: impl Fn(SourceCompilation) -> Event,
) {
    for job in jobs {
        if cancellation.is_cancelled() {
            break;
        }
        let owner = owner_of(&job);
        let completed = contain_panic(job, owner, &mut work);
        if !send_event(events, event_of(completed), cancellation) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    use lsp_server::ErrorCode;
    use lsp_types::request::{self, Request as LspRequest};
    use lsp_types::{GotoDefinitionResponse, SymbolKind};

    use crate::server::tests::{
        EditorSession, REPLY_TIMEOUT, SiteDirectory, load_configuration, marked_cursor,
        site_with_document, uri_string,
    };

    #[test]
    fn routes_answer_with_the_site_address() {
        let site = SiteDirectory::new();
        site.write("content/post.typ", "");
        let mut client = EditorSession::start(&site);

        client.open("content/post.typ", "Body.\n");
        let routes = client.routes("content/post.typ").routes;
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].output, "post/index.html");
        assert_eq!(routes[0].route, "/post/");
        assert_eq!(routes[0].url, None);

        client.shutdown();
    }

    #[test]
    fn mounted_site_answers_its_canonical_url() {
        let site = SiteDirectory::new();
        site.write(
            "tola.toml",
            "[site]\norigin = \"https://myblog.com\"\nbase-path = \"/blog/\"\n",
        );
        site.write("content/post.typ", "");
        let mut client = EditorSession::start(&site);

        client.open("content/post.typ", "Body.\n");
        let routes = client.routes("content/post.typ").routes;
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route, "/blog/post/");
        assert_eq!(
            routes[0].url.as_deref(),
            Some("https://myblog.com/blog/post/")
        );

        client.shutdown();
    }

    #[test]
    fn search_leaves_the_next_request_free() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);
        site.write(
            "site.typ",
            "#document(\"preview/index.html\")[#include \"content/document.typ\"]\n",
        );
        client.open("content/document.typ", "= Searchable\n");
        site.write("content/document.typ", "= Searchable\n");

        let found = client.workspace_symbols("Searchable");
        assert_eq!(found.len(), 1, "{found:?}");
        let routes = client.routes("content/document.typ").routes;
        assert_eq!(routes.len(), 1, "{routes:?}");
        assert_eq!(routes[0].route, "/preview/");

        client.shutdown();
    }

    #[test]
    fn source_served_twice_answers_per_page() {
        let site = SiteDirectory::new();
        site.write(
            "site.typ",
            "#document(\"first/index.html\")[#include \"content/shared.typ\"]\n#document(\"second/index.html\")[#include \"content/shared.typ\"]\n",
        );
        site.write("content/shared.typ", "Shared body.\n");
        let mut client = EditorSession::start(&site);
        client.open("content/shared.typ", "Shared body.\n");

        let routes = client.routes("content/shared.typ").routes;
        assert_eq!(
            routes
                .iter()
                .map(|route| route.route.as_str())
                .collect::<Vec<_>>(),
            ["/first/", "/second/"],
            "{routes:?}"
        );

        let lenses = client.code_lenses("content/shared.typ");
        assert_eq!(lenses.len(), 2, "{lenses:?}");
        assert!(
            lenses.iter().all(|lens| lens
                .command
                .as_ref()
                .is_some_and(|command| command.command == "tola.openPreview")),
            "{lenses:?}"
        );

        client.shutdown();
    }

    /// A lane's answer is shaped like one the connection answers itself: the preview lens names the
    /// address the site's development server serves it at.
    #[test]
    fn lane_answers_have_the_preview_address() {
        let site = site_with_document();
        site.write(
            "site.typ",
            "#document(\"document/index.html\")[#include \"content/document.typ\"]\n",
        );
        site.write(
            tola_build::filesystem::DEV_SERVER_STATE_FILE,
            &serde_json::json!({
                "version": tola_build::filesystem::DEV_SERVER_STATE_VERSION,
                "origin": "http://localhost:4321",
            })
            .to_string(),
        );
        let mut client = EditorSession::start(&site);
        client.open("content/document.typ", "Body.\n");

        let lenses = client.code_lenses("content/document.typ");
        assert_eq!(lenses.len(), 1, "{lenses:?}");
        let route = lenses[0]
            .command
            .as_ref()
            .and_then(crate::protocol::preview_route)
            .expect("the lens names the route it opens");
        let expected = format!("http://localhost:4321{route}");
        assert_eq!(
            lenses[0]
                .data
                .as_ref()
                .and_then(|data| data.get("url"))
                .and_then(serde_json::Value::as_str),
            Some(expected.as_str()),
            "{lenses:?}"
        );

        client.shutdown();
    }

    #[test]
    fn closed_buffer_lets_the_site_compile() {
        let site = SiteDirectory::new();
        site.write(
            "site.typ",
            "#document(\"preview/index.html\")[#include \"content/preview.typ\"]\n",
        );
        site.write("content/preview.typ", "The preview serves this page.\n");
        site.write("content/broken.typ", "#include \"absent.typ\"\n");
        let mut client = EditorSession::start(&site);
        client.open("content/broken.typ", "#include \"absent.typ\"\n");
        client.open("content/preview.typ", "The preview serves this page.\n");

        assert!(
            client.routes("content/preview.typ").routes.is_empty(),
            "an open buffer that does not compile stops the site"
        );

        client.close("content/broken.typ");
        std::fs::remove_file(site.path("content/broken.typ")).unwrap();
        let routes = client.routes("content/preview.typ").routes;
        assert_eq!(routes.len(), 1, "{routes:?}");
        assert_eq!(routes[0].route, "/preview/");

        client.shutdown();
    }

    #[test]
    fn workspace_symbols_answer_every_source() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);
        site.write("content/second.typ", "= Second page\n\n#let marker = 1\n");

        let found = client.workspace_symbols("marker");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].name, "marker");
        assert_eq!(
            found[0].container_name.as_deref(),
            Some("content/second.typ"),
            "{found:?}"
        );
        // A page answers to both facts a search can name it by: the heading its source declares,
        // and the route no source declares.
        let headings = client.workspace_symbols("second");
        assert_eq!(headings.len(), 2, "{headings:?}");
        assert!(
            headings.iter().any(|symbol| symbol.name == "Second page"),
            "{headings:?}"
        );
        assert!(
            headings.iter().any(|symbol| symbol.name == "/second/"
                && symbol.container_name.as_deref() == Some("content/second.typ")),
            "{headings:?}"
        );

        client.shutdown();
    }

    /// A label answers the site-wide search by the name the author gave it.
    #[test]
    fn label_answers_workspace_search() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);
        site.write("content/second.typ", "= Second page <second-page>\n");

        let found = client.workspace_symbols("second-page");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].name, "second-page");
        assert_eq!(found[0].kind, SymbolKind::CONSTANT);
        assert_eq!(
            found[0].container_name.as_deref(),
            Some("content/second.typ"),
            "{found:?}"
        );

        client.shutdown();
    }

    #[test]
    fn local_bindings_answer_definitions() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);

        let definition = client
            .definition_marked("content/document.typ", "#let body = 1\n#bo|dy\n")
            .expect("a definition of the file's own binding");
        let GotoDefinitionResponse::Scalar(location) = definition else {
            panic!("expected one location, got {definition:?}");
        };
        assert_eq!(location.range.start.line, 0);

        client.shutdown();
    }

    #[test]
    fn included_files_answer_definitions() {
        let site = site_with_document();
        site.write("content/other.typ", "Other body.\n");
        let mut client = EditorSession::start(&site);

        let definition = client
            .definition_marked("content/document.typ", "#include \"other.typ|\"\n")
            .expect("a definition of the included file");
        let GotoDefinitionResponse::Scalar(location) = definition else {
            panic!("expected one location, got {definition:?}");
        };
        assert!(
            location.uri.as_str().ends_with("content/other.typ"),
            "{location:?}"
        );

        client.shutdown();
    }

    #[test]
    fn missing_source_answers_nothing() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);
        let missing = site.uri("content/never_written.typ");

        for (method, params) in [
            (
                lsp_types::request::HoverRequest::METHOD,
                serde_json::json!({}),
            ),
            (
                lsp_types::request::GotoDefinition::METHOD,
                serde_json::json!({}),
            ),
            (
                lsp_types::request::References::METHOD,
                serde_json::json!({ "context": { "includeDeclaration": true } }),
            ),
        ] {
            let mut params = params;
            params["textDocument"] = serde_json::json!({ "uri": missing.as_str() });
            params["position"] = serde_json::json!({ "line": 0, "character": 0 });
            // `request` fails the test on a request error, so reaching these assertions is what
            // proves the answer is a result: an empty one.
            let answer = client.request(method, params);
            assert!(answer.is_null(), "{method} answered {answer}");
        }

        client.shutdown();
    }

    #[test]
    fn cursor_questions_read_the_buffer() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);

        let items = client.complete_marked(
            "content/document.typ",
            "#import \"@tola/document:0.0.0\": current-document\n#context current-document().|",
        );
        for label in ["output", "route", "location"] {
            assert!(
                items.iter().any(|item| item.label == label),
                "`{label}` missing from {items:?}"
            );
        }

        let definition = client.definition_marked(
            "content/document.typ",
            "#import \"@tola/docum|ent:0.0.0\": current-document\n#context current-document().",
        );
        let target = match definition {
            Some(lsp_types::GotoDefinitionResponse::Scalar(location)) => location,
            Some(lsp_types::GotoDefinitionResponse::Array(locations)) => {
                locations.into_iter().next().expect("one definition")
            }
            other => panic!("expected a definition location, got {other:?}"),
        };
        assert_eq!(
            target.uri.as_str(),
            "tola-package:/tola/document/0.0.0/lib.typ"
        );

        client.shutdown();
    }

    #[test]
    fn broken_source_does_not_silence_others() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);
        let text = "#let brand = rgb(\"#777\")\n#brand\n";
        client.open("content/document.typ", text);
        site.write("content/document.typ", text);
        site.write("content/broken.typ", "#let nothing = undefined-thing\n");

        let hover = client
            .hover_marked(
                "content/document.typ",
                "#let brand = rgb(\"#777\")\n#bra|nd\n",
            )
            .expect("a hover about a source the site's failure did not touch");
        let lsp_types::HoverContents::Markup(markup) = hover.contents else {
            panic!("plain markdown, got {:?}", hover.contents);
        };
        assert!(
            markup.value.contains("let brand = color;"),
            "{}",
            markup.value
        );

        client.shutdown();
    }

    #[test]
    fn broken_source_resolves_bindings() {
        let site = site_with_document();
        let mut client = EditorSession::start(&site);

        let definition = client
            .definition_marked(
                "content/document.typ",
                "#let brand = rgb(\"#777\")\n#bra|nd\n#icon(\"home\")\n",
            )
            .expect("a definition inside the file the site could not compile");
        let lsp_types::GotoDefinitionResponse::Scalar(location) = definition else {
            panic!("one definition, got {definition:?}");
        };
        assert_eq!(location.range.start.line, 0, "{location:?}");

        client.shutdown();
    }

    #[test]
    fn references_bypass_pending_compilation() {
        struct CompilerPause(Sender<()>);
        impl Drop for CompilerPause {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }

        let site = site_with_document();
        let (notify_pause, compiler_paused) = crossbeam_channel::bounded(1);
        let (resume, resumption) = crossbeam_channel::bounded(1);
        let pause = CompilerPause(resume);
        let mut is_first_load = true;
        let mut client = EditorSession::start_with_loader(&site, move |root, _| {
            if std::mem::take(&mut is_first_load) {
                notify_pause.send(())?;
                resumption.recv()?;
            }
            load_configuration(root)
        });
        compiler_paused
            .recv_timeout(REPLY_TIMEOUT)
            .expect("the compiler reaches its host loader");
        let position = client.type_marked("content/document.typ", "#let marker = 1\n#mar|ker\n");
        let reply = client.request(
            request::References::METHOD,
            serde_json::json!({
                "textDocument": { "uri": site.uri("content/document.typ") },
                "position": position,
                "context": { "includeDeclaration": false },
            }),
        );
        let references: Vec<lsp_types::Location> = serde_json::from_value(reply).unwrap();
        let (_, start) = marked_cursor("#let marker = 1\n#|marker\n");
        let (_, end) = marked_cursor("#let marker = 1\n#marker|\n");
        assert_eq!(
            references,
            vec![lsp_types::Location {
                uri: site.uri("content/document.typ"),
                range: lsp_types::Range::new(start, end),
            }]
        );
        drop(pause);
        client.shutdown();
    }

    /// A job whose lane panicked is answered with an internal failure, and the lane keeps taking
    /// jobs after it.
    #[test]
    fn panicked_job_answers_internal_error() {
        let site = site_with_document();
        let mut client =
            EditorSession::start_with_loader(&site, |_: &Path, _: &[(PathBuf, Arc<str>)]| {
                panic!("a lane job that fails")
            });

        let (code, message) =
            client.error_of("workspace/symbol", serde_json::json!({ "query": "" }));
        assert_eq!(code, ErrorCode::InternalError as i32, "{message}");

        // The lane survived the panic: the next job is answered rather than left silent.
        let (code, _) = client.error_of("workspace/symbol", serde_json::json!({ "query": "" }));
        assert_eq!(code, ErrorCode::InternalError as i32);

        client.shutdown();
    }

    /// A check whose lane panicked is contained: a later revision is still checked and reported.
    #[test]
    fn panicked_check_does_not_stop_later_checks() {
        let site = site_with_document();
        let (panicked, panic_seen) = crossbeam_channel::bounded(1);
        let mut client = EditorSession::start_with_loader(&site, move |root, sources| {
            // The lane loads every revision through this callback, so the revision the test marks
            // panics here; the signal tells the test the panicking check ran before it asks for the
            // next revision, which keeps the sequence deterministic.
            if sources.iter().any(|(_, text)| text.contains("panic-now")) {
                let _ = panicked.try_send(());
                panic!("a check that fails");
            }
            load_configuration(root)
        });

        client.open("content/document.typ", "#let before = 1\n");
        client.set_text(
            "content/document.typ",
            "#let before = 1\n#let marked = 2 // panic-now\n",
        );
        panic_seen
            .recv_timeout(REPLY_TIMEOUT)
            .expect("the panicking check reached its host loader");

        // Only the last text has a diagnostic, so a non-empty report for the version the last
        // edit reported can only be that revision's: the lane kept checking after the panic.
        let version = client.set_text(
            "content/document.typ",
            "#let before = 1\n#undefined_after_the_panic\n",
        );
        let uri = uri_string(site.root(), "content/document.typ");
        let (diagnostics, reported) =
            client.await_diagnostics(&uri, |diagnostics, _| !diagnostics.is_empty());
        assert_eq!(reported, Some(version), "the last revision's report");
        assert!(!diagnostics.is_empty(), "{diagnostics:?}");

        client.shutdown();
    }
}
