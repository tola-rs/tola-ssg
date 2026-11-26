//! Serial rebuilds and publication of development revisions.

#[cfg(all(test, feature = "fsevents-tests"))]
mod tests;
mod watches;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tola_build::build::{
    BuildMode, BuildRequest, BuildSession, BuildTrigger, CheckedRevision, InputFreshness,
    InputKind, InputObservation, RevisionCheck, SiteBuildLock,
};
use tola_build::cancellation::BuildCanceller;
use tola_build::config::SiteConfig;
use tola_diagnostic::{Diagnostic, DiagnosticCode, Severity};

use super::events::Events;
use super::hooks::HookSender;
use super::http::PublicationRequest;
use super::reload::transport::ReloadTransport;
use super::restart::Restarts;
use super::round::{BuildProgress, CompletedRound};
use super::site::CurrentSite;
use super::watch::{
    EventClaim, FileChangeSource, RebuildScope, WatchError, WatchRequirements, WatchSelection,
    watch_requirements_from_observation,
};
use crate::cancellation::Cancellation;
use crate::config::{ConfigCandidate, ConfigLoader, DevConfig, ServerConfig};
use watches::{
    DevFailureWatches, FailureRetry, InvocationSettings, ProducerReuseCoverage, RebuildRequest,
    attached_diagnostics, format_rebuild_summary, invocation_config_change, merge_paths,
    producer_reuse_capabilities, retained_with_previous_warnings, stale_retry_scope,
};

struct RoundConfiguration {
    config: Arc<SiteConfig>,
    candidate: Option<ConfigCandidate>,
}

enum RoundAwait<T> {
    Finished(Result<T, tokio::task::JoinError>),
    Superseded(RebuildRequest),
    Stopped,
}

#[derive(Clone, Copy)]
enum DevBuildTrigger {
    Startup,
    FilesChanged,
}

pub(super) struct RebuildLoop {
    initial_config: Arc<SiteConfig>,
    server: ServerConfig,
    dev: DevConfig,
    disable_hook_reuse: bool,
    config_loader: ConfigLoader,
    sites: CurrentSite,
    changes: FileChangeSource,
    reload: Option<ReloadTransport>,
    shutdown: Cancellation,
    build_session: BuildSession,
    failure_watches: DevFailureWatches,
    reported_diagnostics: Vec<Diagnostic>,
    round: u64,
    output: crate::cli::output::CommandOutput,
    hooks: HookSender,
    events: Events,
    build: u64,
    attempt_diagnostics: Vec<Diagnostic>,
    attempt_published: bool,
    restarts: Restarts,
    publication_tx: tokio::sync::mpsc::Sender<PublicationRequest>,
    publication_rx: tokio::sync::mpsc::Receiver<PublicationRequest>,
}

pub(super) struct RebuildStartup {
    pub(super) initial_config: Arc<SiteConfig>,
    pub(super) server: ServerConfig,
    pub(super) dev: DevConfig,
    pub(super) disable_hook_reuse: bool,
    pub(super) config_loader: ConfigLoader,
    pub(super) build_session: BuildSession,
    pub(super) sites: CurrentSite,
    pub(super) reload: Option<ReloadTransport>,
    pub(super) shutdown: Cancellation,
    pub(super) output: crate::cli::output::CommandOutput,
    pub(super) hooks: HookSender,
    pub(super) events: Events,
}

struct PublishTimings {
    rebuild_started: Instant,
    prepare_elapsed: Duration,
    build_elapsed: Duration,
    rev_elapsed: Duration,
    subscriptions_elapsed: Duration,
    freshness_elapsed: Duration,
    output_count: usize,
}

impl RebuildLoop {
    pub(super) async fn start(input: RebuildStartup) -> Result<Self, WatchError> {
        let changes = FileChangeSource::start(
            &input.initial_config,
            &WatchRequirements::default(),
            &tola_build::hooks::SourceHookOutputs::default(),
            input.output.log().map(crate::cli::log::LogFile::path),
        )
        .await?;
        let (publication_tx, publication_rx) = tokio::sync::mpsc::channel(8);
        Ok(Self {
            initial_config: input.initial_config,
            server: input.server,
            dev: input.dev,
            disable_hook_reuse: input.disable_hook_reuse,
            config_loader: input.config_loader,
            sites: input.sites,
            changes,
            reload: input.reload,
            shutdown: input.shutdown,
            build_session: input.build_session,
            failure_watches: DevFailureWatches::default(),
            reported_diagnostics: Vec::new(),
            round: 0,
            output: input.output,
            hooks: input.hooks,
            events: input.events,
            build: 0,
            attempt_diagnostics: Vec::new(),
            attempt_published: false,
            restarts: Restarts::default(),
            publication_tx,
            publication_rx,
        })
    }

