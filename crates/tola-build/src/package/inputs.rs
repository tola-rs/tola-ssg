//! Prepare the immutable values exposed by Tola's virtual packages.

use anyhow::Result;

use crate::config::ResolvedSiteConfig;
use crate::filesystem::SourceOverrides;
use crate::icon::IconSnapshot;

use crate::cancellation::BuildCancellation;
use crate::resources::BuildResources;

/// Immutable producer values exposed through package bindings.
pub(crate) struct PackageInputs {
    pub(crate) icons: IconSnapshot,
}

/// Each reusable resource is accepted after its own successful preparation.
/// These values do not represent published output or a successful source check.
pub(crate) fn prepare_package_inputs(
    config: &ResolvedSiteConfig,
    resources: &BuildResources,
    cancellation: &BuildCancellation,
    retained_icons: &mut Option<IconSnapshot>,
    overrides: &SourceOverrides,
) -> Result<PackageInputs> {
    let icons = prepare_icons(config, resources, cancellation, retained_icons, overrides)?;
    Ok(PackageInputs { icons })
}

pub(crate) fn prepare_icons(
    config: &ResolvedSiteConfig,
    resources: &BuildResources,
    cancellation: &BuildCancellation,
    retained_icons: &mut Option<IconSnapshot>,
    overrides: &SourceOverrides,
) -> Result<IconSnapshot> {
    cancellation.ensure_active()?;
    let started = std::time::Instant::now();
    let icons = crate::icon::prepare(
        config,
        cancellation,
        retained_icons.as_ref(),
        resources,
        overrides,
    )?;
    cancellation.ensure_active()?;
    *retained_icons = Some(icons.clone());
    tracing::debug!(target: "tola::compile",
        icons_ms = started.elapsed().as_secs_f64() * 1000.0,
        "prepared icon inputs");
    Ok(icons)
}
