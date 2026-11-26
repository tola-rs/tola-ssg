//! Input observations, failed-attempt reads, and final evidence validation.

use anyhow::Result;

use crate::config::SiteConfig;
use crate::observation::{InputKind, InputObservation, InputPathScope};

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
    obs: InputObservation,
    source_hooks: crate::hooks::SourceHookOutputs,
    failed_hook_outputs: Option<crate::hooks::FailedHookOutputs>,
    report: Option<super::BuildReport>,
}

impl BuildFailure {
    pub(super) fn before_attempt(err: anyhow::Error, root: &std::path::Path) -> Self {
        Self::new(err, BuildAttemptInputs::default(), root)
    }

    pub(super) fn new(
        err: anyhow::Error,
        inputs: BuildAttemptInputs,
        root: &std::path::Path,
    ) -> Self {
        let source_hooks = inputs.source_hooks;
        let (physical_reads, pkg_checks) = inputs.compiler.into_parts();
        let mut obs = input_observation(
            physical_reads,
            pkg_checks,
            None,
            None,
            source_hooks.outputs(),
            inputs.font_read_paths,
        );
        if let Some(path) = err
            .chain()
            .find_map(|cause| cause.downcast_ref::<tola_typst::FontLoadError>())
            .and_then(tola_typst::FontLoadError::path)
        {
            obs.exact_paths(InputKind::FontInventory, false, [path.to_path_buf()]);
        }
        obs.normalize_paths();
        Self {
            cancelled: crate::cancellation::is_cancelled(&err),
            source_hooks_failed: err
                .chain()
                .any(|cause| cause.is::<crate::hooks::SourceHookFailure>()),
            error: super::diagnostic::with_site(err, root),
            inputs: Box::new(FailedAttemptInputs {
                obs,
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
        &self.inputs.obs
    }
    pub fn hook_outputs(&self) -> &[crate::hooks::HookOutput] {
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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuildFailure")
            .field("error", &self.error)
            .field("observation", &self.inputs.obs)
            .finish()
    }
}

fn observe_path_identity(
    obs: &mut InputObservation,
    kind: InputKind,
    scope: InputPathScope,
    path: &std::path::Path,
) {
    let id = tola_input::FilesystemSourceIdentity::from_path(path);
    obs.path_pair(kind, scope, false, id.logical_path(), id.physical_path());
}

/// Configuration-derived recovery inputs needed before the first build runs.
pub fn configured_input_observation(config: &SiteConfig) -> InputObservation {
    let mut obs = InputObservation::default();
    obs.filesystem_sources(
        InputKind::Icons,
        &crate::icon::configured_collection_watch_evidence(config),
        false,
    );
    for dir in &config.module_font_directories {
        observe_path_identity(
            &mut obs,
            InputKind::FontInventory,
            InputPathScope::Recursive,
            &dir.path,
        );
    }
    let lock = config.root().join("tola.lock");
    observe_path_identity(
        &mut obs,
        InputKind::ModuleManifests,
        InputPathScope::Exact,
        &lock,
    );
    for (name, decl) in config.modules() {
        if let tola_module::ModuleSource::Tarball { tarball } = &decl.source
            && let tola_input::TarballLocation::File { path } = &tarball.location
        {
            observe_path_identity(
                &mut obs,
                InputKind::ModuleManifests,
                InputPathScope::Exact,
                &config.root().join(path),
            );
        }
        let root = match &decl.source {
            tola_module::declaration::ModuleSource::Local { path } => config.root().join(path),
            tola_module::declaration::ModuleSource::Git { .. }
            | tola_module::declaration::ModuleSource::Tarball { .. }
            | tola_module::declaration::ModuleSource::Builtin { .. } => {
                match crate::inputs::declaration_root(
                    config.root(),
                    config.vendor().path.as_deref(),
                    name,
                    &decl.source,
                ) {
                    Ok(root) => root,
                    Err(_) => continue,
                }
            }
        };
        for path in [root.clone(), root.join("tola-module.toml")] {
            observe_path_identity(
                &mut obs,
                InputKind::ModuleManifests,
                InputPathScope::Exact,
                &path,
            );
        }
    }
    obs.normalize_paths();
    obs
}

pub(super) fn input_observation(
    physical_reads: Vec<std::path::PathBuf>,
    package_checks: Vec<tola_typst::PackageCheck>,
    configured_assets: Option<&crate::asset::SiteAssets>,
    icons: Option<&crate::icon::IconSnapshot>,
    hook_outputs: &[crate::hooks::HookOutput],
    font_read_paths: Vec<std::path::PathBuf>,
) -> InputObservation {
    let mut obs = InputObservation {
        physical_reads,
        package_checks,
        paths: Vec::new(),
    };
    obs.exact_paths(InputKind::FontInventory, true, font_read_paths);
    if let Some(assets) = configured_assets {
        assets.record_watch_inputs(&mut obs);
    }
    if let Some(icons) = icons {
        obs.filesystem_sources(InputKind::Icons, &icons.watch_evidence(), true);
    }
    obs.filesystem_sources(
        InputKind::BeforeBuildHookOutputs,
        &crate::hooks::HookOutput::watch_evidence(hook_outputs),
        true,
    );
    obs
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

/// Revalidate this candidate's evidence before either write boundary commits it.
pub(super) fn input_freshness(
    cfg: &SiteConfig,
    mods: &tola_module::bundle::ModuleBundle,
    caches: &crate::build::RetainedProducerCaches,
    content: &[crate::content::ContentUnit],
    hook_outputs: &[crate::hooks::HookOutput],
    cancel: &crate::cancellation::BuildCancellation,
) -> Result<InputFreshness> {
    cancel.ensure_active()?;
    let started = std::time::Instant::now();
    let icons_fresh = caches.icons.evidence().is_fresh_for(cfg, cancel)?;
    let icons_at = started.elapsed();
    let mut hooks_fresh = true;
    for evidence in hook_outputs {
        if !evidence.is_current_with_cancellation(cancel)? {
            hooks_fresh = false;
            break;
        }
    }
    let hooks_at = started.elapsed();
    let manifests_fresh = mods.manifests_are_fresh(&cancel.bundle_cancellation())?;
    let physical_reads_fresh = caches
        .dependencies
        .physical_reads_are_fresh(caches.host.source_boundary(), cancel)?;
    let reads_at = started.elapsed();
    let package_checks_fresh = caches.dependencies.package_checks_are_fresh(cancel)?;
    let packages_at = started.elapsed();
    let content_inventory_fresh = content_inventory_is_fresh(cfg, content, cancel)?;
    let content_at = started.elapsed();
    let configured_assets_fresh = caches.configured_assets.is_fresh_for(
        cfg,
        crate::modules::site::AppliedAssets {
            modules: &mods.assets,
        },
        cancel,
    )?;
    let assets_at = started.elapsed();
    let font_inventory_fresh = caches.host.font_inventory_is_fresh(cancel)?;
    let total_at = started.elapsed();
    let ms = |span: std::time::Duration| span.as_secs_f64() * 1000.0;
    tracing::debug!(target: "tola::compile",
        icons_freshness_ms = ms(icons_at),
        hooks_freshness_ms = ms(hooks_at - icons_at),
        reads_freshness_ms = ms(reads_at - hooks_at),
        packages_freshness_ms = ms(packages_at - reads_at),
        content_freshness_ms = ms(content_at - packages_at),
        assets_freshness_ms = ms(assets_at - content_at),
        fonts_freshness_ms = ms(total_at - assets_at),
        freshness_ms = ms(total_at),
        "verified complete candidate inputs");
    let config_fresh = match cfg.source() {
        Some(src) => src.is_current(Some(cancel))?,
        None => true,
    };
    let freshness = InputFreshness::from_checks([
        (InputKind::Configuration, config_fresh),
        (InputKind::TypstPhysicalReads, physical_reads_fresh),
        (InputKind::ModuleManifests, manifests_fresh),
        (InputKind::TypstPackageSelection, package_checks_fresh),
        (InputKind::ContentInventory, content_inventory_fresh),
        (InputKind::SiteAssets, configured_assets_fresh),
        (InputKind::Icons, icons_fresh),
        (InputKind::FontInventory, font_inventory_fresh),
        (InputKind::BeforeBuildHookOutputs, hooks_fresh),
    ]);
    cancel.ensure_active()?;
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
    cfg: &SiteConfig,
    candidate: &[crate::content::ContentUnit],
    cancel: &crate::cancellation::BuildCancellation,
) -> Result<bool> {
    cancel.ensure_active()?;
    let current = crate::content::discover_content_units_for_config_with_cancellation(cfg, cancel);
    cancel.ensure_active()?;
    let Ok(current) = current else {
        return Ok(false);
    };
    cancel.ensure_active()?;
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
