//! Serial source compilation, with resources retained across configuration changes.

mod analyze;
mod check;
mod compilations;
mod jobs;
mod readback;
mod worker;

pub(crate) use analyze::analyze;
pub(crate) use check::{CheckedSources, checked_root};
pub(crate) use compilations::RevisionCompilations;
pub(crate) use jobs::{
    AnalysisRequest, CheckRequest, IncomingCallsRead, LensesRead, PackageSourceRead, QueryRequest,
    RenameRead, RouteIndexRead, RouteRead, SelectionRead, SourceInputs, SourceJob, SourceOverrides,
    SymbolsRead,
};
pub(crate) use worker::{SourceCompilation, SourceCompiler, SourceFailure};

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use anyhow::Result;
    use tola_build::BuildResources;
    use tola_build::cancellation::BuildCancellation;
    use tola_build::config::ResolvedSiteConfig;
    use tola_build::config::loading::{BuildOverrides, load_site_config};

    use super::jobs::{SourceInputs, SourceOverrides, UnsavedSource};
    use super::worker::{SourceCompilation, SourceCompiler, SourceFailure};
    use crate::server::ServedWorkspace;

    pub(crate) fn inputs(root: &Path, cancellation: BuildCancellation) -> SourceInputs {
        SourceInputs {
            root: root.to_path_buf(),
            overrides: Arc::default(),
            source_revision: 0,
            cancellation,
        }
    }

    /// One revision's inputs with a fresh cancellation token, with `overrides` as its unsaved
    /// sources.
    pub(crate) fn inputs_at(
        root: &Path,
        source_revision: u64,
        overrides: SourceOverrides,
    ) -> SourceInputs {
        SourceInputs {
            root: root.to_path_buf(),
            overrides,
            source_revision,
            cancellation: BuildCancellation::new(),
        }
    }

    /// A compiler lane answering from one site's configuration, with default build resources.
    pub(crate) fn compiler(
        configuration: &Arc<ResolvedSiteConfig>,
    ) -> SourceCompiler<impl FnMut(&Path, &[UnsavedSource]) -> Result<ServedWorkspace>> {
        let configuration = Arc::clone(configuration);
        SourceCompiler::new(
            move |_, _| Ok(ServedWorkspace::Site(Arc::clone(&configuration))),
            BuildResources::default(),
        )
    }

    pub(crate) fn assert_cancelled(completion: SourceCompilation) {
        match completion {
            SourceCompilation::Checked { checked, .. } => {
                assert!(matches!(checked, Err(SourceFailure::Cancelled)));
            }
            SourceCompilation::Answered { response, .. } => {
                assert!(matches!(response, Err(SourceFailure::Cancelled)));
            }
            SourceCompilation::Continued(_) => panic!("a cancelled analysis is answered"),
            SourceCompilation::Selected { .. } => panic!("a cancelled selection is answered"),
            SourceCompilation::Released => panic!("a cancelled release is answered"),
        }
    }

    /// The configuration a site below `root` loads from its `tola.toml`, written as `text`.
    pub(crate) fn site_configuration(root: &Path, text: &str) -> Arc<ResolvedSiteConfig> {
        let path = root.join("tola.toml");
        std::fs::write(&path, text).unwrap();
        Arc::new(
            load_site_config(
                Some(&path),
                tola_typst::PackageLocations::default(),
                &BuildOverrides::default(),
            )
            .unwrap()
            .into_config(),
        )
    }
}
