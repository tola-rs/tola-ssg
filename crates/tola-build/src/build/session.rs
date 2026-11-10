//! Repeated-build baseline: accepted caches advance only after a write or accepted revision.

use crate::compiler::{
    CompilationDependencies, CompilationReuse, RebuildDecision, SiteProgramCache, TypstHost,
    analysis,
};
use crate::mode::BuildMode;

/// Why an attempt refused before it could compile the site.
#[derive(Debug, thiserror::Error)]
enum AttemptRefusal {
    #[error("the site's modules changed while Tola was preparing the build; run the command again")]
    ManifestsChangedBeforeBuild,
    #[error("the site's modules changed before the build's hooks ran; run the command again")]
    ManifestsChangedBeforeHooks,
    #[error("module watch uses a different input scope")]
    WatchScopeMismatch,
    #[error(
        "`--pure` cannot use `[typst.fonts] system = true`; keep the fonts in `[typst.fonts] paths`, or run the build without `--pure`"
    )]
    SystemFontsInPureBuild,
    #[error("Tola could not reuse the previous build's work; run the command again")]
    StaleCacheOrigin,
}

/// Shared resources and accepted caches for repeated builds of one site.
pub struct BuildSession {
    resources: crate::resources::BuildResources,
    // Attempts share immutable loaded resources even after failure. This lock is
    // held only while taking or replacing a handle, never during filesystem work.
    host_cache: std::sync::Arc<std::sync::Mutex<Option<TypstHost>>>,
    accepted: Option<std::sync::Arc<BuildCacheUpdate>>,
}

pub(crate) struct RetainedProducerCaches {
    pub(crate) host: TypstHost,
    pub(crate) dependencies: CompilationDependencies,
    pub(crate) source_analysis: analysis::SourceAnalysisCache,
    pub(crate) site_program: SiteProgramCache,
    pub(crate) configured_assets: crate::asset::ConfiguredAssetInventory,
    pub(crate) icons: crate::icon::IconInventory,
    pub(crate) images: std::sync::Arc<crate::image::output::ImageOutputs>,
    pub(crate) seo: std::sync::Arc<crate::seo::SeoCompilation>,
    /// Fingerprints of what the before-build hooks wrote for this revision.
    pub(crate) hook_outputs: crate::hooks::SourceHookOutputs,
    /// The previous successful development revision's reusable `generate-outputs` products.
    pub(crate) hook_reuse: crate::hooks::ReusableHookOutputs,
}

/// The session and accepted generation one attempt's caches originated from.
struct BuildCacheOrigin {
    session: std::sync::Weak<std::sync::Mutex<Option<TypstHost>>>,
    accepted: Option<std::sync::Weak<BuildCacheUpdate>>,
}

/// Producer caches kept inseparable from their build or checked revision.
pub(crate) struct BuildCacheUpdate {
    origin: Option<BuildCacheOrigin>,
    caches: RetainedProducerCaches,
    content_inventory: std::sync::Arc<[crate::content::ContentUnit]>,
    content_root: std::path::PathBuf,
    references: crate::site::references::References,
}

impl BuildCacheUpdate {
    pub(super) fn new(
        caches: RetainedProducerCaches,
        content_inventory: std::sync::Arc<[crate::content::ContentUnit]>,
        content_root: std::path::PathBuf,
        references: crate::site::references::References,
    ) -> Self {
        Self {
            origin: None,
            caches,
            content_inventory,
            content_root,
            references,
        }
    }

    pub(super) fn caches(&self) -> &RetainedProducerCaches {
        &self.caches
    }

    /// Attribute these caches to the session and generation that produced them.
    pub(super) fn attribute_to(&mut self, session: &BuildSession) {
        self.origin = Some(BuildCacheOrigin {
            session: std::sync::Arc::downgrade(&session.host_cache),
            accepted: session.accepted.as_ref().map(std::sync::Arc::downgrade),
        });
    }

