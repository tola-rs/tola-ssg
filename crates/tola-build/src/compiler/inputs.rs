//! Filesystem inputs observed by one site-build attempt.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use tola_typst::sort_package_checks;

/// Attempt inputs retained for watcher recovery after failure.
#[derive(Default)]
pub(crate) struct BuildInputs {
    disk_reads: BTreeSet<PathBuf>,
    missing_disk_reads: BTreeSet<PathBuf>,
    package_checks: HashSet<tola_typst::PackageCheck>,
    package_reads: Vec<tola_typst::PackageRead>,
    successful_reads: HashSet<tola_typst::FileRead>,
}

impl BuildInputs {
    pub(crate) fn record_accessed(&mut self, accessed: &tola_typst::AccessedDeps) {
        self.successful_reads.extend(accessed.reads.iter().cloned());
        self.record_disk_reads(
            accessed
                .disk_reads
                .iter()
                .cloned()
                .map(tola_typst::DiskReadPath::into_path),
        );
        self.missing_disk_reads.extend(
            accessed
                .disk_reads
                .iter()
                .filter(|read| read.is_missing())
                .map(|read| read.as_path().to_path_buf()),
        );
        self.record_package_checks(accessed.package_checks.iter().cloned());
        self.package_reads.extend(accessed.package_reads());
    }

    /// The package directories this attempt actually read.
    pub(crate) fn package_reads(&self) -> &[tola_typst::PackageRead] {
        &self.package_reads
    }

    pub(crate) fn successful_reads(&self) -> impl Iterator<Item = &tola_typst::FileRead> {
        self.successful_reads.iter()
    }

    pub(crate) fn take_missing_disk_reads(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.missing_disk_reads)
            .into_iter()
            .collect()
    }

    pub(super) fn record_disk_reads(&mut self, reads: impl IntoIterator<Item = PathBuf>) {
        self.disk_reads.extend(reads);
    }

    pub(crate) fn record_package_checks(
        &mut self,
        checks: impl IntoIterator<Item = tola_typst::PackageCheck>,
    ) {
        self.package_checks.extend(checks);
    }

    pub(crate) fn into_parts(self) -> (Vec<PathBuf>, Vec<tola_typst::PackageCheck>) {
        let mut checks = self.package_checks.into_iter().collect::<Vec<_>>();
        sort_package_checks(&mut checks);
        (self.disk_reads.into_iter().collect(), checks)
    }
}
