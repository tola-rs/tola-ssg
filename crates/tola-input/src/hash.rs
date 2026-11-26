//! Content digests for locked inputs.

use std::collections::BTreeSet;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// SHA-256 digest of exact file bytes or a directory tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    /// Decode 64 hexadecimal digits into a digest.
    pub fn from_hex(hex_str: &str) -> Option<Self> {
        let bytes = hex::decode(hex_str).ok()?;
        bytes.try_into().ok().map(Self)
    }

    /// Compute the SHA-256 digest of `bytes`.
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// Raw 32-byte digest.
    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

impl TryFrom<String> for Sha256Digest {
    type Error = &'static str;

    fn try_from(hex_str: String) -> Result<Self, Self::Error> {
        Self::from_hex(&hex_str).ok_or("a SHA-256 digest must be 64 hexadecimal digits")
    }
}

impl From<Sha256Digest> for String {
    fn from(sha: Sha256Digest) -> Self {
        hex::encode(sha.0)
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

/// Failure hashing a single file.
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

/// Compute the SHA-256 digest of a file's exact bytes.
pub fn file_sha256(path: &Path) -> Result<Sha256Digest, FileHashError> {
    let mut file = std::fs::File::open(path).map_err(|source| FileHashError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|source| FileHashError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(Sha256Digest(hasher.finalize().into()))
}

/// Failure hashing a directory tree.
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
    fn resolve(path: &Path, source: std::io::Error) -> Self {
        Self::Resolve {
            path: path.to_path_buf(),
            source,
        }
    }

    fn read(path: &Path, source: std::io::Error) -> Self {
        Self::Read {
            path: path.to_path_buf(),
            source,
        }
    }

    fn inspect(path: &Path, source: std::io::Error) -> Self {
        Self::Inspect {
            path: path.to_path_buf(),
            source,
        }
    }

    fn escape(path: &Path, root: &Path) -> Self {
        Self::Escapes {
            path: path.to_path_buf(),
            root: root.to_path_buf(),
        }
    }
}

/// Compute a deterministic SHA-256 digest of regular files under `root`.
///
/// Hashes length-prefixed `/`-separated relative path bytes and file contents in sorted path order.
/// Empty directories are ignored; symlinks must stay within `root` without forming cycles.
pub fn tree_sha256(root: &Path) -> Result<Sha256Digest, TreeHashError> {
    let root = root
        .canonicalize()
        .map_err(|err| TreeHashError::resolve(root, err))?;
    if !root.is_dir() {
        return Err(TreeHashError::NotDirectory { path: root });
    }
    let mut files = Vec::new();
    collect(&root, Path::new(""), &mut files, &mut BTreeSet::new())?;
    files.sort_by_cached_key(|f| path_key(&f.rel));
    let mut hasher = Sha256::new();
    for f in &files {
        let key = path_key(&f.rel);
        let bytes = std::fs::read(&f.abs).map_err(|err| TreeHashError::read(&f.abs, err))?;
        hasher.update((key.len() as u64).to_le_bytes());
        hasher.update(&key);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(Sha256Digest(hasher.finalize().into()))
}

struct TreeFile {
    rel: PathBuf,
    abs: PathBuf,
}

fn collect(
    root: &Path,
    rel: &Path,
    files: &mut Vec<TreeFile>,
    ancestors: &mut BTreeSet<PathBuf>,
) -> Result<(), TreeHashError> {
    let dir = root.join(rel);
    let canonical = dir
        .canonicalize()
        .map_err(|err| TreeHashError::resolve(&dir, err))?;
    if !canonical.starts_with(root) {
        return Err(TreeHashError::escape(&dir, root));
    }
    if !ancestors.insert(canonical.clone()) {
        return Err(TreeHashError::Cycle {
            path: rel.to_path_buf(),
        });
    }
    let mut entries = std::fs::read_dir(&dir)
        .map_err(|err| TreeHashError::read(&dir, err))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|err| TreeHashError::read(&dir, err))?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let child = rel.join(entry.file_name());
        let path = entry.path();
        let meta =
            std::fs::symlink_metadata(&path).map_err(|err| TreeHashError::inspect(&path, err))?;
        let ft = meta.file_type();
        let (is_dir, is_file, resolved) = if ft.is_symlink() {
            let target = match path.canonicalize() {
                Ok(target) => target,
                // Broken symlinks have no readable bytes.
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => return Err(TreeHashError::resolve(&path, err)),
            };
            if !target.starts_with(root) {
                return Err(TreeHashError::escape(&path, root));
            }
            let target_meta =
                std::fs::metadata(&target).map_err(|err| TreeHashError::inspect(&target, err))?;
            (target_meta.is_dir(), target_meta.is_file(), target)
        } else {
            (ft.is_dir(), ft.is_file(), path.clone())
        };

        if is_dir {
            collect(root, &child, files, ancestors)?;
        } else if is_file {
            files.push(TreeFile {
                rel: child,
                abs: resolved,
            });
        } else {
            return Err(TreeHashError::NotRegularFile { path });
        }
    }
    ancestors.remove(&canonical);
    Ok(())
}

fn path_key(rel: &Path) -> Vec<u8> {
    let mut key = Vec::new();
    for comp in rel.components() {
        if !key.is_empty() {
            key.push(b'/');
        }
        key.extend_from_slice(comp.as_os_str().as_encoded_bytes());
    }
    key
}
