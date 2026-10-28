//! The reusable baseline of one site's repeated builds: host resources shared across
//! attempts, the producer caches an accepted revision installed, and the attempt
//! preparation that reads accepted evidence.
//!
//! A cache baseline advances only through [`BuildSession::commit_cache`] after a
//! successful write or an accepted checked revision; preparing or dropping an attempt
//! advances nothing.

use crate::compiler::{
    CompilationDependencies, CompilationReuse, RebuildDecision, SiteProgramCache, TypstHost,
    analysis,
};
use crate::mode::BuildMode;

/// Shared resources and accepted caches for repeated builds of one site.
///
/// Ordinary callers use [`Self::build_and_write`]. Custom schedulers use
/// [`Self::prepare`] and [`Self::install_revision`] to advance caches only after
/// their corresponding output takes effect. The session does not own that output.
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
    /// What the before-build hooks wrote when this revision was produced.
    ///
    /// A later attempt compares these content fingerprints to decide whether a hook
    /// that ran actually changed anything, so an unchanged generator keeps its reuse.
    pub(crate) hook_outputs: crate::hooks::SourceHookOutputs,
    /// The previous successful development revision's reusable `generate-outputs` products.
    pub(crate) hook_reuse: crate::hooks::ReusableHookOutputs,
}

/// The originating session and accepted generation captured by one attempt.
///
/// Existing object identities distinguish sessions and accepted generations without
/// retaining older producer caches. Preparing or dropping an attempt advances neither.
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

    /// Attribute these caches to the session and accepted generation that produced them.
    ///
    /// Installation rejects caches attributed to another session, or to a generation
    /// that session has already replaced.
    pub(super) fn attribute_to(&mut self, session: &BuildSession) {
        self.origin = Some(BuildCacheOrigin {
            session: std::sync::Arc::downgrade(&session.host_cache),
            accepted: session.accepted.as_ref().map(std::sync::Arc::downgrade),
        });
    }

    pub(super) fn freshness(
        &self,
        config: &crate::config::ResolvedSiteConfig,
        hook_outputs: &[crate::hooks::HookOutputEvidence],
        attempt_cancellation: &crate::cancellation::BuildCancellation,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> anyhow::Result<super::InputFreshness> {
        attempt_cancellation.ensure_active()?;
        let freshness = super::input::input_freshness(
            config,
            &self.caches,
            &self.content_inventory,
            hook_outputs,
            cancellation,
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
        }
    }
}

impl Default for BuildRequest {
    fn default() -> Self {
        Self::new(BuildMode::Production)
    }
}

/// An owned build attempt suitable for a caller's blocking worker.
///
/// Attempts borrow no mutable session resources. Only a completed write or an
/// accepted revision advances the originating session's reuse baseline.
pub struct BuildAttempt {
    config: std::sync::Arc<crate::config::ResolvedSiteConfig>,
    baseline: BuildSession,
    request: BuildRequest,
}

impl BuildAttempt {
    /// Prepare resources and run the build pipeline.
    pub fn run(mut self) -> Result<super::SiteBuild, super::BuildFailure> {
        if let Err(error) = super::diagnostic::refuse_incomplete_vendor(&self.config) {
            return Err(super::BuildFailure::before_attempt(error));
        }
        let mut producers = super::pipeline::BuildAttemptProducers {
            resources: self.baseline.resources.clone(),
            cancellation: self.request.cancellation.clone(),
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
        let cancellation = producers.cancellation.clone();
        let changed_paths = self.request.trigger.paths().unwrap_or_default();
        let built = super::pipeline::build_site_inner(
            &self.config,
            self.request.mode,
            self.request.hook_execution,
            self.request.reference_checks,
            &mut producers,
            |producers, hooks_executed| -> anyhow::Result<TypstHost> {
                let host = self.baseline.load_host(&self.config, &cancellation)?;
                let accepted_matches = self
                    .baseline
                    .accepted
                    .as_ref()
                    .is_some_and(|accepted| accepted.caches.host.has_same_inputs(&host));
                // A before-build hook that wrote exactly what the previous revision already
                // held changes no input, so the event-qualified reuse stays valid. Only a
                // generator whose declared outputs actually differ withdraws it.
                let hooks_changed_outputs = hooks_executed
                    && !self
                        .baseline
                        .hook_outputs_match_accepted(&producers.source_hooks);
                producers.hook_outputs_unchanged = hooks_executed && !hooks_changed_outputs;
                let permissions = if accepted_matches
                    && !hooks_changed_outputs
                    && matches!(self.request.trigger, BuildTrigger::Paths(_))
                {
                    self.request.reuse
                } else {
                    BuildReuse::default()
                };
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
                producers.images = self
                    .baseline
                    .accepted_caches()
                    .map(|caches| std::sync::Arc::clone(&caches.images));
                producers.seo = self
                    .baseline
                    .accepted_caches()
                    .map(|caches| std::sync::Arc::clone(&caches.seo));
                producers.reference_reuse = self
                    .baseline
                    .accepted
                    .as_ref()
                    .map(|accepted| accepted.references.clone());
                producers.icons = self
                    .baseline
                    .accepted_caches()
                    .map(|caches| caches.icons.clone());
                producers.previous_bundle_entries = self.baseline.bundle_entries();
                producers.configured_assets = permissions
                    .configured_assets
                    .then(|| self.baseline.configured_assets().cloned())
                    .flatten();
                producers.configured_asset_changes = permissions
                    .configured_assets
                    .then(|| super::AcceptedFileChanges::from_watcher(changed_paths.to_vec()));
                producers.content_inventory = content_inventory;
                cancellation.ensure_active()?;
                Ok(host)
            },
        );
        built
            .map(|mut built| {
                built.cache_update.attribute_to(&self.baseline);
                built
            })
            .map_err(|error| super::BuildFailure::new(error, producers.into_inputs()))
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
        }
    }

