//! Input observations, failed-attempt reads, and final evidence validation.

use anyhow::Result;

use crate::config::ResolvedSiteConfig;
use crate::observation::{InputKind, InputObservation};

/// A build that produced no complete site, with the inputs it observed before it
/// stopped. A failure before any attempt ran, such as an unavailable build lock,
/// reports no observed inputs.
pub struct BuildFailure {
    error: anyhow::Error,
    inputs: Box<FailedAttemptInputs>,
}

struct FailedAttemptInputs {
    observation: InputObservation,
    source_hooks: crate::hooks::SourceHookOutputs,
    failed_hook_outputs: Option<crate::hooks::FailedHookOutputs>,
}

impl BuildFailure {
    /// A coordinated build that stopped before any attempt ran.
    pub(super) fn before_attempt(error: anyhow::Error) -> Self {
        Self::new(error, BuildAttemptInputs::default())
    }

    pub(super) fn new(error: anyhow::Error, inputs: BuildAttemptInputs) -> Self {
        let source_hooks = inputs.source_hooks;
        let (physical_reads, package_checks) = inputs.compiler.into_parts();
        let mut observation = input_observation(
            physical_reads,
            package_checks,
            None,
            None,
            source_hooks.outputs(),
            inputs.font_read_paths,
        );
        if let Some(path) = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<tola_typst::FontLoadError>())
            .and_then(tola_typst::FontLoadError::path)
        {
            observation.exact_paths(InputKind::FontInventory, false, [path.to_path_buf()]);
        }
        observation.normalize_paths();
        Self {
            error,
            inputs: Box::new(FailedAttemptInputs {
                observation,
                source_hooks,
                failed_hook_outputs: inputs.failed_hook_outputs,
            }),
        }
    }

    pub fn error(&self) -> &anyhow::Error {
        &self.error
    }
    pub fn is_cancelled(&self) -> bool {
        crate::cancellation::is_cancelled(&self.error)
    }
    pub fn input_observation(&self) -> &InputObservation {
        &self.inputs.observation
    }
    pub fn hook_outputs(&self) -> &[crate::hooks::HookOutputEvidence] {
        self.inputs.source_hooks.outputs()
    }
    pub fn source_hooks(&self) -> &crate::hooks::SourceHookOutputs {
        &self.inputs.source_hooks
    }

    /// Source generation stopped before the candidate's source discovery.
    pub fn source_hooks_failed(&self) -> bool {
        self.error
            .chain()
            .any(|cause| cause.is::<crate::hooks::SourceHookFailure>())
    }

    /// Observed files left by a failed source generator. These can suppress
    /// duplicate filesystem events but never prove successful generation.
    pub fn failed_hook_outputs(&self) -> Option<&crate::hooks::FailedHookOutputs> {
        self.inputs.failed_hook_outputs.as_ref()
    }
    pub fn into_error(self) -> anyhow::Error {
        self.error
    }
}

impl std::fmt::Debug for BuildFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BuildFailure")
            .field("error", &self.error)
            .field("observation", &self.inputs.observation)
            .finish()
    }
}

impl std::fmt::Display for BuildFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.error, formatter)
    }
}

impl std::error::Error for BuildFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error.as_ref())
    }
}

/// Configuration-derived recovery inputs needed before the first build runs.
pub fn configured_input_observation(config: &ResolvedSiteConfig) -> InputObservation {
    let mut observation = InputObservation::default();
    observation.filesystem_sources(
        InputKind::Icons,
        &crate::icon::configured_collection_watch_evidence(config),
        false,
    );
    observation.normalize_paths();
    observation
}

pub(super) fn input_observation(
    physical_reads: Vec<std::path::PathBuf>,
    package_checks: Vec<tola_typst::PackageCheck>,
    configured_assets: Option<&crate::asset::ConfiguredAssetInventory>,
    icons: Option<&crate::icon::IconInventoryEvidence>,
    hook_outputs: &[crate::hooks::HookOutputEvidence],
    font_read_paths: Vec<std::path::PathBuf>,
) -> InputObservation {
    let mut observation = InputObservation {
        physical_reads,
        package_checks,
        paths: Vec::new(),
    };
    observation.exact_paths(InputKind::FontInventory, true, font_read_paths);
    if let Some(assets) = configured_assets {
        observation.filesystem_sources(InputKind::ConfiguredAssets, &assets.watch_evidence(), true);
    }
    if let Some(icons) = icons {
        observation.filesystem_sources(InputKind::Icons, &icons.watch_evidence(), true);
    }
    observation.filesystem_sources(
        InputKind::BeforeBuildHookOutputs,
        &crate::hooks::HookOutputEvidence::watch_evidence(hook_outputs),
        true,
    );
    observation.normalize_paths();
    observation
}

/// Stale filesystem-backed inputs. Callers may use these to choose a retry
/// scope; writing requires the set to be empty.
#[derive(Debug, Clone)]
pub struct InputFreshness {
    stale_inputs: std::collections::BTreeSet<InputKind>,
}

/// Paths from a watcher batch with accepted producer coverage. Without this
/// coverage, producers must rediscover their inputs rather than reuse inventories.
#[derive(Debug, Clone)]
pub(crate) struct AcceptedFileChanges {
    paths: std::sync::Arc<[std::path::PathBuf]>,
}

impl AcceptedFileChanges {
    pub(crate) fn from_watcher(paths: Vec<std::path::PathBuf>) -> Self {
        Self {
            paths: paths.into(),
        }
    }

