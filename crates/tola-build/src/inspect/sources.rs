//! Discovery of metadata for the sources an inspection selected.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::config::ResolvedSiteConfig;
use crate::filesystem::root_relative;
use anyhow::Result;
use tola_input::normalize_path;

use super::InspectedSource;

pub(super) fn inspect(
    files: &[PathBuf],
    units: &[crate::content::ContentUnit],
    config: &ResolvedSiteConfig,
    prepared: &tola_module::prepared::PreparedModules,
    host: &crate::compiler::SiteHost,
    resources: &crate::resources::BuildResources,
    cancellation: &crate::cancellation::BuildCancellation,
) -> Result<(Vec<InspectedSource>, Vec<tola_diagnostic::Diagnostic>)> {
    let mut inputs = crate::compiler::BuildInputs::default();
    let packages = crate::package::prepare_package_inputs(
        config,
        resources,
        cancellation,
        &mut None,
        &Default::default(),
    )?;
    let host = host.with_icons(packages.icons.collections());
    let mut warnings = tola_typst::Diagnostics::new();
    let bindings = crate::package::SiteBindings::from_config(
        config,
        crate::asset::AssetUrls::for_check_with(
            config,
            crate::modules::prepared::AppliedAssets {
                modules: &prepared.assets,
            },
            cancellation,
        )?,
    )
    .prepare_modules(prepared)?;
    let scan = crate::compiler::analysis::analyze(
        config,
        &host,
        units,
        // Inspection renders no configured assets, so every declaration resolves
        // to its configured name and a tree member enumerates from its source
        // directory without being rendered.
        bindings,
        crate::compiler::analysis::SourceAnalysisReuse::None,
        &cancellation.bundle_cancellation(),
        &mut inputs,
    )
    .map_err(|failure| with_prior_diagnostics(failure.into_error(), config, &mut warnings))?;
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

/// A later source failure must not discard the option schema's diagnostics.
fn with_prior_diagnostics(
    error: anyhow::Error,
    config: &ResolvedSiteConfig,
    warnings: &mut tola_typst::Diagnostics,
) -> anyhow::Error {
    let prior = crate::compiler::warning_diagnostics(config, warnings);
    if prior.is_empty() {
        return error;
    }
    let mut diagnostics = tola_diagnostic::attached(&error)
        .map(<[tola_diagnostic::Diagnostic]>::to_vec)
        .or_else(|| crate::compiler::error_diagnostics(&error, config.get_root()))
        .unwrap_or_else(|| {
            vec![tola_diagnostic::fallback(
                tola_diagnostic::codes::typst::SOURCE_ANALYSIS,
                &error,
            )]
        });
    for diagnostic in prior {
        if !diagnostics.contains(&diagnostic) {
            diagnostics.push(diagnostic);
        }
    }
    tola_diagnostic::DiagnosticError::attach(error, diagnostics).into()
}
