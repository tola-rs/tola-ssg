//! Development startup, initial revision, serving, and coordinated shutdown.

use std::sync::Arc;

use anyhow::{Context, Result};
use tola_build::build::{BuildMode, BuildRequest, BuildSession};
use tola_build::cancellation::{BuildCancellation, BuildCanceller};
use tola_build::config::SiteConfig;
use tola_build::filesystem::DevServerAddress;

use super::hooks::{HookQueue, HookSender};
use super::rebuild::{RebuildLoop, RebuildStartup};
use super::reload::transport::ReloadTransport;
use super::site::CurrentSite;
use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;
use crate::config::{ConfigLoader, DevConfig, ServerConfig};

const DEFAULT_WS_PORT: u16 = 35729;
const INITIAL_BUILD_STOPPED: &str =
    "the initial site build stopped unexpectedly; run `tola check` for the full error";

enum ServerPurpose {
    Development {
        dev: DevConfig,
        disable_hook_reuse: bool,
    },
    Preview,
}

impl ServerPurpose {
    fn build_mode(&self) -> BuildMode {
        match self {
            Self::Development { .. } => BuildMode::Development,
            Self::Preview => BuildMode::Production,
        }
    }

    fn command(&self) -> &'static str {
        match self {
            Self::Development { .. } => "tola dev",
            Self::Preview => "tola preview",
        }
    }

    fn build_request(&self, cancellation: BuildCancellation) -> BuildRequest {
        let mut req = BuildRequest::new(self.build_mode());
        req.cancellation = cancellation;
        req.disable_hook_reuse = matches!(
            self,
            Self::Development {
                disable_hook_reuse: true,
                ..
            }
        );
        req
    }
}

/// What a server session needs to start, before the purpose decides how it runs.
pub(crate) struct ServerLaunch {
    pub config: Arc<SiteConfig>,
    pub server: ServerConfig,
    pub config_loader: ConfigLoader,
    pub resources: tola_build::BuildResources,
    pub output: CommandOutput,
    pub shutdown: Cancellation,
    pub events: super::events::Events,
}

pub(crate) fn run(launch: ServerLaunch, dev: DevConfig, disable_hook_reuse: bool) -> Result<()> {
    run_server(
        launch,
        ServerPurpose::Development {
            dev,
            disable_hook_reuse,
        },
    )
}

/// Start a one-build local preview with production output and candidate checks.
pub(crate) fn run_preview(launch: ServerLaunch) -> Result<()> {
    run_server(launch, ServerPurpose::Preview)
}

