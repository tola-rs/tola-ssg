//! Worker lanes: job ownership, panic containment, and dispatch execution.

use std::path::PathBuf;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use lsp_server::RequestId;
use tola_build::cancellation::BuildCancellation;

use super::events::{Event, send_event};
use crate::compiler::{AnalysisRequest, SourceCompilation, SourceFailure, SourceJob};

pub(super) enum AnalysisJob {
    Request(Box<AnalysisRequest>),
    ReleaseIdle,
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("no message")
}

/// Captures the owner of a job before execution so panics can still produce a reply.
pub(super) enum JobOwner {
    Check {
        checked_revision: u64,
    },
    Selection {
        revision: u64,
        serial: u64,
        root: PathBuf,
    },
    Request(RequestId, u64),
    Unclaimed,
}

impl JobOwner {
    pub(super) fn of(job: &SourceJob) -> Self {
        match job {
            SourceJob::Check(req) => Self::Check {
                checked_revision: req.checked_revision,
            },
            SourceJob::Query(req) => Self::Request(req.id.clone(), req.serial),
            SourceJob::Route(req) => Self::Request(req.id.clone(), req.serial),
            SourceJob::Lenses(req) => Self::Request(req.id.clone(), req.serial),
            SourceJob::Rename(req) => Self::Request(req.id.clone(), req.serial),
            SourceJob::Symbols(req) => Self::Request(req.id.clone(), req.serial),
            SourceJob::PackageSource(req) => Self::Request(req.id.clone(), req.serial),
            SourceJob::IncomingCalls(req) => Self::Request(req.id.clone(), req.serial),
            SourceJob::RouteIndex(req) => Self::Request(req.id.clone(), req.serial),
            SourceJob::Selection(req) => Self::Selection {
                revision: req.revision,
                serial: req.serial,
                root: req.root.clone(),
            },
            SourceJob::ReleaseIdle => Self::Unclaimed,
        }
    }

    pub(super) fn of_analysis(job: &AnalysisJob) -> Self {
        match job {
            AnalysisJob::Request(req) => Self::Request(req.id.clone(), req.serial),
            AnalysisJob::ReleaseIdle => Self::Unclaimed,
        }
    }

    fn panicked(self) -> SourceCompilation {
        match self {
            Self::Check { checked_revision } => SourceCompilation::Checked {
                checked_revision,
                checked: Err(SourceFailure::Panicked),
            },
            Self::Request(id, serial) => SourceCompilation::Answered {
                configuration: None,
                read_paths: Vec::new(),
                id,
                serial,
                response: Err(SourceFailure::Panicked),
            },
            Self::Selection {
                revision,
                serial,
                root,
            } => SourceCompilation::Selected {
                revision,
                serial,
                root,
                selected: Err(SourceFailure::Panicked),
            },
            Self::Unclaimed => SourceCompilation::Released,
        }
    }
}

pub(super) struct JobTrace {
    lane: &'static str,
    kind: &'static str,
    checked_revision: Option<u64>,
    serial: Option<u64>,
}

pub(super) trait LaneJob {
    fn trace(&self) -> JobTrace;
}

impl LaneJob for SourceJob {
    fn trace(&self) -> JobTrace {
        JobTrace {
            lane: "compiler",
            kind: SourceJob::kind(self),
            checked_revision: SourceJob::checked_revision(self),
            serial: SourceJob::serial(self),
        }
    }
}

impl LaneJob for AnalysisJob {
    fn trace(&self) -> JobTrace {
        let (kind, serial) = match self {
            Self::Request(req) => (req.kind(), Some(req.serial)),
            Self::ReleaseIdle => ("release", None),
        };
        JobTrace {
            lane: "analysis",
            kind,
            checked_revision: None,
            serial,
        }
    }
}

fn contain_panic<J>(
    job: J,
    owner: JobOwner,
    work: impl FnOnce(J) -> SourceCompilation,
) -> SourceCompilation {
    // Lane caches are keyed and revalidated, so any partial state is overwritten on the next run.
    // Requires `panic = 'unwind'`.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || work(job))) {
        Ok(done) => done,
        Err(payload) => {
            tracing::error!(detail = panic_message(&*payload), "a lane job panicked");
            owner.panicked()
        }
    }
}

