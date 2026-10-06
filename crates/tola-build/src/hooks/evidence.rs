//! Evidence for declared hook outputs: what a hook produced, and whether it still holds.

use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::cancellation::OptionalCancellation;
use crate::config::ResolvedSiteConfig;
use crate::config::section::build::BeforeBuildHookConfig;
use crate::config::section::build::hooks::HookStage;

/// Type- and content-sensitive evidence for one declared input produced by a
/// successful before-build hook chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookOutputEvidence {
    kind: crate::filesystem::FilesystemSourceKind,
    source_identity: crate::filesystem::FilesystemSourceIdentity,
    physical_content: crate::filesystem::PathFingerprint,
}

impl HookOutputEvidence {
    /// Promote a snapshot only after the hook and all declared-output checks succeed.
    pub(super) fn from_successful_snapshot(
        snapshot: HookOutputSnapshot,
        cancellation: Option<&crate::cancellation::BuildCancellation>,
    ) -> Result<Self> {
        cancellation.ensure_active_if_present()?;
        let kind = snapshot.kind.ok_or_else(|| {
            let declared = snapshot.source_identity.logical_path().display();
            super::hook_contract_error(
                format!(
                    "the declared `outputs` path `{declared}` is not a regular file or directory"
                ),
                "declare a regular file or a directory",
            )
        })?;
        Ok(Self {
            kind,
            physical_content: crate::filesystem::snapshot_fingerprint(&snapshot.entries),
            source_identity: snapshot.source_identity,
        })
    }

    pub fn logical_path(&self) -> &Path {
        self.source_identity.logical_path()
    }

    pub fn physical_path(&self) -> &Path {
        self.source_identity.physical_path()
    }

    pub fn covers_path(&self, path: &Path) -> bool {
        let path = crate::filesystem::lexical_path_identity(path);
        [self.logical_path(), self.physical_path()]
            .into_iter()
            .any(|boundary| match self.kind {
                crate::filesystem::FilesystemSourceKind::File => path == boundary,
                crate::filesystem::FilesystemSourceKind::Tree => {
                    path == boundary || path.starts_with(boundary)
                }
            })
    }

    pub(crate) fn watch_evidence(outputs: &[Self]) -> crate::filesystem::FilesystemWatchEvidence {
        crate::filesystem::FilesystemWatchEvidence::from_sources(
            outputs
                .iter()
                .map(|output| (output.kind, output.source_identity.clone())),
        )
    }

    pub fn is_current(&self) -> bool {
        self.observe_current(None).unwrap_or(false)
    }

    pub fn is_current_with_cancellation(
        &self,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> Result<bool> {
        self.observe_current(Some(cancellation))
            .map_err(anyhow::Error::new)
    }

    fn observe_current(
        &self,
        cancellation: Option<&crate::cancellation::BuildCancellation>,
    ) -> Result<bool, crate::cancellation::BuildCancelled> {
        cancellation.ensure_active_if_present()?;
        let identity = crate::filesystem::FilesystemSourceIdentity::from_path(self.logical_path());
        let current = identity == self.source_identity
            && crate::filesystem::path_fingerprint(self.physical_path(), cancellation)
                .is_ok_and(|fingerprint| fingerprint == self.physical_content)
            && crate::filesystem::FilesystemSourceIdentity::from_path(self.logical_path())
                == identity;
        cancellation.ensure_active_if_present()?;
        Ok(current)
    }
}

/// Declared-file observations from successful hooks in one before-build attempt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceHookOutputs {
    pub(super) outputs: Vec<HookOutputEvidence>,
}

impl SourceHookOutputs {
    pub fn outputs(&self) -> &[HookOutputEvidence] {
        &self.outputs
    }
}

