//! Development startup, initial revision, serving, and coordinated shutdown.

use std::sync::Arc;

use anyhow::{Context, Result};

use super::hooks::{HookQueue, HookSender};
use super::rebuild::{RebuildLoop, RebuildStartup};
use super::reload::transport::ReloadTransport;
use super::site::CurrentSite;
use crate::cancellation::Cancellation;
use crate::cli::output::CommandOutput;
use crate::config::{ConfigLoader, DevConfig, ServerConfig};
use tola_build::build::{BuildRequest, BuildSession};
use tola_build::cancellation::{BuildCancellation, BuildCanceller};
use tola_build::config::ResolvedSiteConfig;
use tola_build::filesystem::DevServerAddress;

const DEFAULT_WS_PORT: u16 = 35729;

enum ServerPurpose {
    Development {
        dev: DevConfig,
        disable_hook_reuse: bool,
    },
    Preview,
}

impl ServerPurpose {
    /// The build mode this server serves.
    fn build_mode(&self) -> tola_build::build::BuildMode {
        match self {
            Self::Development { .. } => tola_build::build::BuildMode::Development,
            Self::Preview => tola_build::build::BuildMode::Production,
        }
    }

    /// The command that started this server, as the site author typed it.
    fn command(&self) -> &'static str {
        match self {
            Self::Development { .. } => "tola dev",
            Self::Preview => "tola preview",
        }
    }
}

// TODO: Several development sessions can run against one site today (an
// editor-spawned preview beside a terminal session): each rebuilds every
// writer's changes and none knows the others. Think through later whether a
// site should hold at most one session — a session lock, or an attach path —
// and whether the view and its log should name the session they belong to.

