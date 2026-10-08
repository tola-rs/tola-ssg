//! A temporary demo site served by the ordinary production preview runtime.

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use tola_build::InputScope;
use tola_build::site::SiteRevision;

use super::Demo;
use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;
use crate::config::{ConfigOverrides, ServerOverrides};
use crate::terminal::Terminal;

#[derive(Debug)]
pub(crate) struct PreviewReady {
    pub(crate) url: String,
}

#[derive(Clone, Debug)]
pub(crate) enum PreviewStatus {
    Preparing,
    Ready(Arc<PreviewReady>),
    Failed(Arc<anyhow::Error>),
    Stopped,
}

pub(crate) struct DemoPreview {
    // One replaceable snapshot, not a queue: finishing never waits for a reader to poll Ready.
    status: Arc<Mutex<PreviewStatus>>,
    cancellation: Cancellation,
    worker: Option<JoinHandle<()>>,
    temporary: Option<tempfile::TempDir>,
}

impl DemoPreview {
    pub(crate) fn start(demo: &'static Demo) -> Result<Self> {
        let temporary = tempfile::Builder::new()
            .prefix("tola-demo-")
            .tempdir()
            .context(
                "Tola could not prepare the demo directory; check temporary directory permissions",
            )?;
        let root = temporary.path().to_path_buf();
        let status = Arc::new(Mutex::new(PreviewStatus::Preparing));
        let cancellation = Cancellation::default();
        let serving_status = Arc::clone(&status);
        let shutdown = cancellation.clone();
        let worker = std::thread::Builder::new()
            .name("tola-demo-preview".to_owned())
            .spawn(move || {
                let previewed = serve(demo, &root, &shutdown, &serving_status)
                    .context("Tola could not preview this demo");
                let completion = match previewed {
                    Ok(()) => PreviewStatus::Stopped,
                    Err(error)
                        if error.chain().any(|cause| {
                            cause.is::<tola_build::cancellation::BuildCancelled>()
                        }) =>
                    {
                        PreviewStatus::Stopped
                    }
                    Err(error) => failure(error),
                };
                *serving_status
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = completion;
            })
            .context("Tola could not start the demo preview; run the demo again")?;
        Ok(Self {
            status,
            cancellation,
            worker: Some(worker),
            temporary: Some(temporary),
        })
    }

    pub(crate) fn status(&mut self) -> PreviewStatus {
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished)
            && matches!(*status, PreviewStatus::Preparing | PreviewStatus::Ready(_))
        {
            *status = failure(anyhow::anyhow!(
                "the demo preview stopped unexpectedly; run the demo again"
            ));
        }
        status.clone()
    }

    pub(crate) fn stop(&mut self) -> Result<()> {
        self.cancellation.request();
        let joined = self.worker.take().map(|worker| worker.join());
        // The worker may still read sources during cooperative shutdown, so cleanup follows join.
        let removed = self.temporary.take().map(tempfile::TempDir::close);
        *self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = PreviewStatus::Stopped;
        if joined.is_some_and(|joined| joined.is_err()) {
            anyhow::bail!("the demo preview stopped unexpectedly; run the demo again");
        }
        if let Some(removed) = removed {
            removed.context("Tola could not remove the demo's temporary files")?;
        }
        Ok(())
    }
}

