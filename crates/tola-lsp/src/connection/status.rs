//! Source-check progress, and the site files a prepared configuration reports to an editor.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use tola_build::config::ResolvedSiteConfig;
use tola_build::filesystem::normalize_existing_prefix;
use tola_diagnostic::Diagnostic;

use crate::protocol::CheckProgress;

use super::{Connection, Phase};

impl<W: Write, D: Fn(&[Diagnostic])> Connection<W, D> {
    /// The check progress a query is answered in, which only this connection can tell apart.
    pub(super) fn check_progress(&self) -> CheckProgress {
        if self.check.is_some() || self.running_check == Some(self.revision) {
            return CheckProgress::Checking;
        }
        match self.checked {
            Some(check) if check.revision == self.revision => {
                if check.usable_world {
                    CheckProgress::Checked
                } else {
                    CheckProgress::Failed
                }
            }
            _ => CheckProgress::NotChecked,
        }
    }

    pub(super) fn report_source_check(&mut self, progress: CheckProgress) -> Result<()> {
        self.report_check(progress, None)
    }

    pub(super) fn report_prepared_site(
        &mut self,
        progress: CheckProgress,
        configuration: &ResolvedSiteConfig,
    ) -> Result<()> {
        self.report_check(progress, Some(configuration))
    }

    fn report_check(
        &mut self,
        progress: CheckProgress,
        configuration: Option<&ResolvedSiteConfig>,
    ) -> Result<()> {
        if !matches!(&self.phase, Phase::Ready(workspace) if workspace.source_check_status) {
            return Ok(());
        }
        let mut status = serde_json::json!({ "revision": self.revision, "state": progress });
        if let Some(site) = configuration.and_then(|configuration| self.site_status(configuration))
        {
            status["site"] = site;
        }
        self.notify("tola/sourceCheckStatus", status)
    }

    /// Where one prepared site's files are, in the spellings the client opens them by.
    ///
    /// Sources and package roots are reported only where the boundary reads them, while the
    /// generated roots are named as they are: the client is told to ignore changes below them.
    /// Absent when the site root or its entry document has no URI, which leaves the status without
    /// a site rather than with a half-spelled one.
    fn site_status(&self, configuration: &ResolvedSiteConfig) -> Option<serde_json::Value> {
        let root = crate::uri::from_file_path(configuration.get_root()).ok()?;
        let entry = normalize_existing_prefix(&configuration.build().entry);
        let entry = self
            .open
            .view()
            .named_source(&entry)
            .map(|(uri, _)| uri.to_owned())
            .or_else(|| {
                self.client_root()
                    .address(&entry)
                    .ok()
                    .map(|uri| uri.as_str().to_owned())
            })?;
        let boundary = self.resources.source_boundary(configuration);
        let client_uri = |path: &Path| {
            crate::uri::from_file_path(path)
                .ok()
                .map(|uri| uri.as_str().to_owned())
        };
        let readable = |path: &Path| {
            let physical = normalize_existing_prefix(path);
            boundary
                .check(&physical)
                .ok()
                .and_then(|()| client_uri(&physical))
        };
        let sorted_unique = |mut uris: Vec<String>| {
            uris.sort();
            uris.dedup();
            uris
        };
        let locations = self.resources.package_locations(configuration);
        let package_roots = sorted_unique(
            locations
                .declared()
                .iter()
                .chain(locations.data())
                .chain(locations.cache())
                .filter_map(|location| readable(location.root()))
                .collect(),
        );
        let assets = configuration.assets();
        let asset_files = sorted_unique(
            assets
                .files
                .iter()
                .filter_map(|asset| readable(asset.source()))
                .collect(),
        );
        let asset_trees = sorted_unique(
            assets
                .trees
                .iter()
                .filter_map(|asset| readable(asset.source()))
                .collect(),
        );
        let generated_roots = sorted_unique(
            configuration
                .generated_exclusions()
                .into_iter()
                .map(|exclusion| exclusion.directory)
                .chain([
                    configuration
                        .get_root()
                        .join(tola_build::filesystem::INTERNAL_DIR),
                    configuration
                        .get_root()
                        .join(tola_build::filesystem::SITE_BUILD_LOCK_FILE),
                ])
                .filter_map(|path| client_uri(&normalize_existing_prefix(&path)))
                .collect(),
        );
        let config_path = normalize_existing_prefix(configuration.config_path());
        let config_versions: BTreeMap<_, _> = self
            .open
            .versions_for_path(&config_path)
            .map(|(uri, version)| (uri.to_owned(), version))
            .collect();
        Some(serde_json::json!({
            "root": root.as_str(),
            "entry": entry,
            "configVersions": config_versions,
            "packageRoots": package_roots,
            "assetSources": { "files": asset_files, "trees": asset_trees },
            "generatedRoots": generated_roots,
        }))
    }
}
