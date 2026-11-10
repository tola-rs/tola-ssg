//! Serial rebuilds and publication of development revisions.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use super::site::CurrentSite;
use crate::cancellation::Cancellation;
use crate::config::{ConfigLoader, DevConfig, ServerConfig};
use crate::dev::reload::transport::ReloadTransport;
use crate::dev::watch::{
    FileChangeBatch, FileChangeSource, ProducerKind, RebuildScope, WatchError, WatchRequirements,
    append_observed_paths, append_typst_inputs, watch_requirements_from_observation,
};
use tola_build::build::{BuildTrigger, InputObservation, RevisionCheck};
use tola_build::config::ResolvedSiteConfig;
use tola_diagnostic::{Diagnostic, DiagnosticCode, Severity};

#[derive(Default)]
struct DevFailureWatches {
    latest_failed_paths: BTreeSet<PathBuf>,
    latest_failed_packages: Vec<tola_typst::PackageCheck>,
    latest_failed_observed_paths: Vec<tola_build::build::ObservedInputPath>,
    typst_recovery_roots: Vec<PathBuf>,
    module_hook_inputs: Option<tola_module::HookWatchInputs>,
}

impl DevFailureWatches {
    fn observe_failure(&mut self, failure: &tola_build::build::BuildFailure) {
        let observation = failure.input_observation();
        let paths = observation.physical_read_paths().iter().cloned().chain(
            failure.hook_outputs().iter().flat_map(|evidence| {
                [
                    evidence.logical_path().to_path_buf(),
                    evidence.physical_path().to_path_buf(),
                ]
            }),
        );
        self.build_failed(paths, observation.package_checks().to_vec());
        self.latest_failed_observed_paths = observation.paths().to_vec();
    }

    fn build_failed(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
        package_checks: Vec<tola_typst::PackageCheck>,
    ) {
        *self = Self::default();
        self.latest_failed_paths = paths.into_iter().collect();
        self.latest_failed_packages = package_checks;
    }

    fn build_task_failed(&mut self, config: &ResolvedSiteConfig) {
        *self = Self::default();
        self.typst_recovery_roots = std::iter::once(config.get_root().to_path_buf())
            .chain(config.modules().iter().filter_map(|(name, module)| {
                tola_build::inputs::declaration_root(
                    config.get_root(),
                    config.vendor().path.as_deref(),
                    name,
                    &module.source,
                )
                .ok()
            }))
            .chain(
                [
                    config.package_locations().data(),
                    config.package_locations().cache(),
                ]
                .into_iter()
                .flatten()
                .map(|location| location.root().to_path_buf()),
            )
            .collect();
        self.typst_recovery_roots.sort_unstable();
        self.typst_recovery_roots.dedup();
    }

    fn current_site_replaced(&mut self) {
        *self = Self::default();
    }

    fn all_package_checks(
        &self,
        completed_site: &[tola_typst::PackageCheck],
    ) -> Vec<tola_typst::PackageCheck> {
        let mut checks = completed_site
            .iter()
            .chain(&self.latest_failed_packages)
            .cloned()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        tola_typst::sort_package_checks(&mut checks);
        checks
    }

    fn all_paths(&self, published_reads: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
        let mut paths = published_reads.into_iter().collect::<BTreeSet<_>>();
        paths.extend(self.latest_failed_paths.iter().cloned());
        paths.into_iter().collect()
    }

    fn requirements(&self, published: &InputObservation) -> WatchRequirements {
        let paths = self.all_paths(published.physical_read_paths().iter().cloned());
        let package_checks = self.all_package_checks(published.package_checks());
        let mut requirements = watch_requirements_from_observation(published);
        append_typst_inputs(&mut requirements, paths, package_checks);
        append_observed_paths(&mut requirements, &self.latest_failed_observed_paths);
        if let Some(inputs) = &self.module_hook_inputs {
            requirements.add_module_hook_inputs(inputs);
        }
        for root in &self.typst_recovery_roots {
            requirements.typst_recovery(root.clone());
        }
        requirements
    }
}

/// The configuration one round builds with, and the accepted candidate it came from.
struct RoundConfiguration {
    config: Arc<ResolvedSiteConfig>,
    candidate: Option<crate::config::ConfigCandidate>,
}

#[derive(Debug)]
struct RebuildRequest {
    paths: Vec<PathBuf>,
    rebuild_scope: RebuildScope,
}

impl RebuildRequest {
    fn from_batch(batch: FileChangeBatch) -> Self {
        let (paths, rebuild_scope) = batch.into_parts();
        Self {
            paths,
            rebuild_scope,
        }
    }

    fn merge_batch(&mut self, batch: FileChangeBatch) {
        let (paths, rebuild_scope) = batch.into_parts();
        self.rebuild_scope = self
            .rebuild_scope
            .max(rebuild_scope)
            .max(merge_paths(&mut self.paths, paths));
    }

    fn after_failure(
        mut paths: Vec<PathBuf>,
        observation_extended: bool,
        hook_retry: Option<Self>,
        retry: &mut FailureRetry,
    ) -> Option<Self> {
        if !observation_extended {
            return retry.admit(hook_retry);
        }
        if let Some(hook_retry) = hook_retry {
            merge_paths(&mut paths, hook_retry.paths);
        }
        // Retry once after attaching watches: inputs may have changed since the failed read.
        retry.admit(Some(Self {
            paths,
            rebuild_scope: RebuildScope::FullSite,
        }))
    }
}
/// The one immediate failed-attempt recheck a settle call admits.
///
/// One attempt is what an immediate recheck covers: reads that happened before their watch was
/// attached. Every later change arrives as its own event, including the missing directories a
/// recovery discovers one layer at a time, so a retried attempt that fails again ends its settle
/// call instead of extending observation once per failed attempt.
#[derive(Default)]
struct FailureRetry {
    spent: bool,
}