impl Drop for DemoPreview {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn failure(error: anyhow::Error) -> PreviewStatus {
    let diagnostics = crate::cli::output::attached_or_fallback(&error, crate::codes::demo::PREVIEW);
    let error = tola_build::diagnostic::DiagnosticError::attach(error, diagnostics);
    PreviewStatus::Failed(Arc::new(error.into()))
}

fn serve(
    demo: &Demo,
    root: &Path,
    shutdown: &Cancellation,
    status: &Arc<Mutex<PreviewStatus>>,
) -> Result<()> {
    let token = shutdown.token();
    super::export::sources(demo, root)?.apply(&token)?;
    token.ensure_active()?;
    let overrides = ConfigOverrides {
        server: ServerOverrides {
            interface: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            port: Some(0),
        },
        ..ConfigOverrides::default()
    };
    let loaded = crate::config::load(
        Some(&root.join("tola.toml")),
        InputScope::Pure,
        tola_typst::PackageLocations::from_absolute_roots(None, None)?,
        &overrides,
    )?;
    let output = CommandOutput::new(Terminal::silent(), None);
    output.apply_diagnostic_limits(loaded.diagnostics());
    output.install_source_files(
        loaded.config().get_root().to_path_buf(),
        loaded.config().package_locations().clone(),
    );
    let (config, server, _, loader) = loaded.into_parts();
    let ready = |url: &str, revision: Arc<SiteRevision>| {
        token.ensure_active()?;
        let ready = ready_preview(demo, url, &revision)?;
        *status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            PreviewStatus::Ready(Arc::new(ready));
        Ok(())
    };
    crate::dev::run_preview_with_ready(
        config,
        server,
        loader,
        tola_build::BuildResources::new().with_input_scope(InputScope::Pure),
        output,
        shutdown.clone(),
        &ready,
    )
}

fn ready_preview(demo: &Demo, url: &str, revision: &SiteRevision) -> Result<PreviewReady> {
    let landing = tola_address::OutputPath::parse(demo.landing_output())?;
    anyhow::ensure!(
        revision.outputs().output(&landing).is_some(),
        "the demo's starting page was not built"
    );
    let route = tola_address::route_for_output(&landing);
    let path = revision.config().url_mount().browser_path(&route);
    let url = url::Url::parse(url)?.join(&path)?.to_string();
    Ok(PreviewReady { url })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_until_ready(preview: &mut DemoPreview) -> Arc<PreviewReady> {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match preview.status() {
                PreviewStatus::Ready(ready) => return ready,
                PreviewStatus::Failed(error) => panic!("demo failed: {error:?}"),
                PreviewStatus::Stopped => panic!("demo stopped before it was ready"),
                PreviewStatus::Preparing => {}
            }
            assert!(Instant::now() < deadline, "demo never became ready");
            std::thread::yield_now();
        }
    }

    #[test]
    fn preview_serves_the_installed_output() {
        use std::io::{Read, Write};
        let demo = crate::demos::find("backlinks").unwrap();
        let mut preview = DemoPreview::start(demo).unwrap();
        let temporary = preview.temporary.as_ref().unwrap().path().to_path_buf();
        let ready = wait_until_ready(&mut preview);
        let url = url::Url::parse(&ready.url).unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        let port = url.port().unwrap();
        let mut socket = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        write!(
            socket,
            "GET {} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            url.path()
        )
        .unwrap();
        let mut response = Vec::new();
        socket.read_to_end(&mut response).unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200"));
        let body = response
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .unwrap()
            + 4;
        assert!(
            response[body..]
                .windows("Body links become backlinks".len())
                .any(|window| window == b"Body links become backlinks"),
            "the served landing page is the demo's own output"
        );
        preview.stop().unwrap();
        preview.stop().unwrap();
        assert!(!temporary.exists());
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
    }

    #[test]
    fn stopped_startup_releases_sources() {
        let demo = crate::demos::find("backlinks").unwrap();
        let mut preview = DemoPreview::start(demo).unwrap();
        let temporary = preview.temporary.as_ref().unwrap().path().to_path_buf();
        preview.cancellation.request();
        preview.stop().unwrap();
        assert!(matches!(preview.status(), PreviewStatus::Stopped));
        assert!(!temporary.exists());
    }

    #[test]
    fn preview_cancellation_stays_local() {
        let demo = crate::demos::find("backlinks").unwrap();
        let mut first = DemoPreview::start(demo).unwrap();
        let mut second = DemoPreview::start(demo).unwrap();
        first.stop().unwrap();
        assert!(!second.cancellation.is_requested());
        wait_until_ready(&mut second);
        second.stop().unwrap();
    }

    #[test]
    fn startup_failure_keeps_diagnostics() {
        let demo = Box::leak(Box::new(Demo {
            files: &[
                super::super::DemoFile {
                    path: "tola.toml",
                    bytes: b"",
                },
                super::super::DemoFile {
                    path: "site.typ",
                    bytes: b"#panic(\"Demo cannot build\")\n",
                },
            ],
            ..*crate::demos::find("backlinks").unwrap()
        }));
        let mut preview = DemoPreview::start(demo).unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        let failure = loop {
            match preview.status() {
                PreviewStatus::Failed(error) => break error,
                PreviewStatus::Preparing => std::thread::yield_now(),
                other => panic!("invalid demo returned {other:?}"),
            }
            assert!(Instant::now() < deadline, "failure was not reported");
        };
        assert!(tola_build::diagnostic::attached(&failure).is_some());
        assert!(matches!(preview.status(), PreviewStatus::Failed(_)));
        preview.stop().unwrap();
    }
}
