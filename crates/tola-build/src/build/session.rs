//! Repeated-build baseline: accepted caches advance only after a write or accepted revision.

use crate::compiler::{
    CompilationDependencies, CompilationReuse, RebuildDecision, SiteHost, SiteProgramCache,
    analysis,
};
use crate::hooks::HookExecutor;
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
    hook_executor: HookExecutor,
    // Attempts share immutable loaded resources even after failure. This lock is
    // held only while taking or replacing a handle, never during filesystem work.
    host_cache: std::sync::Arc<std::sync::Mutex<Option<SiteHost>>>,
    accepted: Option<std::sync::Arc<BuildCacheUpdate>>,
}

pub(crate) struct RetainedProducerCaches {
    pub(crate) host: SiteHost,
    pub(crate) dependencies: CompilationDependencies,
    pub(crate) source_analysis: analysis::SourceAnalysisCache,
    pub(crate) site_program: SiteProgramCache,
    pub(crate) configured_assets: crate::asset::SiteAssets,
    pub(crate) icons: crate::icon::IconInventory,
    pub(crate) images: std::sync::Arc<crate::image::output::ImageOutputs>,
    pub(crate) seo: std::sync::Arc<crate::seo::SeoCompilation>,
    /// Fingerprints of what the before-build hooks wrote for this revision.
    pub(crate) hook_outputs: crate::hooks::SourceHookOutputs,
    pub(crate) prepared_hooks: crate::hooks::ReadyHooks,
    /// The previous successful development revision's reusable `generate-outputs` products.
    pub(crate) hook_reuse: crate::hooks::ReusableHookOutputs,
}

/// The session and accepted generation one attempt's caches originated from.
struct BuildCacheOrigin {
    session: std::sync::Weak<std::sync::Mutex<Option<SiteHost>>>,
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

    /// Both the build attempt and the caller can stop the freshness walk.
    pub(super) fn freshness(
        &self,
        config: &crate::config::SiteConfig,
        modules: &tola_module::bundle::ModuleBundle,
        hook_outputs: &[crate::hooks::HookOutput],
        attempt_cancellation: &crate::cancellation::BuildCancellation,
        caller_cancellation: &crate::cancellation::BuildCancellation,
    ) -> anyhow::Result<super::InputFreshness> {
        let cancellation = attempt_cancellation.either(caller_cancellation);
        super::input::input_freshness(
            config,
            modules,
            &self.caches,
            &self.content_inventory,
            hook_outputs,
            &cancellation,
        )
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
    /// are invoked separately through [`super::PublishSummary::run_after_publish`].
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
    config: std::sync::Arc<crate::config::SiteConfig>,
    /// Set when the attempt starts from an already prepared hook watch.
    prepared: Option<crate::modules::LoadedSite>,
    baseline: BuildSession,
    hook_epoch: u64,
    request: BuildRequest,
}

impl BuildAttempt {
    /// The executor epoch the caller invalidates before cancelling this attempt.
    pub fn hook_epoch(&self) -> u64 {
        self.hook_epoch
    }

