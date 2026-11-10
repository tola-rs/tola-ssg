//! Content identities for locked inputs.
//!
//! Every digest is computed from the bytes Tola read, so it can be recomputed and compared
//! without trusting a cache, a host path, or a declared digest.

use std::collections::BTreeSet;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The SHA-256 digest of the exact bytes or directory tree one input resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    /// Decode the 64 hexadecimal digits of one digest.
    pub fn from_hex(encoded: &str) -> Option<Self> {
        let bytes = hex::decode(encoded).ok()?;
        bytes.try_into().ok().map(Self)
    }

    /// The digest of `bytes`.
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// The digest's bytes.
    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

impl TryFrom<String> for Sha256Digest {
    type Error = &'static str;

    fn try_from(encoded: String) -> Result<Self, Self::Error> {
        Self::from_hex(&encoded).ok_or("a SHA-256 digest must be 64 hexadecimal digits")
    }
}

impl From<Sha256Digest> for String {
    fn from(digest: Sha256Digest) -> Self {
        hex::encode(digest.0)
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

/// The failure of reading one file's bytes for a digest.
#[derive(Debug, thiserror::Error)]
pub enum FileHashError {
    #[error("Tola could not open `{path}`")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Tola could not read `{path}`")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl FileHashError {
    fn open(path: &Path, source: std::io::Error) -> Self {
        Self::Open {
            path: path.to_path_buf(),
            source,
        }
    }

    fn unreadable(path: &Path, source: std::io::Error) -> Self {
        Self::Read {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// The digest of one file's exact bytes.
pub fn file_sha256(path: &Path) -> Result<Sha256Digest, FileHashError> {
    let mut file = std::fs::File::open(path).map_err(|source| FileHashError::open(path, source))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| FileHashError::unreadable(path, source))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(Sha256Digest(hasher.finalize().into()))
}

/// The failure of hashing one directory tree.
#[derive(Debug, thiserror::Error)]
pub enum TreeHashError {
    #[error("directory link cycle at `{path}`")]
    Cycle { path: PathBuf },
    #[error("Tola could not resolve `{path}`")]
    Resolve {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("`{path}` is not a directory that can be locked")]
    NotDirectory { path: PathBuf },
    #[error("`{path}` resolves outside `{root}`")]
    Escapes { path: PathBuf, root: PathBuf },
    #[error("Tola could not read `{path}`")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Tola could not inspect `{path}`")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("`{path}` is not a regular file; a locked input may contain only regular files")]
    NotRegularFile { path: PathBuf },
}

impl TreeHashError {
    fn unresolved(path: &Path, source: std::io::Error) -> Self {
        Self::Resolve {
            path: path.to_path_buf(),
            source,
        }
    }

    fn unreadable(path: &Path, source: std::io::Error) -> Self {
        Self::Read {
            path: path.to_path_buf(),
            source,
        }
    }

    fn uninspectable(path: &Path, source: std::io::Error) -> Self {
        Self::Inspect {
            path: path.to_path_buf(),
            source,
        }
    }

    fn escaped(path: &Path, root: &Path) -> Self {
        Self::Escapes {
            path: path.to_path_buf(),
            root: root.to_path_buf(),
        }
    }
}

/// The deterministic digest of one directory tree.
///
/// The digest covers regular files only. A file contributes its `/`-separated relative path and
/// its contents, both length-prefixed, in byte order of the paths. Empty directories contribute
/// nothing, so an unused directory cannot change an input's identity. Each logical path is
/// counted, including directory aliases. Links must remain inside `root` and cannot form cycles.
///
/// The digest covers materialized bytes, not a Git tree. A producer that materializes a remote
/// module must pin EOL conversion and disable external filter drivers, or one commit can hash
/// differently on another host.
pub fn tree_sha256(root: &Path) -> Result<Sha256Digest, TreeHashError> {
    let root = root
        .canonicalize()
        .map_err(|source| TreeHashError::unresolved(root, source))?;
    if !root.is_dir() {
        return Err(TreeHashError::NotDirectory { path: root });
    }
    let mut files = Vec::new();
    collect(&root, Path::new(""), &mut files, &mut BTreeSet::new())?;
    files.sort_by_cached_key(|file| path_key(&file.relative));
    let mut hasher = Sha256::new();
    for file in &files {
        let path = path_key(&file.relative);
        let contents = std::fs::read(&file.absolute)
            .map_err(|source| TreeHashError::unreadable(&file.absolute, source))?;
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(&path);
        hasher.update((contents.len() as u64).to_le_bytes());
        hasher.update(&contents);
    }
    Ok(Sha256Digest(hasher.finalize().into()))
}

/// One file a directory hash covers.
struct TreeFile {
    relative: PathBuf,
    absolute: PathBuf,
}

/// Walk one directory, collecting the regular files below it.
fn collect(
    root: &Path,
    relative: &Path,
    files: &mut Vec<TreeFile>,
    ancestors: &mut BTreeSet<PathBuf>,
) -> Result<(), TreeHashError> {
    let directory = root.join(relative);
    let canonical = directory
        .canonicalize()
        .map_err(|source| TreeHashError::unresolved(&directory, source))?;
    if !canonical.starts_with(root) {
        return Err(TreeHashError::escaped(&directory, root));
    }
    if !ancestors.insert(canonical.clone()) {
        return Err(TreeHashError::Cycle {
            path: relative.to_path_buf(),
        });
    }
    let mut entries = std::fs::read_dir(&directory)
        .map_err(|source| TreeHashError::unreadable(&directory, source))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|source| TreeHashError::unreadable(&directory, source))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let child = relative.join(entry.file_name());
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|source| TreeHashError::uninspectable(&path, source))?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            let target = match path.canonicalize() {
                Ok(target) => target,
                // A link to nothing holds no bytes a build can read.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => return Err(TreeHashError::unresolved(&path, source)),
            };
            if !target.starts_with(root) {
                return Err(TreeHashError::escaped(&path, root));
            }
            let target_metadata = std::fs::metadata(&target)
                .map_err(|source| TreeHashError::uninspectable(&target, source))?;
            if target_metadata.is_dir() {
                collect(root, &child, files, ancestors)?;
            } else if target_metadata.is_file() {
                files.push(TreeFile {
                    relative: child,
                    absolute: target,
                });
            } else {
                return Err(TreeHashError::NotRegularFile { path });
            }
        } else if file_type.is_dir() {
            collect(root, &child, files, ancestors)?;
        } else if file_type.is_file() {
            files.push(TreeFile {
                relative: child,
                absolute: path,
            });
        } else {
            return Err(TreeHashError::NotRegularFile { path });
        }
    }
    ancestors.remove(&canonical);
    Ok(())
}

/// One relative path as the `/`-separated bytes a hash covers.
fn path_key(relative: &Path) -> Vec<u8> {
    let mut key = Vec::new();
    for component in relative.components() {
        if !key.is_empty() {
            key.push(b'/');
        }
        key.extend_from_slice(component.as_os_str().as_encoded_bytes());
    }
    key
}
