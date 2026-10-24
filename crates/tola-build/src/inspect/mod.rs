//! Content-source discovery, source metadata, and icon namespaces.

mod icons;
mod sources;

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::ResolvedSiteConfig;
use crate::metadata::SourceMetadata;

pub use crate::content::ContentDiscoveryError;
pub use icons::{icons, presets};

/// Why one source inspection could not finish.
#[derive(Debug, thiserror::Error)]
pub enum SourceInspectionError {
    #[error(transparent)]
    Cancelled(#[from] crate::cancellation::BuildCancelled),
    #[error(transparent)]
    Boundary(#[from] typst::diag::FileError),
    #[error(transparent)]
    Content(#[from] ContentDiscoveryError),
    #[error(transparent)]
    Options(#[from] crate::modules::OptionsFailure),
}

/// Discover selected content sources without evaluating them.
pub fn content_sources(
    paths: &[PathBuf],
    config: &ResolvedSiteConfig,
    resources: &crate::resources::BuildResources,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<Vec<crate::content::ContentUnit>, SourceInspectionError> {
    cancellation.ensure_active()?;
    resources
        .source_boundary(config)
        .check(&config.build().content_dir)?;
    let mut units =
        crate::content::discover_content_units_for_config_with_cancellation(config, cancellation)?;
    let selected: std::collections::HashSet<_> =
        crate::content::select_content_sources(config, paths, &units)?
            .into_iter()
            .collect();
    units.retain(|unit| selected.contains(&unit.source));
    Ok(units)
}

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
    diagnostics: Vec<tola_diagnostic::Diagnostic>,
}

impl InspectedSources {
    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn diagnostics(&self) -> &[tola_diagnostic::Diagnostic] {
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
    let configured = crate::modules::prepared::configure(config, resources.input_scope())?;
    let config = &configured;
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
    fn inspection_loads_imports_and_options() {
        use std::fs;
        use typst::foundations::IntoValue;

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("project");
        let module = directory.path().join("module");
        fs::create_dir_all(root.join("content")).unwrap();
        fs::create_dir(&module).unwrap();
        fs::write(root.join("site.typ"), "").unwrap();
        fs::write(
            root.join("content/post.typ"),
            "#import \"@tola-modules/sample:0.0.0\": note\n#note()",
        )
        .unwrap();
        fs::write(
            module.join("tola-module.toml"),
            r#"
schema-version = 1
name = "sample"
version = "1.0.0"
[typst]
entry = "lib.typ"
[options]
schema = "options.typ"
export = "schema"
"#,
        )
        .unwrap();
        fs::write(
            module.join("lib.typ"),
            r#"
#import "@tola/source:0.0.0": tola-meta
#import "@tola/module:0.0.0": get-module
#let settings = get-module()
#let note() = tola-meta((title: settings.options.title))
"#,
        )
        .unwrap();
        fs::write(
            module.join("options.typ"),
            r#"
#import "@tola/schema:0.0.0" as s
#let schema = s.schema((title: s.optional(str, default: "From schema")))
"#,
        )
        .unwrap();
        let base = "[modules.sample]\npath='../module'\n";
        let config = crate::config::tests::load_test_config(&root, base);
        let resources = crate::BuildResources::new().with_input_scope(crate::InputScope::Offline);
        let cancellation = crate::cancellation::BuildCancellation::default();
        let inspected = inspect_sources(&[], &config, &resources, &cancellation).unwrap();
        assert_eq!(inspected.matched(), 1);
        assert_eq!(
            inspected.sources()[0]
                .metadata()
                .unwrap()
                .to_typst_dict()
                .get("title")
                .unwrap(),
            &"From schema".into_value(),
        );
        let invalid = crate::config::tests::load_test_config(
            &root,
            &format!("{base}[modules.sample.options]\ntitle=7\n"),
        );
        let error = inspect_sources(&[], &invalid, &resources, &cancellation)
            .err()
            .expect("invalid options must stop source inspection");
        let diagnostics =
            tola_diagnostic::attached(&error).expect("structured options diagnostics");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("title")
                    && diagnostic.message.contains("str")
                    && diagnostic.message.contains("int")),
            "diagnostics: {diagnostics:#?}"
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic
                    .location
                    .as_ref()
                    .is_some_and(
                        |location| location.path.ends_with("tola.toml") && location.line == Some(4)
                    )),
            "diagnostics: {diagnostics:#?}"
        );
    }

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