    /// Prepare resources and run the build pipeline.
    pub fn run(mut self) -> Result<super::SiteBuild, super::BuildFailure> {
        let before_attempt = |error| super::BuildFailure::before_attempt(error, self.config.root());
        super::diagnostic::refuse_incomplete_vendor(&self.config).map_err(before_attempt)?;
        let mut site = match self.prepared.take() {
            Some(site) => site,
            None => {
                let bundle = self.request.cancellation.bundle_cancellation();
                crate::modules::site::prepare_site_with_bundle(
                    &self.config,
                    self.baseline.resources.input_mode(),
                    &bundle,
                    &[],
                )
                .map_err(before_attempt)?
            }
        };
        if !site
            .prepared()
            .manifests_are_fresh(&self.request.cancellation.bundle_cancellation())
            .map_err(before_attempt)?
        {
            return Err(before_attempt(anyhow::Error::new(
                AttemptRefusal::ManifestsChangedBeforeBuild,
            )));
        }
        let reusable = self
            .baseline
            .accepted_caches()
            .filter(|_| !self.request.disable_hook_reuse);
        let mut producers = super::pipeline::BuildAttemptProducers {
            resources: self.baseline.resources.clone(),
            cancellation: self.request.cancellation.clone(),
            report: self
                .request
                .capture_report
                .then(super::BuildReport::default),
            generated_files: std::mem::take(&mut self.request.generated_files),
            previous_hook_reuse: reusable.map(|caches| caches.hook_reuse.clone()),
            prepared_hooks: reusable
                .map(|caches| caches.prepared_hooks.clone())
                .unwrap_or_default(),
            ..super::pipeline::BuildAttemptProducers::default()
        };
        let built = (|| -> anyhow::Result<super::SiteBuild> {
            super::install_hooks(&mut site, &producers.resources)?;
            if !site
                .prepared()
                .manifests_are_fresh(&producers.cancellation.bundle_cancellation())?
            {
                return Err(anyhow::Error::new(
                    AttemptRefusal::ManifestsChangedBeforeHooks,
                ));
            }
            let executor = self.baseline.hook_executor();
            executor.configure(
                self.hook_epoch,
                site.config().build.par_hooks,
                site.config().build.hook_jobs,
                &producers.cancellation,
            )?;
            super::pipeline::build_site_inner(
                &site,
                &self.request,
                &mut producers,
                executor,
                |producers, hooks_executed| self.prepare_compiler(producers, hooks_executed, &site),
            )
        })();
        match built {
            Ok(mut built) => {
                built.cache_update.attribute_to(&self.baseline);
                let reads = producers.inputs.package_reads().to_vec();
                if let Err(error) = crate::package::write_package_lock(
                    self.config.root(),
                    &reads,
                    self.baseline.resources.input_mode(),
                ) {
                    return Err(super::BuildFailure::new(
                        error.into(),
                        producers.into_inputs(),
                        self.config.root(),
                    ));
                }
                Ok(built)
            }
            Err(error) => Err(super::BuildFailure::new(
                error,
                producers.into_inputs(),
                self.config.root(),
            )),
        }
    }

