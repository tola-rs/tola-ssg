//! The reusable baseline of one site's repeated builds: host resources shared across
//! attempts, the producer caches an accepted revision installed, and the attempt
//! preparation that reads accepted evidence.
//!
//! A cache baseline advances only through [`BuildSession::commit_cache`] after a
//! successful write or an accepted checked revision; preparing or dropping an attempt
//! advances nothing.

use crate::compiler::{
    CompilationReuse, PublishedDependencies, RebuildDecision, SiteProgramCache, TypstHost, analysis,
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
    pub(crate) dependencies: PublishedDependencies,
    pub(crate) source_analysis: analysis::SourceAnalysisCache,
    pub(crate) site_program: SiteProgramCache,
    pub(crate) configured_assets: crate::asset::ConfiguredAssetInventory,
    pub(crate) icons: crate::icon::IconSnapshot,
    pub(crate) images: std::sync::Arc<crate::image::output::ImageOutputs>,
    pub(crate) seo: std::sync::Arc<crate::seo::SeoCompilation>,
    /// What the before-build hooks wrote when this revision was produced.
    ///
    /// A later attempt compares these content fingerprints to decide whether a hook
    /// that ran actually changed anything, so an unchanged generator keeps its reuse.
    pub(crate) hook_outputs: crate::hooks::SourceHookOutputs,
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
            ..super::pipeline::BuildAttemptProducers::default()
        };
        let cancellation = producers.cancellation.clone();
        let changed_paths = self.request.trigger.paths().unwrap_or_default();
        let built = super::pipeline::build_site_inner(
            &self.config,
            self.request.mode,
            self.request.hook_execution,
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
    use crate::config::section::{
        AssetFileDeclaration, AssetTreeDeclaration, AssetUrl, AssetUrlPrefix,
    };
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

    #[test]
    fn vendor_replacement_requires_committed_workspace() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let mut config = dev_config(root);
        config.vendor.path = Some(root.join("vendor"));
        std::fs::create_dir_all(root.join("vendor")).unwrap();
        let workspace = root.join(".vendor-vendor");
        std::fs::create_dir(&workspace).unwrap();
        // `tola vendor` writes the journal before its first rename and creates `committed`
        // after its last, so a journal without `committed` is a replacement that stopped.
        std::fs::write(workspace.join("replacing"), b"000").unwrap();

        let Err(failure) =
            BuildSession::with_resources(crate::resources::BuildResources::default())
                .prepare(
                    std::sync::Arc::new(config.clone()),
                    BuildRequest::new(BuildMode::Production),
                )
                .run()
        else {
            panic!("a build read a vendor replacement that never committed")
        };
        let error = failure.into_error();
        let diagnostics = crate::diagnostic::attached(&error).expect("an attached diagnostic");
        assert_eq!(diagnostics[0].code, crate::codes::vendor::INCOMPLETE);
        assert!(
            diagnostics[0]
                .message
                .contains("stopped before it finished")
        );
        assert_eq!(diagnostics[0].notes, ["vendor"]);

        // A committed replacement and an absent workspace both leave the build alone.
        std::fs::create_dir(workspace.join("committed")).unwrap();
        assert!(
            BuildSession::with_resources(crate::resources::BuildResources::default())
                .prepare(
                    std::sync::Arc::new(config.clone()),
                    BuildRequest::new(BuildMode::Production)
                )
                .run()
                .is_ok()
        );
        std::fs::remove_dir_all(&workspace).unwrap();
        assert!(
            BuildSession::with_resources(crate::resources::BuildResources::default())
                .prepare(
                    std::sync::Arc::new(config),
                    BuildRequest::new(BuildMode::Production)
                )
                .run()
                .is_ok()
        );
    }

    #[test]
    fn converged_bundle_ignores_failed_branch() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let mut config = dev_config(root);
        let package_data = root.join("package-data");
        config.package_locations =
            tola_typst::PackageLocations::from_absolute_roots(Some(package_data.clone()), None)
                .unwrap();
        std::fs::write(
            config.build.content_dir.join("alpha.typ"),
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((ready: true))",
        )
        .unwrap();
        std::fs::write(
            config.build.content_dir.join("beta.typ"),
            r#"
#import "@tola/source:0.0.0": all-sources
#let alpha = all-sources().find(source => source.id == "alpha.typ")
#if alpha.meta == none {
  import "@local/tola-review-transient-missing:1.0.0": value
}
#import "@tola/source:0.0.0": tola-meta
#tola-meta((ready: alpha.meta != none))
"#,
        )
        .unwrap();
        std::fs::write(
            &config.build.entry,
            r#"
#import "@tola/source:0.0.0": all-sources
#document("index.html")[#all-sources().find(source => source.id == "beta.typ").meta.ready]
"#,
        )
        .unwrap();
        let resources = crate::resources::BuildResources::new()
            .with_network_access(crate::resources::NetworkAccess::Denied);
        let mut session = BuildSession::with_resources(resources);
        let built = session
            .prepare(std::sync::Arc::new(config), super::BuildRequest::default())
            .run()
            .unwrap();
        let html = built
            .graph()
            .outputs()
            .iter()
            .find(|output| output.path().as_str() == "index.html")
            .unwrap();
        assert!(String::from_utf8_lossy(html.bytes()).contains("true"));
        std::fs::create_dir_all(package_data.join("local/tola-review-transient-missing/1.0.0"))
            .unwrap();
        built
            .ensure_fresh(&crate::cancellation::BuildCancellation::default())
            .unwrap();
        let checked = check_fresh(built.into_unchecked_revision(None));
        session.install_revision(checked, |_| Ok(())).unwrap();
    }

    fn loaded_host(
        session: &mut BuildSession,
        config: &crate::config::ResolvedSiteConfig,
    ) -> crate::compiler::TypstHost {
        session
            .prepare(
                std::sync::Arc::new(config.clone()),
                super::BuildRequest::new(super::BuildMode::Production),
            )
            .run()
            .unwrap()
            .cache_update
            .caches
            .host
    }

    #[test]
    fn writes_font_input() {
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
            command: hook_child_command("build::session::tests::writes_font_input"),
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

    #[test]
    fn distinct_site_roots_never_share_hosts() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let nested = root.join("nested");
        std::fs::create_dir_all(nested.join("content")).unwrap();
        std::fs::create_dir_all(nested.join("fonts")).unwrap();
        std::fs::create_dir_all(root.join("vendor/typst-packages")).unwrap();
        std::fs::create_dir_all(nested.join("vendor/typst-packages")).unwrap();
        std::fs::write(root.join("value.txt"), "root-a-value").unwrap();
        std::fs::write(nested.join("value.txt"), "root-b-value").unwrap();
        std::fs::write(
            nested.join("site.typ"),
            "#document(\"index.html\")[#read(\"/value.txt\")]",
        )
        .unwrap();
        let config = |root: &Path, prefix: &str| {
            let mut schema = crate::config::SiteConfigSchema::default();
            schema.build.entry = Path::new(prefix).join("site.typ");
            schema.build.content_dir = Path::new(prefix).join("content");
            schema.typst.fonts.paths = vec![Path::new(prefix).join("fonts")];
            schema.vendor.path = Some(PathBuf::from("vendor"));
            std::sync::Arc::new(
                schema
                    .resolve(
                        &root.join("tola.toml"),
                        tola_typst::PackageLocations::from_absolute_roots(None, None).unwrap(),
                        &crate::config::loading::BuildOverrides::default(),
                    )
                    .unwrap(),
            )
        };
        let mut session = BuildSession::new();
        let build = session
            .prepare(config(root, "nested"), super::BuildRequest::default())
            .run()
            .unwrap();
        accept_build(&mut session, build);
        let host = session.accepted.as_ref().unwrap().caches.host.clone();
        let request = super::BuildRequest {
            trigger: super::BuildTrigger::Paths(Default::default()),
            reuse: super::BuildReuse {
                content_inventory: true,
                typst_compilation: true,
                configured_assets: true,
            },
            ..Default::default()
        };
        let second = session.prepare(config(&nested, ""), request).run().unwrap();

        assert!(!second.cache_update.caches.host.has_same_inputs(&host));
        second
            .ensure_fresh(&crate::cancellation::BuildCancellation::default())
            .unwrap();
        let html = second
            .graph()
            .outputs()
            .iter()
            .find(|output| output.path().as_str() == "index.html")
            .unwrap();
        assert!(String::from_utf8_lossy(html.bytes()).contains("root-b-value"));
        assert!(!String::from_utf8_lossy(html.bytes()).contains("root-a-value"));
    }

    #[test]
    fn generated_files_are_not_retained() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = dev_config(directory.path());
        let mut request = super::BuildRequest::default();
        request.generated_files.push(
            crate::output::GeneratedFile::new(
                "search-index",
                tola_address::OutputPath::parse("provided.txt").unwrap(),
                std::sync::Arc::<[u8]>::from(b"provided bytes".as_slice()),
            )
            .unwrap(),
        );
        let mut session = BuildSession::new();
        session.build_and_write(&config, request).unwrap();
        assert_eq!(
            std::fs::read(config.build.publish_dir.join("provided.txt")).unwrap(),
            b"provided bytes"
        );
        session
            .build_and_write(&config, super::BuildRequest::default())
            .unwrap();
        assert!(!config.build.publish_dir.join("provided.txt").exists());
    }

    #[test]
    fn non_font_changes_keep_loaded_fonts() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = dev_config(directory.path());
        let mut session = BuildSession::new();
        let loaded = loaded_host(&mut session, &config);

        std::fs::write(config.build.content_dir.join("post.typ"), "Changed content").unwrap();
        let content_edit = loaded_host(&mut session, &config);
        assert!(content_edit.has_same_inputs(&loaded));

        std::fs::write(
            config.typst.fonts.paths[0].join("LICENSE"),
            "License update",
        )
        .unwrap();
        let font_directory_edit = loaded_host(&mut session, &config);
        assert!(font_directory_edit.has_same_inputs(&loaded));
        assert!(
            font_directory_edit
                .font_inventory_is_fresh(&crate::cancellation::BuildCancellation::default())
                .unwrap()
        );
    }

    /// One font input change, and what its reload must show.
    struct FontChangeCase {
        name: &'static str,
        /// Change the font inputs before the first host load.
        prepare: fn(&Path),
        /// Change the font inputs after the first host load.
        change: fn(&Path, &mut crate::config::ResolvedSiteConfig),
        /// Whether the loaded inventory must already report this change.
        stale_before_reload: bool,
    }

    /// Every font input change reloads the host, whose inventory is then fresh.
    #[test]
    fn font_input_changes_reload_the_host() {
        for case in [
            FontChangeCase {
                name: "font added",
                prepare: |_| {},
                change: |root, _| {
                    fs::write(root.join("fonts/site.ttf"), b"first font bytes").unwrap()
                },
                stale_before_reload: false,
            },
            FontChangeCase {
                name: "font bytes changed",
                prepare: |root| {
                    fs::write(root.join("fonts/site.ttf"), b"first font bytes").unwrap()
                },
                change: |root, _| {
                    fs::write(root.join("fonts/site.ttf"), b"changed font bytes").unwrap()
                },
                stale_before_reload: true,
            },
            FontChangeCase {
                name: "font removed",
                prepare: |root| {
                    fs::write(root.join("fonts/site.ttf"), b"first font bytes").unwrap()
                },
                change: |root, _| fs::remove_file(root.join("fonts/site.ttf")).unwrap(),
                stale_before_reload: false,
            },
            FontChangeCase {
                name: "font paths changed",
                prepare: |_| {},
                change: |root, config| {
                    config.typst.fonts.paths = vec![root.join("other-fonts")];
                    fs::create_dir_all(&config.typst.fonts.paths[0]).unwrap();
                },
                stale_before_reload: false,
            },
        ] {
            assert_font_change_reloads(case);
        }
    }

    /// A font input change reached through a symlink reloads the host the same way.
    #[cfg(unix)]
    #[test]
    fn symlinked_font_input_changes_reload_the_host() {
        for case in [
            FontChangeCase {
                name: "font symlink retargeted",
                prepare: |root| {
                    use std::os::unix::fs::symlink;
                    fs::create_dir_all(root.join("first-fonts")).unwrap();
                    fs::create_dir_all(root.join("second-fonts")).unwrap();
                    fs::write(root.join("first-fonts/site.ttf"), b"first font bytes").unwrap();
                    fs::write(root.join("second-fonts/site.ttf"), b"changed font bytes").unwrap();
                    symlink(root.join("first-fonts"), root.join("fonts/shared")).unwrap();
                },
                change: |root, _| {
                    use std::os::unix::fs::symlink;
                    fs::remove_file(root.join("fonts/shared")).unwrap();
                    symlink(root.join("second-fonts"), root.join("fonts/shared")).unwrap();
                },
                stale_before_reload: false,
            },
            FontChangeCase {
                name: "font bytes changed through a symlink",
                prepare: |root| {
                    use std::os::unix::fs::symlink;
                    fs::create_dir_all(root.join("first-fonts")).unwrap();
                    fs::write(root.join("first-fonts/site.ttf"), b"first font bytes").unwrap();
                    symlink(root.join("first-fonts"), root.join("fonts/shared")).unwrap();
                },
                change: |root, _| {
                    fs::write(root.join("first-fonts/site.ttf"), b"changed font bytes").unwrap()
                },
                stale_before_reload: false,
            },
        ] {
            assert_font_change_reloads(case);
        }
    }

    /// Run one font input change through a session and assert the reload contract.
    fn assert_font_change_reloads(case: FontChangeCase) {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        let mut config = dev_config(root);
        (case.prepare)(root);
        let mut session = BuildSession::new();
        let initial = loaded_host(&mut session, &config);

        (case.change)(root, &mut config);
        if case.stale_before_reload {
            assert!(
                !initial
                    .font_inventory_is_fresh(&crate::cancellation::BuildCancellation::default())
                    .unwrap(),
                "{}: the loaded inventory missed the change",
                case.name
            );
        }
        let changed = loaded_host(&mut session, &config);
        assert!(!changed.has_same_inputs(&initial), "{}", case.name);
        assert!(
            changed
                .font_inventory_is_fresh(&crate::cancellation::BuildCancellation::default())
                .unwrap(),
            "{}",
            case.name
        );
        assert!(
            changed.matches(&config, &session.resources),
            "{}",
            case.name
        );
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
    fn independent_builds_publish_the_same() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let mut config = dev_config(root);
        std::fs::write(root.join("value.txt"), "Published value").unwrap();
        config.build.references.navigation = crate::config::ReferenceLevel::Warn;
        std::fs::write(
            &config.build.entry,
            r#"#document("index.html")[#read("value.txt") #link("/nowhere-b.html")[B]]
#document("second.html")[#link("/nowhere-a.html")[A]]"#,
        )
        .unwrap();
        let config = std::sync::Arc::new(config);

        let build = || {
            BuildSession::new()
                .prepare(
                    std::sync::Arc::clone(&config),
                    super::BuildRequest::new(super::BuildMode::Development),
                )
                .run()
                .unwrap()
        };
        let located = |build: &SiteBuild| {
            build
                .diagnostics()
                .iter()
                .map(|diagnostic| {
                    (
                        diagnostic.code.as_str(),
                        diagnostic
                            .location
                            .as_ref()
                            .map(|location| (location.path.clone(), location.line)),
                    )
                })
                .collect::<Vec<_>>()
        };

        let first = build();
        let second = build();

        assert_eq!(all_output_bytes(&first), all_output_bytes(&second));
        let reported = located(&first);
        let navigation_warnings = reported
            .iter()
            .filter(|(code, _)| *code == crate::codes::reference::NAVIGATION_MISSING.as_str())
            .count();
        assert_eq!(navigation_warnings, 2, "{reported:?}");
        assert_eq!(reported, located(&second));
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

        let reused = warm
            .cache_update
            .caches
            .source_analysis
            .reused_dependency_readers();
        assert!(
            reused.iter().any(|reader| matches!(reader, crate::compiler::TypstDependencyReader::ContentSource(path)
                    if path.ends_with("plain.typ"))),
            "the untouched source was not reused: {reused:?}"
        );
        // A change must reach every computation that depends on it, or a warm build
        // would publish a stale value that a cold build of the same tree would not.
        assert!(
            !reused
                .iter()
                .any(|reader| matches!(reader, crate::compiler::TypstDependencyReader::ContentSource(path)
                    if path.ends_with("about.typ"))),
            "the edited source must not be reused: {reused:?}"
        );

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
    fn session_advances_only_on_install() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = published_input_site(directory.path());
        let value = directory.path().join("value.txt").canonicalize().unwrap();
        let mut session = BuildSession::new();
        let attempt = session.prepare(
            std::sync::Arc::clone(&config),
            super::BuildRequest::new(super::BuildMode::Development),
        );
        let candidate = std::thread::spawn(move || attempt.run())
            .join()
            .unwrap()
            .unwrap();
        assert!(
            candidate
                .input_observation()
                .physical_read_paths()
                .contains(&value)
        );
        assert!(session.accepted.is_none());
        assert!(
            candidate
                .freshness(&crate::cancellation::BuildCancellation::default())
                .unwrap()
                .all_inputs_are_fresh()
        );
        let unchecked = candidate.into_unchecked_revision(None);
        assert!(!unchecked.site().outputs().outputs().is_empty());
        let checked = check_fresh(unchecked);
        let revision = session
            .install_revision(checked, Ok)
            .expect("a fresh candidate prepared by this session must install");
        assert!(
            revision
                .input_observation()
                .physical_read_paths()
                .contains(&value)
        );
        assert!(
            session
                .accepted
                .as_ref()
                .unwrap()
                .caches
                .dependencies
                .physical_read_paths()
                .contains(&value)
        );
    }

    #[test]
    fn dropped_attempt_keeps_accepted_inputs() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = published_input_site(directory.path());
        let published_value = directory.path().join("value.txt").canonicalize().unwrap();
        let candidate_value = directory.path().join("candidate.txt");
        std::fs::write(&candidate_value, "Unpublished value").unwrap();
        let mut session = BuildSession::new();
        let first = session
            .prepare(
                std::sync::Arc::clone(&config),
                super::BuildRequest::new(super::BuildMode::Production),
            )
            .run()
            .unwrap();
        accept_build(&mut session, first);
        std::fs::write(
            &config.build.entry,
            r#"#document("index.html")[#read("candidate.txt")]"#,
        )
        .unwrap();
        let candidate = session
            .prepare(
                std::sync::Arc::clone(&config),
                super::BuildRequest::new(super::BuildMode::Production),
            )
            .run()
            .unwrap();
        assert!(
            candidate
                .input_observation()
                .physical_read_paths()
                .contains(&candidate_value.canonicalize().unwrap())
        );
        drop(candidate.into_unchecked_revision(None));
        let accepted_reads = session
            .accepted
            .as_ref()
            .unwrap()
            .caches
            .dependencies
            .physical_read_paths();
        assert!(accepted_reads.contains(&published_value));
        assert!(!accepted_reads.contains(&candidate_value.canonicalize().unwrap()));
    }

    #[test]
    fn failed_attempt_keeps_accepted_baseline() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = published_input_site(directory.path());
        let value = directory.path().join("value.txt").canonicalize().unwrap();
        let mut session = BuildSession::new();
        let first = session
            .prepare(
                std::sync::Arc::clone(&config),
                super::BuildRequest::new(super::BuildMode::Development),
            )
            .run()
            .unwrap();
        accept_build(&mut session, first);
        let mut candidate_config = config.as_ref().clone();
        candidate_config.typst.fonts.paths = vec![directory.path().join("candidate-fonts")];
        std::fs::create_dir_all(&candidate_config.typst.fonts.paths[0]).unwrap();
        std::fs::write(&candidate_config.build.entry, "#panic(\"Cannot publish\")").unwrap();
        let failure = session
            .prepare(
                std::sync::Arc::new(candidate_config),
                super::BuildRequest::new(super::BuildMode::Development),
            )
            .run()
            .err()
            .unwrap();
        assert!(!failure.is_cancelled());
        assert!(
            session
                .accepted
                .as_ref()
                .unwrap()
                .caches
                .dependencies
                .physical_read_paths()
                .contains(&value)
        );
        assert!(
            session
                .accepted
                .as_ref()
                .unwrap()
                .caches
                .host
                .matches(&config, &session.resources)
        );
        assert!(
            !session
                .host_cache
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .matches(&config, &session.resources)
        );
    }

    #[test]
    fn failed_attempt_exposes_missing_reads() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = published_input_site(directory.path());
        std::fs::write(
            &config.build.entry,
            r#"#document("index.html")[#read("missing.txt")]"#,
        )
        .unwrap();
        let mut session = BuildSession::new();
        let failure = session
            .prepare(
                std::sync::Arc::clone(&config),
                super::BuildRequest::new(super::BuildMode::Development),
            )
            .run()
            .err()
            .unwrap();
        assert!(!failure.is_cancelled());
        assert!(
            failure
                .input_observation()
                .physical_read_paths()
                .iter()
                .any(|path| path.ends_with("missing.txt"))
        );
        assert!(session.accepted.is_none());
    }

    #[test]
    fn failed_write_keeps_previous_tree() {
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
        let written = std::fs::read(&html_path).unwrap();
        assert!(String::from_utf8_lossy(&written).contains("Second value"));
        let accepted = std::sync::Arc::clone(
            &session
                .accepted
                .as_ref()
                .unwrap()
                .caches
                .site_program
                .compilation,
        );

        std::fs::write(&config.build.entry, "#panic(\"Cannot build\")").unwrap();
        assert!(
            session
                .build_and_write(&config, super::BuildRequest::default())
                .is_err()
        );
        assert_eq!(std::fs::read(&html_path).unwrap(), written);
        assert!(std::sync::Arc::ptr_eq(
            &accepted,
            &session
                .accepted
                .as_ref()
                .unwrap()
                .caches
                .site_program
                .compilation
        ));
    }

    #[test]
    fn cancelled_attempt_yields_no_revision() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = published_input_site(directory.path());
        let mut session = BuildSession::new();
        let canceller = crate::cancellation::BuildCanceller::new();
        let mut request = super::BuildRequest::new(super::BuildMode::Development);
        request.cancellation = canceller.token();
        let candidate = session
            .prepare(config, request)
            .run()
            .unwrap()
            .into_unchecked_revision(None);
        let checked = check_fresh(candidate);
        canceller.cancel();
        let mut installed = false;
        let error = session
            .install_revision(checked, |_| {
                installed = true;
                Ok(())
            })
            .expect_err("the original attempt cancellation must prevent handoff");
        assert!(crate::cancellation::is_cancelled(&error));
        assert!(!installed, "a rejected candidate must not become current");
        assert!(session.accepted.is_none());
    }

    #[test]
    fn stale_check_discards_the_candidate() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = published_input_site(directory.path());
        let mut session = BuildSession::new();
        let built = session
            .prepare(
                config,
                super::BuildRequest::new(super::BuildMode::Production),
            )
            .run()
            .unwrap();
        let candidate = built.into_unchecked_revision(None);
        std::fs::write(directory.path().join("value.txt"), "Changed before check").unwrap();
        let checked = candidate
            .check(&crate::cancellation::BuildCancellation::default())
            .unwrap();
        match checked {
            crate::build::RevisionCheck::Stale(freshness) => {
                assert!(freshness.is_stale(crate::build::InputKind::TypstPhysicalReads));
            }
            crate::build::RevisionCheck::Fresh(_) => {
                panic!("stale inputs cannot yield a checked revision")
            }
        }
        assert!(session.accepted.is_none());
    }

    #[test]
    fn failed_attempts_share_loaded_fonts() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = std::sync::Arc::new(dev_config(directory.path()));
        std::fs::write(&config.build.entry, "#panic(\"source failure\")").unwrap();
        let mut session = BuildSession::new();
        let attempt = session.prepare(
            std::sync::Arc::clone(&config),
            super::BuildRequest::new(super::BuildMode::Production),
        );
        assert!(session.host_cache.lock().unwrap().is_none());
        assert!(attempt.run().is_err());
        let first = session.host_cache.lock().unwrap().clone().unwrap();
        assert!(session.accepted.is_none());
        assert!(
            session
                .prepare(
                    config,
                    super::BuildRequest::new(super::BuildMode::Production),
                )
                .run()
                .is_err()
        );
        let second = session.host_cache.lock().unwrap().clone().unwrap();
        assert!(first.has_same_inputs(&second));
        assert!(session.accepted.is_none());
    }

    #[test]
    fn cancelled_preparation_loads_nothing() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = std::sync::Arc::new(dev_config(directory.path()));
        let mut session = BuildSession::new();
        let canceller = crate::cancellation::BuildCanceller::new();
        let mut request = super::BuildRequest::new(super::BuildMode::Production);
        request.cancellation = canceller.token();
        canceller.cancel();
        let failure = session.prepare(config, request).run().err().unwrap();
        assert!(failure.is_cancelled());
        assert!(session.host_cache.lock().unwrap().is_none());
    }

    #[test]
    fn content_root_change_rediscovers() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = dev_config(directory.path());
        let nested = config.build.content_dir.join("posts");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(config.build.content_dir.join("outside.typ"), "Outside").unwrap();
        std::fs::write(nested.join("inside.typ"), "Inside").unwrap();
        let mut session = BuildSession::new();
        let first = session
            .prepare(
                std::sync::Arc::new(config.clone()),
                super::BuildRequest::new(super::BuildMode::Production),
            )
            .run()
            .unwrap();
        assert_eq!(first.cache_update.content_inventory.len(), 2);
        accept_build(&mut session, first);
        let mut narrowed = config.clone();
        narrowed.build.content_dir = nested.clone();
        let mut request = super::BuildRequest::new(super::BuildMode::Development);
        request.trigger = super::BuildTrigger::Paths(vec![config.config_path.clone()].into());
        request.reuse = super::BuildReuse {
            content_inventory: true,
            typst_compilation: true,
            configured_assets: true,
        };
        let current = session
            .prepare(std::sync::Arc::new(narrowed), request)
            .run()
            .unwrap();
        assert_eq!(current.cache_update.content_inventory.len(), 1);
        assert_eq!(
            current.cache_update.content_inventory[0].source,
            nested.join("inside.typ").canonicalize().unwrap()
        );
        assert_eq!(
            current.cache_update.content_inventory[0].root,
            nested.canonicalize().unwrap()
        );
        assert!(
            current
                .freshness(&crate::cancellation::BuildCancellation::default())
                .unwrap()
                .all_inputs_are_fresh()
        );
    }

    #[test]
    fn entry_change_rediscovers_sources() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path();
        let config =
            crate::config::tests::load_test_config(root, "[build]\nentry = \"content/site.typ\"");
        std::fs::create_dir_all(&config.build.content_dir).unwrap();
        let next_entry = config.build.content_dir.join("next.typ");
        std::fs::write(&next_entry, "Next source").unwrap();
        let program = r#"#import "@tola/source:0.0.0": all-sources
