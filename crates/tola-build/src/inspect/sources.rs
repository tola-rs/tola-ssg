//! Discovery of metadata for the sources an inspection selected.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::config::ResolvedSiteConfig;
use crate::filesystem::{normalize_path, root_relative};
use anyhow::Result;

use super::InspectedSource;

pub(super) fn inspect(
    files: &[PathBuf],
    units: &[crate::content::ContentUnit],
    config: &ResolvedSiteConfig,
    host: &crate::compiler::TypstHost,
    resources: &crate::resources::BuildResources,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<(Vec<InspectedSource>, Vec<crate::diagnostic::Diagnostic>)> {
    let mut inputs = crate::compiler::BuildInputs::default();
    let packages = crate::package::prepare_package_inputs(
        config,
        resources,
        cancellation,
        &mut None,
        &Default::default(),
    )?;
    let host = host.with_icons(packages.icons.collections());
    let scan = crate::compiler::analysis::analyze(
        config,
        &host,
        units,
        // Inspection renders no configured assets, so every declaration resolves
        // to its configured name and a tree member enumerates from its source
        // directory without being rendered.
        crate::package::SiteBindings::from_config(
            config,
            crate::asset::AssetUrls::for_check(config, cancellation)?,
        ),
        crate::compiler::analysis::SourceAnalysisReuse::None,
        &cancellation.bundle_cancellation(),
        &mut inputs,
    )
    .map_err(crate::compiler::analysis::SourceAnalysisFailure::into_error)?;
    let mut warnings = tola_typst::Diagnostics::new();
    warnings.extend_distinct(&scan.diagnostics);
    let selected = files
        .iter()
        .map(|path| normalize_path(path))
        .collect::<HashSet<_>>();
    let root = normalize_path(config.get_root());

    let mut sources = Vec::with_capacity(files.len());
    for source in scan.source_set.sources() {
        cancellation.ensure_active()?;
        let source_path = normalize_path(source.source());
        if !selected.contains(&source_path) {
            continue;
        }
        let path = root_relative(&source_path, &root).to_path_buf();
        sources.push(InspectedSource {
            path,
            metadata: source.metadata().cloned(),
        });
    }

    let diagnostics = crate::compiler::warning_diagnostics(config, &mut warnings);

    Ok((sources, diagnostics))
}