/// Start development with observation active before its first watched build.
/// What a server session needs to start, before the purpose decides how it runs.
pub(crate) struct ServerLaunch {
    pub config: Arc<ResolvedSiteConfig>,
    pub server: ServerConfig,
    pub config_loader: ConfigLoader,
    pub resources: tola_build::BuildResources,
    pub output: CommandOutput,
    pub shutdown: Cancellation,
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
        config,
        server,
        config_loader,
        resources,
        output,
        shutdown,
    } = launch;
    shutdown.token().ensure_active()?;
    let view = match &purpose {
        ServerPurpose::Development { .. } => output
            .begin_dev_view(shutdown.clone())
            .context("could not open the development view; restart `tola dev`")?,
        ServerPurpose::Preview => None,
    };
    output.hook_overview(
        &config.build().hooks,
        purpose.build_mode(),
        match &purpose {
            ServerPurpose::Development { .. } => crate::terminal::EVERY_STAGE,
            ServerPurpose::Preview => crate::terminal::PRE_PUBLICATION_STAGES,
        },
    )?;
    output.activity(match &purpose {
        ServerPurpose::Development { .. } => "Building site…",
        ServerPurpose::Preview => "Building preview…",
    })?;
    let command = purpose.command();
    // Response assembly is this runtime's only CPU work: one worker per core up to four keeps a
    // busy host parallel without multiplying the per-connection buffers a large machine would pay for.
    let workers = std::thread::available_parallelism().map_or(2, |cores| cores.get().clamp(1, 4));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .with_context(|| {
            format!("Tola could not start its serving runtime; restart `{command}`")
        })?;
    let (listener, address, port_fallback) =
        super::http::bind_with_port_fallback(server.interface, server.port)?;
    let network_exposure = network_exposure_diagnostic(server.interface);
    let serving = runtime.block_on(async {
        // Register HTTP first so failure cannot strand rebuild or hook workers.
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let sites = CurrentSite::new();
        let watching = matches!(&purpose, ServerPurpose::Development { dev, .. } if dev.watch);
        let reload = if watching {
            let reload = ReloadTransport::start(server.interface, DEFAULT_WS_PORT, sites.clone())
                .context("Tola could not start the browser reload listener")?;
            Some(reload)
        } else {
            None
        };
        let reload_fallback = reload
            .as_ref()
            .and_then(|reload| reload_port_fallback_diagnostic(reload.endpoint().port()));
        // One batch, so the view keeps the startup's diagnostics in one round.
        let startup_diagnostics: Vec<_> = [port_fallback, network_exposure, reload_fallback]
            .into_iter()
            .flatten()
            .collect();
        if !startup_diagnostics.is_empty() {
            output.diagnostics(&startup_diagnostics)?;
        }
        let browser_diagnostics = reload.as_ref().map(ReloadTransport::diagnostics_sender);
        let hooks = match &purpose {
            ServerPurpose::Development { .. } => Some(HookQueue::start(
                output.clone(),
                browser_diagnostics,
                shutdown.token(),
            )?),
            ServerPurpose::Preview => None,
        };
        let serving = async {
            let (reload_endpoint, mut rebuild) = match purpose {
                ServerPurpose::Development { dev, disable_hook_reuse } if dev.watch => {
                    let reload_endpoint = reload.as_ref().map(ReloadTransport::endpoint);
                    let mut rebuild = RebuildLoop::start(RebuildStartup {
                        initial_config: Arc::clone(&config),
                        server: server.clone(),
                        dev,
                        disable_hook_reuse,
                        config_loader,
                        resources,
                        sites: sites.clone(),
                        reload,
                        shutdown: shutdown.clone(),
                        output: output.clone(),
                        hooks: hooks
                            .as_ref()
                            .expect("development owns a hook queue")
                            .sender(),
                    })
                    .await
                    .map_err(|error| {
                        super::watch::with_diagnostics(error.into(), config.get_root())
                    })?;
                    if let Err(error) = rebuild.build_initial().await {
                        shutdown.request();
                        return Err::<(), _>(error).and(rebuild.run().await);
                    }
                    (reload_endpoint, Some(rebuild))
                }
                purpose => {
                    let canceller = BuildCanceller::default();
                    let build_config = Arc::clone(&config);
                    let build_sites = sites.clone();
                    let build_output = output.clone();
                    let build_cancellation = canceller.token();
                    let build_resources = resources.clone();
                    let hooks = hooks.as_ref().map(HookQueue::sender);
                    let mut building = tokio::task::spawn_blocking(move || {
                        build_site_once(
                            build_config,
                            build_sites,
                            &build_output,
                            purpose,
                            build_cancellation,
                            hooks,
                            build_resources,
                        )
                    });
                    tokio::select! {
                        biased;
                        _ = shutdown.cancelled() => {
                            canceller.cancel();
                            // Blocking work must finish before the hook queue and
                            // runtime are dropped; cancellation is cooperative.
                            let _ = building.await.context(
                                "the initial site build stopped unexpectedly; run `tola check` for the full error",
                            )?;
                            return Ok(());
                        }
                        completed = &mut building => {
                            completed.context(
                                "the initial site build stopped unexpectedly; run `tola check` for the full error",
                            )??;
                        }
                    }
                    (None, None)
                }
            };

            let serving = async {
                if shutdown.is_requested() {
                    return Ok(());
                }
                let mount = config.url_mount();
                let url = if mount.is_root() {
                    format!("http://{address}")
                } else {
                    format!("http://{address}{}", mount.base_path())
                };
                let _written = DevServerAddress::write(config.get_root(), &url).with_context(|| {
                    format!("Tola could not write the address it serves; restart `{command}`")
                })?;
                output.serving(&url)?;
                match (
                    rebuild.is_some(),
                    output.terminal().is_interactive() && view.is_none(),
                ) {
                    (true, true) => {
                        output.status("Watching for changes, press Ctrl+C to stop")?
                    }
                    (true, false) => output.status("Watching for changes")?,
                    (false, true) => output.status("Press Ctrl+C to stop")?,
                    (false, false) => {}
                }
                let http = super::http::serve_http(
                    listener,
                    sites,
                    config,
                    reload_endpoint,
                    rebuild.as_ref().map(RebuildLoop::publication_sender),
                    shutdown.clone(),
                );
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
            .await;

            shutdown.request();
            // Status output can fail before the rebuild future is polled; its watcher still
            // shuts down.
            let observation = match rebuild {
                Some(rebuild) => rebuild.run().await,
                None => Ok(()),
            };
            serving.and(observation)
        }
        .await;

        shutdown.request();
        let view = match view {
            Some(view) => view.finish().context("could not finish the development view; restart `tola dev`"),
            None => Ok(()),
        };
        let hooks = match hooks {
            Some(hooks) => tokio::task::spawn_blocking(move || hooks.finish())
                .await
                .context("Tola could not finish its after-publish commands")
                .and_then(|hooks| hooks),
            None => Ok(()),
        };
        serving.and(view).and(hooks)
    });
    shutdown.request();
    serving
}

