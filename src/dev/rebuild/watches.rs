use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use tola_build::build::{BuildReuse, InputFreshness, InputKind, InputObservation, InputPathScope};
use tola_build::config::SiteConfig;
use tola_diagnostic::{Diagnostic, Severity};

use crate::config::{DevConfig, ServerConfig};
use crate::dev::watch::{
    EVENT_DETAIL_CAPACITY, FileChangeBatch, FileChangeSource, ProducerKind, RebuildScope,
    WatchRequirements, append_observed_paths, append_typst_inputs,
    watch_requirements_from_observation,
};

#[derive(Default)]
pub(super) struct DevFailureWatches {
    pub(super) latest_failed_paths: BTreeSet<PathBuf>,
    pub(super) config_failed_paths: BTreeSet<PathBuf>,
    pub(super) latest_failed_packages: Vec<tola_typst::PackageCheck>,
    pub(super) latest_failed_observed_paths: Vec<tola_build::build::ObservedInputPath>,
    pub(super) typst_recovery_roots: Vec<PathBuf>,
    pub(super) module_hook_inputs: Option<tola_module::HookWatchInputs>,
}

impl DevFailureWatches {
    pub(super) fn observe_failure(&mut self, failure: &tola_build::build::BuildFailure) {
        let obs = failure.input_observation();
        let paths = obs.physical_read_paths().iter().cloned().chain(
            failure.hook_outputs().iter().flat_map(|ev| {
                [
                    ev.logical_path().to_path_buf(),
                    ev.physical_path().to_path_buf(),
                ]
            }),
        );
        self.build_failed(paths, obs.package_checks().to_vec());
        self.latest_failed_observed_paths = obs.paths().to_vec();
    }

    pub(super) fn build_failed(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
        package_checks: Vec<tola_typst::PackageCheck>,
    ) {
        *self = Self::default();
        self.latest_failed_paths = paths.into_iter().collect();
        self.latest_failed_packages = package_checks;
    }

    pub(super) fn build_task_failed(&mut self, cfg: &SiteConfig) {
        *self = Self::default();
        self.typst_recovery_roots = std::iter::once(cfg.root().to_path_buf())
            .chain(cfg.modules().iter().filter_map(|(name, module)| {
                tola_build::inputs::declaration_root(
                    cfg.root(),
                    cfg.vendor().path.as_deref(),
                    name,
                    &module.source,
                )
                .ok()
            }))
            .chain(
                [
                    cfg.package_locations().data(),
                    cfg.package_locations().cache(),
                ]
                .into_iter()
                .flatten()
                .map(|loc| loc.root().to_path_buf()),
            )
            .collect();
        self.typst_recovery_roots.sort_unstable();
        self.typst_recovery_roots.dedup();
    }

    pub(super) fn current_site_replaced(&mut self) {
        *self = Self::default();
    }

    fn all_package_checks(
        &self,
        completed_site: &[tola_typst::PackageCheck],
    ) -> Vec<tola_typst::PackageCheck> {
        let mut checks: Vec<_> = completed_site
            .iter()
            .chain(&self.latest_failed_packages)
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        tola_typst::sort_package_checks(&mut checks);
        checks
    }

    fn all_paths(&self, published_reads: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
        let mut paths = published_reads.into_iter().collect::<BTreeSet<_>>();
        paths.extend(self.latest_failed_paths.iter().cloned());
        paths.into_iter().collect()
    }

    pub(super) fn requirements(&self, published: &InputObservation) -> WatchRequirements {
        let paths = self.all_paths(published.physical_read_paths().iter().cloned());
        let package_checks = self.all_package_checks(published.package_checks());
        let mut reqs = watch_requirements_from_observation(published);
        append_typst_inputs(&mut reqs, paths, package_checks);
        append_observed_paths(&mut reqs, &self.latest_failed_observed_paths);
        reqs.observe_paths(
            ProducerKind::Modules,
            InputPathScope::Exact,
            false,
            self.config_failed_paths.iter().cloned(),
        );
        if let Some(inputs) = &self.module_hook_inputs {
            reqs.add_module_hook_inputs(inputs);
        }
        for root in &self.typst_recovery_roots {
            reqs.typst_recovery(root.clone());
        }
        reqs
    }
}

#[derive(Debug)]
pub(super) struct RebuildRequest {
    pub(super) paths: Vec<PathBuf>,
    pub(super) rebuild_scope: RebuildScope,
}

impl RebuildRequest {
    pub(super) fn from_batch(batch: FileChangeBatch) -> Self {
        let (paths, rebuild_scope) = batch.into_parts();
        Self {
            paths,
            rebuild_scope,
        }
    }