fn run_job<J: LaneJob>(
    job: J,
    owner: JobOwner,
    work: &mut impl FnMut(J) -> SourceCompilation,
) -> SourceCompilation {
    let trace = job.trace();
    let span = tracing::debug_span!(
        "lsp.job",
        lane = trace.lane,
        kind = trace.kind,
        checked_revision = trace.checked_revision,
        serial = trace.serial,
        elapsed_ms = tracing::field::Empty,
        outcome = tracing::field::Empty,
    );
    let started = Instant::now();
    let done = span.in_scope(|| contain_panic(job, owner, work));
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let ended = outcome(&done);
    span.record("elapsed_ms", elapsed_ms);
    span.record("outcome", ended);
    tracing::debug!(parent: &span, elapsed_ms, outcome = ended, "lane job finished");
    done
}

fn outcome(done: &SourceCompilation) -> &'static str {
    let failure = match done {
        SourceCompilation::Checked { checked, .. } => checked.as_ref().err(),
        SourceCompilation::Answered { response, .. } => response.as_ref().err(),
        SourceCompilation::Tokens { response, .. } => response.as_ref().err(),
        SourceCompilation::Selected { selected, .. } => selected.as_ref().err(),
        SourceCompilation::Continued(_) | SourceCompilation::Released => None,
    };
    match failure {
        None => "completed",
        Some(SourceFailure::Cancelled) => "cancelled",
        Some(SourceFailure::Panicked) => "panicked",
        Some(SourceFailure::Failed(_) | SourceFailure::InvalidName(_)) => "failed",
    }
}

pub(super) fn run<J: LaneJob>(
    jobs: Receiver<J>,
    ev_tx: &Sender<Event>,
    cancel: &BuildCancellation,
    owner_of: impl Fn(&J) -> JobOwner,
    mut work: impl FnMut(J) -> SourceCompilation,
    event_of: impl Fn(SourceCompilation) -> Event,
) {
    for job in jobs {
        if cancel.is_cancelled() {
            break;
        }
        let owner = owner_of(&job);
        let done = run_job(job, owner, &mut work);
        if !send_event(ev_tx, event_of(done), cancel) {
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
    fn routes_site_address() {
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
    fn mounted_site_canonical_url() {
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
    fn closed_broken_buffer_unblocks_site() {
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

        assert!(client.routes("content/preview.typ").routes.is_empty());

        client.close("content/broken.typ");
        std::fs::remove_file(site.path("content/broken.typ")).unwrap();
        let routes = client.routes("content/preview.typ").routes;
        assert_eq!(routes.len(), 1, "{routes:?}");
        assert_eq!(routes[0].route, "/preview/");

        client.shutdown();
    }

    #[test]
    fn broken_source_isolated() {
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
            .expect("hover for valid file");
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
        let mut first_load = true;
        let mut client = EditorSession::start_with_loader(&site, move |root, _, _| {
            if std::mem::take(&mut first_load) {
                notify_pause.send(())?;
                resumption.recv()?;
            }
            load_configuration(root)
        });
        compiler_paused
            .recv_timeout(REPLY_TIMEOUT)
            .expect("the compiler reaches its host loader");
        let pos = client.type_marked("content/document.typ", "#let marker = 1\n#mar|ker\n");
        let reply = client.request(
            request::References::METHOD,
            serde_json::json!({
                "textDocument": { "uri": site.uri("content/document.typ") },
                "position": pos,
                "context": { "includeDeclaration": false },
            }),
        );
        let refs: Vec<lsp_types::Location> = serde_json::from_value(reply).unwrap();
        let (_, start) = marked_cursor("#let marker = 1\n#|marker\n");
        let (_, end) = marked_cursor("#let marker = 1\n#marker|\n");
        assert_eq!(
            refs,
            vec![lsp_types::Location {
                uri: site.uri("content/document.typ"),
                range: lsp_types::Range::new(start, end),
            }]
        );
        drop(pause);
        client.shutdown();
    }

    #[test]
    fn panic_leaves_lane_running() {
        let site = site_with_document();
        let (panicked, panic_seen) = crossbeam_channel::bounded(1);
        let mut client = EditorSession::start_with_loader(&site, move |root, sources, _| {
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

        let rev = client.set_text(
            "content/document.typ",
            "#let before = 1\n#undefined_after_the_panic\n",
        );
        let uri = uri_string(site.root(), "content/document.typ");
        let (diags, reported) = client.await_diagnostics(&uri, |diags, _| !diags.is_empty());
        assert_eq!(reported, Some(rev));
        assert!(!diags.is_empty(), "{diags:?}");

        client.shutdown();
    }
}