fn run_server(launch: ServerLaunch, purpose: ServerPurpose) -> Result<()> {
    let ServerLaunch {
        config: cfg,
        server,
        config_loader,
        resources,
        output,
        shutdown,
        events,
    } = launch;
    shutdown.token().ensure_active()?;
    let view =
        match &purpose {
            ServerPurpose::Development { .. } if !events.0 => output
                .begin_dev_view(shutdown.clone())
                .context("could not open the development view; restart `tola dev`")?,
            _ => None,
        };
    let (stages, activity) = match &purpose {
        ServerPurpose::Development { .. } => (crate::terminal::EVERY_STAGE, "Building site…"),
        ServerPurpose::Preview => (crate::terminal::PRE_PUBLICATION_STAGES, "Building preview…"),
    };
    output.hook_overview(cfg.hooks(), purpose.build_mode(), stages)?;
    output.activity(activity)?;
    let cmd = purpose.command();
    let workers = std::thread::available_parallelism().map_or(2, |cores| cores.get().clamp(1, 4));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .with_context(|| format!("Tola could not start its serving runtime; restart `{cmd}`"))?;
    let (listener, addr, port_fallback) =
        super::http::bind_with_port_fallback(server.interface, server.port)?;
    let network_exposure = network_exposure_diagnostic(server.interface);

    let serving = runtime.block_on(async {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let sites = CurrentSite::new();
        let watching = matches!(&purpose, ServerPurpose::Development { dev, .. } if dev.watch);
        let reload = if watching {
            Some(
                ReloadTransport::start(server.interface, DEFAULT_WS_PORT, sites.clone())
                    .context("Tola could not start the browser reload listener")?,
            )
        } else {
            None
        };
        let reload_fallback = reload
            .as_ref()
            .and_then(|r| reload_port_fallback_diagnostic(r.endpoint().port()));
        let mut startup_diags: Vec<_> = [port_fallback, reload_fallback]
            .into_iter()
            .flatten()
            .collect();
        if matches!(&purpose, ServerPurpose::Development { .. }) {
            output.record_diagnostics(None, &startup_diags);
            startup_diags.clear();
        }
        startup_diags.extend(network_exposure);
        if !startup_diags.is_empty() {
            output.diagnostics(&startup_diags)?;
        }
        let browser_diags = reload.as_ref().map(ReloadTransport::diagnostics_sender);
        let build_session = BuildSession::with_resources(resources);
        let hooks = match &purpose {
            ServerPurpose::Development { .. } => Some(HookQueue::start(
                output.clone(),
                browser_diags,
                shutdown.clone(),
                events,
                build_session.hook_executor().clone(),
            )?),
            ServerPurpose::Preview => None,
        };

        let serving = async {
            let (reload_endpoint, mut rebuild) = match purpose {
                ServerPurpose::Development {
                    dev,
                    disable_hook_reuse,
                } if dev.watch => {
                    let reload_endpoint = reload.as_ref().map(ReloadTransport::endpoint);
                    let mut rebuild = RebuildLoop::start(RebuildStartup {
                        initial_config: Arc::clone(&cfg),
                        server: server.clone(),
                        dev,
                        disable_hook_reuse,
                        config_loader,
                        build_session,
                        sites: sites.clone(),
                        reload,
                        shutdown: shutdown.clone(),
                        output: output.clone(),
                        events,
                        hooks: hooks
                            .as_ref()
                            .expect("development owns a hook queue")
                            .sender(),
                    })
                    .await
                    .map_err(|err| super::watch::with_diagnostics(err.into(), cfg.root()))?;
                    if let Err(err) = rebuild.build_initial().await {
                        shutdown.request();
                        return Err::<(), _>(err).and(rebuild.run().await);
                    }
                    (reload_endpoint, Some(rebuild))
                }
                purpose => {
                    run_initial_unwatched_build(
                        Arc::clone(&cfg),
                        sites.clone(),
                        &output,
                        purpose,
                        hooks.as_ref().map(HookQueue::sender),
                        build_session,
                        &shutdown,
                        events,
                    )
                    .await?;
                    (None, None)
                }
            };

            let served = serve_until_stopped(
                listener,
                sites,
                cfg,
                addr,
                cmd,
                reload_endpoint,
                rebuild.take(),
                view.is_none(),
                &output,
                &shutdown,
                events,
            )
            .await;

            shutdown.request();
            let obs = match rebuild {
                Some(rebuild) => rebuild.run().await,
                None => Ok(()),
            };
            served.and(obs)
        }
        .await;

        shutdown.request();
        let view = match view {
            Some(view) => view
                .finish()
                .context("could not finish the development view; restart `tola dev`"),
            None => Ok(()),
        };
        let hooks = match hooks {
            Some(hooks) => tokio::task::spawn_blocking(move || hooks.finish())
                .await
                .context("Tola could not finish its after-publish commands")
                .and_then(|res| res),
            None => Ok(()),
        };
        serving.and(view).and(hooks)
    });
    shutdown.request();
    serving
}

#[allow(clippy::too_many_arguments)]
async fn run_initial_unwatched_build(
    cfg: Arc<SiteConfig>,
    sites: CurrentSite,
    output: &CommandOutput,
    purpose: ServerPurpose,
    hooks: Option<HookSender>,
    mut build_session: BuildSession,
    shutdown: &Cancellation,
    events: super::events::Events,
) -> Result<()> {
    let canceller = BuildCanceller::default();
    let cancellation = canceller.token();
    let attempt = build_session.prepare(
        Arc::clone(&cfg),
        purpose.build_request(cancellation.clone()),
    );
    let hook_epoch = attempt.hook_epoch();
    let executor = build_session.hook_executor().clone();
    let build_output = output.clone();
    let mut building = tokio::task::spawn_blocking(move || {
        build_site_once(BuildAttempt {
            config: cfg,
            sites,
            output: &build_output,
            purpose,
            cancellation,
            hooks,
            build_session: &mut build_session,
            attempt,
            events,
        })
    });
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => {
            executor.invalidate(hook_epoch);
            canceller.cancel();
            let _ = building.await.context(INITIAL_BUILD_STOPPED)?;
            Ok(())
        }
        completed = &mut building => completed.context(INITIAL_BUILD_STOPPED)?,
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve_until_stopped(
    listener: tokio::net::TcpListener,
    sites: CurrentSite,
    cfg: Arc<SiteConfig>,
    addr: std::net::SocketAddr,
    cmd: &str,
    reload_endpoint: Option<super::reload::transport::ReloadEndpoint>,
    mut rebuild: Option<RebuildLoop>,
    no_dev_view: bool,
    output: &CommandOutput,
    shutdown: &Cancellation,
    events: super::events::Events,
) -> Result<()> {
    if shutdown.is_requested() {
        return Ok(());
    }
    let mount = cfg.url_mount();
    let url = if mount.is_root() {
        format!("http://{addr}")
    } else {
        format!("http://{addr}{}", mount.base_path())
    };
    let _dev_server_address = DevServerAddress::write(cfg.root(), &url)
        .with_context(|| format!("Tola could not write the address it serves; restart `{cmd}`"))?;
    output.serving(&url)?;
    let watching = rebuild.is_some();
    let interactive = output.terminal().is_interactive() && no_dev_view;
    if watching || interactive {
        output.status(match (watching, interactive) {
            (true, true) => "Watching for changes, press Ctrl+C to stop",
            (true, false) => "Watching for changes",
            (false, _) => "Press Ctrl+C to stop",
        })?;
    }
    let http = super::http::serve_http(super::http::HttpServices {
        listener,
        sites,
        initial_config: cfg,
        reload_endpoint,
        publications: rebuild.as_ref().map(RebuildLoop::publication_sender),
        shutdown: shutdown.clone(),
        events,
        url,
    });
    tokio::pin!(http);
    if let Some(rebuild) = rebuild.take() {
        let rebuilding = rebuild.run();
        tokio::pin!(rebuilding);
        tokio::select! {
            served = &mut http => {
                shutdown.request();
                served.and(rebuilding.await)
            }
            rebuilt = &mut rebuilding => {
                shutdown.request();
                rebuilt.and(http.await)
            }
        }
    } else {
        http.await
    }
}

struct BuildAttempt<'a> {
    config: Arc<SiteConfig>,
    sites: CurrentSite,
    output: &'a CommandOutput,
    purpose: ServerPurpose,
    cancellation: BuildCancellation,
    hooks: Option<HookSender>,
    build_session: &'a mut BuildSession,
    attempt: tola_build::build::BuildAttempt,
    events: super::events::Events,
}

