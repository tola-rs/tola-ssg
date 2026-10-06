//! Read-only inspection of sources, icon namespaces, and complete site builds.

mod icons;
mod site;
mod sources;

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::ResolvedSiteConfig;
use crate::metadata::SourceMetadata;

pub use icons::icons;
pub use site::{documents, outputs, references, routes};

/// A selected source path and its optional discovered metadata.
#[derive(Debug)]
pub struct InspectedSource {
    pub(super) path: PathBuf,
    pub(super) metadata: Option<SourceMetadata>,
}

impl InspectedSource {
    /// Source path relative to the site root, or absolute when outside it.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn metadata(&self) -> Option<&SourceMetadata> {
        self.metadata.as_ref()
    }
}

pub struct InspectedSources {
    sources: Vec<InspectedSource>,
    selected: usize,
    diagnostics: Vec<crate::diagnostic::Diagnostic>,
}

impl InspectedSources {
    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn diagnostics(&self) -> &[crate::diagnostic::Diagnostic] {
        &self.diagnostics
    }

    /// Number of selected sources with declared metadata, including empty dictionaries.
    pub fn matched(&self) -> usize {
        self.sources
            .iter()
            .filter(|source| source.metadata().is_some())
            .count()
    }

    /// Includes sources without metadata.
    pub fn sources(&self) -> &[InspectedSource] {
        &self.sources
    }
}

/// Inspect selected sources and their optional metadata, respecting cancellation.
pub fn inspect_sources(
    paths: &[std::path::PathBuf],
    config: &ResolvedSiteConfig,
    resources: &crate::resources::BuildResources,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<InspectedSources> {
    cancellation.ensure_active()?;
    resources
        .source_boundary(config)
        .check(&config.build.content_dir)?;
    let host =
        crate::compiler::TypstHost::for_config_with_resources(config, resources, cancellation)?;
    let units =
        crate::content::discover_content_units_for_config_with_cancellation(config, cancellation)?;
    let files = crate::content::select_content_sources(config, paths, &units)?;
    cancellation.ensure_active()?;
    let selected = files.len();
    let (sources, diagnostics) =
        sources::inspect(&files, &units, config, &host, resources, cancellation)?;
    Ok(InspectedSources {
        sources,
        selected,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_keeps_metadata_free_sources() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("content")).unwrap();
        std::fs::write(
            root.join("content/document.typ"),
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((:))",
        )
        .unwrap();
        std::fs::write(root.join("content/plain.typ"), "").unwrap();
        std::fs::write(root.join("site.typ"), "").unwrap();
        let config = crate::config::tests::load_test_config(root, "");
        let inspected = inspect_sources(
            &[],
            &config,
            &crate::resources::BuildResources::default(),
            &crate::cancellation::BuildCancellation::new(),
        )
        .unwrap();

        assert_eq!(inspected.selected(), 2);
        assert_eq!(inspected.matched(), 1);
        assert_eq!(
            inspected
                .sources()
                .iter()
                .map(|source| (source.path(), source.metadata().is_some()))
                .collect::<Vec<_>>(),
            [
                (std::path::Path::new("content/document.typ"), true),
                (std::path::Path::new("content/plain.typ"), false),
            ]
        );
    }
}