    /// A cancelled attempt recorded partial evidence, so its cancellation is checked
    /// around the caller's own freshness walk.
    pub(super) fn freshness(
        &self,
        config: &crate::config::ResolvedSiteConfig,
        hook_outputs: &[crate::hooks::HookOutputEvidence],
        attempt_cancellation: &crate::cancellation::BuildCancellation,
        caller_cancellation: &crate::cancellation::BuildCancellation,
    ) -> anyhow::Result<super::InputFreshness> {
        attempt_cancellation.ensure_active()?;
        let freshness = super::input::input_freshness(
            config,
            &self.caches,
            &self.content_inventory,
            hook_outputs,
            caller_cancellation,
        )?;
        attempt_cancellation.ensure_active()?;
        Ok(freshness)
    }
}

/// Cache reuse permitted by the caller's input observations.
///
/// All permissions default to false. Producers still check their retained evidence.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuildReuse {
    pub content_inventory: bool,
    pub typst_compilation: bool,
    pub configured_assets: bool,
}

/// Why this build runs, kept distinct from cache-reuse permissions.
#[derive(Debug, Clone, Default)]
pub enum BuildTrigger {
    #[default]
    Initial,
    Paths(std::sync::Arc<[std::path::PathBuf]>),
    AllInputs(std::sync::Arc<[std::path::PathBuf]>),
}

impl BuildTrigger {
    /// Changed input paths, when this attempt follows an input invalidation.
    pub fn paths(&self) -> Option<&[std::path::PathBuf]> {
        match self {
            Self::Initial => None,
            Self::Paths(paths) | Self::AllInputs(paths) => Some(paths),
        }
    }
}

/// Inputs for one build attempt, movable to a worker thread.
pub struct BuildRequest {
    pub mode: BuildMode,
    pub trigger: BuildTrigger,
    pub reuse: BuildReuse,
    pub generated_files: Vec<crate::output::GeneratedFile>,
    pub cancellation: crate::cancellation::BuildCancellation,
    /// Whether candidate construction runs hook commands. Publication consumers
    /// are invoked separately through [`super::WriteOutcome::run_after_publish`].
    pub hook_execution: crate::hooks::HookExecution,
    /// Whether the attempt checks page references against the published site.
    pub reference_checks: super::ReferenceChecks,
    /// Whether a development attempt must run every reusable generator instead of reusing it.
    pub disable_hook_reuse: bool,
    /// Retain description-only projections, including stages completed before failure.
    pub capture_report: bool,
}

impl BuildRequest {
    /// Create a full-build request with no event-qualified cache reuse.
    pub fn new(mode: BuildMode) -> Self {
        Self {
            mode,
            trigger: BuildTrigger::Initial,
            reuse: BuildReuse::default(),
            generated_files: Vec::new(),
            cancellation: crate::cancellation::BuildCancellation::default(),
            hook_execution: crate::hooks::HookExecution::Run,
            reference_checks: super::ReferenceChecks::Full,
            disable_hook_reuse: false,
            capture_report: false,
        }
    }
}

impl Default for BuildRequest {
    fn default() -> Self {
        Self::new(BuildMode::Production)
    }
}

/// An owned build attempt suitable for a caller's blocking worker.
pub struct BuildAttempt {
    config: std::sync::Arc<crate::config::ResolvedSiteConfig>,
    baseline: BuildSession,
    request: BuildRequest,
    prepared_modules: bool,
}