    /// Load the compiler host and the accepted-evidence reuse, after hooks wrote their inputs.
    fn prepare_compiler(
        &self,
        producers: &mut super::pipeline::BuildAttemptProducers,
        hooks_executed: bool,
        site: &crate::modules::LoadedSite,
    ) -> anyhow::Result<SiteHost> {
        let cancellation = producers.cancellation.clone();
        anyhow::ensure!(
            site.prepared()
                .manifests_are_fresh(&cancellation.bundle_cancellation())?,
            "module manifests changed during before-build hooks; keep generated inputs separate from module declarations",
        );
        site.validate_module_entry()?;
        let host = self
            .baseline
            .load_host(site.config(), site.prepared(), &cancellation)?;
        let accepted = self.baseline.accepted.as_deref();
        let host_inputs_match =
            accepted.is_some_and(|accepted| accepted.caches.host.has_same_inputs(&host));
        // A before-build hook that wrote exactly what the previous revision already
        // held changes no input, so the event-qualified reuse stays valid. Only a
        // generator whose declared outputs actually differ withdraws it.
        let hooks_unchanged = !hooks_executed
            || self
                .baseline
                .hook_outputs_match_accepted(&producers.source_hooks);
        let event_paths = match &self.request.trigger {
            BuildTrigger::Paths(paths) if host_inputs_match && hooks_unchanged => Some(paths),
            _ => None,
        };
        let permissions = if event_paths.is_some() {
            self.request.reuse
        } else {
            BuildReuse::default()
        };
        let changed_paths = self.request.trigger.paths().unwrap_or_default();
        let content_inventory = self.baseline.reusable_content_inventory(
            site.config(),
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
        if hooks_unchanged {
            producers.previous_bundle_entries = self.baseline.bundle_entries();
        }
        if let Some(paths) = event_paths.filter(|_| permissions.configured_assets)
            && let Some(assets) = self.baseline.configured_assets()
        {
            producers.asset_reuse = Some((assets.clone(), paths.clone()));
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
            hook_executor: HookExecutor::new(),
            host_cache: std::sync::Arc::new(std::sync::Mutex::new(None)),
            accepted: None,
        }
    }

    /// The executor shared by this site's builds and publication consumers.
    pub fn hook_executor(&self) -> &HookExecutor {
        &self.hook_executor
    }

    /// The rules for acquiring and reading this site's inputs.
    pub fn input_mode(&self) -> tola_input::InputMode {
        self.resources.input_mode()
    }

    /// Build and commit the output tree, then accept its caches.
    /// Consumers run only when the caller invokes [`super::PublishSummary::run_after_publish`]
    /// on the returned outcome, after the build lock has been released.
    pub fn build_and_write(
        &mut self,
        config: &crate::config::SiteConfig,
        request: BuildRequest,
    ) -> anyhow::Result<super::PublishSummary> {
        let locked = super::SiteBuildGuard::build_for_publication(
            self,
            std::sync::Arc::new(config.clone()),
            request,
            || {},
        )?;
        self.ensure_cache_origin(&locked.site().cache_update)?;
        let outcome = locked.write_site()?;
        let built = locked.release();
        self.commit_cache(built.cache_update);
        Ok(outcome)
    }

    /// Capture a request and its accepted baseline without filesystem work.
    pub fn prepare(
        &mut self,
        config: std::sync::Arc<crate::config::SiteConfig>,
        request: BuildRequest,
    ) -> BuildAttempt {
        BuildAttempt {
            config,
            prepared: None,
            baseline: Self {
                resources: self.resources.clone(),
                hook_executor: self.hook_executor.clone(),
                host_cache: std::sync::Arc::clone(&self.host_cache),
                accepted: self.accepted.clone(),
            },
            hook_epoch: self.hook_executor.begin_attempt(),
            request,
        }
    }

    /// Start an attempt from the prepared hook watch whose paths were staged.
    pub fn prepare_with_hook_watch(
        &mut self,
        watch: &crate::modules::HookWatchEntry,
        request: BuildRequest,
    ) -> anyhow::Result<BuildAttempt> {
        if watch.scope() != self.resources.input_scope() {
            return Err(anyhow::Error::new(AttemptRefusal::WatchScopeMismatch));
        }
        let mut attempt = self.prepare(std::sync::Arc::new(watch.config().clone()), request);
        attempt.prepared = Some(watch.site().clone());
        Ok(attempt)
    }

    fn load_host(
        &self,
        config: &crate::config::SiteConfig,
        prepared: &tola_module::bundle::ModuleBundle,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> anyhow::Result<SiteHost> {
        cancellation.ensure_active()?;
        // Host fonts cannot join a pure build; the setting is contradictory, not ignorable.
        if self.resources.input_scope() == tola_input::InputScope::Pure && config.fonts().system {
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
            _ => SiteHost::for_config(
                config,
                &self.resources,
                std::sync::Arc::clone(&prepared.packages),
                cancellation,
            )?,
        };
        let lock = tola_input::InputLock::read(config.root())?.unwrap_or_default();
        let host = match crate::package::verifier(&lock, self.resources.input_mode(), config.root())
        {
            Some(verifier) => host.with_package_verifier(verifier),
            None => host,
        };
        cancellation.ensure_active()?;
        self.retain_host(&host);
        Ok(host.with_local_packages(std::sync::Arc::clone(&prepared.packages)))
    }

    fn retain_host(&self, host: &SiteHost) {
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
    fn hook_outputs_match_accepted(&self, outputs: &crate::hooks::SourceHookOutputs) -> bool {
        !outputs.outputs().is_empty()
            && self
                .accepted_caches()
                .is_some_and(|accepted| &accepted.hook_outputs == outputs)
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
        config: &crate::config::SiteConfig,
        changed_paths: &[std::path::PathBuf],
        allow_pre_scan_reuse: bool,
    ) -> Option<std::sync::Arc<[crate::content::ContentUnit]>> {
        if !allow_pre_scan_reuse {
            return None;
        }
        let accepted = self.accepted.as_ref()?;
        let current_root = tola_input::normalize_existing_prefix(&config.build.content_dir);
        if current_root != accepted.content_root
            || tola_input::normalize_existing_prefix(&config.build.entry)
                != tola_input::normalize_existing_prefix(&accepted.caches.site_program.entry)
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

    pub(crate) fn configured_assets(&self) -> Option<&crate::asset::SiteAssets> {
        self.accepted_caches()
            .map(|caches| &caches.configured_assets)
    }
}

#[cfg(test)]
mod tests {
    use super::BuildSession;
    use crate::build::tests::*;
    use crate::config::section::build::BeforeBuildHookConfig;

    use crate::config::section::{AssetTreeDeclaration, AssetUrlPrefix};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn dev_config(root: &Path) -> crate::config::SiteConfig {
        let mut cfg = site_config(root);
        std::fs::write(&cfg.build.entry, "#document(\"index.html\")[Site]").unwrap();
        cfg.typst.fonts.paths = vec![root.join("fonts")];
        std::fs::create_dir_all(&cfg.typst.fonts.paths[0]).unwrap();
        cfg
    }

    fn published_input_site(root: &Path) -> std::sync::Arc<crate::config::SiteConfig> {
        let cfg = dev_config(root);
        std::fs::write(root.join("value.txt"), "Published value").unwrap();
        std::fs::write(
            &cfg.build.entry,
            r#"#document("index.html")[#read("value.txt")]"#,
        )
        .unwrap();
        std::sync::Arc::new(cfg)
    }

    /// Executed as an independent hook child, never directly by the test harness.
    #[test]
    fn child_copies_stylesheet_trigger() {
        if !crate::build::tests::is_hook_child_for("before-build") {
            return;
        }
        fs::create_dir_all("generated").unwrap();
        fs::write(
            "generated/site.css",
            fs::read_to_string("src/style.txt").unwrap(),
        )
        .unwrap();
    }

    fn all_output_bytes(
        build: &crate::build::SiteBuild,
    ) -> std::collections::BTreeMap<String, Vec<u8>> {
        build
            .graph()
            .outputs()
            .iter()
            .map(|out| (out.path().as_str().to_owned(), out.bytes().to_vec()))
            .collect()
    }

    #[test]
    fn warm_reuse_matches_cold() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfg = published_input_site(dir.path());
        // Edits reach `about` via metadata while leaving `plain` reusable.
        std::fs::write(
            cfg.build.content_dir.join("about.typ"),
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: "About"))"#,
        )
        .unwrap();
        std::fs::write(
            cfg.build.content_dir.join("plain.typ"),
            r#"#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: "Plain"))"#,
        )
        .unwrap();
        std::fs::write(
            &cfg.build.entry,
            r#"#import "@tola/source:0.0.0": all-sources
#let about = all-sources().find(source => source.id == "about.typ")
#let plain = all-sources().find(source => source.id == "plain.typ")
#document("index.html")[#read("value.txt") / #about.meta.title / #plain.meta.title]"#,
        )
        .unwrap();

        let mut session = BuildSession::new();
        let cold = session
            .prepare(
                std::sync::Arc::clone(&cfg),
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
            cfg.build.content_dir.join("about.typ"),
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/source:0.0.0": tola-meta
#tola-meta((title: "Changed"))"#,
        )
        .unwrap();
        let changed = cfg.build.content_dir.join("about.typ");
        let warm = session
            .prepare(
                std::sync::Arc::clone(&cfg),
                super::BuildRequest {
                    trigger: super::BuildTrigger::Paths([changed].into()),
                    reuse: super::BuildReuse {
                        content_inventory: true,
                        typst_compilation: true,
                        configured_assets: true,
                    },
                    ..super::BuildRequest::new(super::BuildMode::Development)
                },
            )
            .run()
            .unwrap();

        let plain = crate::compiler::TypstDependencyReader::ContentSource(
            tola_input::normalize_path(&cfg.build.content_dir.join("plain.typ")),
        );
        assert!(
            warm.cache_update
                .caches()
                .source_analysis
                .reused_dependency_readers()
                .contains(&plain)
        );
        let cold_again = crate::build::build_site(&cfg, super::BuildMode::Development).unwrap();

        assert_eq!(all_output_bytes(&warm), all_output_bytes(&cold_again));
        assert_eq!(warm.diagnostics(), cold_again.diagnostics());
        let warm_html =
            String::from_utf8_lossy(&all_output_bytes(&warm)["index.html"]).into_owned();
        assert!(warm_html.contains("Changed"), "{warm_html}");
        assert!(warm_html.contains("Plain"), "{warm_html}");
    }

    #[test]
    fn rebuild_publishes_updates() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfg = published_input_site(dir.path());
        let mut session = BuildSession::new();
        let first = session
            .build_and_write(&cfg, super::BuildRequest::default())
            .unwrap();
        assert_eq!(first.counts().pages, 1);
        assert_eq!(first.output_root(), cfg.build.publish_dir.as_path());
        let html_path = first.output_root().join("index.html");
        assert!(
            std::fs::read_to_string(&html_path)
                .unwrap()
                .contains("Published value")
        );

        std::fs::write(dir.path().join("value.txt"), "Second value").unwrap();
        let second = session
            .build_and_write(&cfg, super::BuildRequest::default())
            .unwrap();
        assert_eq!(second.counts().pages, 1);
        assert!(
            String::from_utf8_lossy(&std::fs::read(&html_path).unwrap()).contains("Second value")
        );
    }

