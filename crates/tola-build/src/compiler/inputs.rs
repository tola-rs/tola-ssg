//! Filesystem inputs observed by one site-build attempt.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use tola_typst::sort_package_checks;

/// Attempt inputs retained for watcher recovery after failure.
#[derive(Default)]
pub(crate) struct BuildInputs {
    file_reads: FileReads,
    package_checks: PackageChecks,
    package_reads: Vec<tola_typst::PackageRead>,
    successful_reads: HashSet<tola_typst::FileRead>,
}

impl BuildInputs {
    pub(crate) fn record_accessed(&mut self, accessed: &tola_typst::AccessedDeps) {
        self.successful_reads.extend(accessed.reads.iter().cloned());
        self.file_reads.record(
            accessed
                .disk_reads
                .iter()
                .cloned()
                .map(tola_typst::DiskReadPath::into_path),
        );
        self.package_checks
            .record(accessed.package_checks.iter().cloned());
        self.package_reads.extend(accessed.package_reads());
    }

    /// The package directories this attempt actually read.
    pub(crate) fn package_reads(&self) -> &[tola_typst::PackageRead] {
        &self.package_reads
    }

    pub(crate) fn successful_reads(&self) -> impl Iterator<Item = &tola_typst::FileRead> {
        self.successful_reads.iter()
    }

    pub(crate) fn file_reads_mut(&mut self) -> &mut FileReads {
        &mut self.file_reads
    }

    pub(crate) fn record_package_checks(
        &mut self,
        checks: impl IntoIterator<Item = tola_typst::PackageCheck>,
    ) {
        self.package_checks.record(checks);
    }

    pub(crate) fn into_parts(self) -> (Vec<PathBuf>, Vec<tola_typst::PackageCheck>) {
        let Self {
            file_reads,
            package_checks,
            ..
        } = self;
        (file_reads.into_paths(), package_checks.into_checks())
    }
}

/// Physical file reads relevant to a failed build attempt.
#[derive(Default)]
pub(crate) struct FileReads {
    reads: BTreeSet<PathBuf>,
}

impl FileReads {
    pub(super) fn record(&mut self, reads: impl IntoIterator<Item = PathBuf>) {
        self.reads.extend(reads);
    }

    fn into_paths(self) -> Vec<PathBuf> {
        self.reads.into_iter().collect()
    }
}

/// Package-location checks observed by one build attempt.
#[derive(Default)]
pub(crate) struct PackageChecks {
    checks: HashSet<tola_typst::PackageCheck>,
}

impl PackageChecks {
    fn record(&mut self, checks: impl IntoIterator<Item = tola_typst::PackageCheck>) {
        self.checks.extend(checks);
    }

    fn into_checks(self) -> Vec<tola_typst::PackageCheck> {
        let mut checks = self.checks.into_iter().collect::<Vec<_>>();
        sort_package_checks(&mut checks);
        checks
    }
}