impl BuildAttempt {
    /// Prepare resources and run the build pipeline.
    pub fn run(mut self) -> Result<super::SiteBuild, super::BuildFailure> {
        if !self.prepared_modules {
            self.config = std::sync::Arc::new(
                crate::modules::prepared::configure(
                    &self.config,
                    self.baseline.resources.input_mode(),
                    &self.request.cancellation,
                )
                .map_err(super::BuildFailure::before_attempt)?,
            );
        }
        if !self
            .config
            .module_inputs
            .manifests_are_fresh(&self.request.cancellation)
            .map_err(super::BuildFailure::before_attempt)?
        {
            return Err(super::BuildFailure::before_attempt(anyhow::Error::new(
                AttemptRefusal::ManifestsChangedBeforeBuild,
            )));
        }
        if let Err(error) = super::diagnostic::refuse_incomplete_vendor(&self.config) {
            return Err(super::BuildFailure::before_attempt(error));
        }
        let mut producers = super::pipeline::BuildAttemptProducers {
            resources: self.baseline.resources.clone(),
            cancellation: self.request.cancellation.clone(),
            report: self
                .request
                .capture_report
                .then(super::BuildReport::default),
            generated_files: std::mem::take(&mut self.request.generated_files),
            previous_hook_reuse: if self.request.disable_hook_reuse {
                None
            } else {
                self.baseline
                    .accepted_caches()
                    .map(|caches| caches.hook_reuse.clone())
            },
            ..super::pipeline::BuildAttemptProducers::default()
        };
        let built = (|| -> anyhow::Result<super::SiteBuild> {
            let bootstrap = self.prepare_module_hooks(&mut producers)?;
            if !self
                .config
                .module_inputs
                .manifests_are_fresh(&producers.cancellation)?
            {
                return Err(anyhow::Error::new(
                    AttemptRefusal::ManifestsChangedBeforeHooks,
                ));
            }
            super::pipeline::build_site_inner(
                &self.config,
                &self.request,
                &mut producers,
                |producers, hooks_executed| {
                    self.prepare_compiler(producers, hooks_executed, bootstrap.as_ref())
                },
            )
        })();
        match built {
            Ok(mut built) => {
                built.cache_update.attribute_to(&self.baseline);
                let reads = producers.inputs.package_reads().to_vec();
                if !reads.is_empty()
                    && let Err(error) = crate::package::write_package_lock(
                        self.config.get_root(),
                        &reads,
                        self.baseline.resources.input_mode(),
                    )
                {
                    return Err(super::BuildFailure::new(
                        error.into(),
                        producers.into_inputs(),
                    ));
                }
                Ok(built)
            }
            Err(error) => Err(super::BuildFailure::new(error, producers.into_inputs())),
        }
    }

    /// Install the hook stages modules declare before any generator runs.
    fn prepare_module_hooks(
        &mut self,
        producers: &mut super::pipeline::BuildAttemptProducers,
    ) -> anyhow::Result<Option<crate::modules::bootstrap::Bootstrap>> {
        let declares_hooks = self
            .config
            .module_inputs
            .configured
            .values()
            .any(|configured| !configured.module.hooks.is_empty());
        if !declares_hooks {
            return Ok(None);
        }
        let bootstrap = crate::modules::bootstrap::Bootstrap::prepare(
            &self.config,
            &producers.resources,
            &producers.cancellation,
        )
        .map_err(|mut error| {
            let carried = super::pipeline::record_module_preparation_failure(
                &mut error,
                &self.config,
                producers.inputs_mut(),
            );
            match carried {
                Some(mut diagnostics) => {
                    diagnostics
                        .extend(super::diagnostic::for_error(&error, self.config.get_root()));
                    anyhow::Error::new(tola_diagnostic::DiagnosticError::attach(error, diagnostics))
                }
                None => error,
            }
        })?;
        producers
            .inputs_mut()
            .record_accessed(&bootstrap.options.accessed);
        bootstrap.install(std::sync::Arc::make_mut(&mut self.config))?;
        bootstrap.ensure_current(&self.config, &producers.cancellation)?;
        Ok(Some(bootstrap))
    }