    #[test]
    fn reuse_follows_hook_outputs() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("content")).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        let trigger = root.join("src/style.txt");
        fs::write(&trigger, "first").unwrap();
        fs::write(
            root.join("site.typ"),
            "#let style = read(\"generated/site.css\")\n#document(\"index.html\")[#style]",
        )
        .unwrap();
        let generated = root.join("generated");

        let mut cfg = site_config(root);
        cfg.build.publish_dir = root.join("public");
        cfg.assets.trees = vec![AssetTreeDeclaration::new(
            &generated,
            AssetUrlPrefix::parse("/assets").unwrap(),
        )];
        cfg.hooks.before_build.push(BeforeBuildHookConfig {
            name: "stylesheet".into(),
            argv: Some(hook_child_command(
                "build::session::tests::child_copies_stylesheet_trigger",
            )),
            dev: crate::config::section::build::DevParticipation::Run,
            rerun_on: tola_module::hooks::RerunOn {
                include: vec![tola_module::hooks::RerunPath::Relative(
                    "src/style.txt".into(),
                )],
                ..Default::default()
            },
            generates: vec!["generated/site.css".into()],
            ..BeforeBuildHookConfig::default()
        });

        let cfg = std::sync::Arc::new(cfg);
        let mut session = BuildSession::new();
        let first = session
            .prepare(
                cfg.clone(),
                super::BuildRequest::new(super::BuildMode::Development),
            )
            .run()
            .unwrap();
        accept_build(&mut session, first);