fn build_site_once(attempt: BuildAttempt<'_>) -> Result<()> {
    let events = attempt.events;
    let root = attempt.config.root().to_path_buf();
    events.build_started(1)?;
    let mut diags = Vec::new();
    let mut published = false;
    let outcome = build_site_once_inner(attempt, &mut diags, &mut published);
    if events.0 && !published {
        let status = if outcome
            .as_ref()
            .err()
            .is_some_and(tola_build::cancellation::is_cancelled)
        {
            diags.clear();
            "cancelled"
        } else {
            if diags.is_empty()
                && let Err(err) = &outcome
            {
                diags = tola_build::build::error_diagnostics(err, &root);
            }
            "failed"
        };
        events.build_finished(1, status, false, None, &diags)?;
    }
    outcome
}

fn build_site_once_inner(
    attempt: BuildAttempt<'_>,
    attempt_diags: &mut Vec<tola_diagnostic::Diagnostic>,
    published: &mut bool,
) -> Result<()> {
    let BuildAttempt {
        config: cfg,
        sites,
        output,
        purpose,
        cancellation,
        hooks,
        build_session: session,
        attempt,
        events,
    } = attempt;
    cancellation.ensure_active()?;
    let started = std::time::Instant::now();
    let build_lock = tola_build::build::SiteBuildLock::acquire(&cfg, &cancellation, || {
        let _ = output.waiting_for_build();
    })?;
    let candidate = attempt
        .run()
        .map_err(anyhow::Error::from)
        .and_then(|build| {
            let counts = build.graph().counts();
            let checked = build
                .into_unchecked_revision(None)
                .check(&cancellation)?
                .into_checked()?;
            Ok((checked, counts))
        });
    let (checked, counts) = match candidate {
        Ok(candidate) => candidate,
        Err(err) => {
            cancellation.ensure_active()?;
            if !matches!(&purpose, ServerPurpose::Development { .. }) {
                return Err(err);
            }
            let mut diags = tola_build::config::diagnostic::warning_diagnostics(&cfg);
            diags.extend(tola_build::build::error_diagnostics(&err, cfg.root()));
            if events.0 {
                attempt_diags.clone_from(&diags);
            }
            output.report_round(
                crate::dev::round::CompletedRound::failed(
                    None,
                    "Build failed; fix the errors and restart `tola dev`",
                ),
                &diags,
            )?;
            return Ok(());
        }
    };
    let (_, cur) = sites.replace(session, checked)?;
    *published = true;
    events.build_finished(
        1,
        "succeeded",
        true,
        Some(cur.manifest().revision().as_str()),
        cur.diagnostics(),
    )?;
    drop(build_lock);
    if let Some(hooks) = hooks {
        hooks.enqueue(Arc::clone(&cur), None)?;
    }
    tracing::debug!(
        target: "tola::dev",
        routes = cur.address().resource_count(),
        references = cur.references().references().len(),
        diagnostics = cur.diagnostics().len(),
        "installed initial site revision"
    );
    let summary = crate::terminal::site_summary(
        "Built",
        started.elapsed(),
        &crate::terminal::describe_outputs(counts),
    );
    let diags = cur.diagnostics().to_vec();
    output.report_round(crate::dev::round::CompletedRound::initial(&summary), &diags)?;
    Ok(())
}