    /// Load the compiler host and the accepted-evidence reuse, after hooks wrote their inputs.
    fn prepare_compiler(
        &self,
        producers: &mut super::pipeline::BuildAttemptProducers,
        hooks_executed: bool,
        bootstrap: Option<&crate::modules::bootstrap::Bootstrap>,
    ) -> anyhow::Result<TypstHost> {
        let cancellation = producers.cancellation.clone();
        if let Some(bootstrap) = bootstrap {
            bootstrap.ensure_current(&self.config, &cancellation)?;
        }
        let host = self.baseline.load_host(&self.config, &cancellation)?;
        let accepted = self.baseline.accepted.as_deref();
        let host_inputs_match =
            accepted.is_some_and(|accepted| accepted.caches.host.has_same_inputs(&host));
        // A before-build hook that wrote exactly what the previous revision already
        // held changes no input, so the event-qualified reuse stays valid. Only a
        // generator whose declared outputs actually differ withdraws it.
        let hook_outputs_match = self
            .baseline
            .hook_outputs_match_accepted(&producers.source_hooks);
        producers.hook_outputs_unchanged = hooks_executed && hook_outputs_match;
        let permissions = if host_inputs_match
            && (!hooks_executed || hook_outputs_match)
            && matches!(self.request.trigger, BuildTrigger::Paths(_))
        {
            self.request.reuse
        } else {
            BuildReuse::default()
        };
        let changed_paths = self.request.trigger.paths().unwrap_or_default();
        let content_inventory = self.baseline.reusable_content_inventory(
            &self.config,
            changed_paths,
            permissions.content_inventory,
        );
        cancellation.ensure_active()?;
        let decision = self
            .baseline
            .rebuild_decision(changed_paths, content_inventory.is_none());
        tracing::debug!(
            target: "tola::compile",
            source_priority = ?decision.source_analysis,
            site_priority = ?decision.site_program,
            direct = ?decision.direct_readers,
            affected = ?decision.affected_readers,
            "selected producer recomputation from observed changes"
        );
        producers.reuse = self.baseline.reusable_compilation(
            &decision,
            permissions.typst_compilation && content_inventory.is_some(),
        );
        producers.icons = accepted.map(|accepted| accepted.caches.icons.clone());
        producers.images = accepted.map(|accepted| std::sync::Arc::clone(&accepted.caches.images));
        producers.seo = accepted.map(|accepted| std::sync::Arc::clone(&accepted.caches.seo));
        producers.reference_reuse = accepted.map(|accepted| accepted.references.clone());
        producers.previous_bundle_entries = self.baseline.bundle_entries();
        if permissions.configured_assets {
            producers.configured_assets = self.baseline.configured_assets().cloned();
            producers.configured_asset_changes = Some(super::AcceptedFileChanges::from_watcher(
                changed_paths.to_vec(),
            ));
        }
        producers.content_inventory = content_inventory;
        cancellation.ensure_active()?;
        Ok(host)
    }
}

impl Default for BuildSession {
    fn default() -> Self {
        Self::new()
    }
}

impl BuildSession {
    /// Create an empty resource cache. Host loading starts only when an attempt runs.
    pub fn new() -> Self {
        Self::with_resources(crate::resources::BuildResources::default())
    }

    pub fn with_resources(resources: crate::resources::BuildResources) -> Self {
        Self {
            resources,
            host_cache: std::sync::Arc::new(std::sync::Mutex::new(None)),
            accepted: None,
        }
    }

    /// Build and commit the output tree, then accept its caches.
    /// Consumers run only when the caller invokes [`super::WriteOutcome::run_after_publish`]
    /// on the returned outcome, after the build lock has been released.
    pub fn build_and_write(
        &mut self,
        config: &crate::config::ResolvedSiteConfig,
        request: BuildRequest,
    ) -> anyhow::Result<super::WriteOutcome> {
        let locked = super::SiteBuildGuard::build_for_publication(
            self,
            std::sync::Arc::new(config.clone()),
            request,
            || {},
        )
        .map_err(super::BuildFailure::into_error)?;
        self.ensure_cache_origin(&locked.site().cache_update)?;
        let outcome = locked.write_site()?;
        let built = locked.release();
        self.commit_cache(built.cache_update);
        Ok(outcome)
    }

