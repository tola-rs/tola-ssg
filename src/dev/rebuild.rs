//! Serial rebuilds and publication of development revisions.

use std::sync::Arc;

use anyhow::Result;

use super::site::CurrentSite;
use crate::cancellation::Cancellation;
use crate::config::{ConfigLoader, DevConfig, ServerConfig};
use crate::dev::reload::transport::ReloadTransport;
use crate::dev::watch::{FileChangeSource, WatchError};
use crate::dev::watch::{
    append_observed_paths, append_typst_inputs, watch_requirements_from_observation,
};
use tola_build::config::ResolvedSiteConfig;

#[derive(Default)]
struct DevFailureWatches {
    latest_failed_paths: std::collections::BTreeSet<std::path::PathBuf>,
    latest_failed_packages: Vec<tola_typst::PackageCheck>,
    latest_failed_observed_paths: Vec<tola_build::build::ObservedInputPath>,
    typst_recovery_roots: Vec<std::path::PathBuf>,
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
        paths: impl IntoIterator<Item = std::path::PathBuf>,
        package_checks: Vec<tola_typst::PackageCheck>,
    ) {
        self.latest_failed_paths = paths.into_iter().collect();
        self.latest_failed_packages = package_checks;
        self.latest_failed_observed_paths.clear();
        self.typst_recovery_roots.clear();
    }

    fn build_task_failed(&mut self, config: &ResolvedSiteConfig) {
        self.latest_failed_paths.clear();
        self.latest_failed_packages.clear();
        self.latest_failed_observed_paths.clear();
        self.typst_recovery_roots = std::iter::once(config.get_root().to_path_buf())
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
        self.latest_failed_paths.clear();
        self.latest_failed_packages.clear();
        self.latest_failed_observed_paths.clear();
        self.typst_recovery_roots.clear();
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

    fn all_paths(
        &self,
        completed_site: impl IntoIterator<Item = std::path::PathBuf>,
    ) -> Vec<std::path::PathBuf> {
        let mut paths = completed_site
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        paths.extend(self.latest_failed_paths.iter().cloned());
        paths.into_iter().collect()
    }

    fn requirements(
        &self,
        published: &tola_build::build::InputObservation,
    ) -> crate::dev::watch::WatchRequirements {
        let paths = self.all_paths(published.physical_read_paths().iter().cloned());
        let package_checks = self.all_package_checks(published.package_checks());
        let mut requirements = watch_requirements_from_observation(published);
        append_typst_inputs(&mut requirements, paths, package_checks);
        append_observed_paths(&mut requirements, &self.latest_failed_observed_paths);
        for root in &self.typst_recovery_roots {
            requirements.typst_recovery(root.clone());
        }
        requirements
    }
}

#[derive(Debug)]
struct RebuildRequest {
    paths: Vec<std::path::PathBuf>,
    rebuild_scope: crate::dev::watch::RebuildScope,
}

impl RebuildRequest {
    fn from_batch(batch: crate::dev::watch::FileChangeBatch) -> Self {
        let (paths, rebuild_scope) = batch.into_parts();
        Self {
            paths,
            rebuild_scope,
        }
    }

    fn merge_batch(&mut self, batch: crate::dev::watch::FileChangeBatch) {
        let (paths, rebuild_scope) = batch.into_parts();
        self.rebuild_scope = self
            .rebuild_scope
            .max(rebuild_scope)
            .max(merge_paths(&mut self.paths, paths));
    }

