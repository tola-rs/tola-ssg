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
    Selection(u64, u64, PathBuf),
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
                Self::Selection(request.revision, request.serial, request.root.clone())
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
            Self::Selection(revision, serial, root) => SourceCompilation::Selected {
                revision,
                serial,
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

    use lsp_types::request::{self, Request as LspRequest};

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
    /// A check whose lane panicked is contained: a later revision is still checked and reported.
    #[test]
    fn panic_leaves_lane_running() {
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