    /// Capture a request and its accepted baseline without filesystem work.
    pub fn prepare(
        &mut self,
        config: std::sync::Arc<crate::config::ResolvedSiteConfig>,
        request: BuildRequest,
    ) -> BuildAttempt {
        BuildAttempt {
            config,
            baseline: Self {
                resources: self.resources.clone(),
                host_cache: std::sync::Arc::clone(&self.host_cache),
                accepted: self.accepted.clone(),
            },
            request,
            prepared_modules: false,
        }
    }

    /// Start an attempt from the prepared hook watch whose paths were staged.
    pub fn prepare_with_hook_watch(
        &mut self,
        watch: &crate::modules::PreparedHookWatch,
        request: BuildRequest,
    ) -> anyhow::Result<BuildAttempt> {
        if watch.scope() != self.resources.input_scope() {
            return Err(anyhow::Error::new(AttemptRefusal::WatchScopeMismatch));
        }
        let mut attempt = self.prepare(std::sync::Arc::clone(watch.config()), request);
        attempt.prepared_modules = true;
        Ok(attempt)
    }

    fn load_host(
        &self,
        config: &crate::config::ResolvedSiteConfig,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> anyhow::Result<TypstHost> {
        cancellation.ensure_active()?;
        // Host fonts cannot join a pure build; the setting is contradictory, not ignorable.
        if self.resources.input_scope() == crate::InputScope::Pure && config.fonts().system {
            return Err(anyhow::Error::new(AttemptRefusal::SystemFontsInPureBuild));
        }
        let cached = {
            self.host_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        };
        let host = match cached {
            Some(host)
                if host.matches(config, &self.resources)
                    && host.font_inventory_is_fresh(cancellation)? =>
            {
                host
            }
            _ => TypstHost::for_config_with_resources(config, &self.resources, cancellation)?,
        };
        let lock = crate::inputs::InputLock::read(config.get_root())?.unwrap_or_default();
        let host =
            match crate::package::verifier(&lock, self.resources.input_mode(), config.get_root()) {
                Some(verifier) => host.with_package_verifier(verifier),
                None => host,
            };
        cancellation.ensure_active()?;
        self.retain_host(&host);
        Ok(host.with_local_packages(std::sync::Arc::clone(&config.module_inputs.packages)))
    }

    fn retain_host(&self, host: &TypstHost) {
        let replaced = {
            self.host_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .replace(host.clone())
        };
        // Releasing a resource generation must not hold the shared cache lock.
        drop(replaced);
    }

    /// Accepted caches installed by the last checked revision.
    fn accepted_caches(&self) -> Option<&RetainedProducerCaches> {
        self.accepted.as_ref().map(|accepted| &accepted.caches)
    }

    /// Whether the hooks this attempt ran wrote exactly what the accepted revision holds.
    pub(crate) fn hook_outputs_match_accepted(
        &self,
        produced: &crate::hooks::SourceHookOutputs,
    ) -> bool {
        let Some(accepted) = self.accepted_caches() else {
            return false;
        };
        let previous = accepted.hook_outputs.outputs();
        let current = produced.outputs();
        !current.is_empty()
            && current.len() == previous.len()
            && current
                .iter()
                .zip(previous)
                .all(|(produced, accepted)| produced == accepted)
    }

    pub(crate) fn rebuild_decision(
        &self,
        changed_paths: &[std::path::PathBuf],
        inventory_may_change: bool,
    ) -> RebuildDecision {
        RebuildDecision::for_paths(
            self.accepted_caches().map(|caches| &caches.dependencies),
            changed_paths,
            inventory_may_change,
        )
    }

    /// The accepted content inventory, when this rebuild can reuse it.
    pub(crate) fn reusable_content_inventory(
        &self,
        config: &crate::config::ResolvedSiteConfig,
        changed_paths: &[std::path::PathBuf],
        allow_pre_scan_reuse: bool,
    ) -> Option<std::sync::Arc<[crate::content::ContentUnit]>> {
        if !allow_pre_scan_reuse {
            return None;
        }
        let accepted = self.accepted.as_ref()?;
        let current_root = crate::filesystem::normalize_existing_prefix(&config.build.content_dir);
        if current_root != accepted.content_root
            || crate::filesystem::normalize_existing_prefix(&config.build.entry)
                != crate::filesystem::normalize_existing_prefix(&accepted.caches.site_program.entry)
        {
            return None;
        }
        let previous = &accepted.content_inventory;
        (!crate::content::inventory_may_change(config, previous, changed_paths))
            .then(|| std::sync::Arc::clone(previous))
    }

    pub(crate) fn reusable_compilation(
        &self,
        decision: &RebuildDecision,
        allow_reuse: bool,
    ) -> Option<CompilationReuse> {
        if !allow_reuse {
            return None;
        }
        let caches = self.accepted_caches()?;
        let project_entry = matches!(
            typst::World::main(caches.site_program.world.as_ref()).root(),
            typst::syntax::VirtualRoot::Project
        )
        .then_some(caches.site_program.entry.as_path());
        let source_analysis = caches
            .source_analysis
            .clone()
            .prepare_for_rebuild(decision, project_entry);
        if decision.reuses_site_program() && decision.reuses_source_analysis() {
            Some(CompilationReuse::SiteProgram {
                site_program: Box::new(caches.site_program.clone()),
                dependencies: Box::new(caches.dependencies.clone()),
                source_analysis: Box::new(source_analysis),
            })
        } else {
            Some(CompilationReuse::SourceAnalysis {
                source_analysis: Box::new(source_analysis),
                dependencies: Box::new(caches.dependencies.clone()),
            })
        }
    }

    pub(crate) fn bundle_entries(&self) -> Option<tola_typst::BundleEntries> {
        self.accepted_caches()
            .map(|caches| caches.site_program.bundle_entries.clone())
    }

    /// Install a checked revision whose origin matches, then advance the cache baseline.
    pub fn install_revision<T>(
        &mut self,
        checked: super::CheckedRevision,
        install: impl FnOnce(crate::site::SiteRevision) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let (revision, cache_update) = checked.into_parts()?;
        self.ensure_cache_origin(&cache_update)?;
        let installed = install(revision)?;
        self.commit_cache(cache_update);
        Ok(installed)
    }

    fn ensure_cache_origin(&self, update: &BuildCacheUpdate) -> anyhow::Result<()> {
        let same_origin = update.origin.as_ref().is_some_and(|origin| {
            std::ptr::eq(
                origin.session.as_ptr(),
                std::sync::Arc::as_ptr(&self.host_cache),
            ) && origin.accepted.as_ref().map(std::sync::Weak::as_ptr)
                == self.accepted.as_ref().map(std::sync::Arc::as_ptr)
        });
        if !same_origin {
            return Err(anyhow::Error::new(AttemptRefusal::StaleCacheOrigin));
        }
        Ok(())
    }

    /// Infallible after origin validation and successful publication.
    fn commit_cache(&mut self, committed: BuildCacheUpdate) {
        self.retain_host(&committed.caches.host);
        self.accepted = Some(std::sync::Arc::new(committed));
    }

    pub(crate) fn configured_assets(&self) -> Option<&crate::asset::ConfiguredAssetInventory> {
        self.accepted_caches()
            .map(|caches| &caches.configured_assets)
    }
}

#[cfg(test)]
mod tests {
    use super::BuildSession;
    use crate::build::pipeline::BuildAttemptProducers;
    use crate::build::tests::*;
    use crate::build::*;
    use crate::config::section::build::BeforeBuildHookConfig;

