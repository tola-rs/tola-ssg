//! Physical source limits shared by file and font consumers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use typst::diag::{FileError, FileResult};

/// Physical source containment and excluded generated directories.
///
/// The default accepts host paths. Embedded bytes do not cross this boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceBoundary {
    limits: Option<Arc<SourceLimits>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SourceLimits {
    root: Option<PathBuf>,
    contained: bool,
    excluded: Vec<GeneratedSourceBoundary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GeneratedSourceBoundary {
    directory: PathBuf,
    candidate: Option<PathBuf>,
}

/// Why a physical path is not a source this boundary reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRefusal {
    /// The path is generated or published by Tola rather than a source file.
    GeneratedState,
    /// The path resolves outside the site root.
    OutsideSite,
}

impl SourceBoundary {
    /// Name diagnostics relative to `root`, optionally requiring physical containment.
    pub fn new(root: &Path, contained: bool) -> Self {
        Self {
            limits: Some(Arc::new(SourceLimits {
                root: Some(super::normalize_path(root)),
                contained,
                excluded: Vec::new(),
            })),
        }
    }

    /// Exclude a generated path, including aliases of its physical target.
    pub fn excluding(mut self, path: PathBuf) -> Self {
        Arc::make_mut(self.limits.get_or_insert_default())
            .excluded
            .push(GeneratedSourceBoundary {
                directory: path,
                candidate: None,
            });
        self
    }

    /// Permit one candidate subtree of generated storage, without relaxing other exclusions
    /// or physical root containment. A sibling or parent cannot be granted by this exception.
    pub fn excluding_except(mut self, directory: PathBuf, candidate: PathBuf) -> Self {
        Arc::make_mut(self.limits.get_or_insert_default())
            .excluded
            .push(GeneratedSourceBoundary {
                directory,
                candidate: Some(candidate),
            });
        self
    }

    /// Reject forbidden paths before opening files or accepting unsaved source bytes.
    pub fn check(&self, path: &Path) -> FileResult<()> {
        match self.refusal(path)? {
            Some(refusal) => Err(refusal.into_file_error(self.display_path(path))),
            None => Ok(()),
        }
    }

    /// The reason this path is not a source, in the boundary's own vocabulary.
    ///
    /// A caller that renders its own diagnostic receives the reason as a value instead of parsing
    /// the message a [`FileError`] has.
    pub fn refusal(&self, path: &Path) -> FileResult<Option<SourceRefusal>> {
        let Some(limits) = &self.limits else {
            return Ok(None);
        };
        let shown = self.display_path(path);
        let physical =
            physical_path(path, 128).map_err(|error| FileError::from_io(error, shown))?;
        let forbidden = limits.excluded.iter().any(|excluded| {
            let generated = physical_path(&excluded.directory, 128).ok();
            let inside = path.starts_with(&excluded.directory)
                || physical.starts_with(&excluded.directory)
                || generated
                    .as_ref()
                    .is_some_and(|generated| physical.starts_with(generated));
            if !inside {
                return false;
            }
            let permitted = excluded.candidate.as_ref().is_some_and(|candidate| {
                let Ok(source) = physical_path(candidate, 128) else {
                    return false;
                };
                generated
                    .as_ref()
                    .is_some_and(|generated| source != *generated && source.starts_with(generated))
                    && physical.starts_with(&source)
                    && (path.starts_with(candidate) || path.starts_with(&source))
            });
            !permitted
        });
        if forbidden {
            return Ok(Some(SourceRefusal::GeneratedState));
        }
        if limits.contained
            && limits
                .root
                .as_ref()
                .is_some_and(|root| !physical.starts_with(root))
        {
            return Ok(Some(SourceRefusal::OutsideSite));
        }
        Ok(None)
    }

    /// The path as a diagnostic names it: relative to the root when it is inside.
    fn display_path<'a>(&self, path: &'a Path) -> &'a Path {
        self.limits
            .as_ref()
            .and_then(|limits| limits.root.as_ref())
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or_else(|| Path::new(path.file_name().unwrap_or(path.as_os_str())))
    }
}

impl SourceRefusal {
    /// The reason in the site author's words.
    pub fn reason(self) -> &'static str {
        match self {
            Self::GeneratedState => "is generated by Tola; read the input it comes from",
            Self::OutsideSite => "is outside the site root",
        }
    }

    fn into_file_error(self, shown: &Path) -> FileError {
        FileError::Other(Some(
            format!(
                "source `{}` {}",
                shown.to_string_lossy().replace('\\', "/"),
                self.reason()
            )
            .into(),
        ))
    }
}

// Missing editor files can have dangling linked prefixes; canonicalization alone cannot contain them.
fn physical_path(path: &Path, links_left: u8) -> std::io::Result<PathBuf> {
    if let Ok(physical) = path.canonicalize() {
        return Ok(physical);
    }
    let absolute = std::path::absolute(path)?;
    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => {
                if let Ok(target) = std::fs::read_link(ancestor) {
                    if links_left == 0 {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "too many symbolic links in the source path",
                        ));
                    }
                    let mut target = ancestor.parent().unwrap_or(ancestor).join(target);
                    for name in suffix.iter().rev() {
                        target.push(name);
                    }
                    return physical_path(&target, links_left - 1);
                }
                let mut physical = ancestor.canonicalize()?;
                for name in suffix.iter().rev() {
                    physical.push(name);
                }
                return Ok(physical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = ancestor.file_name() else {
                    return Err(error);
                };
                suffix.push(name);
                ancestor = ancestor.parent().ok_or(error)?;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn candidate_grants_keep_other_limits() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("site");
        let generated = root.join("generated");
        let candidate = generated.join("candidate");
        let internal = root.join(".tola");
        std::fs::create_dir_all(&candidate).unwrap();
        std::fs::create_dir_all(&internal).unwrap();
        std::fs::write(candidate.join("allowed.typ"), "candidate source").unwrap();
        std::fs::write(generated.join("previous.typ"), "old source").unwrap();
        std::fs::write(internal.join("cached.typ"), "cache source").unwrap();
        let outside = directory.path().join("host.typ");
        std::fs::write(&outside, "host source").unwrap();
        std::os::unix::fs::symlink(internal.join("cached.typ"), candidate.join("cache.typ"))
            .unwrap();
        std::os::unix::fs::symlink(&outside, candidate.join("host.typ")).unwrap();
        std::os::unix::fs::symlink(candidate.join("allowed.typ"), generated.join("alias.typ"))
            .unwrap();
        let files = crate::FileResolver::new().with_source_boundary(
            SourceBoundary::new(&root, true)
                .excluding(internal)
                .excluding_except(generated, candidate),
        );
        assert_eq!(
            files
                .read(crate::file_id("generated/candidate/allowed.typ"), &root)
                .unwrap(),
            b"candidate source",
        );
        for path in [
            "generated/previous.typ",
            "generated/alias.typ",
            "generated/candidate/cache.typ",
            "generated/candidate/host.typ",
        ] {
            assert!(files.read(crate::file_id(path), &root).is_err(), "{path}");
        }
    }
}