impl FailureRetry {
    /// The retry to run, or `None` once this settle call admitted one.
    fn admit(&mut self, retry: Option<RebuildRequest>) -> Option<RebuildRequest> {
        if self.spent {
            return None;
        }
        self.spent = retry.is_some();
        retry
    }
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
    initial_config: Arc<ResolvedSiteConfig>,
    server: ServerConfig,
    dev: DevConfig,
    disable_hook_reuse: bool,
    config_loader: ConfigLoader,
    sites: CurrentSite,
    changes: FileChangeSource,
    reload: Option<ReloadTransport>,
    shutdown: Cancellation,
    build_session: tola_build::build::BuildSession,
    input_mode: tola_input::InputMode,
    failure_watches: DevFailureWatches,
    /// The diagnostics the installed revision has, plus any an unpublished attempt added.
    ///
    /// One set serves the terminal and the log, so a diagnostic is never resolved on one surface
    /// while the other still shows it.
    reported_diagnostics: Vec<Diagnostic>,
    /// Monotonic count of the rounds this session reported.
    round: u64,
    output: crate::cli::output::CommandOutput,
    hooks: crate::dev::hooks::HookSender,
    events: super::events::Events,
    build: u64,
    attempt_diagnostics: Vec<Diagnostic>,
    attempt_published: bool,
    publication_sender: tokio::sync::mpsc::Sender<super::http::PublicationRequest>,
    publication_requests: tokio::sync::mpsc::Receiver<super::http::PublicationRequest>,
}