    pub(crate) fn paths(&self) -> &[std::path::PathBuf] {
        &self.paths
    }
}

impl InputFreshness {
    fn from_checks(checks: impl IntoIterator<Item = (InputKind, bool)>) -> Self {
        Self {
            stale_inputs: checks
                .into_iter()
                .filter_map(|(input, fresh)| (!fresh).then_some(input))
                .collect(),
        }
    }

    pub fn all_inputs_are_fresh(&self) -> bool {
        self.stale_inputs.is_empty()
    }

    pub fn is_stale(&self, input: InputKind) -> bool {
        self.stale_inputs.contains(&input)
    }

    pub fn is_fresh(&self, input: InputKind) -> bool {
        !self.is_stale(input)
    }

    fn stale_input_names(&self) -> Vec<&'static str> {
        self.stale_inputs
            .iter()
            .map(|input| input.display_name())
            .collect()
    }
}

/// Revalidate this candidate's evidence before either write policy commits it.
pub(super) fn input_freshness(
    config: &ResolvedSiteConfig,
    producer_caches: &crate::build::RetainedProducerCaches,
    content: &[crate::content::ContentUnit],
    hook_outputs: &[crate::hooks::HookOutputEvidence],
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<InputFreshness> {
    cancellation.ensure_active()?;
    let started = std::time::Instant::now();
    let icons_started = started.elapsed();
    let icons = producer_caches
        .icons
        .evidence()
        .is_fresh_for(config, cancellation)?;
    let icons_elapsed = started.elapsed();
    let mut hooks_fresh = true;
    for evidence in hook_outputs {
        if !evidence.is_current_with_cancellation(cancellation)? {
            hooks_fresh = false;
            break;
        }
    }
    let hooks_elapsed = started.elapsed();
    let physical_reads_fresh = producer_caches
        .dependencies
        .physical_reads_are_fresh(producer_caches.host.source_boundary(), cancellation)?;
    let reads_elapsed = started.elapsed();
    let package_checks_fresh = producer_caches
        .dependencies
        .package_checks_are_fresh(cancellation)?;
    let packages_elapsed = started.elapsed();
    let content_inventory_fresh = content_inventory_is_fresh(config, content, cancellation)?;
    let content_elapsed = started.elapsed();
    let configured_assets_fresh = producer_caches
        .configured_assets
        .is_fresh_for(config, cancellation)?;
    let assets_elapsed = started.elapsed();
    let font_inventory_fresh = producer_caches.host.font_inventory_is_fresh(cancellation)?;
    let total_elapsed = started.elapsed();
    tracing::debug!(target: "tola::compile",
        icons_freshness_ms = (icons_elapsed - icons_started).as_secs_f64() * 1000.0,
        hooks_freshness_ms = (hooks_elapsed - icons_elapsed).as_secs_f64() * 1000.0,
        reads_freshness_ms = (reads_elapsed - hooks_elapsed).as_secs_f64() * 1000.0,
        packages_freshness_ms = (packages_elapsed - reads_elapsed).as_secs_f64() * 1000.0,
        content_freshness_ms = (content_elapsed - packages_elapsed).as_secs_f64() * 1000.0,
        assets_freshness_ms = (assets_elapsed - content_elapsed).as_secs_f64() * 1000.0,
        fonts_freshness_ms = (total_elapsed - assets_elapsed).as_secs_f64() * 1000.0,
        freshness_ms = total_elapsed.as_secs_f64() * 1000.0,
        "verified complete candidate inputs");
    let freshness = InputFreshness::from_checks([
        (InputKind::TypstPhysicalReads, physical_reads_fresh),
        (InputKind::TypstPackageSelection, package_checks_fresh),
        (InputKind::ContentInventory, content_inventory_fresh),
        (InputKind::ConfiguredAssets, configured_assets_fresh),
        (InputKind::Icons, icons),
        (InputKind::FontInventory, font_inventory_fresh),
        (InputKind::BeforeBuildHookOutputs, hooks_fresh),
    ]);
    cancellation.ensure_active()?;
    Ok(freshness)
}

pub(super) fn stale_input_error(freshness: &InputFreshness) -> anyhow::Error {
    crate::diagnostic::DiagnosticError::new(
        "site inputs changed before the output was written",
        vec![
            crate::diagnostic::Diagnostic::new(
                crate::codes::write::STALE,
                crate::diagnostic::Severity::Error,
                "a site input changed while the site was being built",
            )
            .with_note(format!(
                "changed: {}",
                freshness.stale_input_names().join(", ")
            ))
            .with_help("Run the build again once edits have stopped"),
        ],
    )
    .into()
}

fn content_inventory_is_fresh(
    config: &ResolvedSiteConfig,
    candidate: &[crate::content::ContentUnit],
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<bool> {
    cancellation.ensure_active()?;
    let current =
        crate::content::discover_content_units_for_config_with_cancellation(config, cancellation);
    cancellation.ensure_active()?;
    let Ok(current) = current else {
        return Ok(false);
    };
    cancellation.ensure_active()?;
    Ok(current == candidate)
}

#[derive(Default)]
pub(crate) struct BuildAttemptInputs {
    pub(super) compiler: crate::compiler::BuildInputs,
    pub(super) font_read_paths: Vec<std::path::PathBuf>,
    pub(super) source_hooks: crate::hooks::SourceHookOutputs,
    pub(super) failed_hook_outputs: Option<crate::hooks::FailedHookOutputs>,
}