fn build_site_once(
    config: Arc<ResolvedSiteConfig>,
    sites: CurrentSite,
    output: &CommandOutput,
    purpose: ServerPurpose,
    cancellation: BuildCancellation,
    hooks: Option<HookSender>,
    resources: tola_build::BuildResources,
) -> Result<()> {
    cancellation.ensure_active()?;
    let started = std::time::Instant::now();
    let mut session = BuildSession::with_resources(resources);
    let mut request = BuildRequest::new(purpose.build_mode());
    request.cancellation = cancellation.clone();
    let build_lock = tola_build::build::SiteBuildLock::acquire(&config, &cancellation, || {
        let _ = output.waiting_for_build();
    })?;
    let candidate = session
        .prepare(Arc::clone(&config), request)
        .run()
        .map_err(|failure| failure.into_error())
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
        Err(error) => {
            cancellation.ensure_active()?;
            if !matches!(&purpose, ServerPurpose::Development { .. }) {
                return Err(error);
            }
            let mut diagnostics = tola_build::config::diagnostic::warning_diagnostics(&config);
            diagnostics.extend(tola_build::build::error_diagnostics(
                &error,
                config.get_root(),
            ));
            output.report_round(
                crate::cli::output::CompletedRound::failed(
                    None,
                    "Build failed; fix the errors and restart `tola dev`",
                ),
                &diagnostics,
            )?;
            return Ok(());
        }
    };
    let (_, current) = sites.replace(&mut session, checked)?;
    drop(build_lock);
    if let Some(hooks) = hooks {
        hooks.enqueue(Arc::clone(&current), None)?;
    }
    tracing::debug!(
        target: "tola::dev",
        routes = current.address().resource_count(),
        references = current.references().references().len(),
        diagnostics = current.diagnostics().len(),
        "installed initial site revision"
    );
    let summary = crate::terminal::site_summary(
        "Built",
        started.elapsed(),
        &crate::terminal::describe_outputs(counts),
    );
    let diagnostics = current.diagnostics().to_vec();
    output.report_round(
        crate::cli::output::CompletedRound::initial(&summary),
        &diagnostics,
    )?;
    Ok(())
}

fn network_exposure_diagnostic(
    interface: std::net::IpAddr,
) -> Option<tola_build::diagnostic::Diagnostic> {
    (!interface.is_loopback()).then(|| {
        tola_build::diagnostic::Diagnostic::new(
            crate::codes::server::NETWORK_EXPOSED,
            tola_build::diagnostic::Severity::Warning,
            "the local server is reachable from other machines",
        )
        .with_note(format!("bound to `{interface}` without authentication"))
        .with_help("Use `--interface 127.0.0.1` to allow only local connections")
    })
}

/// The warning for a reload listener that took a port other than the preferred one.
fn reload_port_fallback_diagnostic(port: u16) -> Option<tola_build::diagnostic::Diagnostic> {
    (port != DEFAULT_WS_PORT).then(|| {
        tola_build::diagnostic::Diagnostic::new(
            crate::codes::reload::PORT_IN_USE,
            tola_build::diagnostic::Severity::Warning,
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
        config: Arc<ResolvedSiteConfig>,
        sites: CurrentSite,
        output: &CommandOutput,
        purpose: ServerPurpose,
    ) -> Result<()> {
        let hooks = matches!(&purpose, ServerPurpose::Development { .. })
            .then(|| HookQueue::start(output.clone(), None, BuildCancellation::default()))
            .transpose()
            .expect("start test hook consumer");
        let built = build_site_once(
            config,
            sites,
            output,
            purpose,
            BuildCancellation::default(),
            hooks.as_ref().map(HookQueue::sender),
            tola_build::BuildResources::default(),
        );
        if let Some(hooks) = hooks {
            hooks.finish().expect("finish test hook consumer");
        }
        built
    }

    fn site_config(root: &std::path::Path) -> Arc<ResolvedSiteConfig> {
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
    fn failed_unwatched_build_publishes_nothing() {
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
            let directory = TempDir::new().unwrap();
            let config = site_config(directory.path());
            std::fs::create_dir_all(&config.build().content_dir).unwrap();
            std::fs::write(
                directory.path().join("site.typ"),
                "#panic(\"Initial failure\")",
            )
            .unwrap();
            let sites = CurrentSite::new();
            let output = crate::cli::output::CommandOutput::new(
                crate::terminal::Terminal::new(clap::ColorChoice::Never, false, None),
                None,
            );

            let built = run_unwatched_build(config, sites.clone(), &output, purpose);

            assert!(sites.revision().is_none(), "{name} published a revision");
            assert_eq!(built.is_ok(), keeps_serving);
        }
    }

    #[test]
    fn network_exposure_warns_off_loopback() {
        for interface in ["127.0.0.1", "::1"] {
            assert!(
                network_exposure_diagnostic(interface.parse().unwrap()).is_none(),
                "{interface}"
            );
        }
        for interface in ["0.0.0.0", "::", "192.168.1.20"] {
            let diagnostic = network_exposure_diagnostic(interface.parse().unwrap())
                .unwrap_or_else(|| panic!("{interface} must warn"));
            assert_eq!(diagnostic.code, crate::codes::server::NETWORK_EXPOSED);
        }
    }

    #[test]
    fn reload_fallback_reports_port_in_use() {
        assert!(reload_port_fallback_diagnostic(DEFAULT_WS_PORT).is_none());
        let port = DEFAULT_WS_PORT + 1;
        let fallback = reload_port_fallback_diagnostic(port)
            .expect("a fallback reload port reports the warning");
        assert_eq!(fallback.code, crate::codes::reload::PORT_IN_USE);
        assert!(
            fallback
                .notes
                .iter()
                .any(|note| note.contains(&port.to_string())),
            "the note names the port the listener took"
        );
    }
}