pub(super) struct RebuildStartup {
    pub(super) initial_config: Arc<ResolvedSiteConfig>,
    pub(super) server: ServerConfig,
    pub(super) dev: DevConfig,
    pub(super) disable_hook_reuse: bool,
    pub(super) config_loader: ConfigLoader,
    pub(super) resources: tola_build::BuildResources,
    pub(super) sites: CurrentSite,
    pub(super) reload: Option<ReloadTransport>,
    pub(super) shutdown: Cancellation,
    pub(super) output: crate::cli::output::CommandOutput,
    pub(super) hooks: crate::dev::hooks::HookSender,
    pub(super) events: super::events::Events,
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
        let input_mode = input.resources.input_mode();
        let build_session = tola_build::build::BuildSession::with_resources(input.resources);
        let (publication_sender, publication_requests) = tokio::sync::mpsc::channel(8);
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
            build_session,
            input_mode,
            failure_watches: DevFailureWatches::default(),
            reported_diagnostics: Vec::new(),
            round: 0,
            output: input.output,
            hooks: input.hooks,
            events: input.events,
            build: 0,
            attempt_diagnostics: Vec::new(),
            attempt_published: false,
            publication_sender,
            publication_requests,
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
        .await?;
        Ok(())
    }

    pub(super) fn publication_sender(
        &self,
    ) -> tokio::sync::mpsc::Sender<super::http::PublicationRequest> {
        self.publication_sender.clone()
    }

    pub(super) async fn run(mut self) -> Result<()> {
        let run = self.run_inner().await;
        let reload = match self.reload.take() {
            Some(reload) => reload.shutdown().await,
            None => Ok(()),
        };
        let observation = self.changes.shutdown().await;
        run.and(reload)
            .and(observation.map_err(Into::into))
            .map_err(|error| {
                let root = self.initial_config.get_root().to_path_buf();
                crate::dev::watch::with_diagnostics(error, &root)
            })
    }

    async fn run_inner(&mut self) -> Result<()> {
        tracing::debug!(target: "tola::dev", "development rebuild loop started");
        while !self.shutdown.is_requested() {
            self.changes.refresh_pending().await?;
            let config = self.current_config();
            let batch = tokio::select! {
                _ = self.shutdown.cancelled() => break,
                publication = self.publication_requests.recv() => {
                    if let Some(publication) = publication {
                        let previous = self.sites.revision();
                        // The request arrives after the editor's saves. FullSite validates those
                        // disk inputs even when the resulting output revision ID is unchanged.
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
                        let published = self.sites.revision().filter(|current| {
                            previous.as_ref().is_none_or(|previous| !Arc::ptr_eq(previous, current))
                        });
                        let publication_reply = published.ok_or_else(|| {
                            self.reported_diagnostics.iter()
                                .filter(|diagnostic| diagnostic.severity == Severity::Error)
                                .count().max(1)
                        });
                        let _ = publication.send(publication_reply);
                    }
                    continue;
                }
                batch = self.changes.next(&config) => batch?,
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
        mut request: RebuildRequest,
        trigger: DevBuildTrigger,
    ) -> Result<()> {
        self.round += 1;
        // Hook events a round runs hold this span, so a log reader can place a command that
        // outlives the round that published it.
        let round = tracing::info_span!(target: "tola::dev", "round", round = self.round);
        // This loop holds one guard at a time, so the newest guard is the browser's rebuild state.
        let _rebuild_status = self.reload.as_ref().map(ReloadTransport::begin_rebuild);
        let mut failure_retry = FailureRetry::default();
        loop {
            self.build += 1;
            self.attempt_diagnostics.clear();
            self.attempt_published = false;
            self.events.build_started(self.build)?;
            let completion = self
                .rebuild(request, trigger, &round, &mut failure_retry)
                .await;
            if self.events.0 && !self.attempt_published {
                let status = if !self.attempt_diagnostics.is_empty()
                    || completion
                        .as_ref()
                        .err()
                        .is_some_and(|error| !tola_build::cancellation::is_cancelled(error))
                {
                    "failed"
                } else {
                    "cancelled"
                };
                if self.attempt_diagnostics.is_empty()
                    && status == "failed"
                    && let Err(error) = &completion
                {
                    self.attempt_diagnostics =
                        tola_build::build::error_diagnostics(error, self.initial_config.get_root());
                }
                let revision = self.sites.revision();
                self.events.build_finished(
                    self.build,
                    status,
                    false,
                    revision
                        .as_ref()
                        .map(|site| site.manifest().revision().as_str()),
                    &self.attempt_diagnostics,
                )?;
            }
            // Release deferred hook events even if failure preceded worker startup; paths
            // left unacknowledged stay pending in the event epoch.
            let hook_changes = self.changes.reject_hook_candidate();
            let mut superseding = match completion {
                Ok(Some(superseding)) => superseding,
                Ok(None) => break,
                Err(error)
                    if self.shutdown.is_requested()
                        && error.chain().any(|cause| {
                            cause.is::<tola_build::cancellation::BuildCancelled>()
                        }) =>
                {
                    break;
                }
                Err(error) => return Err(error),
            };
            if self.shutdown.is_requested() {
                break;
            }
            if let Some(hook_changes) = hook_changes {
                superseding.merge_batch(hook_changes);
            }
            request = superseding;
        }
        Ok(())
    }

    /// Await one task while edits are watched; a change cancels it and becomes the next request.
    async fn await_task<T>(
        &mut self,
        mut build: tokio::task::JoinHandle<T>,
        canceller: &tola_build::cancellation::BuildCanceller,
        config: &ResolvedSiteConfig,
        changed_paths: &[PathBuf],
        rebuild_scope: RebuildScope,
    ) -> Result<RoundAwait<T>> {
        let mut superseding: Option<RebuildRequest> = None;
        let mut cancellation_started = None;
        let completion = loop {
            tokio::select! {
                biased;
                _ = self.shutdown.cancelled() => {
                    canceller.cancel();
                    break build.await;
                }
                batch = self.changes.next(config) => {
                    let batch = match batch {
                        Ok(batch) => batch,
                        Err(error) => {
                            canceller.cancel();
                            let _ = build.await;
                            return Err(error.into());
                        }
                    };
                    if superseding.is_none() {
                        cancellation_started = Some(std::time::Instant::now());
                        superseding = Some(RebuildRequest {
                            paths: changed_paths.to_vec(),
                            rebuild_scope,
                        });
                    }
                    let superseding = superseding.as_mut().expect("initialized above");
                    superseding.merge_batch(batch);
                    canceller.cancel();
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
                cancel_wait_ms = cancellation_started
                    .map(|started| started.elapsed().as_secs_f64() * 1000.0),
                "discarded superseded site candidate"
            );
            return Ok(RoundAwait::Superseded(superseding));
        }
        Ok(RoundAwait::Finished(completion))
    }

    fn current_config(&self) -> Arc<ResolvedSiteConfig> {
        self.sites
            .revision()
            .map(|site| Arc::clone(site.config()))
            .unwrap_or_else(|| Arc::clone(&self.initial_config))
    }

    /// Resolve the configuration this round builds with: the reloaded candidate when it is valid
    /// and changes nothing the running server owns, otherwise the current revision's.
    fn resolve_round_config(
        &mut self,
        current_config: &Arc<ResolvedSiteConfig>,
    ) -> Result<Option<RoundConfiguration>> {
        let config_path = current_config.config_path().to_path_buf();
        let candidate = match self.config_loader.load_candidate() {
            Ok(candidate) => candidate,
            Err(error) => {
                let diagnostics = tola_diagnostic::attached(&error)
                    .map(|diagnostics| diagnostics.to_vec())
                    .unwrap_or_else(|| {
                        vec![
                            tola_diagnostic::fallback(crate::codes::config::RELOAD, &error)
                                .with_path(crate::terminal::display_path_within(
                                    &config_path,
                                    current_config.get_root(),
                                )),
                        ]
                    });
                self.report_diagnostics(diagnostics);
                return Ok(None);
            }
        };
        if let Some(candidate) = candidate.as_ref()
            && let Some(change) = invocation_config_change(
                InvocationSettings {
                    config: current_config,
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
                    crate::terminal::display_path_within(&config_path, current_config.get_root()),
                    detail,
                )
                .with_help("Restart `tola dev`"),
            ]);
            self.config_loader.acknowledge(candidate);
            return Ok(None);
        }
        let config = candidate
            .as_ref()
            .map(crate::config::ConfigCandidate::config)
            .unwrap_or_else(|| Arc::clone(current_config));
        if let Err(error) =
            crate::cli::log::destination::start_site(&self.output, &config, &self.shutdown.token())
        {
            if self.shutdown.is_requested() {
                return Ok(None);
            }
            // Only the path conflicts of an already-started log remain here; the
            // complete chain belongs to the debug log.
            self.output.record_failure(&error);
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
        Ok(Some(RoundConfiguration { config, candidate }))
    }

    async fn rebuild(
        &mut self,
        request: RebuildRequest,
        trigger: DevBuildTrigger,
        round: &tracing::Span,
        failure_retry: &mut FailureRetry,
    ) -> Result<Option<RebuildRequest>> {
        // A closure cannot return from `rebuild`, so this is the one place an awaited task's
        // outcome becomes the round's own outcome.
        macro_rules! round_await {
            ($task:expr, $canceller:expr, $config:expr, $paths:expr, $scope:expr) => {
                match self
                    .await_task($task, $canceller, $config, $paths, $scope)
                    .await?
                {
                    RoundAwait::Finished(value) => value,
                    RoundAwait::Superseded(request) => return Ok(Some(request)),
                    RoundAwait::Stopped => return Ok(None),
                }
            };
        }
        let rebuild_started = std::time::Instant::now();
        self.changes.refresh_pending().await?;
        let current_site = self.sites.revision();
        let current_config = current_site
            .as_ref()
            .map(|site| Arc::clone(site.config()))
            .unwrap_or_else(|| Arc::clone(&self.initial_config));
        let empty_observation = InputObservation::default();
        let current_observation = current_site
            .as_ref()
            .map_or(&empty_observation, |site| site.input_observation());
        let event_claim = self.changes.begin_candidate(&current_config);
        let build_event_epoch = event_claim.revision();
        let mut changed_paths = request.paths;
        let mut rebuild_scope = request.rebuild_scope.max(event_claim.rebuild_scope());
        rebuild_scope = rebuild_scope.max(event_claim.merge_into(&mut changed_paths));
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
        let Some(round_config) = self.resolve_round_config(&current_config)? else {
            return Ok(None);
        };
        let RoundConfiguration { config, candidate } = round_config;
        self.changes.update_hook_candidate_claims(&config);
        let current_watch_requirements = watch_requirements_from_observation(current_observation);
        self.changes
            .stage_config(&config, &current_watch_requirements)
            .await?;

        // Prepare one hook watch before staging any potential module-hook path.
        // The attempt below consumes the same watch instead of preparing it again.
        let hook_watch = if config.modules().is_empty() {
            None
        } else {
            match tola_build::modules::prepare_hook_watch(
                &config,
                self.input_mode,
                &Default::default(),
            ) {
                Ok(watch) => Some(watch),
                Err(error) => {
                    self.failure_watches.build_task_failed(&config);
                    self.reconfigure_failure_watches(&current_config, &config)
                        .await?;
                    self.report_build_failure(&error);
                    return Ok(None);
                }
            }
        };
        let watch_config = hook_watch
            .as_ref()
            .map_or_else(|| Arc::clone(&config), |watch| watch.config_arc().clone());
        let hook_inputs = hook_watch
            .as_ref()
            .map(|watch| watch.inputs().clone())
            .unwrap_or_default();
        let mut staged_requirements = current_watch_requirements;
        staged_requirements.add_module_hook_inputs(&hook_inputs);
        self.changes.update_hook_candidate_claims(&watch_config);
        self.changes.extend_hook_candidate_claims(&hook_inputs);
        self.changes
            .stage_config(&watch_config, &staged_requirements)
            .await?;
        // Resolve newly attached declared coverage before a generator can write into it.
        self.changes.refresh_pending().await?;

        let reuse_capabilities =
            producer_reuse_capabilities(rebuild_scope, ProducerReuseCoverage::of(&self.changes));
        let canceller = tola_build::cancellation::BuildCanceller::new();
        let request = tola_build::build::BuildRequest {
            mode: tola_build::build::BuildMode::Development,
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
            reuse: reuse_capabilities,
            generated_files: Vec::new(),
            cancellation: canceller.token(),
            hook_execution: tola_build::build::HookExecution::Run,
            reference_checks: tola_build::build::ReferenceChecks::Full,
            disable_hook_reuse: self.disable_hook_reuse,
            capture_report: false,
        };
        let attempt = if let Some(hook_watch) = &hook_watch {
            self.build_session
                .prepare_with_hook_watch(hook_watch, request)?
        } else {
            self.build_session.prepare(Arc::clone(&config), request)
        };
        let prepare_elapsed = rebuild_started.elapsed();
        let build_started = std::time::Instant::now();
        let lock_config = Arc::clone(&config);
        let lock_cancellation = canceller.token();
        let lock_output = self.output.clone();
        let locking = tokio::task::spawn_blocking(move || {
            tola_build::build::SiteBuildLock::acquire(&lock_config, &lock_cancellation, || {
                let _ = lock_output.waiting_for_build();
            })
        });
        let build_lock =
            match round_await!(locking, &canceller, &config, &changed_paths, rebuild_scope) {
                Ok(Ok(lock)) => lock,
                Ok(Err(error)) => {
                    self.report_build_failure(&anyhow::Error::new(error));
                    return Ok(None);
                }
                Err(error) => {
                    self.output.record_failure(&anyhow::Error::new(error));
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
        // Entering the round span inside the blocking build keeps it off every awaited task, and
        // makes the hook events this thread records hold the round they belong to.
        let build_round = round.clone();
        let build = tokio::task::spawn_blocking(move || build_round.in_scope(|| attempt.run()));
        let built = match round_await!(
            build,
            &canceller,
            &watch_config,
            &changed_paths,
            rebuild_scope
        ) {
            Ok(Ok(built)) => built,
            Ok(Err(failure)) => {
                if failure.is_cancelled() {
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
                let observation_extended = self
                    .reconfigure_failure_watches(&current_config, &config)
                    .await?;
                self.report_build_failure(failure.error());
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
                    // Source generation stopped before discovery: attaching its partial output
                    // paths alone is not a reason to rerun the failed script; a new input event
                    // can retry it.
                    observation_extended && !failure.source_hooks_failed(),
                    hook_retry,
                    failure_retry,
                ));
            }
            Err(error) => {
                self.failure_watches.build_task_failed(&config);
                self.failure_watches.module_hook_inputs = Some(hook_inputs.clone());
                let observation_extended = self
                    .reconfigure_failure_watches(&current_config, &config)
                    .await?;
                self.output.record_failure(&anyhow::Error::new(error));
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
        let effective_config = Arc::clone(built.config());
        self.changes.update_hook_candidate_claims(&effective_config);
        let candidate_observation = built.input_observation();
        let candidate_watch_requirements =
            watch_requirements_from_observation(&candidate_observation);
        let candidate_subscriptions = self
            .changes
            .stage_config(&effective_config, &candidate_watch_requirements)
            .await?;
        // A generator may have created a previously missing watched directory. Attach
        // its coverage before checking the candidate; pending events remain in the
        // epoch and are accounted for by freshness and hook-output evidence below.
        self.changes.refresh_pending().await?;
        let subscriptions_elapsed = build_started.elapsed() - build_elapsed;
        let output_count = built.graph().outputs().len();
        let source_hooks = built.source_hooks().clone();
        let hook_outputs = source_hooks.outputs();
        let previous_site = current_site;
        let revision_cancellation = canceller.token();
        let preparing = tokio::task::spawn_blocking(move || {
            let started = std::time::Instant::now();
            let candidate =
                built.into_unchecked_revision(previous_site.as_ref().map(|site| site.outputs()));
            let revision_elapsed = started.elapsed();
            let diagnostics = candidate.site().diagnostics().to_vec();
            let checked = candidate.check(&revision_cancellation);
            let freshness_elapsed = started.elapsed() - revision_elapsed;
            (checked, revision_elapsed, freshness_elapsed, diagnostics)
        });
        let (checked, revision_elapsed, freshness_elapsed, diagnostics) = round_await!(
            preparing,
            &canceller,
            &effective_config,
            &changed_paths,
            rebuild_scope
        )
        .map_err(|error| anyhow::Error::new(error).context("Tola could not finish the rebuild"))?;
        let checked = match checked? {
            RevisionCheck::Fresh(checked) => checked,
            RevisionCheck::Stale(freshness) => {
                return Ok(Some(self.retry_for_stale_candidate(
                    freshness,
                    &candidate_observation,
                    changed_paths,
                    rebuild_scope,
                )));
            }
        };
        let publication_started = std::time::Instant::now();
        let guarded_event_epoch = self.changes.event_epoch();
        let guarded_changes = self
            .changes
            .rebuild_changes_after(build_event_epoch, &effective_config);
        let (previous, current) = {
            let shutdown = self.shutdown.token();
            let event_epoch_guard = match tokio::select! {
                _ = self.shutdown.cancelled() => return Ok(None),
                guarded = self.changes.guard_epoch(
                    build_event_epoch,
                    &effective_config,
                    hook_outputs,
                    &shutdown,
                ) => guarded?,
            } {
                Some(guard) => guard,
                None => {
                    let (newer_paths, _) = guarded_changes.into_parts();
                    merge_paths(&mut changed_paths, newer_paths);
                    rebuild_scope = RebuildScope::FullSite;
                    merge_paths(
                        &mut changed_paths,
                        std::iter::once(effective_config.get_root().to_path_buf()),
                    );
                    tracing::debug!(
                        target: "tola::compile",
                        build_event_epoch,
                        guarded_event_epoch,
                        changes = changed_paths.len(),
                        rebuild_scope = ?rebuild_scope,
                        "discarded candidate after a newer filesystem event was accepted"
                    );
                    return Ok(Some(RebuildRequest {
                        paths: changed_paths,
                        rebuild_scope,
                    }));
                }
            };
            if self.shutdown.is_requested() {
                drop(event_epoch_guard);
                self.changes.reject_hook_candidate();
                return Ok(None);
            }
            let (previous, current) = self.sites.replace(&mut self.build_session, checked)?;
            self.failure_watches.current_site_replaced();
            event_epoch_guard.commit(event_claim, candidate_subscriptions);
            (previous, current)
        };
        self.attempt_published = true;
        self.events.build_finished(
            self.build,
            "succeeded",
            true,
            Some(current.manifest().revision().as_str()),
            &diagnostics,
        )?;
        drop(build_lock);
        self.changes.complete_hook_candidate(&source_hooks);
        let previous_reported = std::mem::replace(&mut self.reported_diagnostics, diagnostics);
        let recovered = previous_reported
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error);
        let revision_diff = previous
            .as_ref()
            .map(|previous| previous.manifest().diff(current.manifest()));
        if let Some(paths) = revision_diff
            .as_ref()
            .and_then(|diff| diff.describe_changed_paths())
        {
            tracing::debug!(
                target: "tola::compile",
                round = self.round,
                "changed outputs\n{paths}"
            );
        }
        let changed_groups = revision_diff
            .as_ref()
            .map(|diff| diff.changed_output_groups());
        if let Some(reload) = self.reload.as_ref() {
            match revision_diff {
                Some(diff) => reload.revision_replaced(diff, &self.reported_diagnostics),
                None => reload.first_revision_ready(
                    current.manifest().revision().clone(),
                    &self.reported_diagnostics,
                ),
            }
        }
        self.hooks.enqueue(Arc::clone(&current), Some(self.round))?;
        tracing::debug!(
            target: "tola::compile",
            round = self.round,
            build_event_epoch,
            revision = current.manifest().revision().as_str(),
            prepare_ms = prepare_elapsed.as_secs_f64() * 1000.0,
            build_ms = build_elapsed.as_secs_f64() * 1000.0,
            revision_ms = revision_elapsed.as_secs_f64() * 1000.0,
            subscriptions_ms = subscriptions_elapsed.as_secs_f64() * 1000.0,
            freshness_ms = freshness_elapsed.as_secs_f64() * 1000.0,
            publication_ms = publication_started.elapsed().as_secs_f64() * 1000.0,
            rebuild_ms = rebuild_started.elapsed().as_secs_f64() * 1000.0,
            outputs = output_count,
            routes = current.address().resource_count(),
            references = current.references().references().len(),
            diagnostics = current.diagnostics().len(),
            "replaced current site"
        );
        if let Some(candidate) = candidate.as_ref() {
            self.config_loader.acknowledge(candidate);
            self.output.apply_diagnostic_limits(candidate.diagnostics());
        }
        let summary = format_rebuild_summary(changed_groups, rebuild_started.elapsed(), recovered);
        let round = crate::dev::round::CompletedRound::published(
            self.round,
            current.manifest().revision().as_str(),
            &summary,
        );
        if self.output.log().is_some() {
            for diagnostic in resolved_diagnostics(&previous_reported, &self.reported_diagnostics) {
                self.output.record_resolved(round.identity(), diagnostic);
            }
        }
        if let Err(error) = self.output.report_round(round, &self.reported_diagnostics) {
            self.output.record_failure(&anyhow::Error::new(error));
            self.report_build_error(
                crate::codes::terminal::WRITE,
                "Tola could not write diagnostics to this terminal".into(),
            );
        }
        Ok(None)
    }

    /// The retry one stale candidate asks for: the reads whose evidence changed during its build
    /// join the paths and widen the scope, and the hook claim it never published is released.
    fn retry_for_stale_candidate(
        &mut self,
        freshness: tola_build::build::InputFreshness,
        observation: &InputObservation,
        mut changed_paths: Vec<PathBuf>,
        mut rebuild_scope: RebuildScope,
    ) -> RebuildRequest {
        use tola_build::build::InputKind;

        if freshness.is_stale(InputKind::TypstPhysicalReads) {
            rebuild_scope = rebuild_scope.max(merge_paths(
                &mut changed_paths,
                observation.physical_read_paths().iter().cloned(),
            ));
        }
        if freshness.is_stale(InputKind::TypstPackageSelection) {
            rebuild_scope = rebuild_scope.max(merge_paths(
                &mut changed_paths,
                observation
                    .package_checks()
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
            configured_assets_fresh = freshness.is_fresh(InputKind::ConfiguredAssets),
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

    fn report_build_failure(&mut self, error: &anyhow::Error) {
        let config = self.current_config();
        let diagnostics = tola_build::build::error_diagnostics(error, config.get_root());
        tracing::debug!(
            target: "tola::compile",
            round = self.round,
            errors = diagnostics.len(),
            "site rebuild failed"
        );
        self.report_diagnostics(diagnostics);
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
        current_config: &ResolvedSiteConfig,
        attempted_config: &ResolvedSiteConfig,
    ) -> Result<bool, WatchError> {
        let current_site = self.sites.revision();
        let empty_observation = InputObservation::default();
        let observed = current_site
            .as_ref()
            .map_or(&empty_observation, |site| site.input_observation());
        let dynamic = self.failure_watches.requirements(observed);
        self.changes
            .reconfigure_configs([current_config, attempted_config], &dynamic)
            .await
    }

    fn report_diagnostics(&mut self, diagnostics: Vec<Diagnostic>) {
        if self.events.0 && !self.attempt_published {
            self.attempt_diagnostics.clone_from(&diagnostics);
        }
        let status = if diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == "terminal.write")
        {
            "Could not write diagnostics to the terminal"
        } else if diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == crate::codes::config::SERVE_RESTART)
        {
            "Restart required; the current site is unchanged"
        } else if self.sites.revision().is_some() {
            "Build failed; still serving the previous site"
        } else {
            "Build failed; fix the errors and save to rebuild"
        };
        let diagnostics = retained_with_previous_warnings(&self.reported_diagnostics, diagnostics);
        self.reported_diagnostics.clone_from(&diagnostics);
        if let Err(error) = self.output.report_round(
            crate::dev::round::CompletedRound::failed(Some(self.round), status),
            &self.reported_diagnostics,
        ) {
            self.output.record_failure(&anyhow::Error::new(error));
        }
        if let Some(reload) = self.reload.as_ref() {
            reload.replace_diagnostics(&self.reported_diagnostics);
        }
    }
}

/// Whether two records are the same diagnostic.
///
/// A round reads fresh excerpts and importers for a diagnostic it still reports, so the whole
/// record changes while the diagnostic stays. Code, position, and message decide identity;
/// excerpt, importers, notes, help, and trace do not.
fn same_diagnostic(left: &Diagnostic, right: &Diagnostic) -> bool {
    if left.code != right.code || left.message != right.message {
        return false;
    }
    match (left.location.as_ref(), right.location.as_ref()) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.path == right.path && left.line == right.line && left.column == right.column
        }
        _ => false,
    }
}

/// Keep previously reported warnings that a round did not report again.
///
/// Only an error aborts the work a warning came from: until a successful round stops reporting a
/// warning, the warning is still true, so a failed round shows it next to its own diagnostics. A
/// warning the round does report again is not repeated under its earlier presentation.
fn retained_with_previous_warnings(
    previous: &[Diagnostic],
    current: Vec<Diagnostic>,
) -> Vec<Diagnostic> {
    let mut retained = current;
    for earlier in previous {
        if earlier.severity == Severity::Warning
            && !retained
                .iter()
                .any(|current| same_diagnostic(current, earlier))
        {
            retained.push(earlier.clone());
        }
    }
    retained
}

fn resolved_diagnostics<'a>(
    previous: &'a [Diagnostic],
    current: &'a [Diagnostic],
) -> impl Iterator<Item = &'a Diagnostic> {
    previous.iter().filter(|diagnostic| {
        !current
            .iter()
            .any(|current| same_diagnostic(current, diagnostic))
    })
}

fn format_rebuild_summary(
    groups: Option<tola_build::output::summary::ChangedOutputGroups>,
    elapsed: std::time::Duration,
    recovered: bool,
) -> String {
    if recovered {
        return format!(
            "Build succeeded in {}",
            crate::terminal::format_duration(elapsed)
        );
    }
    match groups {
        // The first installed revision has no earlier one to compare against.
        None => crate::terminal::site_summary("Built", elapsed, ""),
        Some(groups) => crate::terminal::site_summary(
            "Rebuilt",
            elapsed,
            &crate::terminal::describe_changes(groups),
        ),
    }
}

fn merge_paths(
    paths: &mut Vec<PathBuf>,
    incoming: impl IntoIterator<Item = PathBuf>,
) -> RebuildScope {
    for path in incoming {
        if !paths.contains(&path) {
            if paths.len() == crate::dev::watch::EVENT_DETAIL_CAPACITY {
                paths.clear();
                return RebuildScope::FullSite;
            }
            paths.push(path);
        }
    }
    paths.sort_unstable();
    RebuildScope::Paths
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProducerReuseCoverage {
    content: bool,
    typst_reads: bool,
    configured_assets: bool,
}

impl ProducerReuseCoverage {
    /// Whether each producer's events still reach its own watch.
    fn of(changes: &FileChangeSource) -> Self {
        Self {
            content: changes.accepts_events_for([ProducerKind::Content]),
            typst_reads: changes.accepts_events_for([ProducerKind::TypstReads]),
            configured_assets: changes.accepts_events_for([ProducerKind::ConfiguredAssets]),
        }
    }
}

/// Inputs whose stale evidence requires a fresh observation of the whole site.
const STALE_INVENTORY_INPUTS: [tola_build::build::InputKind; 4] = [
    tola_build::build::InputKind::ModuleManifests,
    tola_build::build::InputKind::ContentInventory,
    tola_build::build::InputKind::ConfiguredAssets,
    tola_build::build::InputKind::Icons,
];

/// Inputs whose stale evidence requires recompiling every Typst unit.
const STALE_TYPST_INPUTS: [tola_build::build::InputKind; 2] = [
    tola_build::build::InputKind::TypstPackageSelection,
    tola_build::build::InputKind::FontInventory,
];

/// Rebuild scope a discarded candidate needs to recheck its stale inputs.
fn stale_retry_scope(freshness: &tola_build::build::InputFreshness) -> RebuildScope {
    if STALE_INVENTORY_INPUTS
        .into_iter()
        .any(|input| freshness.is_stale(input))
    {
        RebuildScope::FullSite
    } else if STALE_TYPST_INPUTS
        .into_iter()
        .any(|input| freshness.is_stale(input))
    {
        RebuildScope::FullTypst
    } else {
        RebuildScope::Paths
    }
}

fn producer_reuse_capabilities(
    rebuild_scope: RebuildScope,
    coverage: ProducerReuseCoverage,
) -> tola_build::build::BuildReuse {
    let inventories = rebuild_scope != RebuildScope::FullSite;
    tola_build::build::BuildReuse {
        content_inventory: inventories && coverage.content,
        typst_compilation: rebuild_scope < RebuildScope::FullTypst
            && coverage.content
            && coverage.typst_reads,
        configured_assets: inventories && coverage.configured_assets,
    }
}

/// The settings a running development server fixed at startup: the bound listener, the watcher,
/// and the mounted URL. A reload that changes one of them needs a restart.
struct InvocationSettings<'a> {
    config: &'a ResolvedSiteConfig,
    server: &'a ServerConfig,
    dev: &'a DevConfig,
}

/// The named setting a reloaded configuration changed that the running server cannot apply.
fn invocation_config_change(
    current: InvocationSettings<'_>,
    candidate: InvocationSettings<'_>,
) -> Option<&'static str> {
    if current.server.interface != candidate.server.interface {
        Some("server.interface")
    } else if current.server.port != candidate.server.port {
        Some("server.port")
    } else if current.dev.watch != candidate.dev.watch {
        Some("dev.watch")
    } else if current.config.url_mount() != candidate.config.url_mount() {
        Some("site.base-path")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tola_build::config::ResolvedSiteConfig;

    use super::DevFailureWatches;

    fn resolve_schema(
        root: &Path,
        schema: tola_build::config::SiteConfigSchema,
    ) -> ResolvedSiteConfig {
        std::fs::create_dir_all(root).unwrap();
        let path = root.join("tola.toml");
        std::fs::write(&path, toml::to_string(&schema).unwrap()).unwrap();
        schema
            .resolve(
                &path,
                tola_typst::PackageLocations::from_absolute_roots(None, None).unwrap(),
                &tola_build::config::loading::BuildOverrides::default(),
            )
            .unwrap()
    }

    /// Tests that open a real FSEvent stream, which a macOS sandbox cannot start.
    #[cfg(feature = "fsevents-tests")]
    mod fsevents_tests {
        use super::*;
        use crate::dev::watch::{FileChangeSource, RebuildScope, WatchError};

        #[tokio::test]
        async fn rejected_config_frees_generator_events() {
            for replacement in [
                "[build",
                "[site]\norigin = \"https://example.test\"\nbase-path = \"/moved/\"\n",
            ] {
                let directory = tempfile::TempDir::new().unwrap();
                let root = directory.path().canonicalize().unwrap();
                std::fs::create_dir(root.join("content")).unwrap();
                std::fs::create_dir(root.join("generated")).unwrap();
                std::fs::write(root.join("site.typ"), "#document(\"index.html\")[Site]").unwrap();
                let generated = root.join("generated/input.txt");
                std::fs::write(&generated, "first").unwrap();
                let config_path = root.join("tola.toml");
                std::fs::write(&config_path,
                    "[[hooks.before-build]]\nname = \"unused\"\nargv = [\"unused-before-config-rejection\"]\ngenerates = [\"generated\"]\n",
                ).unwrap();
                let loaded = crate::config::load(
                    Some(&config_path),
                    tola_input::InputScope::Pure,
                    tola_typst::PackageLocations::from_absolute_roots(None, None).unwrap(),
                    &crate::config::ConfigOverrides::default(),
                )
                .unwrap();
                let (config, server, dev, config_loader) = loaded.into_parts();
                let cancellation = crate::cancellation::Cancellation::default();
                let output = crate::cli::output::CommandOutput::new(
                    crate::terminal::Terminal::new(clap::ColorChoice::Never, false, None),
                    None,
                );
                let hooks = crate::dev::hooks::HookQueue::start(
                    output.clone(),
                    None,
                    cancellation.clone(),
                    crate::dev::events::Events(false),
                )
                .unwrap();
                let mut rebuilding =
                    super::super::RebuildLoop::start(super::super::RebuildStartup {
                        initial_config: std::sync::Arc::clone(&config),
                        server,
                        dev,
                        disable_hook_reuse: false,
                        config_loader,
                        resources: tola_build::BuildResources::new()
                            .with_input_scope(tola_input::InputScope::Pure),
                        sites: crate::dev::site::CurrentSite::new(),
                        reload: None,
                        shutdown: cancellation,
                        output,
                        hooks: hooks.sender(),
                        events: crate::dev::events::Events(false),
                    })
                    .await
                    .unwrap();
                std::fs::write(&config_path, replacement).unwrap();
                let rejected = rebuilding
                    .settle_build(
                        super::super::RebuildRequest {
                            paths: vec![config_path],
                            rebuild_scope: RebuildScope::FullSite,
                        },
                        super::super::DevBuildTrigger::FilesChanged,
                    )
                    .await;

                std::fs::write(&generated, "edited after config rejection").unwrap();
                let observed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    loop {
                        let (paths, _) = rebuilding.changes.next(&config).await?.into_parts();
                        if paths.iter().any(|path| generated.starts_with(path)) {
                            return Ok::<_, WatchError>(());
                        }
                    }
                })
                .await;
                let stopped = rebuilding.changes.shutdown().await;
                let hooks_stopped = hooks.finish();

                rejected.unwrap();
                observed
                    .expect("generated input edits remained quarantined after config rejection")
                    .unwrap();
                stopped.unwrap();
                hooks_stopped.unwrap();
            }
        }

        #[cfg(unix)]
        #[tokio::test]
        async fn failed_font_reads_remain_watched() {
            let directory = tempfile::TempDir::new().unwrap();
            let root = directory.path().canonicalize().unwrap();
            let site = root.join("site");
            std::fs::create_dir_all(site.join("content")).unwrap();
            std::fs::create_dir_all(site.join("fonts")).unwrap();
            let fonts = tola_typst::FontStore::with_options(
                tola_typst::FontOptions::new().with_system_fonts(false),
            );
            let font = fonts
                .load(&tola_typst::BundleCancellation::new())
                .unwrap()
                .font(0)
                .unwrap();
            let physical_font = root.join("outside.ttf");
            std::fs::write(&physical_font, font.data().as_slice()).unwrap();
            std::os::unix::fs::symlink(&physical_font, site.join("fonts/linked.ttf")).unwrap();
            std::fs::write(
                site.join("site.typ"),
                format!(
                    "#set text(font: {:?})\n#document(\"index.html\")[#context {{ measure([Font]); panic(\"Failure after reading font\") }}]",
                    font.info().family,
                ),
            )
            .unwrap();
            let mut schema = tola_build::config::SiteConfigSchema::default();
            schema.typst.fonts.paths = vec![PathBuf::from("fonts")];
            let config = std::sync::Arc::new(resolve_schema(&site, schema));
            let mut session =
                tola_build::build::BuildSession::with_resources(tola_build::BuildResources::new());
            let failure = session
                .prepare(
                    std::sync::Arc::clone(&config),
                    tola_build::build::BuildRequest::new(tola_build::build::BuildMode::Development),
                )
                .run()
                .err()
                .expect("the source fails after measuring its text");
            assert!(failure.input_observation().paths().iter().any(|path| {
                path.input() == tola_build::build::InputKind::FontInventory
                    && path.path() == physical_font
            }));
            let mut watches = DevFailureWatches::default();
            watches.observe_failure(&failure);
            let dynamic = watches.requirements(&Default::default());
            let mut changes = FileChangeSource::start(&config, &dynamic, &Default::default(), None)
                .await
                .unwrap();
            let _ = changes.begin_candidate(&config);
            changes.reject_hook_candidate();
            std::fs::write(&physical_font, b"repaired font bytes").unwrap();
            let observed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    let (paths, _) = changes.next(&config).await?.into_parts();
                    if paths.contains(&physical_font) {
                        return Ok::<_, WatchError>(());
                    }
                }
            })
            .await;
            changes.shutdown().await.unwrap();
            observed
                .expect("a font read by the failed build lost its recovery watch")
                .unwrap();

            for transition in ["replaced failure", "lost attempt", "published revision"] {
                watches.observe_failure(&failure);
                assert!(!watches.latest_failed_observed_paths.is_empty());
                match transition {
                    "replaced failure" => watches.build_failed(std::iter::empty(), Vec::new()),
                    "lost attempt" => watches.build_task_failed(&config),
                    "published revision" => watches.current_site_replaced(),
                    _ => unreachable!(),
                }
                assert!(
                    watches.latest_failed_observed_paths.is_empty(),
                    "{transition}"
                );
            }
        }
    }
}
