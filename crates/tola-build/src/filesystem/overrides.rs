//! Immutable unsaved text, identified by the same physical file across producers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cancellation::BuildCancellation;
use tola_typst::ContentDigest;

#[derive(Debug, Default)]
pub(crate) struct SourceOverrides {
    logical_paths: BTreeSet<PathBuf>,
    sources: BTreeMap<PathBuf, SourceOverride>,
}

#[derive(Debug)]
pub(crate) struct SourceOverride {
    pub(crate) text: Arc<str>,
    pub(crate) digest: ContentDigest,
}

#[derive(Debug, thiserror::Error)]
enum SourceOverrideError {
    #[error("an unsaved source path must be an absolute file path")]
    Relative,
    #[error("an unsaved source is a directory, not a file")]
    Directory,
    #[error("two unsaved sources supply different text for the same file")]
    Conflicting,
}

impl SourceOverrides {
    pub(crate) fn new(
        sources: Vec<(PathBuf, Arc<str>)>,
        cancellation: &BuildCancellation,
    ) -> anyhow::Result<Self> {
        let mut overrides = Self::default();
        for (path, text) in sources {
            cancellation.ensure_active()?;
            if !path.is_absolute() {
                return Err(SourceOverrideError::Relative.into());
            }
            let logical = super::lexical_path_identity(&path);
            let physical = super::normalize_existing_prefix(&logical);
            if physical.is_dir() {
                return Err(SourceOverrideError::Directory.into());
            }
            if let Some(previous) = overrides.sources.get(&physical) {
                if previous.text != text {
                    return Err(SourceOverrideError::Conflicting.into());
                }
            } else {
                let digest = ContentDigest::of(text.as_bytes());
                overrides
                    .sources
                    .insert(physical.clone(), SourceOverride { text, digest });
            }
            overrides.logical_paths.insert(logical);
        }
        cancellation.ensure_active()?;
        Ok(overrides)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    pub(crate) fn get_physical(&self, path: &Path) -> Option<&SourceOverride> {
        self.sources.get(path)
    }

    /// Both spellings participate in discovery, including a missing child of a symlinked root.
    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.logical_paths
            .iter()
            .chain(self.sources.keys())
            .map(PathBuf::as_path)
    }

    pub(crate) fn sources(&self) -> impl Iterator<Item = (&Path, &SourceOverride)> {
        self.sources
            .iter()
            .map(|(path, source)| (path.as_path(), source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicting_spellings_are_rejected() {
        let directory = tempfile::TempDir::new().unwrap();
        let source = directory.path().join("page.typ");
        let error = SourceOverrides::new(
            vec![
                (source.clone(), Arc::from("one")),
                (
                    directory.path().join("nested/../page.typ"),
                    Arc::from("two"),
                ),
            ],
            &BuildCancellation::new(),
        )
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<SourceOverrideError>(),
            Some(SourceOverrideError::Conflicting)
        ));
        assert!(!source.exists());
    }

    #[cfg(unix)]
    #[test]
    fn aliased_root_shares_override_text() {
        let directory = tempfile::TempDir::new().unwrap();
        let physical = directory.path().join("content");
        let alias = directory.path().join("alias");
        std::fs::create_dir(&physical).unwrap();
        std::os::unix::fs::symlink(&physical, &alias).unwrap();
        let text = Arc::<str>::from("unsaved");
        let overrides = SourceOverrides::new(
            vec![
                (alias.join("new.typ"), Arc::clone(&text)),
                (physical.join("new.typ"), Arc::clone(&text)),
            ],
            &BuildCancellation::new(),
        )
        .unwrap();
        assert_eq!(overrides.sources().count(), 1);
        let normalized = crate::filesystem::normalize_existing_prefix(&physical.join("new.typ"));
        let resolved = overrides.get_physical(&normalized).unwrap();
        assert!(Arc::ptr_eq(&resolved.text, &text));
        assert!(
            SourceOverrides::new(
                vec![
                    (alias.join("new.typ"), Arc::from("one")),
                    (physical.join("new.typ"), Arc::from("two"))
                ],
                &BuildCancellation::new(),
            )
            .is_err()
        );
    }
}
