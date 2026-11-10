//! Realized site documents, route indexes, references, and immutable revisions.

use std::sync::Arc;

use crate::config::ResolvedSiteConfig;
use crate::output::PageAvailability;

mod index;
pub mod references;
mod resolve;
mod resource;
mod space;

pub use index::SiteIndex;
pub use resolve::AddressResolution;
pub use resource::{HtmlPage, Resource, SiteAssetRoute};
pub use space::{AddressRegistrationError, AddressSpace};

/// Outputs, addresses, references, configuration, diagnostics, and input observations
/// from one build, obtained through [`crate::build::BuildSession::install_revision`].
/// The application selects which revision is current.
pub struct SiteRevision {
    pub(crate) config: Arc<ResolvedSiteConfig>,
    pub(crate) outputs: crate::output::revision::OutputRevision,
    pub(crate) index: SiteIndex,
    pub(crate) references: references::References,
    pub(crate) diagnostics: Vec<tola_diagnostic::Diagnostic>,
    pub(crate) mode: crate::mode::BuildMode,
    pub(crate) scope: tola_input::InputScope,
    pub(crate) input_observation: crate::observation::InputObservation,
}

impl std::fmt::Debug for SiteRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SiteRevision")
            .field("revision", &self.outputs.manifest().revision())
            .field("references", &self.references.references().len())
            .field("diagnostics", &self.diagnostics.len())
            .finish_non_exhaustive()
    }
}

impl SiteRevision {
    /// Input boundaries belonging to this immutable revision, independent of caches.
    pub fn input_observation(&self) -> &crate::observation::InputObservation {
        &self.input_observation
    }

    /// Output policy used to construct this revision.
    pub fn mode(&self) -> crate::mode::BuildMode {
        self.mode
    }

    /// The input scope this revision was built under, for after-publish hooks.
    pub fn scope(&self) -> tola_input::InputScope {
        self.scope
    }

    /// A read-only filesystem view of this revision's complete output, for a consumer
    /// that needs real paths.
    ///
    /// Every call materializes the revision again; the returned handle keeps the
    /// temporary files alive.
    pub fn hook_output(
        &self,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> anyhow::Result<crate::output::files::HookOutputFiles> {
        cancellation.ensure_active()?;
        crate::output::files::HookOutputFiles::materialize_revision(
            self.config.get_root(),
            &self.outputs,
            cancellation,
            crate::compiler::minified_languages(&self.config),
        )
    }

    /// What an after-publish run of this revision consumes.
    pub fn after_publish_dependencies(
        &self,
    ) -> anyhow::Result<crate::hooks::AfterPublishDependencies> {
        crate::hooks::observe_after_publish(
            &self.config,
            self.mode,
            self.outputs.manifest().revision(),
        )
    }

    pub fn config(&self) -> &Arc<ResolvedSiteConfig> {
        &self.config
    }

    pub fn outputs(&self) -> &crate::output::revision::OutputRevision {
        &self.outputs
    }

    pub fn address(&self) -> &AddressSpace {
        self.index.address()
    }

    pub fn references(&self) -> &references::References {
        &self.references
    }

    pub fn diagnostics(&self) -> &[tola_diagnostic::Diagnostic] {
        &self.diagnostics
    }

    pub fn page_availability(&self) -> PageAvailability {
        self.outputs.page_availability()
    }

    pub fn manifest(&self) -> &crate::output::manifest::SiteManifest {
        self.outputs.manifest()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::OwnedSiteConfig;
    use crate::output::graph::OutputGraphBuilder;
    use crate::output::revision::OutputRevision;
    use crate::output::semantics::OutputDeclaration;
    use crate::output::tests::{output_file, output_graph};

    fn empty_revision() -> OutputRevision {
        OutputRevision::from_graph(&OutputGraphBuilder::new().finish())
    }

    fn site_revision(config: Arc<ResolvedSiteConfig>, outputs: OutputRevision) -> SiteRevision {
        SiteRevision {
            config,
            outputs,
            index: SiteIndex::default(),
            references: references::References::default(),
            diagnostics: Vec::new(),
            mode: crate::mode::BuildMode::Production,
            scope: tola_input::InputScope::Online,
            input_observation: crate::observation::InputObservation::default(),
        }
    }

    #[test]
    fn cancelled_consumer_gets_no_output_view() {
        let site_config = OwnedSiteConfig::new("");
        let canceller = crate::cancellation::BuildCanceller::default();
        let revision = site_revision(Arc::new(site_config.config.clone()), empty_revision());

        canceller.cancel();
        let error = revision.hook_output(&canceller.token()).unwrap_err();

        assert!(error.is::<crate::cancellation::BuildCancelled>());
    }

    #[test]
    fn page_availability_follows_html_output() {
        let site_config = OwnedSiteConfig::new("");
        let empty = site_revision(Arc::new(site_config.config.clone()), empty_revision());
        assert_eq!(empty.page_availability(), PageAvailability::Empty);

        let outputs = OutputRevision::from_graph(&output_graph([output_file(
            "index.html",
            OutputDeclaration::html_document(),
            b"page",
        )]));
        let present = site_revision(Arc::new(site_config.config), outputs);
        assert_eq!(present.page_availability(), PageAvailability::Present);
    }

    const CONSUMER_CONFIG: &str = "\
[[hooks.after-publish]]
name = \"consumer\"
argv = [\"consume\"]
rerun-on = { include = [\"consume.mjs\"] }
";

    #[test]
    fn identical_revisions_share_after_publish_dependencies() {
        let site_config = OwnedSiteConfig::new(CONSUMER_CONFIG);
        let config = Arc::new(site_config.config.clone());
        let first = site_revision(Arc::clone(&config), empty_revision());
        let second = site_revision(config, empty_revision());

        assert_eq!(
            first.after_publish_dependencies().unwrap(),
            second.after_publish_dependencies().unwrap()
        );
    }

    #[test]
    fn rerun_on_edit_changes_after_publish_dependencies() {
        let site_config = OwnedSiteConfig::new(CONSUMER_CONFIG);
        let script = site_config.config.get_root().join("consume.mjs");
        std::fs::write(&script, "first").unwrap();
        let revision = site_revision(Arc::new(site_config.config.clone()), empty_revision());
        let before = revision.after_publish_dependencies().unwrap();

        std::fs::write(&script, "second").unwrap();

        assert_ne!(revision.after_publish_dependencies().unwrap(), before);
    }
}