    fn after_failure(
        mut paths: Vec<std::path::PathBuf>,
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
            rebuild_scope: crate::dev::watch::RebuildScope::FullSite,
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

enum CandidateTaskCompletion<T> {
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
    config_loader: ConfigLoader,
    sites: CurrentSite,
    changes: FileChangeSource,
    reload: Option<ReloadTransport>,
    shutdown: Cancellation,
    build_session: tola_build::build::BuildSession,
    failure_watches: DevFailureWatches,
    /// The diagnostics the installed revision has, plus any an unpublished attempt added.
    ///
    /// One set serves the terminal and the log, so a diagnostic is never resolved on one surface
    /// while the other still shows it.
    reported_diagnostics: Vec<tola_build::diagnostic::Diagnostic>,
    /// Monotonic count of the rounds this session reported.
    round: u64,
    output: crate::cli::output::CommandOutput,
    hooks: crate::dev::hooks::HookSender,
    publication_sender: tokio::sync::mpsc::Sender<super::http::PublicationRequest>,
    publication_requests: tokio::sync::mpsc::Receiver<super::http::PublicationRequest>,
}

pub(super) struct RebuildStartup {
    pub(super) initial_config: Arc<ResolvedSiteConfig>,
    pub(super) server: ServerConfig,
    pub(super) dev: DevConfig,
    pub(super) config_loader: ConfigLoader,
    pub(super) resources: tola_build::BuildResources,
    pub(super) sites: CurrentSite,
    pub(super) reload: Option<ReloadTransport>,
    pub(super) shutdown: Cancellation,
    pub(super) output: crate::cli::output::CommandOutput,
    pub(super) hooks: crate::dev::hooks::HookSender,
}

impl RebuildLoop {
    pub(super) async fn start(input: RebuildStartup) -> Result<Self, WatchError> {
        let changes = FileChangeSource::start(
            &input.initial_config,
            &crate::dev::watch::WatchRequirements::default(),
            &tola_build::hooks::SourceHookOutputs::default(),
            input.output.log().map(crate::cli::log::LogFile::path),
        )
        .await?;
        let build_session = tola_build::build::BuildSession::with_resources(input.resources);
        let (publication_sender, publication_requests) = tokio::sync::mpsc::channel(8);
        Ok(Self {
            initial_config: input.initial_config,
            server: input.server,
            dev: input.dev,
            config_loader: input.config_loader,
            sites: input.sites,
            changes,
            reload: input.reload,
            shutdown: input.shutdown,
            build_session,
            failure_watches: DevFailureWatches::default(),
            reported_diagnostics: Vec::new(),
            round: 0,
            output: input.output,
            hooks: input.hooks,
            publication_sender,
            publication_requests,
        })
    }

    pub(super) async fn build_initial(&mut self) -> Result<()> {
        self.settle_build(
            RebuildRequest {
                paths: Vec::new(),
                rebuild_scope: crate::dev::watch::RebuildScope::FullSite,
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
                                rebuild_scope: crate::dev::watch::RebuildScope::FullSite,
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
                                .filter(|diagnostic| diagnostic.severity == tola_build::diagnostic::Severity::Error)
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
            let completion = self
                .rebuild(request, trigger, &round, &mut failure_retry)
                .await;
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

    /// Observe edits until the worker stops; discard superseded build results.
    async fn await_candidate_task<T>(
        &mut self,
        mut build: tokio::task::JoinHandle<T>,
        canceller: &tola_build::cancellation::BuildCanceller,
        config: &ResolvedSiteConfig,
        changed_paths: &[std::path::PathBuf],
        rebuild_scope: crate::dev::watch::RebuildScope,
    ) -> Result<CandidateTaskCompletion<T>> {
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
            return Ok(CandidateTaskCompletion::Stopped);
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
            return Ok(CandidateTaskCompletion::Superseded(superseding));
        }
        Ok(CandidateTaskCompletion::Finished(completion))
    }

    fn current_config(&self) -> Arc<ResolvedSiteConfig> {
        self.sites
            .revision()
            .map(|site| Arc::clone(site.config()))
            .unwrap_or_else(|| Arc::clone(&self.initial_config))
    }

    async fn rebuild(
        &mut self,
        request: RebuildRequest,
        trigger: DevBuildTrigger,
        round: &tracing::Span,
        failure_retry: &mut FailureRetry,
    ) -> Result<Option<RebuildRequest>> {
        let rebuild_started = std::time::Instant::now();
        self.changes.refresh_pending().await?;
        let current_site = self.sites.revision();
        let current_config = current_site
            .as_ref()
            .map(|site| Arc::clone(site.config()))
            .unwrap_or_else(|| Arc::clone(&self.initial_config));
        let empty_observation = tola_build::build::InputObservation::default();
        let current_observation = current_site
            .as_ref()
            .map(|site| site.input_observation())
            .unwrap_or(&empty_observation);
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
        let config_path = current_config.config_path().to_path_buf();
        let candidate = match self.config_loader.load_candidate() {
            Ok(candidate) => candidate,
            Err(error) => {
                let diagnostics = tola_build::diagnostic::attached(&error)
                    .map(|diagnostics| diagnostics.to_vec())
                    .unwrap_or_else(|| {
                        vec![
                            tola_build::diagnostic::fallback(crate::codes::config::RELOAD, &error)
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
                    config: &current_config,
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
                tola_build::diagnostic::Diagnostic::at_path(
                    crate::codes::config::SERVE_RESTART,
                    tola_build::diagnostic::Severity::Error,
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
            .unwrap_or_else(|| Arc::clone(&current_config));
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
                tola_build::diagnostic::Diagnostic::new(
                    crate::codes::log::LOCATION,
                    tola_build::diagnostic::Severity::Error,
                    "the log file conflicts with a site input or output",
                )
                .with_help("Choose a `--log-file` path outside the site"),
            ]);
            return Ok(None);
        }
        self.changes.update_hook_candidate_claims(&config);
        let current_watch_requirements = watch_requirements_from_observation(current_observation);
        self.changes
            .stage_config(&config, &current_watch_requirements)
            .await?;

        let reuse_capabilities = producer_reuse_capabilities(
            rebuild_scope,
            ProducerReuseCoverage {
                content: self
                    .changes
                    .accepts_events_for([crate::dev::watch::ProducerKind::Content]),
                typst_reads: self
                    .changes
                    .accepts_events_for([crate::dev::watch::ProducerKind::TypstReads]),
                configured_assets: self
                    .changes
                    .accepts_events_for([crate::dev::watch::ProducerKind::ConfiguredAssets]),
            },
        );
        let canceller = tola_build::cancellation::BuildCanceller::new();
        let attempt = self.build_session.prepare(
            Arc::clone(&config),
            tola_build::build::BuildRequest {
                mode: tola_build::build::BuildMode::Development,
                trigger: match trigger {
                    DevBuildTrigger::Startup => tola_build::build::BuildTrigger::Initial,
                    DevBuildTrigger::FilesChanged => {
                        let paths = changed_paths.clone().into();
                        if rebuild_scope == crate::dev::watch::RebuildScope::FullSite {
                            tola_build::build::BuildTrigger::AllInputs(paths)
                        } else {
                            tola_build::build::BuildTrigger::Paths(paths)
                        }
                    }
                },
                reuse: reuse_capabilities,
                generated_files: Vec::new(),
                cancellation: canceller.token(),
                hook_execution: tola_build::build::HookExecution::Run,
            },
        );
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
        let build_lock = match self
            .await_candidate_task(locking, &canceller, &config, &changed_paths, rebuild_scope)
            .await?
        {
            CandidateTaskCompletion::Finished(locked) => match locked {
                Ok(Ok(lock)) => lock,
                Ok(Err(error)) => {
                    self.report_build_failure(&error);
                    return Ok(None);
                }
                Err(error) => {
                    self.output.record_failure(&anyhow::Error::new(error));
                    self.report_diagnostics(vec![
                        tola_build::diagnostic::Diagnostic::new(
                            crate::codes::build::TASK,
                            tola_build::diagnostic::Severity::Error,
                            "Tola could not wait for another build of this site",
                        )
                        .with_help(
                            "Wait for the other `tola` command to finish, then save a file to rebuild",
                        ),
                    ]);
                    return Ok(None);
                }
            },
            CandidateTaskCompletion::Superseded(request) => return Ok(Some(request)),
            CandidateTaskCompletion::Stopped => return Ok(None),
        };
        // Entering the round span inside the blocking build keeps it off every awaited task, and
        // makes the hook events this thread records hold the round they belong to.
        let build_round = round.clone();
        let build = tokio::task::spawn_blocking(move || build_round.in_scope(|| attempt.run()));
        let build = match self
            .await_candidate_task(build, &canceller, &config, &changed_paths, rebuild_scope)
            .await?
        {
            CandidateTaskCompletion::Finished(build) => build,
            CandidateTaskCompletion::Superseded(request) => return Ok(Some(request)),
            CandidateTaskCompletion::Stopped => return Ok(None),
        };
        let built = match build {
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
                let observation_extended = self
                    .reconfigure_failure_watches(&current_config, &config)
                    .await?;
                self.output.record_failure(&anyhow::Error::new(error));
                self.report_diagnostics(vec![
                    tola_build::diagnostic::Diagnostic::new(
                        crate::codes::build::TASK,
                        tola_build::diagnostic::Severity::Error,
                        "the site build stopped unexpectedly",
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
        let candidate_observation = built.input_observation();
        let candidate_watch_requirements =
            watch_requirements_from_observation(&candidate_observation);
        let candidate_subscriptions = self
            .changes
            .stage_config(&config, &candidate_watch_requirements)
            .await?;
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
        let (checked, revision_elapsed, freshness_elapsed, diagnostics) = match self
            .await_candidate_task(
                preparing,
                &canceller,
                &config,
                &changed_paths,
                rebuild_scope,
            )
            .await?
        {
            CandidateTaskCompletion::Finished(prepared) => prepared.map_err(|error| {
                anyhow::Error::new(error).context("Tola could not finish the rebuild")
            })?,
            CandidateTaskCompletion::Superseded(request) => return Ok(Some(request)),
            CandidateTaskCompletion::Stopped => return Ok(None),
        };
        let checked = match checked? {
            tola_build::build::RevisionCheck::Fresh(checked) => checked,
            tola_build::build::RevisionCheck::Stale(freshness) => {
                use tola_build::build::InputKind;

                if freshness.is_stale(InputKind::TypstPhysicalReads) {
                    rebuild_scope = rebuild_scope.max(merge_paths(
                        &mut changed_paths,
                        candidate_observation.physical_read_paths().iter().cloned(),
                    ));
                }
                if freshness.is_stale(InputKind::TypstPackageSelection) {
                    rebuild_scope = rebuild_scope.max(merge_paths(
                        &mut changed_paths,
                        candidate_observation
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
                return Ok(Some(retry));
            }
        };
        let publication_started = std::time::Instant::now();
        let guarded_event_epoch = self.changes.event_epoch();
        let guarded_changes = self
            .changes
            .rebuild_changes_after(build_event_epoch, &config);
        let (previous, current) = {
            let shutdown = self.shutdown.token();
            let event_epoch_guard = match tokio::select! {
                _ = self.shutdown.cancelled() => return Ok(None),
                guarded = self.changes.guard_epoch(
                    build_event_epoch,
                    &config,
                    hook_outputs,
                    &shutdown,
                ) => guarded?,
            } {
                Some(guard) => guard,
                None => {
                    let (newer_paths, _) = guarded_changes.into_parts();
                    merge_paths(&mut changed_paths, newer_paths);
                    rebuild_scope = crate::dev::watch::RebuildScope::FullSite;
                    merge_paths(
                        &mut changed_paths,
                        std::iter::once(config.get_root().to_path_buf()),
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
        drop(build_lock);
        self.changes.complete_hook_candidate(&source_hooks);
        let previous_reported = std::mem::replace(&mut self.reported_diagnostics, diagnostics);
        let recovered = previous_reported
            .iter()
            .any(|diagnostic| diagnostic.severity == tola_build::diagnostic::Severity::Error);
        let revision_diff = previous
            .as_ref()
            .map(|previous| previous.manifest().diff(current.manifest()));
        let changed_counts = revision_diff
            .as_ref()
            .map(|diff| diff.changed_output_counts());
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
        let summary = format_rebuild_summary(changed_counts, rebuild_started.elapsed(), recovered);
        let round = crate::cli::output::CompletedRound::published(
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

    fn report_build_error(&mut self, code: tola_build::diagnostic::DiagnosticCode, detail: String) {
        tracing::debug!(
            target: "tola::compile",
            round = self.round,
            code = code.as_str(),
            "site rebuild failed"
        );
        self.report_diagnostics(vec![tola_build::diagnostic::Diagnostic::new(
            code,
            tola_build::diagnostic::Severity::Error,
            detail,
        )]);
    }

    async fn reconfigure_failure_watches(
        &mut self,
        current_config: &ResolvedSiteConfig,
        attempted_config: &ResolvedSiteConfig,
    ) -> Result<bool, WatchError> {
        let current_site = self.sites.revision();
        let empty_observation = tola_build::build::InputObservation::default();
        let observed = current_site
            .as_ref()
            .map(|site| site.input_observation())
            .unwrap_or(&empty_observation);
        let dynamic = self.failure_watches.requirements(observed);
        self.changes
            .reconfigure_configs([current_config, attempted_config], &dynamic)
            .await
    }

    fn report_diagnostics(&mut self, diagnostics: Vec<tola_build::diagnostic::Diagnostic>) {
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
            crate::cli::output::CompletedRound::failed(Some(self.round), status),
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
fn same_diagnostic(
    left: &tola_build::diagnostic::Diagnostic,
    right: &tola_build::diagnostic::Diagnostic,
) -> bool {
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
    previous: &[tola_build::diagnostic::Diagnostic],
    current: Vec<tola_build::diagnostic::Diagnostic>,
) -> Vec<tola_build::diagnostic::Diagnostic> {
    let mut retained = current;
    for earlier in previous {
        if earlier.severity == tola_build::diagnostic::Severity::Warning
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
    previous: &'a [tola_build::diagnostic::Diagnostic],
    current: &'a [tola_build::diagnostic::Diagnostic],
) -> impl Iterator<Item = &'a tola_build::diagnostic::Diagnostic> {
    previous.iter().filter(|diagnostic| {
        !current
            .iter()
            .any(|current| same_diagnostic(current, diagnostic))
    })
}

fn format_rebuild_summary(
    counts: Option<tola_build::output::summary::OutputCounts>,
    elapsed: std::time::Duration,
    recovered: bool,
) -> String {
    if recovered {
        return format!(
            "Build succeeded in {}",
            crate::terminal::format_duration(elapsed)
        );
    }
    match counts {
        // The first installed revision has no earlier one to compare against.
        None => crate::terminal::site_summary("Built", elapsed, ""),
        Some(counts) => crate::terminal::site_summary(
            "Rebuilt",
            elapsed,
            &crate::terminal::describe_change(counts),
        ),
    }
}

fn merge_paths(
    paths: &mut Vec<std::path::PathBuf>,
    incoming: impl IntoIterator<Item = std::path::PathBuf>,
) -> crate::dev::watch::RebuildScope {
    for path in incoming {
        if !paths.contains(&path) {
            if paths.len() == crate::dev::watch::EVENT_DETAIL_CAPACITY {
                paths.clear();
                return crate::dev::watch::RebuildScope::FullSite;
            }
            paths.push(path);
        }
    }
    paths.sort_unstable();
    crate::dev::watch::RebuildScope::Paths
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProducerReuseCoverage {
    content: bool,
    typst_reads: bool,
    configured_assets: bool,
}

/// Inputs whose stale evidence requires a fresh observation of the whole site.
const STALE_INVENTORY_INPUTS: [tola_build::build::InputKind; 3] = [
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
fn stale_retry_scope(
    freshness: &tola_build::build::InputFreshness,
) -> crate::dev::watch::RebuildScope {
    if STALE_INVENTORY_INPUTS
        .into_iter()
        .any(|input| freshness.is_stale(input))
    {
        crate::dev::watch::RebuildScope::FullSite
    } else if STALE_TYPST_INPUTS
        .into_iter()
        .any(|input| freshness.is_stale(input))
    {
        crate::dev::watch::RebuildScope::FullTypst
    } else {
        crate::dev::watch::RebuildScope::Paths
    }
}

fn producer_reuse_capabilities(
    rebuild_scope: crate::dev::watch::RebuildScope,
    coverage: ProducerReuseCoverage,
) -> tola_build::build::BuildReuse {
    let inventories = rebuild_scope != crate::dev::watch::RebuildScope::FullSite;
    tola_build::build::BuildReuse {
        content_inventory: inventories && coverage.content,
        typst_compilation: rebuild_scope < crate::dev::watch::RebuildScope::FullTypst
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
    use super::{
        DevFailureWatches, InvocationSettings, ProducerReuseCoverage, invocation_config_change,
        producer_reuse_capabilities, resolved_diagnostics, retained_with_previous_warnings,
    };
    use crate::config::{DevConfig, ServerConfig};
    use std::collections::BTreeSet;
    use tola_build::config::ResolvedSiteConfig;

    #[test]
    fn unresolved_warnings_are_retained() {
        let warning = tola_build::diagnostic::Diagnostic::new(
            tola_build::codes::typst::COMPILE,
            tola_build::diagnostic::Severity::Warning,
            "layout was ignored during HTML export",
        );
        let error = tola_build::diagnostic::Diagnostic::new(
            tola_build::codes::typst::COMPILE,
            tola_build::diagnostic::Severity::Error,
            "unclosed delimiter",
        );

        assert_eq!(
            retained_with_previous_warnings(std::slice::from_ref(&warning), vec![error.clone()]),
            [error, warning.clone()]
        );
        assert_eq!(
            retained_with_previous_warnings(std::slice::from_ref(&warning), vec![warning.clone()]),
            std::slice::from_ref(&warning)
        );
    }

    #[test]
    fn resolved_warnings_are_reported_once() {
        let warning = tola_build::diagnostic::Diagnostic::new(
            tola_build::codes::typst::COMPILE,
            tola_build::diagnostic::Severity::Warning,
            "layout was ignored during HTML export",
        );
        let error = tola_build::diagnostic::Diagnostic::new(
            tola_build::codes::typst::COMPILE,
            tola_build::diagnostic::Severity::Error,
            "unclosed delimiter",
        );

        assert!(
            resolved_diagnostics(
                &[warning.clone(), error.clone()],
                std::slice::from_ref(&error)
            )
            .eq([&warning])
        );
        assert!(
            resolved_diagnostics(
                std::slice::from_ref(&warning),
                std::slice::from_ref(&warning)
            )
            .next()
            .is_none()
        );
    }

    /// One warning about `content/post.typ` line `line`, as the round that read `excerpt` saw it.
    fn warning_with_excerpt(line: usize, excerpt: &str) -> tola_build::diagnostic::Diagnostic {
        tola_build::diagnostic::Diagnostic::at_location(
            tola_build::codes::typst::COMPILE,
            tola_build::diagnostic::Severity::Warning,
            tola_build::diagnostic::Location {
                path: "content/post.typ".to_owned(),
                line: Some(line),
                column: Some(1),
                range: None,
                source_lines: vec![tola_build::diagnostic::SourceLine::new(
                    line,
                    excerpt,
                    Some((0, excerpt.len())),
                )],
            },
            "layout was ignored during HTML export",
        )
    }

    #[test]
    fn edited_excerpt_is_not_the_same_warning() {
        let warning = warning_with_excerpt(3, "#let title = \"first\"");
        let edited = warning_with_excerpt(3, "#let title = \"second\"");

        assert!(
            resolved_diagnostics(
                std::slice::from_ref(&warning),
                std::slice::from_ref(&edited)
            )
            .next()
            .is_none()
        );
        assert_eq!(
            retained_with_previous_warnings(std::slice::from_ref(&warning), vec![edited.clone()]),
            [edited]
        );
    }

    #[test]
    fn same_position_warnings_resolve_separately() {
        let warning = warning_with_excerpt(3, "#let title = \"Site\"");
        let removed = tola_build::diagnostic::Diagnostic {
            message: "an unused variable was removed".to_owned(),
            ..warning.clone()
        };

        assert_eq!(
            resolved_diagnostics(
                &[warning.clone(), removed.clone()],
                std::slice::from_ref(&warning)
            )
            .cloned()
            .collect::<Vec<_>>(),
            [removed]
        );
    }

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
                "[[build.hooks.before-build]]\nname = \"unused\"\ncommand = [\"unused-before-config-rejection\"]\ngenerates = [\"generated\"]\n",
            ).unwrap();
            let loaded = crate::config::load(
                Some(&config_path),
                tola_build::InputScope::Pure,
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
            let hooks =
                crate::dev::hooks::HookQueue::start(output.clone(), None, cancellation.token())
                    .unwrap();
            let mut rebuilding = super::RebuildLoop::start(super::RebuildStartup {
                initial_config: std::sync::Arc::clone(&config),
                server,
                dev,
                config_loader,
                resources: tola_build::BuildResources::new()
                    .with_input_scope(tola_build::InputScope::Pure),
                sites: crate::dev::site::CurrentSite::new(),
                reload: None,
                shutdown: cancellation,
                output,
                hooks: hooks.sender(),
            })
            .await
            .unwrap();
            std::fs::write(&config_path, replacement).unwrap();
            let rejected = rebuilding
                .settle_build(
                    super::RebuildRequest {
                        paths: vec![config_path],
                        rebuild_scope: crate::dev::watch::RebuildScope::FullSite,
                    },
                    super::DevBuildTrigger::FilesChanged,
                )
                .await;

            std::fs::write(&generated, "edited after config rejection").unwrap();
            let observed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    let (paths, _) = rebuilding.changes.next(&config).await?.into_parts();
                    if paths.iter().any(|path| generated.starts_with(path)) {
                        return Ok::<_, crate::dev::watch::WatchError>(());
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

    fn resolve_schema(
        root: &std::path::Path,
        schema: tola_build::config::SiteConfigSchema,
    ) -> ResolvedSiteConfig {
        std::fs::create_dir_all(root).unwrap();
        schema
            .resolve(
                &root.join("tola.toml"),
                tola_typst::PackageLocations::from_absolute_roots(None, None).unwrap(),
                &tola_build::config::loading::BuildOverrides::default(),
            )
            .unwrap()
    }

    /// The named setting the development invocation must restart for.
    fn restart_setting_for(
        root: &std::path::Path,
        change: impl FnOnce(&mut tola_build::config::SiteConfigSchema),
        change_server: impl FnOnce(&mut ServerConfig),
        change_dev: impl FnOnce(&mut DevConfig),
    ) -> Option<&'static str> {
        let current = resolve_schema(root, tola_build::config::SiteConfigSchema::default());
        let mut schema = tola_build::config::SiteConfigSchema::default();
        change(&mut schema);
        let candidate = resolve_schema(root, schema);
        let server = ServerConfig::default();
        let mut changed_server = server.clone();
        change_server(&mut changed_server);
        let dev = DevConfig::default();
        let mut changed_dev = dev.clone();
        change_dev(&mut changed_dev);
        invocation_config_change(
            InvocationSettings {
                config: &current,
                server: &server,
                dev: &dev,
            },
            InvocationSettings {
                config: &candidate,
                server: &changed_server,
                dev: &changed_dev,
            },
        )
    }

    #[test]
    fn invocation_settings_decide_restart() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();

        assert_eq!(
            restart_setting_for(root, |_| {}, |server| server.port += 1, |_| {}),
            Some("server.port")
        );
        assert_eq!(
            restart_setting_for(
                root,
                |_| {},
                |server| server.interface = "0.0.0.0".parse().unwrap(),
                |_| {}
            ),
            Some("server.interface")
        );
        assert_eq!(
            restart_setting_for(root, |_| {}, |_| {}, |dev| dev.watch = !dev.watch),
            Some("dev.watch")
        );
        assert_eq!(
            restart_setting_for(
                root,
                |schema| schema.site.base_path = "/docs/".to_owned(),
                |_| {},
                |_| {}
            ),
            Some("site.base-path")
        );
        assert_eq!(
            restart_setting_for(root, |_| {}, |_| {}, |_| {}),
            None,
            "an unchanged configuration keeps its invocation"
        );

        assert_eq!(
            restart_setting_for(
                root,
                |schema| schema.build.minify.html = !schema.build.minify.html,
                |_| {},
                |_| {}
            ),
            None,
            "a build setting keeps the invocation"
        );
    }

    #[test]
    fn producer_uncertainty_confined_to_owner() {
        let full = ProducerReuseCoverage {
            content: true,
            typst_reads: true,
            configured_assets: true,
        };
        let capabilities = |coverage| {
            producer_reuse_capabilities(crate::dev::watch::RebuildScope::Paths, coverage)
        };

        let mut configured_uncertain = full;
        configured_uncertain.configured_assets = false;
        let configured = capabilities(configured_uncertain);
        assert!(!configured.configured_assets);
        assert!(configured.typst_compilation);

        let mut typst_uncertain = full;
        typst_uncertain.typst_reads = false;
        let typst = capabilities(typst_uncertain);
        assert!(!typst.typst_compilation);
        assert!(typst.configured_assets);

        let mut content_uncertain = full;
        content_uncertain.content = false;
        let content = capabilities(content_uncertain);
        assert!(!content.content_inventory);
        assert!(!content.typst_compilation);
        assert!(content.configured_assets);

        let full_typst =
            producer_reuse_capabilities(crate::dev::watch::RebuildScope::FullTypst, full);
        assert!(full_typst.content_inventory);
        assert!(!full_typst.typst_compilation);
        assert!(full_typst.configured_assets);

        let full_site =
            producer_reuse_capabilities(crate::dev::watch::RebuildScope::FullSite, full);
        assert!(!full_site.content_inventory);
        assert!(!full_site.typst_compilation);
        assert!(!full_site.configured_assets);
    }

    #[test]
    fn failure_rechecks_keep_hook_paths() {
        let source = path(1);
        let generated = path(2);
        let hook_retry = || {
            Some(super::RebuildRequest {
                paths: vec![source.clone(), generated.clone()],
                rebuild_scope: crate::dev::watch::RebuildScope::FullTypst,
            })
        };

        // An immediate recheck scans the full site and inherits pending hook paths; a
        // continuing watch retries only the pending hook work.
        for (hook_retry, expected_paths) in [
            (None, vec![source.clone()]),
            (hook_retry(), vec![source.clone(), generated.clone()]),
        ] {
            let retry = super::RebuildRequest::after_failure(
                vec![source.clone()],
                true,
                hook_retry,
                &mut super::FailureRetry::default(),
            )
            .expect("new failure watches require an immediate recheck");
            assert_eq!(
                retry.rebuild_scope,
                crate::dev::watch::RebuildScope::FullSite
            );
            assert_eq!(retry.paths, expected_paths);
        }

        let continuous = super::RebuildRequest::after_failure(
            vec![source.clone()],
            false,
            hook_retry(),
            &mut super::FailureRetry::default(),
        )
        .unwrap();
        assert_eq!(continuous.paths, [source, generated]);
        assert_eq!(
            continuous.rebuild_scope,
            crate::dev::watch::RebuildScope::FullTypst
        );
        assert!(
            super::RebuildRequest::after_failure(
                vec![path(1)],
                false,
                None,
                &mut super::FailureRetry::default(),
            )
            .is_none()
        );
    }

    #[test]
    fn failure_retry_admits_one_per_settle() {
        let mut retry = super::FailureRetry::default();
        let source = path(1);

        super::RebuildRequest::after_failure(vec![source.clone()], true, None, &mut retry)
            .expect("the first failure of a settle call is rechecked");
        assert!(
            super::RebuildRequest::after_failure(vec![source], true, None, &mut retry).is_none()
        );
    }

    #[test]
    fn failed_build_drops_watched_paths() {
        let mut state = DevFailureWatches::default();
        let missing = path(1);

        state.build_failed([missing], Vec::new());
        state.build_failed(std::iter::empty(), Vec::new());

        assert!(state.all_paths(std::iter::empty()).is_empty());
    }

    #[test]
    fn lost_build_watches_recovery_roots() {
        let mut watches = DevFailureWatches::default();
        let previous_failed_path = path(1);
        let directory = tempfile::TempDir::new().unwrap();
        let site_root = directory.path().join("site");
        std::fs::create_dir_all(&site_root).unwrap();
        let site_root = site_root.canonicalize().unwrap();
        let site_parent = site_root.parent().unwrap();
        let package_data = site_parent.join("package-data");
        let package_cache = site_parent.join("package-cache");
        let package_locations = tola_typst::PackageLocations::from_absolute_roots(
            Some(package_data.clone()),
            Some(package_cache.clone()),
        )
        .unwrap();
        let config = tola_build::config::SiteConfigSchema::default()
            .resolve(
                &site_root.join("tola.toml"),
                package_locations,
                &tola_build::config::loading::BuildOverrides::default(),
            )
            .unwrap();

        watches.build_failed([previous_failed_path], Vec::new());
        watches.build_task_failed(&config);

        assert!(watches.all_paths(std::iter::empty()).is_empty());
        assert_eq!(
            watches
                .typst_recovery_roots
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([package_cache, package_data, site_root])
        );

        watches.build_failed([path(2)], Vec::new());
        assert!(watches.typst_recovery_roots.is_empty());
    }

    fn path(id: u8) -> std::path::PathBuf {
        format!("/site/read-{id}.typ").into()
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
        schema.typst.fonts.paths = vec![std::path::PathBuf::from("fonts")];
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
        let mut changes = crate::dev::watch::FileChangeSource::start(
            &config,
            &dynamic,
            &Default::default(),
            None,
        )
        .await
        .unwrap();
        let _ = changes.begin_candidate(&config);
        changes.reject_hook_candidate();
        std::fs::write(&physical_font, b"repaired font bytes").unwrap();
        let observed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let (paths, _) = changes.next(&config).await?.into_parts();
                if paths.contains(&physical_font) {
                    return Ok::<_, crate::dev::watch::WatchError>(());
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

    #[test]
    fn read_watch_paths_include_failures() {
        let mut state = DevFailureWatches::default();

        state.build_failed([path(2), path(3)], Vec::new());
        assert_eq!(
            state.all_paths([path(3), path(1)]),
            [path(1), path(2), path(3)]
        );

        state.current_site_replaced();
        assert_eq!(state.all_paths([path(1)]), [path(1)]);

        state.build_failed([path(4)], Vec::new());
        assert_eq!(state.all_paths([path(1), path(4)]), [path(1), path(4)]);
    }
}