    use crate::config::section::{AssetTreeDeclaration, AssetUrlPrefix};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn dev_config(root: &Path) -> crate::config::ResolvedSiteConfig {
        let mut config = site_config(root);
        std::fs::write(&config.build.entry, "#document(\"index.html\")[Site]").unwrap();
        config.typst.fonts.paths = vec![root.join("fonts")];
        std::fs::create_dir_all(&config.typst.fonts.paths[0]).unwrap();
        config
    }

    fn published_input_site(root: &Path) -> std::sync::Arc<crate::config::ResolvedSiteConfig> {
        let config = dev_config(root);
        std::fs::write(root.join("value.txt"), "Published value").unwrap();
        std::fs::write(
            &config.build.entry,
            r#"#document("index.html")[#read("value.txt")]"#,
        )
        .unwrap();
        std::sync::Arc::new(config)
    }

    /// Executed as an independent hook child, never directly by the test harness.
    #[test]
    fn child_copies_stylesheet_trigger() {
        if std::env::var("TOLA_HOOK_STAGE").as_deref() != Ok("before-build") {
            return;
        }
        fs::create_dir_all("generated").unwrap();
        fs::write(
            "generated/site.css",
            fs::read_to_string("src/style.txt").unwrap(),
        )
        .unwrap();
    }

