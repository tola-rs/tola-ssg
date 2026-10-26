//! Watch requirements derived from observed build inputs.

use super::fs::{ProducerKind, WatchRequirements};
use tola_build::build::{InputKind, InputObservation, InputPathScope, ObservedInputPath};
use tola_build::filesystem::path_is_within;

pub(crate) fn watch_requirements_from_observation(
    observation: &InputObservation,
) -> WatchRequirements {
    let mut requirements = WatchRequirements::default();
    append_typst_inputs(
        &mut requirements,
        observation.physical_read_paths().iter().cloned(),
        observation.package_checks().iter().cloned(),
    );
    append_observed_paths(&mut requirements, observation.paths());
    requirements
}

pub(crate) fn append_observed_paths<'a>(
    requirements: &mut WatchRequirements,
    paths: impl IntoIterator<Item = &'a ObservedInputPath>,
) {
    for path in paths {
        let producer = match path.input() {
            InputKind::TypstPhysicalReads => ProducerKind::TypstReads,
            InputKind::ModuleManifests => ProducerKind::Modules,
            InputKind::TypstPackageSelection => ProducerKind::PackageResolution,
            InputKind::ContentInventory => ProducerKind::Content,
            InputKind::ConfiguredAssets => ProducerKind::ConfiguredAssets,
            InputKind::Icons => ProducerKind::Icons,
            InputKind::FontInventory => ProducerKind::Fonts,
            InputKind::BeforeBuildHookOutputs => ProducerKind::Hooks,
        };
        requirements.observe_paths(
            producer,
            path.scope(),
            path.required(),
            [path.path().to_path_buf()],
        );
    }
}

/// Split physical reads into package files and site files using the selected package roots.
pub(crate) fn append_typst_inputs(
    requirements: &mut WatchRequirements,
    physical_reads: impl IntoIterator<Item = std::path::PathBuf>,
    package_checks: impl IntoIterator<Item = tola_typst::PackageCheck>,
) {
    let package_checks = package_checks.into_iter().collect::<Vec<_>>();
    let package_roots = package_checks
        .iter()
        .filter(|check| check.was_selected())
        .flat_map(|check| std::iter::once(check.candidate()).chain(check.canonical_target()))
        .collect::<Vec<_>>();
    let (package_files, root_files): (Vec<_>, Vec<_>) = physical_reads
        .into_iter()
        .partition(|path| package_roots.iter().any(|root| path_is_within(path, root)));
    requirements.observe_paths(
        ProducerKind::TypstReads,
        InputPathScope::Exact,
        true,
        root_files,
    );
    requirements.package_files(package_files);
    requirements.package_checks(package_checks);
}
