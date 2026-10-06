//! Watch requirements derived from observed build inputs.

use super::fs::{ProducerKind, WatchRequirements};
use tola_build::build::{InputKind, InputObservation, InputScope, ObservedInputPath};
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
            InputKind::TypstPackageSelection => ProducerKind::PackageResolution,
            InputKind::ContentInventory => ProducerKind::Content,
            InputKind::ConfiguredAssets => ProducerKind::ConfiguredAssets,
            InputKind::Icons => ProducerKind::Icons,
            InputKind::FontInventory => ProducerKind::Fonts,
            InputKind::BeforeBuildHookOutputs => ProducerKind::Hooks,
        };
        let paths = [path.path().to_path_buf()];
        match (path.scope(), path.required()) {
            (InputScope::Exact, true) => requirements.exact(producer, paths),
            (InputScope::Exact, false) => requirements.optional_exact(producer, paths),
            (InputScope::Children, true) => requirements.children(producer, paths),
            (InputScope::Children, false) => requirements.optional_children(producer, paths),
            (InputScope::Recursive, true) => requirements.recursive(producer, paths),
            (InputScope::Recursive, false) => requirements.optional_recursive(producer, paths),
        }
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
    requirements.exact(ProducerKind::TypstReads, root_files);
    requirements.package_files(package_files);
    requirements.package_checks(package_checks);
}