    pub(super) async fn build_initial(&mut self) -> Result<()> {
        self.settle_build(
            RebuildRequest {
                paths: Vec::new(),
                rebuild_scope: RebuildScope::FullSite,
            },
            DevBuildTrigger::Startup,
        )
        .await
    }

    pub(super) fn publication_sender(&self) -> tokio::sync::mpsc::Sender<PublicationRequest> {
        self.publication_tx.clone()
    }

    pub(super) async fn run(mut self) -> Result<()> {
        let run = self.run_inner().await;
        let reload = match self.reload.take() {
            Some(reload) => reload.shutdown().await,
            None => Ok(()),
        };
        let obs = self.changes.shutdown().await;
        run.and(reload).and(obs.map_err(Into::into)).map_err(|err| {
            let root = self.initial_config.root().to_path_buf();
            crate::dev::watch::with_diagnostics(err, &root)
        })
    }

    async fn run_inner(&mut self) -> Result<()> {
        tracing::debug!(target: "tola::dev", "development rebuild loop started");
        while !self.shutdown.is_requested() {
            self.changes.refresh_pending().await?;
            let cfg = self.current_config();
            let batch = tokio::select! {
                _ = self.shutdown.cancelled() => break,
                publication = self.publication_rx.recv() => {
                    if let Some(publication) = publication {
                        let prev = self.sites.revision();
                        // FullSite validates disk inputs even when the output revision ID stays equal.
                        self.settle_build(
                            RebuildRequest {
                                paths: Vec::new(),
                                rebuild_scope: RebuildScope::FullSite,
                            },
                            DevBuildTrigger::FilesChanged,
                        ).await?;
                        if self.shutdown.is_requested() {
                            break;
                        }
                        let published = self.sites.revision().filter(|cur| {
                            prev.as_ref().is_none_or(|prev| !Arc::ptr_eq(prev, cur))
                        });
                        let reply = published.ok_or_else(|| {
                            self.reported_diagnostics
                                .iter()
                                .filter(|diag| diag.severity == Severity::Error)
                                .count()
                                .max(1)
                        });
                        let _ = publication.send(reply);
                    }
                    continue;
                }
                batch = self.changes.next(&cfg) => batch?,
            };
            self.settle_build(
                RebuildRequest::from_batch(batch),
                DevBuildTrigger::FilesChanged,
            )
            .await?;
        }
        tracing::debug!(target: "tola::dev", "development rebuild loop stopped");
        Ok(())
    }

    async fn settle_build(
        &mut self,
        mut req: RebuildRequest,
        trigger: DevBuildTrigger,
    ) -> Result<()> {
        self.round += 1;
        // Hook events carry this span so after-publish logs stay linked to their originating round.
        let round = tracing::info_span!(target: "tola::dev", "round", round = self.round);
        let _rebuild_status = self.reload.as_ref().map(ReloadTransport::begin_rebuild);
        self.restarts = Restarts::default();
        self.report_progress(
            Some(BuildProgress {
                title: "Building site…".to_owned(),
                detail: String::new(),
                warning: false,
            }),
            true,
        )?;
        let mut failure_retry = FailureRetry::default();
        loop {
            self.build += 1;
            self.attempt_diagnostics.clear();
            self.attempt_published = false;
            self.events.build_started(self.build)?;
            let completion = self.rebuild(req, trigger, &round, &mut failure_retry).await;
            if self.events.0 && !self.attempt_published {
                self.emit_unpublished_event(&completion)?;
            }
            // Release deferred hook events even if failure preceded worker startup.
            let hook_changes = self.changes.reject_hook_candidate();
            let mut superseding = match completion {
                Ok(Some(superseding)) => superseding,
                Ok(None) => break,
                Err(err)
                    if self.shutdown.is_requested()
                        && err.chain().any(|cause| {
                            cause.is::<tola_build::cancellation::BuildCancelled>()
                        }) =>
                {
                    break;
                }
                Err(err) => return Err(err),
            };
            if self.shutdown.is_requested() {
                break;
            }
            if let Some(hook_changes) = hook_changes {
                superseding.merge_batch(hook_changes);
            }
            req = superseding;
        }
        self.report_progress(None, false)?;
        Ok(())
    }

    fn emit_unpublished_event(
        &mut self,
        completion: &Result<Option<RebuildRequest>>,
    ) -> Result<()> {
        let status = if !self.attempt_diagnostics.is_empty()
            || completion
                .as_ref()
                .err()
                .is_some_and(|err| !tola_build::cancellation::is_cancelled(err))
        {
            "failed"
        } else {
            "cancelled"
        };
        if self.attempt_diagnostics.is_empty()
            && status == "failed"
            && let Err(err) = completion
        {
            self.attempt_diagnostics =
                tola_build::build::error_diagnostics(err, self.initial_config.root());
        }
        let rev = self.sites.revision();
        self.events.build_finished(
            self.build,
            status,
            false,
            rev.as_ref().map(|site| site.manifest().revision().as_str()),
            &retained_with_previous_warnings(
                &self.reported_diagnostics,
                std::mem::take(&mut self.attempt_diagnostics),
            ),
        )
    }

