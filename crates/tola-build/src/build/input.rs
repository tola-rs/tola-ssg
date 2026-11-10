//! Input observations, failed-attempt reads, and final evidence validation.

use anyhow::Result;

use crate::config::ResolvedSiteConfig;
use crate::observation::{InputKind, InputObservation};

/// A build that produced no complete site, with the inputs it observed before it
/// stopped. A failure before any attempt ran, such as an unavailable build lock,
/// reports no observed inputs.
#[derive(thiserror::Error)]
#[error("{error}")]
pub struct BuildFailure {
    #[source]
    error: anyhow::Error,
    inputs: Box<FailedAttemptInputs>,
    cancelled: bool,
    source_hooks_failed: bool,
}

struct FailedAttemptInputs {
    observation: InputObservation,
    source_hooks: crate::hooks::SourceHookOutputs,
    failed_hook_outputs: Option<crate::hooks::FailedHookOutputs>,
    report: Option<super::BuildReport>,
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
            cancelled: crate::cancellation::is_cancelled(&error),
            source_hooks_failed: error
                .chain()
                .any(|cause| cause.is::<crate::hooks::SourceHookFailure>()),
            error,
            inputs: Box::new(FailedAttemptInputs {
                observation,
                source_hooks,
                failed_hook_outputs: inputs.failed_hook_outputs,
                report: inputs.report,
            }),
        }
    }

    /// The diagnostics for the failed attempt, projected relative to the site root.
    pub fn diagnostics(&self, root: &std::path::Path) -> Vec<tola_diagnostic::Diagnostic> {
        if self.cancelled {
            return Vec::new();
        }
        super::diagnostic::reports(&self.error, root, tola_diagnostic::codes::build::SITE)
    }
    /// Completed projections, absent when capture was disabled or no attempt began.
    pub fn report(&self) -> Option<&super::BuildReport> {
        self.inputs.report.as_ref()
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled
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
        self.source_hooks_failed
    }

    /// Observed files left by a failed source generator. These can suppress
    /// duplicate filesystem events but never prove successful generation.
    pub fn failed_hook_outputs(&self) -> Option<&crate::hooks::FailedHookOutputs> {
        self.inputs.failed_hook_outputs.as_ref()
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

/// Configuration-derived recovery inputs needed before the first build runs.
pub fn configured_input_observation(config: &ResolvedSiteConfig) -> InputObservation {
    let mut observation = InputObservation::default();
    observation.filesystem_sources(
        InputKind::Icons,
        &crate::icon::configured_collection_watch_evidence(config),
        false,
    );
    for directory in &config.module_font_directories {
        let identity = tola_input::FilesystemSourceIdentity::from_path(&directory.path);
        observation.path_pair(
            InputKind::FontInventory,
            crate::observation::InputPathScope::Recursive,
            false,
            identity.logical_path(),
            identity.physical_path(),
        );
    }
    let lock = config.get_root().join("tola.lock");
    let identity = tola_input::FilesystemSourceIdentity::from_path(&lock);
    observation.path_pair(
        InputKind::ModuleManifests,
        crate::observation::InputPathScope::Exact,
        false,
        identity.logical_path(),
        identity.physical_path(),
    );
    for (name, declaration) in config.modules() {
        if let tola_module::ModuleSource::Tarball { tarball } = &declaration.source
            && let tola_input::TarballLocation::File { path } = &tarball.location
        {
            let identity =
                tola_input::FilesystemSourceIdentity::from_path(&config.get_root().join(path));
            observation.path_pair(
                InputKind::ModuleManifests,
                crate::observation::InputPathScope::Exact,
                false,
                identity.logical_path(),
                identity.physical_path(),
            );
        }
        let root = match &declaration.source {
            tola_module::declaration::ModuleSource::Local { path } => config.get_root().join(path),
            tola_module::declaration::ModuleSource::Git { .. }
            | tola_module::declaration::ModuleSource::Tarball { .. } => {
                match crate::inputs::declaration_root(
                    config.get_root(),
                    config.vendor().path.as_deref(),
                    name,
                    &declaration.source,
                ) {
                    Ok(root) => root,
                    Err(_) => continue,
                }
            }
        };
        for path in [root.clone(), root.join("tola-module.toml")] {
            let identity = tola_input::FilesystemSourceIdentity::from_path(&path);
            observation.path_pair(
                InputKind::ModuleManifests,
                crate::observation::InputPathScope::Exact,
                false,
                identity.logical_path(),
                identity.physical_path(),
            );
        }
    }
    observation.normalize_paths();
    observation
}

pub(super) fn input_observation(
    physical_reads: Vec<std::path::PathBuf>,
    package_checks: Vec<tola_typst::PackageCheck>,
    configured_assets: Option<&crate::asset::ConfiguredAssets>,
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
        assets.record_watch_inputs(&mut observation);
    }
    if let Some(icons) = icons {
        observation.filesystem_sources(InputKind::Icons, &icons.watch_evidence(), true);
    }
    observation.filesystem_sources(
        InputKind::BeforeBuildHookOutputs,
        &crate::hooks::HookOutputEvidence::watch_evidence(hook_outputs),
        true,
    );
    observation
}

/// Stale filesystem-backed inputs. Callers may use these to choose a retry
/// scope; writing requires the set to be empty.
#[derive(Debug, Clone)]
pub struct InputFreshness {
    stale_inputs: std::collections::BTreeSet<InputKind>,
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

/// The changed inputs as one sentence reads them: `a`, `b` and `c`.
fn joined_inputs(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [head @ .., last] => format!("{} and {last}", head.join(", ")),
    }
}

/// Revalidate this candidate's evidence before either write policy commits it.
pub(super) fn input_freshness(
    config: &ResolvedSiteConfig,
    modules: &tola_module::prepared::PreparedModules,
    producer_caches: &crate::build::RetainedProducerCaches,
    content: &[crate::content::ContentUnit],
    hook_outputs: &[crate::hooks::HookOutputEvidence],
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<InputFreshness> {
    cancellation.ensure_active()?;
    let started = std::time::Instant::now();
    let icons_started = started.elapsed();
    let icons_fresh = producer_caches
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
    let manifests_fresh = modules.manifests_are_fresh(&cancellation.bundle_cancellation())?;
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
    let configured_assets_fresh = producer_caches.configured_assets.is_fresh_for(
        config,
        crate::modules::prepared::AppliedAssets {
            modules: &modules.assets,
        },
        cancellation,
    )?;
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
        (InputKind::ModuleManifests, manifests_fresh),
        (InputKind::TypstPackageSelection, package_checks_fresh),
        (InputKind::ContentInventory, content_inventory_fresh),
        (InputKind::ConfiguredAssets, configured_assets_fresh),
        (InputKind::Icons, icons_fresh),
        (InputKind::FontInventory, font_inventory_fresh),
        (InputKind::BeforeBuildHookOutputs, hooks_fresh),
    ]);
    cancellation.ensure_active()?;
    Ok(freshness)
}

pub(super) fn stale_input_error(freshness: &InputFreshness) -> anyhow::Error {
    tola_diagnostic::DiagnosticError::new(
        "site inputs changed before the output was written",
        vec![
            tola_diagnostic::Diagnostic::new(
                tola_diagnostic::codes::write::STALE,
                tola_diagnostic::Severity::Error,
                "a site input changed while the site was being built",
            )
            .with_note(format!(
                "{} no longer match what Tola read",
                joined_inputs(&freshness.stale_input_names())
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
    pub(super) report: Option<super::BuildReport>,
}