    fn load_host(
        &self,
        config: &crate::config::ResolvedSiteConfig,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> anyhow::Result<TypstHost> {
        cancellation.ensure_active()?;
        // Host fonts cannot join a pure build; the setting is contradictory, not ignorable.
        if self.resources.input_scope() == crate::InputScope::Pure && config.fonts().system {
            anyhow::bail!(
                "`--pure` cannot use `[typst.fonts] system = true`; keep the fonts in \
                 `[typst.fonts] paths`, or run the build without `--pure`"
            );
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
        cancellation.ensure_active()?;
        self.retain_host(&host);
        Ok(host)
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
    ///
    /// A generator that rewrote identical bytes changed no input, so the reuse an
    /// event-qualified rebuild already qualified for stays valid. An accepted revision
    /// that declared no outputs at all cannot prove this, so it reports `false`.
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

    /// Return the accepted content inventory when this rebuild can reuse it.
    ///
    /// Ordinary edits share the inventory without walking the tree. Possible
    /// structural changes, hooks, or configuration changes require discovery
    /// after before-build hooks.
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
        let source_analysis = caches
            .source_analysis
            .clone()
            .prepare_for_rebuild(decision, Some(caches.site_program.entry.as_path()));
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

    /// Install a checked revision, then advance its originating cache baseline.
    ///
    /// The callback runs only after the session, accepted generation, and original
    /// cancellation match. The caller must hold its event guard through this call.
    /// No filesystem checks or hooks run here. Return `Err` only if the revision did
    /// not become current; after installation, return `Ok` and handle hook failures
    /// separately. An error or a dropped candidate leaves accepted caches unchanged.
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
        let origin = update.origin.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Tola could not reuse the previous build's work; run the command again")
        })?;
        anyhow::ensure!(
            std::ptr::eq(
                origin.session.as_ptr(),
                std::sync::Arc::as_ptr(&self.host_cache)
            ),
            "Tola could not reuse the previous build's work; run the command again"
        );
        anyhow::ensure!(
            origin.accepted.as_ref().map(std::sync::Weak::as_ptr)
                == self.accepted.as_ref().map(std::sync::Arc::as_ptr),
            "Tola could not reuse the previous build's work; run the command again"
        );
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
    use crate::config::section::build::hooks::{
        CommandOutput, DevParticipation, OutputCommandConfig,
    };
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

    /// Records how many times a generator actually ran, beside the site root.
    #[test]
    fn child_counts_generator_runs() {
        if !crate::build::tests::is_hook_child_for("generate-outputs") {
            return;
        }
        let path = std::path::Path::new("generator-runs.txt");
        let count = fs::read_to_string(path)
            .ok()
            .and_then(|runs| runs.trim().parse::<u32>().ok())
            .unwrap_or(0);
        fs::write(path, (count + 1).to_string()).unwrap();
        fs::write(
            std::path::PathBuf::from(std::env::var_os("TOLA_HOOK_OUTPUT_DIR").unwrap())
                .join("search.json"),
            b"{}",
        )
        .unwrap();
    }

    /// The accepted development revision carries generator outputs into the next attempt.
    #[test]
    fn development_reuses_unchanged_generator_outputs() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let mut config = dev_config(root);
        config.build.hooks.generate_outputs = vec![OutputCommandConfig {
            name: "search".into(),
            command: crate::build::tests::hook_child_command(
                "build::session::tests::child_counts_generator_runs",
            ),
            dev: DevParticipation::Reuse,
            outputs: vec![CommandOutput::File(
                tola_address::OutputPath::parse("search.json").unwrap(),
            )],
            ..OutputCommandConfig::default()
        }];
        let mut session = BuildSession::new();

        session
            .build_and_write(&config, BuildRequest::new(BuildMode::Development))
            .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("generator-runs.txt")).unwrap(),
            "1"
        );

        session
            .build_and_write(&config, BuildRequest::new(BuildMode::Development))
            .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("generator-runs.txt")).unwrap(),
            "1",
            "an unchanged development revision must reuse the generator output"
        );
        assert!(config.build.publish_dir.join("search.json").is_file());
    }

    #[test]
    fn child_writes_font_input() {
        if !is_hook_child_for("before-build") {
            return;
        }
        std::fs::create_dir_all("fonts").unwrap();
        std::fs::write("fonts/generated.ttf", b"observed font source").unwrap();
    }

    #[test]
    fn builds_load_fonts_after_generation() {
        let directory = tempfile::TempDir::new().unwrap();
        let mut config = dev_config(directory.path());
        config.build.hooks.before_build.push(BeforeBuildHookConfig {
            name: "font-input".into(),
            command: hook_child_command("build::session::tests::child_writes_font_input"),
            generates: vec!["fonts/generated.ttf".into()],
            ..Default::default()
        });
        for through_session in [false, true] {
            let font = config.typst.fonts.paths[0].join("generated.ttf");
            if font.exists() {
                std::fs::remove_file(&font).unwrap();
            }
            let built = if through_session {
                BuildSession::new()
                    .prepare(
                        std::sync::Arc::new(config.clone()),
                        super::BuildRequest::default(),
                    )
                    .run()
                    .unwrap()
            } else {
                crate::build::build_site(&config, super::BuildMode::Production).unwrap()
            };
            assert!(font.is_file());
            assert_eq!(built.hook_outputs().len(), 1);
            built
                .ensure_fresh(&crate::cancellation::BuildCancellation::default())
                .unwrap();
        }
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

    /// A published build replaces the bytes earlier revisions wrote.
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

    /// A build that fails keeps the tree its last successful build published.
    #[test]
    fn failed_build_keeps_published_tree() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = published_input_site(directory.path());
        let mut session = BuildSession::new();
        let first = session
            .build_and_write(&config, super::BuildRequest::default())
            .unwrap();
        let html_path = first.output_root().join("index.html");
        let published = std::fs::read(&html_path).unwrap();

        std::fs::write(&config.build.entry, "#panic(\"Cannot build\")").unwrap();
        assert!(
            session
                .build_and_write(&config, super::BuildRequest::default())
                .is_err()
        );
        assert_eq!(std::fs::read(&html_path).unwrap(), published);
    }

    #[test]
    fn failed_candidate_keeps_warnings() {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir(root.join("content")).unwrap();
        fs::write(
            root.join("site.typ"),
            r##"#document("index.html")[
#show strong: none
= Real
#context {
  let count = query(heading).len()
  count * [= Generated]
}
#link("/missing/")[Missing page]
#link("#missing")[Missing fragment]
]"##,
        )
        .unwrap();
        let mut config = site_config(root);
        config.build.references.fragments = crate::config::ReferenceLevel::Warn;
        let failure = BuildSession::new()
            .prepare(std::sync::Arc::new(config.clone()), BuildRequest::default())
            .run()
            .err()
            .expect("missing navigation must reject the official Bundle candidate");
        let diagnostics = error_diagnostics(failure.error(), config.get_root());
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "reference.fragment_missing"
                    && diagnostic.severity == crate::diagnostic::Severity::Warning
            }),
            "{diagnostics:?}"
        );
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.code.surface() == "typst"
                    && diagnostic.severity == crate::diagnostic::Severity::Warning
            }),
            "{diagnostics:?}"
        );
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "reference.navigation_missing"
                    && diagnostic.severity == crate::diagnostic::Severity::Error
            }),
            "{diagnostics:?}"
        );
        assert!(!config.build.publish_dir.exists());
    }

    /// A before-build generator's declared output decides reuse: an unchanged output keeps
    /// the reused compilation, changed bytes withdraw it.
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

    #[test]
    fn child_copies_stylesheet_trigger() {
        if !is_hook_child_for("before-build") {
            return;
        }
        fs::create_dir_all("generated").unwrap();
        fs::write(
            "generated/site.css",
            fs::read_to_string("src/style.txt").unwrap(),
        )
        .unwrap();
    }
}