    fn report_progress(&self, progress: Option<BuildProgress>, print: bool) -> Result<()> {
        self.events.build_progress(progress.as_ref())?;
        self.output.build_progress(progress.as_ref(), print)?;
        Ok(())
    }

    fn report_restart(&mut self, reason: &str, paths: &[PathBuf], cfg: &SiteConfig) -> Result<()> {
        let Some(print) = self.restarts.record(Instant::now()) else {
            return Ok(());
        };
        let watches = self
            .changes
            .hook_watches(paths)
            .into_iter()
            .map(|watch| {
                let file = crate::terminal::display_path_within(&watch.file, cfg.root());
                format!(
                    "`{}` in `{file}` (`hooks.{}.rerun-on`)",
                    watch.name, watch.stage
                )
            })
            .collect::<Vec<_>>();
        let paths = paths
            .iter()
            .map(|path| crate::terminal::display_path_within(path, cfg.root()))
            .collect::<Vec<_>>();
        let progress =
            self.restarts
                .progress(reason, &paths, &watches, self.sites.revision().is_some());
        self.report_progress(Some(progress), print)
    }

    async fn await_task<T>(
        &mut self,
        mut build: tokio::task::JoinHandle<T>,
        (canceller, hook_epoch): (&BuildCanceller, Option<u64>),
        cfg: &SiteConfig,
        changed_paths: &[PathBuf],
        rebuild_scope: RebuildScope,
    ) -> Result<RoundAwait<T>> {
        let mut superseding: Option<RebuildRequest> = None;
        let mut cancellation_started = None;
        let cancel = |session: &BuildSession| {
            if let Some(epoch) = hook_epoch {
                session.hook_executor().invalidate(epoch);
            }
            canceller.cancel();
        };
        let completion = loop {
            tokio::select! {
                biased;
                _ = self.shutdown.cancelled() => {
                    cancel(&self.build_session);
                    break build.await;
                }
                batch = self.changes.next(cfg) => {
                    let batch = match batch {
                        Ok(batch) => batch,
                        Err(err) => {
                            cancel(&self.build_session);
                            let _ = build.await;
                            return Err(err.into());
                        }
                    };
                    if superseding.is_none() {
                        cancellation_started = Some(Instant::now());
                        cancel(&self.build_session);
                        let reason = if batch.paths().is_empty() {
                            "File watching lost change details"
                        } else {
                            "Files changed during the build"
                        };
                        if let Err(err) = self.report_restart(reason, batch.paths(), cfg) {
                            let _ = build.await;
                            return Err(err);
                        }
                        superseding = Some(RebuildRequest {
                            paths: changed_paths.to_vec(),
                            rebuild_scope,
                        });
                    }
                    superseding.as_mut().expect("initialized above").merge_batch(batch);
                }
                completion = &mut build => break completion,
            }
        };
        if self.shutdown.is_requested() {
            self.changes.reject_hook_candidate();
            return Ok(RoundAwait::Stopped);
        }
        if let Some(mut superseding) = superseding {
            if let Some(hook_changes) = self.changes.reject_hook_candidate() {
                superseding.merge_batch(hook_changes);
            }
            tracing::debug!(
                target: "tola::compile",
                changes = superseding.paths.len(),
                rebuild_scope = ?superseding.rebuild_scope,
                cancel_wait_ms = cancellation_started.map(|s| s.elapsed().as_secs_f64() * 1000.0),
                "discarded superseded site candidate"
            );
            return Ok(RoundAwait::Superseded(superseding));
        }
        Ok(RoundAwait::Finished(completion))
    }

    fn current_config(&self) -> Arc<SiteConfig> {
        self.sites
            .revision()
            .map(|site| Arc::clone(site.config()))
            .unwrap_or_else(|| Arc::clone(&self.initial_config))
    }