#document("index.html")[#all-sources().map(source => source.id).join(", ")]"#;
        std::fs::write(&config.build.entry, program).unwrap();
        let mut session = BuildSession::new();
        let first = session
            .prepare(
                std::sync::Arc::new(config.clone()),
                super::BuildRequest::new(super::BuildMode::Development),
            )
            .run()
            .unwrap();
        let first_html =
            String::from_utf8_lossy(&all_output_bytes(&first)["index.html"]).into_owned();
        assert!(first_html.contains("next.typ"), "{first_html}");
        accept_build(&mut session, first);

        std::fs::write(&config.build.entry, "Previous entry source").unwrap();
        std::fs::write(&next_entry, program).unwrap();
        let config =
            crate::config::tests::load_test_config(root, "[build]\nentry = \"content/next.typ\"");
        let mut request = super::BuildRequest::new(super::BuildMode::Development);
        request.trigger = super::BuildTrigger::Paths(Vec::new().into());
        request.reuse = super::BuildReuse {
            content_inventory: true,
            typst_compilation: true,
            configured_assets: true,
        };
        let current = session
            .prepare(std::sync::Arc::new(config), request)
            .run()
            .unwrap();
        let current_html =
            String::from_utf8_lossy(&all_output_bytes(&current)["index.html"]).into_owned();
        assert!(current_html.contains("site.typ"), "{current_html}");
        assert_eq!(current.cache_update.content_inventory.len(), 1);
        let cancellation = crate::cancellation::BuildCancellation::default();
        current.ensure_fresh(&cancellation).unwrap();
        std::fs::write(&next_entry, "#document(\"index.html\")[Changed entry]").unwrap();
        assert!(
            !current
                .freshness(&cancellation)
                .unwrap()
                .all_inputs_are_fresh()
        );
    }

    #[test]
    fn empty_inventory_keeps_root_identity() {
        let directory = tempfile::TempDir::new().unwrap();
        let config = dev_config(directory.path());
        let nested = config.build.content_dir.join("empty");
        std::fs::create_dir_all(&nested).unwrap();
        let mut session = BuildSession::new();
        let first = session
            .prepare(
                std::sync::Arc::new(config.clone()),
                super::BuildRequest::new(super::BuildMode::Production),
            )
            .run()
            .unwrap();
        assert!(first.cache_update.content_inventory.is_empty());
        accept_build(&mut session, first);
        assert!(
            session
                .reusable_content_inventory(&config, &[], true)
                .is_some()
        );
        let mut changed = config;
        changed.build.content_dir = nested;
        assert!(
            session
                .reusable_content_inventory(&changed, &[], true)
                .is_none()
        );
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

    #[test]
    fn reuse_rebuilds_configured_assets() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let assets = root.join("assets");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&assets).unwrap();
        fs::write(assets.join("status.txt"), "first").unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            "#document(\"index.html\")[Hello]\n#asset(\"raw.bin\", bytes(\"raw\"))",
        )
        .unwrap();

        let mut config = site_config(root);
        config.assets.trees = vec![AssetTreeDeclaration::new(
            &assets,
            AssetUrlPrefix::parse("/assets").unwrap(),
        )];

        let mut session = crate::build::BuildSession::new();
        let host = compiler_host(&config).unwrap();
        let first = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers::default(),
        )
        .unwrap();
        let SiteBuild {
            graph: first_graph, ..
        } = &first;
        let first_html = output_bytes(first_graph, "index.html").to_vec();
        install_build(&mut session, first);

        let changed = assets.join("status.txt");
        fs::write(&changed, "second").unwrap();
        let decision = session.rebuild_decision(std::slice::from_ref(&changed), false);
        let reuse = session.reusable_compilation(&decision, true).unwrap();
        assert!(matches!(
            &reuse,
            crate::compiler::CompilationReuse::SiteProgram { .. }
        ));
        let second = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers {
                reuse: Some(reuse),
                ..BuildAttemptProducers::default()
            },
        )
        .unwrap();
        let SiteBuild {
            graph: second_graph,
            cache_update,
            ..
        } = &second;

        assert_eq!(output_bytes(second_graph, "index.html"), first_html);
        assert_eq!(output_bytes(second_graph, "assets/status.txt"), b"second");

        let second_html = output_bytes(second_graph, "index.html").to_vec();
        let second_bundle_entries = cache_update.caches().site_program.bundle_entries.clone();
        let second_compilation =
            std::sync::Arc::clone(&cache_update.caches().site_program.compilation);
        install_build(&mut session, second);
        let decision = session.rebuild_decision(&[], false);
        let reuse = session.reusable_compilation(&decision, true).unwrap();
        let previous_bundle_entries = session.bundle_entries();
        let mut changed_config = config.clone();
        changed_config.build.minify.html = !config.build.minify.html;
        let third = build_site_with_host(
            &changed_config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers {
                reuse: Some(reuse),
                previous_bundle_entries,
                ..BuildAttemptProducers::default()
            },
        )
        .unwrap();
        assert!(std::sync::Arc::ptr_eq(
            &second_compilation,
            &third.cache_update.caches().site_program.compilation
        ));
        assert_ne!(output_bytes(&third.graph, "index.html"), second_html);
        assert_eq!(
            third.cache_update.caches().site_program.pretty_html,
            !changed_config.build.minify.html
        );
        let second_asset = bundle_entry(&second_bundle_entries, "/raw.bin");
        let third_asset = bundle_entry(
            &third.cache_update.caches().site_program.bundle_entries,
            "/raw.bin",
        );
        assert!(shares_backing(
            second_asset.bytes().as_slice(),
            third_asset.bytes().as_slice()
        ));
        assert_eq!(second_asset.digest(), third_asset.digest());
    }

    #[test]
    fn minify_changes_refresh_inline_styles() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("content")).unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            r#"#document("index.html")[#html.style(".a { color: red; }")Body]"#,
        )
        .unwrap();

        let mut config = site_config(root);
        config.build.minify.css = false;

        let mut session = crate::build::BuildSession::new();
        let host = compiler_host(&config).unwrap();
        let first = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers::default(),
        )
        .unwrap();
        let published = |build: &SiteBuild| {
            String::from_utf8(output_bytes(&build.graph, "index.html").to_vec()).unwrap()
        };
        assert!(published(&first).contains(".a { color: red; }"));
        install_build(&mut session, first);

        let mut minified = config.clone();
        minified.build.minify.css = true;
        let decision = session.rebuild_decision(&[], false);
        let reuse = session.reusable_compilation(&decision, true).unwrap();
        let second = build_site_with_host(
            &minified,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers {
                reuse: Some(reuse),
                ..BuildAttemptProducers::default()
            },
        )
        .unwrap();
        assert!(published(&second).contains(".a{color:red}"));
        install_build(&mut session, second);

        let decision = session.rebuild_decision(&[], false);
        let reuse = session.reusable_compilation(&decision, true).unwrap();
        let third = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers {
                reuse: Some(reuse),
                ..BuildAttemptProducers::default()
            },
        )
        .unwrap();
        assert!(published(&third).contains(".a { color: red; }"));
    }

    /// Changing the configured-asset declaration set rebuilds only asset outputs: the
    /// documents stay byte-identical and each declaration's output appears or disappears.
    #[test]
    fn configured_asset_changes_leave_documents_intact() {
        let cases = [
            (
                "added",
                &[("app.css", "/app.css")][..],
                &[("app.css", "/app.css"), ("unread.css", "/unread.css")][..],
            ),
            ("removed", &[("site.css", "/site.css")][..], &[][..]),
        ];
        for (name, first, second) in cases {
            let directory = TempDir::new().unwrap();
            let root = directory.path();
            fs::create_dir_all(root.join("content")).unwrap();
            let entry = root.join("site.typ");
            fs::write(&entry, "#document(\"index.html\")[Static]").unwrap();
            for (file, _) in first.iter().chain(second.iter()) {
                fs::write(root.join(file), format!("{file} bytes")).unwrap();
            }
            let declare = |files: &[(&str, &str)]| {
                files
                    .iter()
                    .map(|(file, url)| {
                        AssetFileDeclaration::new(root.join(file), AssetUrl::parse(url).unwrap())
                    })
                    .collect::<Vec<_>>()
            };
            let mut config = site_config(root);
            config.assets.files = declare(first);

            let mut session = crate::build::BuildSession::new();
            let host = compiler_host(&config).unwrap();
            let first_build = build_site_with_host(
                &config,
                &host,
                BuildMode::Production,
                &mut BuildAttemptProducers::default(),
            )
            .unwrap();
            let first_revision =
                crate::output::revision::OutputRevision::from_graph(first_build.graph());
            let first_document = output_bytes(&first_build.graph, "index.html").to_vec();
            let first_bytes = first
                .iter()
                .map(|(_, url)| {
                    let published = url.trim_start_matches('/');
                    (
                        published.to_owned(),
                        output_bytes(&first_build.graph, published).to_vec(),
                    )
                })
                .collect::<Vec<_>>();
            install_build(&mut session, first_build);

            let mut changed = config;
            changed.assets.files = declare(second);
            let decision = session.rebuild_decision(&[], false);
            let reuse = session.reusable_compilation(&decision, true).unwrap();
            let second_build = build_site_with_host(
                &changed,
                &compiler_host(&changed).unwrap(),
                BuildMode::Production,
                &mut BuildAttemptProducers {
                    reuse: Some(reuse),
                    previous_bundle_entries: session.bundle_entries(),
                    configured_assets: session.configured_assets().cloned(),
                    configured_asset_changes: Some(AcceptedFileChanges::from_watcher(Vec::new())),
                    ..BuildAttemptProducers::default()
                },
            )
            .unwrap();

            assert_eq!(
                output_bytes(&second_build.graph, "index.html"),
                first_document,
                "{name}: the document changed"
            );
            for (file, url) in first {
                let published = url.trim_start_matches('/');
                match second.iter().find(|(_, second_url)| second_url == url) {
                    None => assert!(
                        !has_output(&second_build.graph, published),
                        "{name}: {file}"
                    ),
                    Some(_) => {
                        let (_, bytes) = first_bytes
                            .iter()
                            .find(|(path, _)| path.as_str() == published)
                            .unwrap();
                        assert_eq!(
                            output_bytes(&second_build.graph, published),
                            bytes.as_slice(),
                            "{name}: {file} changed bytes"
                        );
                    }
                }
            }
            for (_, url) in second {
                assert!(
                    has_output(&second_build.graph, url.trim_start_matches('/')),
                    "{name}: {url} missing"
                );
            }
            let second_revision = crate::output::revision::OutputRevision::from_graph_reusing(
                &first_revision,
                &second_build.graph,
            );
            assert_ne!(
                first_revision.manifest().revision(),
                second_revision.manifest().revision(),
                "{name}"
            );
        }
    }

    #[test]
    fn metadata_template_change_rebuilds() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let templates = root.join("templates");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&templates).unwrap();
        let template = templates.join("meta.typ");
        fs::write(&template, "#let title = [First]").unwrap();
        fs::write(
            content.join("post.typ"),
            "#import \"/templates/meta.typ\": title\n#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: title))",
        )
        .unwrap();
        fs::write(
            root.join("site.typ"),
            r#"#import "@tola/address:0.0.0": route, route-to-output
#import "@tola/source:0.0.0": all-sources
#for source in all-sources() {
  document(route-to-output(route(source.route-segments)), [#source.meta.title])
}"#,
        )
        .unwrap();

        let config = site_config(root);

        let mut session = crate::build::BuildSession::new();
        let host = compiler_host(&config).unwrap();
        let first = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers::default(),
        )
        .unwrap();
        assert!(
            String::from_utf8_lossy(output_bytes(first.graph(), "post/index.html"))
                .contains("First")
        );
        install_build(&mut session, first);

        fs::write(&template, "#let title = [Second]").unwrap();
        let decision = session.rebuild_decision(std::slice::from_ref(&template), false);
        let reuse = session.reusable_compilation(&decision, true).unwrap();
        assert!(matches!(
            reuse,
            crate::compiler::CompilationReuse::SourceAnalysis { .. }
        ));
        let second = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers {
                reuse: Some(reuse),
                ..BuildAttemptProducers::default()
            },
        )
        .unwrap();
        let second_graph = second.graph;
        let html = String::from_utf8_lossy(output_bytes(&second_graph, "post/index.html"));
        assert!(html.contains("Second"), "{html}");
    }

    #[test]
    fn rebuild_preserves_template_rules() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let templates = root.join("templates");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&templates).unwrap();
        let template = templates.join("document.typ");
        fs::write(
            &template,
            r#"#let document-template(body) = [
  #set text(fill: red)
  #show strong: it => html.strong(class: "first")[#it.body]
  #context html.span(class: "set-fill")[#if text.fill == red { "red" } else { "wrong" }]
  #body
]"#,
        )
        .unwrap();
        fs::write(
            content.join("post.typ"),
            r#"#import "@tola/document:0.0.0": current-document
#import "/templates/document.typ": document-template
#document-template[
  #context html.span(class: "route")[#current-document().route]
  *Styled*
]"#,
        )
        .unwrap();
        fs::write(
            root.join("site.typ"),
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": route, route-to-output
#for source in all-sources() {
  document(route-to-output(route(source.route-segments)))[#include source.file]
}"#,
        )
        .unwrap();

        let config = site_config(root);

        let mut session = crate::build::BuildSession::new();
        let host = compiler_host(&config).unwrap();
        let first = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers::default(),
        )
        .unwrap();
        let first_html =
            String::from_utf8_lossy(output_bytes(first.graph(), "post/index.html")).into_owned();
        assert!(
            first_html.contains(r#"class="route">/post/</span>"#),
            "{first_html}"
        );
        assert!(first_html.contains(r#"class="first""#), "{first_html}");
        assert!(
            first_html.contains(r#"class="set-fill">red</span>"#),
            "{first_html}"
        );
        install_build(&mut session, first);

        fs::write(
            &template,
            r#"#let document-template(body) = [
  #set text(fill: blue)
  #show strong: it => html.strong(class: "second")[#it.body]
  #context html.span(class: "set-fill")[#if text.fill == blue { "blue" } else { "wrong" }]
  #body
]"#,
        )
        .unwrap();
        let decision = session.rebuild_decision(std::slice::from_ref(&template), false);
        let reuse = session.reusable_compilation(&decision, true).unwrap();
        assert!(matches!(
            reuse,
            crate::compiler::CompilationReuse::SourceAnalysis { .. }
        ));
        let second = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers {
                reuse: Some(reuse),
                ..BuildAttemptProducers::default()
            },
        )
        .unwrap();
        let second_graph = second.graph;
        let second_html = String::from_utf8_lossy(output_bytes(&second_graph, "post/index.html"));
        assert!(
            second_html.contains(r#"class="route">/post/</span>"#),
            "{second_html}"
        );
        assert!(second_html.contains(r#"class="second""#), "{second_html}");
        assert!(
            second_html.contains(r#"class="set-fill">blue</span>"#),
            "{second_html}"
        );
    }

    #[test]
    fn source_inventory_changes_rebuild() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        fs::create_dir_all(&content).unwrap();
        let a = content.join("a.typ");
        let b = content.join("b.typ");
        fs::write(&a, "A").unwrap();
        fs::write(
            root.join("site.typ"),
            r#"#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": route, route-to-output
#for source in all-sources() {
  document(route-to-output(route(source.route-segments)))[#include source.file]
}"#,
        )
        .unwrap();

        let config = site_config(root);

        let mut session = crate::build::BuildSession::new();
        let host = compiler_host(&config).unwrap();

        let first = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers::default(),
        )
        .unwrap();
        assert!(has_output(first.graph(), "a/index.html"));
        assert!(!has_output(first.graph(), "b/index.html"));

        install_build(&mut session, first);
        let published_content = session
            .reusable_content_inventory(&config, &[], true)
            .unwrap();

        fs::write(&a, "A updated").unwrap();

        let cached_inventory = session
            .reusable_content_inventory(&config, std::slice::from_ref(&a), true)
            .expect("an ordinary source edit reuses content discovery");
        assert!(std::sync::Arc::ptr_eq(
            &cached_inventory,
            &published_content
        ));

        let decision = session.rebuild_decision(std::slice::from_ref(&a), false);
        let reuse = session.reusable_compilation(&decision, true).unwrap();
        let edited = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers {
                reuse: Some(reuse),
                content_inventory: Some(std::sync::Arc::clone(&cached_inventory)),
                ..BuildAttemptProducers::default()
            },
        )
        .unwrap();

        install_build(&mut session, edited);
        assert!(std::sync::Arc::ptr_eq(
            &session
                .reusable_content_inventory(&config, &[], true)
                .unwrap(),
            &cached_inventory,
        ));

        fs::write(&b, "B").unwrap();

        let added_inventory =
            session.reusable_content_inventory(&config, std::slice::from_ref(&b), true);
        assert!(added_inventory.is_none());

        let added_decision =
            session.rebuild_decision(std::slice::from_ref(&b), added_inventory.is_none());
        let reuse = session.reusable_compilation(&added_decision, true).unwrap();
        assert!(matches!(
            reuse,
            crate::compiler::CompilationReuse::SourceAnalysis { .. }
        ));
        let second = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers {
                reuse: Some(reuse),
                content_inventory: added_inventory,
                ..BuildAttemptProducers::default()
            },
        )
        .unwrap();
        assert!(has_output(second.graph(), "a/index.html"));
        assert!(has_output(second.graph(), "b/index.html"));
        install_build(&mut session, second);

        fs::remove_file(&a).unwrap();

        let removed_inventory =
            session.reusable_content_inventory(&config, std::slice::from_ref(&a), true);
        assert!(removed_inventory.is_none());

        let removed_decision =
            session.rebuild_decision(std::slice::from_ref(&a), removed_inventory.is_none());
        let reuse = session
            .reusable_compilation(&removed_decision, true)
            .unwrap();
        let third = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers {
                reuse: Some(reuse),
                content_inventory: removed_inventory,
                ..BuildAttemptProducers::default()
            },
        )
        .unwrap();

        assert!(!has_output(third.graph(), "a/index.html"));
        assert!(has_output(third.graph(), "b/index.html"));
        install_build(&mut session, third);
        let last_good_inventory = session
            .reusable_content_inventory(&config, &[], true)
            .unwrap();

        let unavailable = root.join("content-unavailable");
        fs::rename(&content, &unavailable).unwrap();

        let missing_inventory =
            session.reusable_content_inventory(&config, std::slice::from_ref(&content), true);
        assert!(missing_inventory.is_none());

        let missing_decision =
            session.rebuild_decision(std::slice::from_ref(&content), missing_inventory.is_none());
        let missing_reuse = session
            .reusable_compilation(&missing_decision, true)
            .unwrap();
        let error = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers {
                reuse: Some(missing_reuse),
                content_inventory: missing_inventory,
                ..BuildAttemptProducers::default()
            },
        )
        .err()
        .expect("missing content root must fail the build");
        assert!(error.to_string().contains("content root"), "{error:#}");

        fs::rename(&unavailable, &content).unwrap();

        let retained = session
            .reusable_content_inventory(&config, std::slice::from_ref(&b), true)
            .expect("the last successful inventory remains reusable");

        assert!(std::sync::Arc::ptr_eq(&retained, &last_good_inventory));
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].source, crate::filesystem::normalize_path(&b));

        let hook_invalidated_inventory =
            session.reusable_content_inventory(&config, std::slice::from_ref(&b), false);
        assert!(hook_invalidated_inventory.is_none());
    }

    #[test]
    fn tree_asset_read_invalidates_reuse() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let assets = root.join("assets");
        fs::create_dir_all(&content).unwrap();
        fs::create_dir_all(&assets).unwrap();
        let data = assets.join("message.txt");
        fs::write(&data, "first").unwrap();
        let entry = root.join("site.typ");
        fs::write(
            &entry,
            "#document(\"index.html\")[#read(\"/assets/message.txt\")]",
        )
        .unwrap();

        let mut config = site_config(root);
        config.assets.trees = vec![AssetTreeDeclaration::new(
            &assets,
            AssetUrlPrefix::parse("/assets").unwrap(),
        )];

        let mut session = crate::build::BuildSession::new();
        let host = compiler_host(&config).unwrap();
        let first = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers::default(),
        )
        .unwrap();
        install_build(&mut session, first);

        fs::write(&data, "second").unwrap();
        let decision = session.rebuild_decision(&[data], false);
        let reuse = session.reusable_compilation(&decision, true).unwrap();
        assert!(matches!(
            reuse,
            crate::compiler::CompilationReuse::SourceAnalysis { .. }
        ));
        let second = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers {
                reuse: Some(reuse),
                ..BuildAttemptProducers::default()
            },
        )
        .unwrap();
        let graph = second.graph;
        let html = String::from_utf8_lossy(output_bytes(&graph, "index.html"));
        assert!(html.contains("second"), "{html}");
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
            command: hook_child_command("build::session::tests::copies_stylesheet_trigger"),
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
    fn bundle_entries_share_unchanged_backing() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content = root.join("content");
        let output = root.join("public");
        let entry = root.join("site.typ");
        fs::create_dir_all(&content).unwrap();
        fs::write(
            &entry,
            r#"#document("stable/index.html")[Stable]
#asset("changed.txt", "first")
#asset("removed.txt", "removed")
"#,
        )
        .unwrap();

        let config = site_config(root);
        let host = compiler_host(&config).unwrap();

        let first = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers::default(),
        )
        .unwrap();
        write_site(&first).unwrap();
        let previous = first
            .cache_update
            .caches()
            .site_program
            .bundle_entries
            .clone();
        assert_eq!(fs::read(output.join("changed.txt")).unwrap(), b"first");
        assert!(output.join("removed.txt").is_file());

        fs::write(
            &entry,
            r#"#document("stable/index.html")[Stable]
#asset("changed.txt", "second")
#asset("added.txt", "added")
"#,
        )
        .unwrap();
        let second = build_site_with_host(
            &config,
            &host,
            BuildMode::Production,
            &mut BuildAttemptProducers {
                previous_bundle_entries: Some(previous.clone()),
                ..BuildAttemptProducers::default()
            },
        )
        .unwrap();
        let current = &second.cache_update.caches().site_program.bundle_entries;

        let first_stable = bundle_entry(&previous, "/stable/index.html");
        let second_stable = bundle_entry(current, "/stable/index.html");
        assert_eq!(first_stable.digest(), second_stable.digest());
        assert!(shares_backing(
            first_stable.bytes().as_slice(),
            second_stable.bytes().as_slice()
        ));
        assert!(shares_backing(
            output_bytes(&first.graph, "stable/index.html"),
            output_bytes(&second.graph, "stable/index.html")
        ));

        let first_changed = bundle_entry(&previous, "/changed.txt");
        let second_changed = bundle_entry(current, "/changed.txt");
        assert_eq!(first_changed.bytes().as_slice(), b"first");
        assert_eq!(second_changed.bytes().as_slice(), b"second");
        assert_ne!(first_changed.digest(), second_changed.digest());
        assert!(!shares_backing(
            first_changed.bytes().as_slice(),
            second_changed.bytes().as_slice()
        ));
        assert_eq!(output_bytes(&second.graph, "changed.txt"), b"second");

        assert!(
            current
                .as_slice()
                .iter()
                .all(|entry| entry.path().get_with_slash() != "/removed.txt")
        );
        assert!(!has_output(&second.graph, "removed.txt"));
        assert_eq!(
            bundle_entry(current, "/added.txt").bytes().as_slice(),
            b"added"
        );

        write_site(&second).unwrap();
        assert_eq!(fs::read(output.join("changed.txt")).unwrap(), b"second");
        assert_eq!(fs::read(output.join("added.txt")).unwrap(), b"added");
        assert!(!output.join("removed.txt").exists());
    }

    #[test]
    fn copies_stylesheet_trigger() {
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

    pub(super) fn bundle_entry<'a>(
        entries: &'a tola_typst::BundleEntries,
        path: &str,
    ) -> &'a tola_typst::BundleEntry {
        entries
            .as_slice()
            .iter()
            .find(|entry| entry.path().get_with_slash() == path)
            .unwrap_or_else(|| panic!("missing Bundle entry {path}"))
    }

    pub(super) fn shares_backing(left: &[u8], right: &[u8]) -> bool {
        left.len() == right.len()
            && (left.is_empty() || std::ptr::eq(left.as_ptr(), right.as_ptr()))
    }
}