fn network_exposure_diagnostic(interface: std::net::IpAddr) -> Option<tola_diagnostic::Diagnostic> {
    (!interface.is_loopback()).then(|| {
        tola_diagnostic::Diagnostic::new(
            crate::codes::server::NETWORK_EXPOSED,
            tola_diagnostic::Severity::Warning,
            "the local server is reachable from other machines",
        )
        .with_note(format!("bound to `{interface}` without authentication"))
        .with_help("Use `--interface 127.0.0.1` to allow only local connections")
    })
}

fn reload_port_fallback_diagnostic(port: u16) -> Option<tola_diagnostic::Diagnostic> {
    (port != DEFAULT_WS_PORT).then(|| {
        tola_diagnostic::Diagnostic::new(
            crate::codes::reload::PORT_IN_USE,
            tola_diagnostic::Severity::Warning,
            format!("port `{DEFAULT_WS_PORT}` is already in use"),
        )
        .with_note(format!("serving browser reload on port `{port}` instead"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn run_unwatched_build(
        cfg: Arc<SiteConfig>,
        sites: CurrentSite,
        output: &CommandOutput,
        purpose: ServerPurpose,
    ) -> Result<()> {
        let mut build_session = BuildSession::new();
        let hooks = matches!(&purpose, ServerPurpose::Development { .. })
            .then(|| {
                HookQueue::start(
                    output.clone(),
                    None,
                    Cancellation::default(),
                    super::super::events::Events(false),
                    build_session.hook_executor().clone(),
                )
            })
            .transpose()
            .expect("start test hook consumer");
        let cancellation = BuildCancellation::default();
        let attempt = build_session.prepare(
            Arc::clone(&cfg),
            purpose.build_request(cancellation.clone()),
        );
        let built = build_site_once(BuildAttempt {
            config: cfg,
            sites,
            output,
            purpose,
            cancellation,
            hooks: hooks.as_ref().map(HookQueue::sender),
            build_session: &mut build_session,
            attempt,
            events: super::super::events::Events(false),
        });
        if let Some(hooks) = hooks {
            hooks.finish().expect("finish test hook consumer");
        }
        built
    }

    fn site_config(root: &std::path::Path) -> Arc<SiteConfig> {
        Arc::new(
            tola_build::config::loading::resolve_site_config(
                &root.join("tola.toml"),
                "",
                tola_typst::PackageLocations::default(),
                &tola_build::config::loading::BuildOverrides::default(),
            )
            .unwrap()
            .into_config(),
        )
    }

    #[test]
    fn failed_unwatched_build_unready() {
        for (purpose, name, keeps_serving) in [
            (
                ServerPurpose::Development {
                    dev: DevConfig { watch: false },
                    disable_hook_reuse: false,
                },
                "dev",
                true,
            ),
            (ServerPurpose::Preview, "preview", false),
        ] {
            let dir = TempDir::new().unwrap();
            let cfg = site_config(dir.path());
            std::fs::create_dir_all(&cfg.build().content_dir).unwrap();
            std::fs::write(dir.path().join("site.typ"), "#panic(\"Initial failure\")").unwrap();
            let sites = CurrentSite::new();
            let output = CommandOutput::new(
                crate::terminal::Terminal::new(clap::ColorChoice::Never, false, None),
                None,
            );

            let built = run_unwatched_build(cfg, sites.clone(), &output, purpose);

            assert!(sites.revision().is_none(), "{name} published a revision");
            assert_eq!(built.is_ok(), keeps_serving);
        }
    }

    #[test]
    fn network_exposure_off_loopback() {
        for iface in ["127.0.0.1", "::1"] {
            assert!(
                network_exposure_diagnostic(iface.parse().unwrap()).is_none(),
                "{iface}"
            );
        }
        for iface in ["0.0.0.0", "::", "192.168.1.20"] {
            let diag = network_exposure_diagnostic(iface.parse().unwrap())
                .unwrap_or_else(|| panic!("{iface} must warn"));
            assert_eq!(diag.code, crate::codes::server::NETWORK_EXPOSED);
        }
    }

    #[test]
    fn reload_fallback_port_in_use() {
        assert!(reload_port_fallback_diagnostic(DEFAULT_WS_PORT).is_none());
        let port = DEFAULT_WS_PORT + 1;
        let fallback = reload_port_fallback_diagnostic(port).unwrap();
        assert_eq!(fallback.code, crate::codes::reload::PORT_IN_USE);
        assert!(
            fallback
                .notes
                .iter()
                .any(|note| note.contains(&port.to_string()))
        );
    }
}