        // Unrelated watcher trigger; hook output comparison must still invalidate stale reuse.
        let mut rebuild = || {
            session
                .prepare(
                    cfg.clone(),
                    super::BuildRequest {
                        trigger: super::BuildTrigger::Paths(
                            [root.join("notes/unrelated.txt")].into(),
                        ),
                        reuse: super::BuildReuse {
                            content_inventory: true,
                            typst_compilation: true,
                            configured_assets: true,
                        },
                        ..super::BuildRequest::new(super::BuildMode::Development)
                    },
                )
                .run()
                .unwrap()
        };

        let unchanged = rebuild();
        assert_eq!(unchanged.hook_outputs().len(), 1);
        assert!(unchanged.hook_outputs()[0].is_current());
        assert_eq!(output_bytes(&unchanged.graph, "assets/site.css"), b"first");
        assert!(
            String::from_utf8_lossy(output_bytes(&unchanged.graph, "index.html")).contains("first")
        );

        fs::write(&trigger, "second").unwrap();
        let changed = rebuild();
        assert_eq!(output_bytes(&changed.graph, "assets/site.css"), b"second");
        assert!(
            String::from_utf8_lossy(output_bytes(&changed.graph, "index.html")).contains("second")
        );
        assert_eq!(changed.hook_outputs().len(), 1);
        assert!(changed.hook_outputs()[0].is_current());
    }
}
