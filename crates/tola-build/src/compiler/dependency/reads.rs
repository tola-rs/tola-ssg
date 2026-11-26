use std::path::{Path, PathBuf};

use tola_typst::{FileRead, ReadLocator, ReadOrigin, ReadRecord};

use crate::cancellation::{BuildCancellation, BuildCancelled};

#[derive(Debug, thiserror::Error)]
pub(super) enum ReuseProtocolError {
    #[error("Tola could not reuse the files the previous build read; run the build again")]
    MissingReaderEvidence,
    #[error("Tola could not reuse the packages the previous build read; run the build again")]
    ChangedReaderPackages,
    #[error("Tola could not tell which part of the build read a package; run the build again")]
    UnattributedPackageCheck,
    #[error(
        "Tola could not read `{path}`, which this build uses; restore it or remove the reference"
    )]
    UnreadableRead {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone)]
pub(super) enum DepRead {
    Physical {
        evidence: ReadRecord,
        logical_path: PathBuf,
        /// Canonical target so symlink retargets invalidate even with identical bytes.
        canonical_target: PathBuf,
        watch_paths: Vec<PathBuf>,
    },
    Virtual {
        evidence: ReadRecord,
        process_stable: bool,
    },
}

#[derive(Debug, Clone)]
pub(super) struct DepReader {
    pub(super) reads: Vec<DepRead>,
    pub(super) package_checks: Vec<tola_typst::PackageCheck>,
}

impl DepRead {
    pub(super) fn freeze(
        read: FileRead,
        host: &crate::compiler::SiteHost,
    ) -> Result<Self, ReuseProtocolError> {
        let (evidence, origin) = read.into_evidence_and_origin();
        match origin {
            ReadOrigin::Disk(path) => {
                let logical_path = path.into_path();
                let canonical_target = std::fs::canonicalize(&logical_path).map_err(|err| {
                    ReuseProtocolError::UnreadableRead {
                        path: read_locator_display(evidence.locator()),
                        source: err,
                    }
                })?;
                let watch_paths = physical_watch_paths(&logical_path, &canonical_target);
                Ok(Self::Physical {
                    evidence,
                    logical_path,
                    canonical_target,
                    watch_paths,
                })
            }
            ReadOrigin::Provider | ReadOrigin::NonPersistent => Ok(Self::Virtual {
                process_stable: matches!(origin, ReadOrigin::Provider)
                    && host.read_is_process_stable(evidence.locator()),
                evidence,
            }),
        }
    }

    pub(super) fn requires_each_build(&self) -> bool {
        match self {
            Self::Virtual {
                evidence,
                process_stable: false,
            } => {
                // Icon files are compared against current collections before reuse.
                !matches!(
                    evidence.locator(),
                    ReadLocator::ProvidedPackage { package, path }
                        if tola_packages::is_icon_file(package, path)
                )
            }
            _ => false,
        }
    }

    pub(super) fn watch_paths(&self) -> &[PathBuf] {
        match self {
            Self::Physical { watch_paths, .. } => watch_paths,
            Self::Virtual { .. } => &[],
        }
    }

    pub(super) fn evidence(&self) -> &ReadRecord {
        match self {
            Self::Physical { evidence, .. } | Self::Virtual { evidence, .. } => evidence,
        }
    }
}

pub(super) fn physical_read_is_fresh(
    read: &DepRead,
    boundary: &tola_typst::SourceBoundary,
    cancellation: &BuildCancellation,
) -> Result<bool, BuildCancelled> {
    cancellation.ensure_active()?;
    let DepRead::Physical {
        evidence,
        logical_path,
        canonical_target,
        ..
    } = read
    else {
        return Ok(true);
    };
    if boundary.check(logical_path).is_err() {
        return Ok(false);
    }
    if !std::fs::canonicalize(logical_path).is_ok_and(|target| target == *canonical_target) {
        cancellation.ensure_active()?;
        return Ok(false);
    }
    if !std::fs::read(logical_path)
        .is_ok_and(|bytes| tola_typst::ContentDigest::of(&bytes) == evidence.digest())
    {
        cancellation.ensure_active()?;
        return Ok(false);
    }
    cancellation.ensure_active()?;
    let matches =
        std::fs::canonicalize(logical_path).is_ok_and(|target| target == *canonical_target);
    cancellation.ensure_active()?;
    Ok(matches)
}

pub(super) fn package_check_is_fresh(check: &tola_typst::PackageCheck) -> bool {
    match check.availability() {
        tola_typst::PackageAvailability::Missing => std::fs::metadata(check.candidate())
            .is_err_and(|err| err.kind() == std::io::ErrorKind::NotFound),
        tola_typst::PackageAvailability::Present => {
            std::fs::metadata(check.candidate()).is_ok_and(|meta| meta.is_dir())
                && check.canonical_target().is_some_and(|expected| {
                    std::fs::canonicalize(check.candidate()).is_ok_and(|cur| cur == expected)
                })
        }
        tola_typst::PackageAvailability::NotDirectory
        | tola_typst::PackageAvailability::Unreadable => false,
    }
}

pub(super) fn physical_watch_paths(logical_path: &Path, canonical_target: &Path) -> Vec<PathBuf> {
    let mut paths = vec![logical_path.to_path_buf(), canonical_target.to_path_buf()];
    paths.sort_unstable();
    paths.dedup();
    paths
}

fn read_locator_display(locator: &ReadLocator) -> String {
    match locator {
        ReadLocator::Root(path) | ReadLocator::ProvidedRoot(path) => path.display().to_string(),
        ReadLocator::Package { package, path } | ReadLocator::ProvidedPackage { package, path } => {
            format!("{package}/{}", path.display())
        }
        ReadLocator::NonPersistent(_) => "a generated file".to_owned(),
    }
}