    pub(super) fn merge_batch(&mut self, batch: FileChangeBatch) {
        let (paths, rebuild_scope) = batch.into_parts();
        self.rebuild_scope = self
            .rebuild_scope
            .max(rebuild_scope)
            .max(merge_paths(&mut self.paths, paths));
    }

    pub(super) fn after_failure(
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
        // Retry once after attaching watches so inputs modified before attachment are rechecked.
        retry.admit(Some(Self {
            paths,
            rebuild_scope: RebuildScope::FullSite,
        }))
    }
}

/// Bounds one immediate failed-attempt recheck per settle call.
#[derive(Default)]
pub(super) struct FailureRetry {
    spent: bool,
}

impl FailureRetry {
    fn admit(&mut self, retry: Option<RebuildRequest>) -> Option<RebuildRequest> {
        if self.spent {
            return None;
        }
        self.spent = retry.is_some();
        retry
    }
}

pub(super) fn same_diagnostic(left: &Diagnostic, right: &Diagnostic) -> bool {
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

/// Keep prior warnings that were not re-emitted until a successful round clears them.
pub(super) fn retained_with_previous_warnings(
    prev: &[Diagnostic],
    current: Vec<Diagnostic>,
) -> Vec<Diagnostic> {
    let mut retained = current;
    for earlier in prev {
        if earlier.severity == Severity::Warning
            && !retained.iter().any(|cur| same_diagnostic(cur, earlier))
        {
            retained.push(earlier.clone());
        }
    }
    retained
}

pub(super) fn attached_diagnostics<'a>(
    prev: &'a [Diagnostic],
    current: &'a [Diagnostic],
) -> impl Iterator<Item = &'a Diagnostic> {
    prev.iter()
        .filter(|diag| !current.iter().any(|cur| same_diagnostic(cur, diag)))
}

pub(super) fn format_rebuild_summary(
    groups: Option<tola_build::output::summary::ChangedOutputGroups>,
    elapsed: Duration,
    recovered: bool,
) -> String {
    if recovered {
        return format!(
            "Build succeeded in {}",
            crate::terminal::format_duration(elapsed)
        );
    }
    match groups {
        None => crate::terminal::site_summary("Built", elapsed, ""),
        Some(groups) => crate::terminal::site_summary(
            "Rebuilt",
            elapsed,
            &crate::terminal::describe_changes(groups),
        ),
    }
}

pub(super) fn merge_paths(
    paths: &mut Vec<PathBuf>,
    incoming: impl IntoIterator<Item = PathBuf>,
) -> RebuildScope {
    for path in incoming {
        if !paths.contains(&path) {
            if paths.len() == EVENT_DETAIL_CAPACITY {
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
pub(super) struct ProducerReuseCoverage {
    pub(super) content: bool,
    pub(super) typst_reads: bool,
    pub(super) configured_assets: bool,
}

impl ProducerReuseCoverage {
    pub(super) fn of(changes: &FileChangeSource) -> Self {
        Self {
            content: changes.accepts_events_for([ProducerKind::Content]),
            typst_reads: changes.accepts_events_for([ProducerKind::TypstReads]),
            configured_assets: changes.accepts_events_for([ProducerKind::SiteAssets]),
        }
    }
}

const STALE_INVENTORY_INPUTS: [InputKind; 5] = [
    InputKind::Configuration,
    InputKind::ModuleManifests,
    InputKind::ContentInventory,
    InputKind::SiteAssets,
    InputKind::Icons,
];

const STALE_TYPST_INPUTS: [InputKind; 2] =
    [InputKind::TypstPackageSelection, InputKind::FontInventory];

pub(super) fn stale_retry_scope(freshness: &InputFreshness) -> RebuildScope {
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

pub(super) fn producer_reuse_capabilities(
    rebuild_scope: RebuildScope,
    coverage: ProducerReuseCoverage,
) -> BuildReuse {
    let inventories = rebuild_scope != RebuildScope::FullSite;
    BuildReuse {
        content_inventory: inventories && coverage.content,
        typst_compilation: rebuild_scope < RebuildScope::FullTypst
            && coverage.content
            && coverage.typst_reads,
        configured_assets: inventories && coverage.configured_assets,
    }
}

pub(super) struct InvocationSettings<'a> {
    pub(super) config: &'a SiteConfig,
    pub(super) server: &'a ServerConfig,
    pub(super) dev: &'a DevConfig,
}

pub(super) fn invocation_config_change(
    cur: InvocationSettings<'_>,
    candidate: InvocationSettings<'_>,
) -> Option<&'static str> {
    if cur.server.interface != candidate.server.interface {
        Some("server.interface")
    } else if cur.server.port != candidate.server.port {
        Some("server.port")
    } else if cur.dev.watch != candidate.dev.watch {
        Some("dev.watch")
    } else if cur.config.url_mount() != candidate.config.url_mount() {
        Some("site.base-path")
    } else {
        None
    }
}