    async fn resolve_round_config(
        &mut self,
        cur_cfg: &Arc<SiteConfig>,
    ) -> Result<Option<RoundConfiguration>> {
        let cfg_path = cur_cfg.config_path().to_path_buf();
        let candidate = match self.config_loader.load_candidate() {
            Ok(candidate) => candidate,
            Err(err) => {
                self.failure_watches.config_failed_paths = err
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<tola_build::config::ConfigSourceError>())
                    .map(|err| err.paths().iter().cloned().collect())
                    .unwrap_or_default();
                self.reconfigure_failure_watches(cur_cfg, cur_cfg).await?;
                let diags = tola_diagnostic::attached(&err)
                    .map(|diags| diags.to_vec())
                    .unwrap_or_else(|| {
                        vec![
                            tola_diagnostic::fallback(crate::codes::config::RELOAD, &err)
                                .with_path(crate::terminal::display_path_within(
                                    &cfg_path,
                                    cur_cfg.root(),
                                )),
                        ]
                    });
                self.report_diagnostics(diags);
                return Ok(None);
            }
        };
        self.failure_watches.config_failed_paths.clear();
        if let Some(candidate) = candidate.as_ref()
            && let Some(change) = invocation_config_change(
                InvocationSettings {
                    config: cur_cfg,
                    server: &self.server,
                    dev: &self.dev,
                },
                InvocationSettings {
                    config: &candidate.config(),
                    server: candidate.server(),
                    dev: candidate.dev(),
                },
            )
        {
            let detail =
                format!("`{change}` cannot be changed while the development server is running");
            self.report_diagnostics(vec![
                Diagnostic::at_path(
                    crate::codes::config::SERVE_RESTART,
                    Severity::Error,
                    crate::terminal::display_path_within(&cfg_path, cur_cfg.root()),
                    detail,
                )
                .with_help("Restart `tola dev`"),
            ]);
            self.config_loader.acknowledge(candidate);
            return Ok(None);
        }
        let cfg = candidate
            .as_ref()
            .map(ConfigCandidate::config)
            .unwrap_or_else(|| Arc::clone(cur_cfg));
        if let Err(err) =
            crate::cli::log::destination::start_site(&self.output, &cfg, &self.shutdown.token())
        {
            if self.shutdown.is_requested() {
                return Ok(None);
            }
            self.output.record_failure(&err);
            self.report_diagnostics(vec![
                Diagnostic::new(
                    crate::codes::log::LOCATION,
                    Severity::Error,
                    "the log file conflicts with a site input or output",
                )
                .with_help("Choose a `--log-file` path outside the site"),
            ]);
            return Ok(None);
        }
        Ok(Some(RoundConfiguration {
            config: cfg,
            candidate,
        }))
    }

    async fn rebuild(
        &mut self,
        req: RebuildRequest,
        trigger: DevBuildTrigger,
        round: &tracing::Span,
        failure_retry: &mut FailureRetry,
    ) -> Result<Option<RebuildRequest>> {
        macro_rules! round_await {
            ($task:expr, $cancellation:expr, $cfg:expr, $paths:expr, $scope:expr) => {
                match self
                    .await_task($task, $cancellation, $cfg, $paths, $scope)
                    .await?
                {
                    RoundAwait::Finished(val) => val,
                    RoundAwait::Superseded(req) => return Ok(Some(req)),
                    RoundAwait::Stopped => return Ok(None),
                }
            };
        }

        let rebuild_started = Instant::now();
        self.changes.refresh_pending().await?;
        let cur_site = self.sites.revision();
        let cur_cfg = cur_site
            .as_ref()
            .map(|site| Arc::clone(site.config()))
            .unwrap_or_else(|| Arc::clone(&self.initial_config));
        let empty_obs = InputObservation::default();
        let cur_obs = cur_site
            .as_ref()
            .map_or(&empty_obs, |site| site.input_observation());
        let event_claim = self.changes.begin_candidate(&cur_cfg);
        let build_event_epoch = event_claim.revision();
        let mut changed_paths = req.paths;
        let rebuild_scope = req
            .rebuild_scope
            .max(event_claim.rebuild_scope())
            .max(event_claim.merge_into(&mut changed_paths));
        let sample = changed_paths
            .iter()
            .take(crate::cli::log::LOGGED_PATH_SAMPLE)
            .collect::<Vec<_>>();
        tracing::debug!(
            target: "tola::compile",
            round = self.round,
            build_event_epoch,
            rebuild_scope = ?rebuild_scope,
            changed_path_count = changed_paths.len(),
            sample_paths = ?sample,
            "starting site build"
        );
        let Some(round_cfg) = self.resolve_round_config(&cur_cfg).await? else {
            return Ok(None);
        };
        let RoundConfiguration {
            config: cfg,
            candidate,
        } = round_cfg;
        self.changes.update_hook_candidate_claims(&cfg);
        let cur_watch_reqs = watch_requirements_from_observation(cur_obs);
        self.changes.stage_config(&cfg, &cur_watch_reqs).await?;

        let canceller = BuildCanceller::new();
        let hook_watch = if cfg.modules().is_empty() {
            None
        } else {
            let watch_cfg = Arc::clone(&cfg);
            let mode = self.build_session.input_mode();
            let cancellation = canceller.token();
            let prep = tokio::task::spawn_blocking(move || {
                tola_build::modules::prepare_hook_watch(&watch_cfg, mode, &cancellation)
            });
            match round_await!(
                prep,
                (&canceller, None),
                &cfg,
                &changed_paths,
                rebuild_scope
            ) {
                Ok(Ok(watch)) => Some(watch),
                Ok(Err(err)) => {
                    self.failure_watches.build_task_failed(&cfg);
                    self.reconfigure_failure_watches(&cur_cfg, &cfg).await?;
                    self.report_build_failure(&err);
                    return Ok(None);
                }
                Err(err) => {
                    return Err(anyhow::Error::new(err)
                        .context("Tola could not finish preparing module hook watches"));
                }
            }
        };
        let watch_cfg = hook_watch
            .as_ref()
            .map_or_else(|| Arc::clone(&cfg), |watch| watch.config_arc().clone());
        let hook_inputs = hook_watch
            .as_ref()
            .map(|watch| watch.inputs().clone())
            .unwrap_or_default();
        let mut staged_reqs = cur_watch_reqs;
        staged_reqs.add_module_hook_inputs(&hook_inputs);
        self.changes.update_hook_candidate_claims(&watch_cfg);
        self.changes.extend_hook_candidate_claims(&hook_inputs);
        self.changes.stage_config(&watch_cfg, &staged_reqs).await?;
        // Attach declared hook coverage before any generator can write into it.
        self.changes.refresh_pending().await?;

        let build_req = BuildRequest {
            mode: BuildMode::Development,
            trigger: match trigger {
                DevBuildTrigger::Startup => BuildTrigger::Initial,
                DevBuildTrigger::FilesChanged => {
                    let paths = changed_paths.clone().into();
                    if rebuild_scope == RebuildScope::FullSite {
                        BuildTrigger::AllInputs(paths)
                    } else {
                        BuildTrigger::Paths(paths)
                    }
                }
            },
            reuse: producer_reuse_capabilities(
                rebuild_scope,
                ProducerReuseCoverage::of(&self.changes),
            ),
            generated_files: Vec::new(),
            cancellation: canceller.token(),
            hook_execution: tola_build::build::HookExecution::Run,
            reference_checks: tola_build::build::ReferenceChecks::Full,
            disable_hook_reuse: self.disable_hook_reuse,
            capture_report: false,
        };
        let attempt = if let Some(hook_watch) = &hook_watch {
            self.build_session
                .prepare_with_hook_watch(hook_watch, build_req)?
        } else {
            self.build_session.prepare(Arc::clone(&cfg), build_req)
        };
        let attempt_cancellation = (&canceller, Some(attempt.hook_epoch()));
        let prepare_elapsed = rebuild_started.elapsed();
        let build_started = Instant::now();
        let lock_cfg = Arc::clone(&cfg);
        let lock_cancellation = canceller.token();
        let lock_output = self.output.clone();
        let locking = tokio::task::spawn_blocking(move || {
            SiteBuildLock::acquire(&lock_cfg, &lock_cancellation, || {
                let _ = lock_output.waiting_for_build();
            })
        });
        let build_lock = match round_await!(
            locking,
            attempt_cancellation,
            &cfg,
            &changed_paths,
            rebuild_scope
        ) {
            Ok(Ok(lock)) => lock,
            Ok(Err(err)) => {
                self.report_build_failure(&anyhow::Error::new(err));
                return Ok(None);
            }
            Err(err) => {
                self.output.record_failure(&anyhow::Error::new(err));
                self.report_diagnostics(vec![
                    Diagnostic::new(
                        crate::codes::build::TASK,
                        Severity::Error,
                        "Tola could not wait for another build of this site",
                    )
                    .with_help(
                        "Wait for the other `tola` command to finish, then save a file to rebuild",
                    ),
                ]);
                return Ok(None);
            }
        };

        let build_round = round.clone();
        let build = tokio::task::spawn_blocking(move || build_round.in_scope(|| attempt.run()));
        let built = match round_await!(
            build,
            attempt_cancellation,
            &watch_cfg,
            &changed_paths,
            rebuild_scope
        ) {
            Ok(Ok(built)) => built,
            Ok(Err(failure)) => {
                if failure.is_cancelled() {
                    self.report_restart("The build was cancelled", &[], &watch_cfg)?;
                    let mut retry = RebuildRequest {
                        paths: changed_paths,
                        rebuild_scope,
                    };
                    if let Some(hook_changes) = self.changes.reject_hook_candidate() {
                        retry.merge_batch(hook_changes);
                    }
                    return Ok(Some(retry));
                }
                self.failure_watches.observe_failure(&failure);
                self.failure_watches.module_hook_inputs = Some(hook_inputs.clone());
                let observation_extended = self.reconfigure_failure_watches(&cur_cfg, &cfg).await?;
                let diags = failure.diagnostics(self.current_config().root());
                self.report_build_diagnostics(diags);
                let shutdown = self.shutdown.token();
                let hook_retry = tokio::select! {
                    _ = self.shutdown.cancelled() => return Ok(None),
                    settled = self.changes.settle_failed_hook_candidate(
                        failure.source_hooks(),
                        failure.failed_hook_outputs(),
                        &shutdown,
                    ) => settled?,
                }
                .map(RebuildRequest::from_batch);
                return Ok(RebuildRequest::after_failure(
                    changed_paths,
                    // Partial hook outputs alone are not a reason to rerun a failed source hook.
                    observation_extended && !failure.source_hooks_failed(),
                    hook_retry,
                    failure_retry,
                ));
            }
            Err(err) => {
                self.failure_watches.build_task_failed(&cfg);
                self.failure_watches.module_hook_inputs = Some(hook_inputs.clone());
                let observation_extended = self.reconfigure_failure_watches(&cur_cfg, &cfg).await?;
                self.output.record_failure(&anyhow::Error::new(err));
                self.report_diagnostics(vec![
                    Diagnostic::new(
                        crate::codes::build::TASK,
                        Severity::Error,
                        "Tola could not finish the build",
                    )
                    .with_help("Save a file to rebuild"),
                ]);
                let hook_retry = self
                    .changes
                    .reject_hook_candidate()
                    .map(RebuildRequest::from_batch);
                return Ok(RebuildRequest::after_failure(
                    changed_paths,
                    observation_extended,
                    hook_retry,
                    failure_retry,
                ));
            }
        };

        let build_elapsed = build_started.elapsed();
        let effective_cfg = Arc::clone(built.config());
        self.changes.update_hook_candidate_claims(&effective_cfg);
        let candidate_obs = built.input_observation();
        let candidate_watch_reqs = watch_requirements_from_observation(&candidate_obs);
        let candidate_subs = self
            .changes
            .stage_config(&effective_cfg, &candidate_watch_reqs)
            .await?;
        self.changes.refresh_pending().await?;
        let subscriptions_elapsed = build_started.elapsed() - build_elapsed;
        let output_count = built.graph().outputs().len();
        let source_hooks = built.source_hooks().clone();
        let hook_outputs = source_hooks.outputs().to_vec();
        let prev_site = cur_site;
        let rev_cancellation = canceller.token();
        let preparing = tokio::task::spawn_blocking(move || {
            let started = Instant::now();
            let candidate =
                built.into_unchecked_revision(prev_site.as_ref().map(|site| site.outputs()));
            let rev_elapsed = started.elapsed();
            let diags = candidate.site().diagnostics().to_vec();
            let checked = candidate.check(&rev_cancellation);
            let freshness_elapsed = started.elapsed() - rev_elapsed;
            (checked, rev_elapsed, freshness_elapsed, diags)
        });
        let (checked, rev_elapsed, freshness_elapsed, diags) = round_await!(
            preparing,
            attempt_cancellation,
            &effective_cfg,
            &changed_paths,
            rebuild_scope
        )
        .map_err(|err| anyhow::Error::new(err).context("Tola could not finish the rebuild"))?;
        let checked = match checked? {
            RevisionCheck::Fresh(checked) => checked,
            RevisionCheck::Stale(freshness) => {
                self.report_restart(
                    "Inputs changed before the build could be published",
                    &[],
                    &effective_cfg,
                )?;
                return Ok(Some(self.retry_for_stale_candidate(
                    freshness,
                    &candidate_obs,
                    changed_paths,
                    rebuild_scope,
                )));
            }
        };

        self.publish_checked_revision(
            checked,
            event_claim,
            build_event_epoch,
            candidate_subs,
            candidate.as_ref(),
            &effective_cfg,
            &hook_outputs,
            &source_hooks,
            build_lock,
            diags,
            changed_paths,
            PublishTimings {
                rebuild_started,
                prepare_elapsed,
                build_elapsed,
                rev_elapsed,
                subscriptions_elapsed,
                freshness_elapsed,
                output_count,
            },
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn publish_checked_revision(
        &mut self,
        checked: CheckedRevision,
        event_claim: EventClaim,
        build_event_epoch: u64,
        candidate_subs: WatchSelection,
        candidate_cfg: Option<&ConfigCandidate>,
        effective_cfg: &Arc<SiteConfig>,
        hook_outputs: &[tola_build::hooks::HookOutput],
        source_hooks: &tola_build::hooks::SourceHookOutputs,
        build_lock: SiteBuildLock,
        diags: Vec<Diagnostic>,
        mut changed_paths: Vec<PathBuf>,
        timings: PublishTimings,
    ) -> Result<Option<RebuildRequest>> {
        let publication_started = Instant::now();
        let guarded_event_epoch = self.changes.event_epoch();
        let guarded_changes = self
            .changes
            .rebuild_changes_after(build_event_epoch, effective_cfg);
        let (prev, cur) = {
            let shutdown = self.shutdown.token();
            let guarded = tokio::select! {
                _ = self.shutdown.cancelled() => return Ok(None),
                guarded = self.changes.guard_epoch(
                    build_event_epoch,
                    effective_cfg,
                    hook_outputs,
                    &shutdown,
                ) => guarded?,
            };
            if guarded.is_none() {
                drop(guarded);
                let (newer_paths, _) = guarded_changes.into_parts();
                self.report_restart(
                    "Inputs changed before the build could be published",
                    &newer_paths,
                    effective_cfg,
                )?;
                merge_paths(&mut changed_paths, newer_paths);
                merge_paths(
                    &mut changed_paths,
                    std::iter::once(effective_cfg.root().to_path_buf()),
                );
                tracing::debug!(
                    target: "tola::compile",
                    build_event_epoch,
                    guarded_event_epoch,
                    changes = changed_paths.len(),
                    rebuild_scope = ?RebuildScope::FullSite,
                    "discarded candidate after a newer filesystem event was accepted"
                );
                return Ok(Some(RebuildRequest {
                    paths: changed_paths,
                    rebuild_scope: RebuildScope::FullSite,
                }));
            }
            let epoch_guard = guarded.expect("checked guarded epoch");
            if self.shutdown.is_requested() {
                drop(epoch_guard);
                self.changes.reject_hook_candidate();
                return Ok(None);
            }
            let (prev, cur) = self.sites.replace(&mut self.build_session, checked)?;
            self.failure_watches.current_site_replaced();
            epoch_guard.commit(event_claim, candidate_subs);
            (prev, cur)
        };
        self.attempt_published = true;
        self.events.build_finished(
            self.build,
            "succeeded",
            true,
            Some(cur.manifest().revision().as_str()),
            &diags,
        )?;
        drop(build_lock);
        self.changes.complete_hook_candidate(source_hooks);
        let prev_reported = std::mem::replace(&mut self.reported_diagnostics, diags);
        let recovered = prev_reported
            .iter()
            .any(|diag| diag.severity == Severity::Error);
        let rev_diff = prev
            .as_ref()
            .map(|prev| prev.manifest().diff(cur.manifest()));
        if let Some(paths) = rev_diff
            .as_ref()
            .and_then(|diff| diff.describe_changed_paths())
        {
            tracing::debug!(
                target: "tola::compile",
                round = self.round,
                "changed outputs\n{paths}"
            );
        }
        let changed_groups = rev_diff.as_ref().map(|diff| diff.changed_output_groups());
        if let Some(reload) = self.reload.as_ref() {
            match rev_diff {
                Some(diff) => reload.revision_replaced(diff, &self.reported_diagnostics),
                None => reload.first_revision_ready(
                    cur.manifest().revision().clone(),
                    &self.reported_diagnostics,
                ),
            }
        }
        self.hooks.enqueue(Arc::clone(&cur), Some(self.round))?;
        tracing::debug!(
            target: "tola::compile",
            round = self.round,
            build_event_epoch,
            revision = cur.manifest().revision().as_str(),
            prepare_ms = timings.prepare_elapsed.as_secs_f64() * 1000.0,
            build_ms = timings.build_elapsed.as_secs_f64() * 1000.0,
            revision_ms = timings.rev_elapsed.as_secs_f64() * 1000.0,
            subscriptions_ms = timings.subscriptions_elapsed.as_secs_f64() * 1000.0,
            freshness_ms = timings.freshness_elapsed.as_secs_f64() * 1000.0,
            publication_ms = publication_started.elapsed().as_secs_f64() * 1000.0,
            rebuild_ms = timings.rebuild_started.elapsed().as_secs_f64() * 1000.0,
            outputs = timings.output_count,
            routes = cur.address().resource_count(),
            references = cur.references().references().len(),
            diagnostics = cur.diagnostics().len(),
            "replaced current site"
        );
        if let Some(candidate) = candidate_cfg {
            self.config_loader.acknowledge(candidate);
            self.output.apply_diagnostic_limits(candidate.diagnostics());
        }
        let summary =
            format_rebuild_summary(changed_groups, timings.rebuild_started.elapsed(), recovered);
        let round =
            CompletedRound::published(self.round, cur.manifest().revision().as_str(), &summary);
        if self.output.log().is_some() {
            for diag in attached_diagnostics(&prev_reported, &self.reported_diagnostics) {
                self.output.record_round_result(round.identity(), diag);
            }
        }
        if let Err(err) = self.output.report_round(round, &self.reported_diagnostics) {
            self.output.record_failure(&anyhow::Error::new(err));
            self.report_build_error(
                crate::codes::terminal::WRITE,
                "Tola could not write diagnostics to this terminal".into(),
            );
        }
        Ok(None)
    }

    fn retry_for_stale_candidate(
        &mut self,
        freshness: InputFreshness,
        obs: &InputObservation,
        mut changed_paths: Vec<PathBuf>,
        mut rebuild_scope: RebuildScope,
    ) -> RebuildRequest {
        if freshness.is_stale(InputKind::TypstPhysicalReads) {
            rebuild_scope = rebuild_scope.max(merge_paths(
                &mut changed_paths,
                obs.physical_read_paths().iter().cloned(),
            ));
        }
        if freshness.is_stale(InputKind::TypstPackageSelection) {
            rebuild_scope = rebuild_scope.max(merge_paths(
                &mut changed_paths,
                obs.package_checks()
                    .iter()
                    .map(|check| check.candidate().to_path_buf()),
            ));
        }
        rebuild_scope = rebuild_scope.max(stale_retry_scope(&freshness));
        tracing::debug!(
            target: "tola::compile",
            changes = changed_paths.len(),
            rebuild_scope = ?rebuild_scope,
            physical_reads_fresh = freshness.is_fresh(InputKind::TypstPhysicalReads),
            package_checks_fresh = freshness.is_fresh(InputKind::TypstPackageSelection),
            content_inventory_fresh = freshness.is_fresh(InputKind::ContentInventory),
            configured_assets_fresh = freshness.is_fresh(InputKind::SiteAssets),
            font_inventory_fresh = freshness.is_fresh(InputKind::FontInventory),
            "discarded candidate after input evidence changed during its build"
        );
        let mut retry = RebuildRequest {
            paths: changed_paths,
            rebuild_scope,
        };
        if let Some(hook_changes) = self.changes.reject_hook_candidate() {
            retry.merge_batch(hook_changes);
        }
        retry
    }

    fn report_build_failure(&mut self, err: &anyhow::Error) {
        let cfg = self.current_config();
        let diags = tola_build::build::error_diagnostics(err, cfg.root());
        self.report_build_diagnostics(diags);
    }

    fn report_build_diagnostics(&mut self, diags: Vec<Diagnostic>) {
        tracing::debug!(
            target: "tola::compile",
            round = self.round,
            errors = diags.len(),
            "site rebuild failed"
        );
        self.report_diagnostics(diags);
    }

    fn report_build_error(&mut self, code: DiagnosticCode, detail: String) {
        tracing::debug!(
            target: "tola::compile",
            round = self.round,
            code = code.as_str(),
            "site rebuild failed"
        );
        self.report_diagnostics(vec![Diagnostic::new(code, Severity::Error, detail)]);
    }

    async fn reconfigure_failure_watches(
        &mut self,
        cur_cfg: &SiteConfig,
        attempted_cfg: &SiteConfig,
    ) -> Result<bool, WatchError> {
        let cur_site = self.sites.revision();
        let empty_obs = InputObservation::default();
        let observed = cur_site
            .as_ref()
            .map_or(&empty_obs, |site| site.input_observation());
        let dynamic = self.failure_watches.requirements(observed);
        self.changes
            .reconfigure_configs([cur_cfg, attempted_cfg], &dynamic)
            .await
    }

    fn report_diagnostics(&mut self, diags: Vec<Diagnostic>) {
        if self.events.0 && !self.attempt_published {
            self.attempt_diagnostics.clone_from(&diags);
        }
        let status = if diags.iter().all(|diag| diag.code == "terminal.write") {
            "Could not write diagnostics to the terminal"
        } else if diags
            .iter()
            .all(|diag| diag.code == crate::codes::config::SERVE_RESTART)
        {
            "Restart required; the current site is unchanged"
        } else if self.sites.revision().is_some() {
            "Build failed. Still serving the last successful build."
        } else {
            "Build failed; fix the errors and save to rebuild"
        };
        let diags = retained_with_previous_warnings(&self.reported_diagnostics, diags);
        self.reported_diagnostics.clone_from(&diags);
        if let Err(err) = self.output.report_round(
            CompletedRound::failed(Some(self.round), status),
            &self.reported_diagnostics,
        ) {
            self.output.record_failure(&anyhow::Error::new(err));
        }
        if let Some(reload) = self.reload.as_ref() {
            reload.replace_diagnostics(&self.reported_diagnostics);
        }
    }
}
