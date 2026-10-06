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
    config: Arc<ResolvedSiteConfig>,
    outputs: crate::output::revision::OutputRevision,
    index: SiteIndex,
    references: references::References,
    diagnostics: Vec<crate::diagnostic::Diagnostic>,
    mode: crate::mode::BuildMode,
    input_observation: crate::observation::InputObservation,
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
    pub(crate) fn new(
        config: Arc<ResolvedSiteConfig>,
        outputs: crate::output::revision::OutputRevision,
        index: SiteIndex,
        references: references::References,
        diagnostics: Vec<crate::diagnostic::Diagnostic>,
        mode: crate::mode::BuildMode,
        input_observation: crate::observation::InputObservation,
    ) -> Self {
        Self {
            config,
            outputs,
            index,
            references,
            diagnostics,
            mode,
            input_observation,
        }
    }

    /// Input boundaries belonging to this immutable revision, independent of caches.
    pub fn input_observation(&self) -> &crate::observation::InputObservation {
        &self.input_observation
    }

    /// Output policy used to construct this revision.
    pub fn mode(&self) -> crate::mode::BuildMode {
        self.mode
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

    pub fn diagnostics(&self) -> &[crate::diagnostic::Diagnostic] {
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
        SiteRevision::new(
            config,
            outputs,
            SiteIndex::default(),
            references::References::default(),
            Vec::new(),
            crate::mode::BuildMode::Production,
            crate::observation::InputObservation::default(),
        )
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
}