pub(super) fn validate_declared_output_boundaries(
    hook: &BeforeBuildHookConfig,
    config: &ResolvedSiteConfig,
    published_root: &Path,
) -> Result<Vec<crate::filesystem::FilesystemSourceIdentity>> {
    let named_hook = super::hook_identity(HookStage::BeforeBuild, &hook.name);
    let root = config.get_root();
    let boundaries = crate::filesystem::DeclaredOutputBoundaries::for_site(
        root,
        &config.config_path,
        published_root,
        config.vendor_workspace.as_deref(),
    );
    let mut identities = Vec::with_capacity(hook.generates.len());
    for declared in &hook.generates {
        let identity = boundaries.resolve(declared);
        // Symlinks may have moved declared outputs across boundaries since config loading.
        match boundaries.violation(&identity, &identities) {
            None => {}
            Some(crate::filesystem::DeclaredOutputViolation::OutsideSite) => {
                return Err(super::hook_contract_error(
                    format!(
                        "{named_hook} declares `outputs` path `{}` outside the site root",
                        declared.display()
                    ),
                    "declare a path inside the site",
                ));
            }
            Some(crate::filesystem::DeclaredOutputViolation::InsideReserved { kind, path }) => {
                return Err(super::hook_contract_error(
                    format!(
                        "{named_hook} declares `outputs` path `{}` that intersects {kind} `{}`",
                        declared.display(),
                        crate::filesystem::display_path(&path, root),
                    ),
                    "declare a path elsewhere in the site",
                ));
            }
            Some(crate::filesystem::DeclaredOutputViolation::InsidePublished) => {
                return Err(super::hook_contract_error(
                    format!(
                        "{named_hook} declares `outputs` path `{}` that intersects `build.publish-dir` `{}`",
                        declared.display(),
                        crate::filesystem::display_path(published_root, root),
                    ),
                    "declare a path elsewhere in the site",
                ));
            }
            Some(crate::filesystem::DeclaredOutputViolation::OwnedElsewhere(_)) => {
                return Err(super::hook_contract_error(
                    format!(
                        "{named_hook} declares `outputs` path `{}` that overlaps another entry",
                        declared.display()
                    ),
                    "keep one of the two",
                ));
            }
        }
        identities.push(identity);
    }
    for other in config
        .build
        .hooks
        .before_build
        .iter()
        .filter(|other| other.enable && !std::ptr::eq(*other, hook))
    {
        for declared in &other.generates {
            let other_identity = boundaries.resolve(declared);
            if identities
                .iter()
                .any(|identity| identity.intersects(&other_identity))
            {
                return Err(super::hook_contract_error(
                    format!(
                        "{named_hook} writes a path that overlaps `{}` from {}",
                        declared.display(),
                        super::hook_identity(HookStage::BeforeBuild, &other.name)
                    ),
                    "keep one of the two `outputs` entries",
                ));
            }
        }
    }
    Ok(identities)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HookOutputSnapshot {
    kind: Option<crate::filesystem::FilesystemSourceKind>,
    pub(super) source_identity: crate::filesystem::FilesystemSourceIdentity,
    pub(super) entries: BTreeMap<PathBuf, crate::filesystem::PathFingerprint>,
}

impl HookOutputSnapshot {
    fn capture(
        source_identity: crate::filesystem::FilesystemSourceIdentity,
        cancellation: Option<&crate::cancellation::BuildCancellation>,
    ) -> Result<Self> {
        let crate::filesystem::PathSnapshot { kind, entries } =
            crate::filesystem::entry_snapshot(source_identity.physical_path(), cancellation)?;
        let current_identity =
            crate::filesystem::FilesystemSourceIdentity::from_path(source_identity.logical_path());
        if current_identity != source_identity {
            let declared = source_identity.logical_path().display();
            return Err(super::hook_contract_error(
                format!("the declared `outputs` path `{declared}` changed while the build ran"),
                "write it only from that hook",
            ));
        }
        Ok(Self {
            kind,
            source_identity,
            entries,
        })
    }

    fn observe_current(
        &self,
        cancellation: Option<&crate::cancellation::BuildCancellation>,
    ) -> Result<bool, crate::cancellation::BuildCancelled> {
        cancellation.ensure_active_if_present()?;
        let identity = crate::filesystem::FilesystemSourceIdentity::from_path(
            self.source_identity.logical_path(),
        );
        let current = Self::capture(identity, cancellation);
        cancellation.ensure_active_if_present()?;
        Ok(current.is_ok_and(|current| current == *self))
    }

    fn covers_path(&self, path: &Path) -> bool {
        [
            self.source_identity.logical_path(),
            self.source_identity.physical_path(),
        ]
        .into_iter()
        .any(|boundary| path == boundary || path.starts_with(boundary))
    }
}

pub(super) fn declared_output_snapshots(
    hook: &BeforeBuildHookConfig,
    config: &ResolvedSiteConfig,
    cancellation: Option<&crate::cancellation::BuildCancellation>,
) -> Result<Vec<HookOutputSnapshot>> {
    validate_declared_output_boundaries(hook, config, &config.build.publish_dir)?
        .into_iter()
        .map(|identity| HookOutputSnapshot::capture(identity, cancellation))
        .collect()
}

/// Observations of declared inputs left by a failed before-build chain.
///
/// These observations only identify filesystem events already represented by
/// the failure. They do not establish successful generation or permit reuse.
#[derive(Clone, Debug)]
pub struct FailedHookOutputs {
    outputs: Vec<HookOutputSnapshot>,
}

impl FailedHookOutputs {
    pub(super) fn capture(
        paths: &BTreeSet<PathBuf>,
        cancellation: Option<&crate::cancellation::BuildCancellation>,
    ) -> Result<Option<Self>> {
        let mut outputs = Vec::with_capacity(paths.len());
        for path in paths {
            cancellation.ensure_active_if_present()?;
            let identity = crate::filesystem::FilesystemSourceIdentity::from_path(path);
            // An unreadable boundary supplies no evidence for event suppression.
            // The command failure remains the diagnostic for this attempt.
            if let Ok(output) = HookOutputSnapshot::capture(identity, cancellation) {
                outputs.push(output);
            }
        }
        cancellation.ensure_active_if_present()?;
        Ok((!outputs.is_empty()).then_some(Self { outputs }))
    }

    pub fn is_current(&self) -> bool {
        self.outputs
            .iter()
            .all(|output| output.observe_current(None).unwrap_or(false))
    }

    /// Retain current failure observations intersecting the supplied event paths.
    pub fn current_for_paths(
        &self,
        paths: &[PathBuf],
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> Result<Option<Self>, crate::cancellation::BuildCancelled> {
        cancellation.ensure_active()?;
        let paths = paths
            .iter()
            .map(|path| crate::filesystem::lexical_path_identity(path))
            .collect::<Vec<_>>();
        let mut outputs = Vec::new();
        for output in &self.outputs {
            cancellation.ensure_active()?;
            if paths.iter().any(|path| output.covers_path(path))
                && output.observe_current(Some(cancellation))?
            {
                outputs.push(output.clone());
            }
        }
        cancellation.ensure_active()?;
        Ok((!outputs.is_empty()).then_some(Self { outputs }))
    }

    pub fn covers_path(&self, path: &Path) -> bool {
        let path = crate::filesystem::lexical_path_identity(path);
        self.outputs.iter().any(|output| output.covers_path(&path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn file_evidence(path: PathBuf) -> HookOutputEvidence {
        let identity = crate::filesystem::FilesystemSourceIdentity::from_path(&path);
        let snapshot = HookOutputSnapshot::capture(identity, None).unwrap();
        HookOutputEvidence::from_successful_snapshot(snapshot, None).unwrap()
    }
    #[test]
    fn cancellation_keeps_watch_evidence() {
        let directory = TempDir::new().unwrap();
        let output = directory.path().join("generated");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("entry.txt"), "generated").unwrap();
        let evidence = file_evidence(output);
        let canceller = crate::cancellation::BuildCanceller::new();
        let cancellation = canceller.token();
        canceller.cancel();

        let error = evidence
            .is_current_with_cancellation(&cancellation)
            .unwrap_err();

        assert!(
            error
                .downcast_ref::<crate::cancellation::BuildCancelled>()
                .is_some()
        );
        assert!(evidence.is_current());
    }
    /// Evidence stays current while the snapshotted output is unchanged and goes stale
    /// on any change, whether the output is one file or a tree.
    #[test]
    fn evidence_goes_stale_when_output_changes() {
        struct Case {
            name: &'static str,
            /// Create the output under `root` and return it.
            create: fn(&Path) -> PathBuf,
            /// Rewrite the output with the bytes it already holds.
            rewrite: fn(&Path),
            /// Change the output after its evidence was captured.
            change: fn(&Path),
        }
        let cases = [
            Case {
                name: "file",
                create: |root| {
                    let output = root.join("generated.css");
                    fs::write(&output, "stable").unwrap();
                    output
                },
                rewrite: |root| fs::write(root.join("generated.css"), "stable").unwrap(),
                change: |root| fs::write(root.join("generated.css"), "external").unwrap(),
            },
            Case {
                name: "tree",
                create: |root| {
                    let output = root.join("generated");
                    fs::create_dir(&output).unwrap();
                    fs::write(output.join("site.css"), "stable").unwrap();
                    output
                },
                rewrite: |root| fs::write(root.join("generated/site.css"), "stable").unwrap(),
                change: |root| fs::write(root.join("generated/extra.css"), "external").unwrap(),
            },
        ];
        for case in cases {
            let dir = TempDir::new().unwrap();
            let root = dir.path();
            let evidence = file_evidence((case.create)(root));

            (case.rewrite)(root);
            assert!(evidence.is_current(), "{}", case.name);
            (case.change)(root);
            assert!(!evidence.is_current(), "{}", case.name);
        }
    }
    #[cfg(unix)]
    #[test]
    fn evidence_rejects_symlink_retarget() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("first.css"), "same").unwrap();
        fs::write(dir.path().join("second.css"), "same").unwrap();
        let output = dir.path().join("generated.css");
        symlink("first.css", &output).unwrap();
        let evidence = file_evidence(output.clone());

        fs::write(dir.path().join("first.css"), "changed").unwrap();
        assert!(!evidence.is_current());
        fs::write(dir.path().join("first.css"), "same").unwrap();
        assert!(evidence.is_current());

        fs::remove_file(&output).unwrap();
        symlink("second.css", &output).unwrap();
        assert!(!evidence.is_current());
    }
}