    /// Every output of one build, keyed by logical output path.
    fn all_output_bytes(build: &SiteBuild) -> std::collections::BTreeMap<String, Vec<u8>> {
        build
            .graph()
            .outputs()
            .iter()
            .map(|output| (output.path().as_str().to_owned(), output.bytes().to_vec()))
            .collect()
    }

    #[test]
    fn reuse_matches_cold_build() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let config = published_input_site(root);
        // The edit reaches `about` through the site metadata but cannot reach
        // `plain`, whose analysis must therefore be reused verbatim.
        std::fs::write(
            config.build.content_dir.join("about.typ"),
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: "About"))"#,
        )
        .unwrap();
        std::fs::write(
            config.build.content_dir.join("plain.typ"),
            r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: "Plain"))"#,
        )
        .unwrap();
        std::fs::write(
            &config.build.entry,
            r#"#import "@tola/source:0.0.0": all-sources
#let about = all-sources().find(source => source.id == "about.typ")
#let plain = all-sources().find(source => source.id == "plain.typ")
#document("index.html")[#read("value.txt") / #about.meta.title / #plain.meta.title]"#,
        )
        .unwrap();

        let mut session = BuildSession::new();
        let cold = session
            .prepare(
                std::sync::Arc::clone(&config),
                super::BuildRequest::new(super::BuildMode::Development),
            )
            .run()
            .unwrap();
        let cold_html =
            String::from_utf8_lossy(&all_output_bytes(&cold)["index.html"]).into_owned();
        assert!(cold_html.contains("About"), "{cold_html}");
        assert!(cold_html.contains("Plain"), "{cold_html}");
        accept_build(&mut session, cold);

        std::fs::write(
            config.build.content_dir.join("about.typ"),
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: "Changed"))"#,
        )
        .unwrap();
        let changed = config.build.content_dir.join("about.typ");
        let warm = session
            .prepare(
                std::sync::Arc::clone(&config),
                super::BuildRequest {
                    trigger: super::BuildTrigger::Paths(std::sync::Arc::from(
                        [changed.clone()].as_slice(),
                    )),
                    reuse: super::BuildReuse {
                        content_inventory: true,
                        typst_compilation: true,
                        configured_assets: true,
                    },
                    ..Default::default()
                },
            )
            .run()
            .unwrap();

        let cold_again = crate::build::build_site(&config, super::BuildMode::Development).unwrap();

        // Cache reuse is an optimization, never a second semantic path: a warm
        // revision must be indistinguishable from the cold one it replaces, in
        // every output byte and in every diagnostic the author would read.
        assert_eq!(all_output_bytes(&warm), all_output_bytes(&cold_again));
        assert_eq!(warm.diagnostics(), cold_again.diagnostics());
        let warm_html =
            String::from_utf8_lossy(&all_output_bytes(&warm)["index.html"]).into_owned();
        assert!(
            warm_html.contains("Changed"),
            "the warm build must publish the edit, not a reused stale value: {warm_html}"
        );
        assert!(warm_html.contains("Plain"), "{warm_html}");
    }

    #[test]
    fn rebuild_publishes_new_bytes() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = published_input_site(directory.path());
        let mut session = BuildSession::new();
        let first = session
            .build_and_write(&config, super::BuildRequest::default())
            .unwrap();
        assert_eq!(first.counts().pages, 1);
        assert_eq!(first.output_root(), config.build.publish_dir.as_path());
        let html_path = first.output_root().join("index.html");
        assert!(
            std::fs::read_to_string(&html_path)
                .unwrap()
                .contains("Published value")
        );

        std::fs::write(directory.path().join("value.txt"), "Second value").unwrap();
        let second = session
            .build_and_write(&config, super::BuildRequest::default())
            .unwrap();
        assert_eq!(second.counts().pages, 1);
        assert!(
            String::from_utf8_lossy(&std::fs::read(&html_path).unwrap()).contains("Second value")
        );
    }

    #[test]
    fn reuse_follows_hook_output_bytes() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("content")).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        let trigger = root.join("src/style.txt");
        fs::write(&trigger, "first").unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            "#let style = read(\"generated/site.css\")\n#document(\"index.html\")[#style]",
        )
        .unwrap();
        let generated = root.join("generated");

        let mut config = site_config(root);
        config.build.publish_dir = root.join("public");
        config.assets.trees = vec![AssetTreeDeclaration::new(
            &generated,
            AssetUrlPrefix::parse("/assets").unwrap(),
        )];
        config.build.hooks.before_build.push(BeforeBuildHookConfig {
            name: "stylesheet".into(),
            command: hook_child_command("build::session::tests::child_copies_stylesheet_trigger"),
            dev: crate::config::section::build::DevParticipation::Run,
            rerun_on: vec!["src/style.txt".into()],
            generates: vec!["generated/site.css".into()],
            ..BeforeBuildHookConfig::default()
        });

        let mut session = crate::build::BuildSession::new();
        let host = compiler_host(&config).unwrap();
        let first = build_site_with_host(
            &config,
            &host,
            BuildMode::Development,
            &mut BuildAttemptProducers::default(),
        )
        .unwrap();
        install_build(&mut session, first);

        let changed_paths = vec![root.join("notes/unrelated.txt")];
        let rebuild = |changed_paths: &[PathBuf]| -> SiteBuild {
            let content_inventory = session
                .reusable_content_inventory(&config, changed_paths, true)
                .expect("published content inventory is reusable before the hook executes");
            let decision = session.rebuild_decision(changed_paths, false);
            let reuse = session
                .reusable_compilation(&decision, true)
                .expect("published compilation is reusable before the hook executes");
            let mut producers = BuildAttemptProducers {
                reuse: Some(reuse),
                previous_bundle_entries: session.bundle_entries(),
                configured_assets: session.configured_assets().cloned(),
                configured_asset_changes: Some(AcceptedFileChanges::from_watcher(
                    changed_paths.to_vec(),
                )),
                content_inventory: Some(content_inventory),
                ..BuildAttemptProducers::default()
            };
            build_site_with_host(&config, &host, BuildMode::Development, &mut producers).unwrap()
        };

        let unchanged = rebuild(&changed_paths);
        assert_eq!(unchanged.hook_outputs().len(), 1);
        assert!(unchanged.hook_outputs()[0].is_current());
        assert_eq!(output_bytes(&unchanged.graph, "assets/site.css"), b"first");
        assert!(
            String::from_utf8_lossy(output_bytes(&unchanged.graph, "index.html")).contains("first")
        );

        fs::write(&trigger, "second").unwrap();
        let changed = rebuild(&changed_paths);

        let stylesheet = changed
            .graph
            .outputs()
            .iter()
            .find(|output| output.path().as_str() == "assets/site.css")
            .expect("configured stylesheet output");
        let document = changed
            .graph
            .outputs()
            .iter()
            .find(|output| output.path().as_str() == "index.html")
            .expect("site document output");
        assert_eq!(stylesheet.bytes(), b"second");
        assert!(String::from_utf8_lossy(document.bytes()).contains("second"));
        assert_eq!(changed.hook_outputs().len(), 1);
        assert!(changed.hook_outputs()[0].is_current());
    }
}
