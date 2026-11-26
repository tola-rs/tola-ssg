//! Immutable evidence for bytes consumed by a Typst world.

use std::fmt;
use std::path::{Path, PathBuf};

use typst::diag::{FileError, FileResult};

use super::super::package::{PackageCheck, PackageSpec};

/// A BLAKE3 digest of the exact bytes returned by a successful read.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentDigest([u8; 32]);

impl ContentDigest {
    /// Hash bytes with BLAKE3.
    #[inline]
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// Restore a previously recorded 32-byte BLAKE3 digest without hashing it again.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Return the raw digest bytes.
    #[inline]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Return the lowercase hexadecimal digest.
    #[inline]
    pub fn to_hex(self) -> String {
        blake3::Hash::from(self.0).to_hex().to_string()
    }
}

/// Absorb `value` into `hasher` with its length prefixed, so field boundaries cannot alias.
///
/// Persisted digests depend on this framing: it is shared by source fingerprints, output
/// manifest identity, and derivative identities, so changing it invalidates every stored digest.
#[inline]
pub fn hash_length_prefixed(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}

impl fmt::Debug for ContentDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ContentDigest")
            .field(&self.to_hex())
            .finish()
    }
}

/// Stable identity of the source that produced a read.
///
/// Root and package paths are relative to their Typst root. Provider
/// variants remain distinct because provided bytes can shadow a disk file.
/// [`Self::is_persistent`] reports whether the read can be reproduced from a path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReadLocator {
    /// A physical root file, relative to the compilation root.
    Root(PathBuf),
    /// A physical package file, relative to the package root.
    Package {
        /// Package identity.
        package: PackageSpec,
        /// Path relative to the package root.
        path: PathBuf,
    },
    /// A root file supplied by a provider.
    ProvidedRoot(PathBuf),
    /// A package file supplied by a provider.
    ProvidedPackage {
        /// Package identity.
        package: PackageSpec,
        /// Path relative to the virtual package root.
        path: PathBuf,
    },
    /// Stdin or a unique generated file. It cannot be reproduced from a path.
    NonPersistent(String),
}

impl ReadLocator {
    /// Whether this identity is stable across sessions.
    ///
    /// Locator stability does not imply that a provider returns unchanged bytes.
    #[inline]
    pub fn is_persistent(&self) -> bool {
        !matches!(self, Self::NonPersistent(_))
    }

    /// Stable ordering key used to make dependency output deterministic.
    pub(crate) fn sort_key(&self) -> String {
        match self {
            Self::Root(path) => format!("root:{}", path.display()),
            Self::Package { package, path } => format!("package:{package}:{:?}", path),
            Self::ProvidedRoot(path) => format!("provided-root:{}", path.display()),
            Self::ProvidedPackage { package, path } => {
                format!("provided-package:{package}:{:?}", path)
            }
            Self::NonPersistent(name) => format!("non-persistent:{name}"),
        }
    }
}

/// A successful read and the evidence for that exact read.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReadRecord {
    locator: ReadLocator,
    digest: ContentDigest,
}

/// Absolute path passed to a real filesystem read attempt.
///
/// The path need not exist or be canonical. The resolver records it immediately
/// before the filesystem attempt; it does not guarantee watcher coverage.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DiskReadPath {
    path: PathBuf,
    missing: bool,
}

impl DiskReadPath {
    pub(super) fn new(path: PathBuf) -> FileResult<Self> {
        if !path.is_absolute() {
            return Err(FileError::AccessDenied);
        }
        Ok(Self {
            path,
            missing: false,
        })
    }

    pub(super) fn with_error(mut self, error: &FileError) -> Self {
        self.missing = matches!(error, FileError::NotFound(_));
        self
    }

    /// Whether the filesystem reported that the attempted input was missing.
    pub fn is_missing(&self) -> bool {
        self.missing
    }

    /// Return the exact path used for the filesystem attempt.
    pub fn as_path(&self) -> &Path {
        &self.path
    }

    /// Consume the identity and return its path.
    pub fn into_path(self) -> PathBuf {
        self.path
    }
}

/// Origin of bytes returned by a successful file read.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReadOrigin {
    /// Bytes came from this actual disk read.
    Disk(DiskReadPath),
    /// Bytes came from a pure file provider.
    Provider,
    /// Bytes came from stdin or another process-local source.
    NonPersistent,
}

/// One successful read and the origin of its exact bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileRead {
    evidence: ReadRecord,
    origin: ReadOrigin,
}

impl FileRead {
    pub(super) fn new(evidence: ReadRecord, origin: ReadOrigin) -> Self {
        Self { evidence, origin }
    }

    /// Return the logical source identity and byte digest.
    pub fn evidence(&self) -> &ReadRecord {
        &self.evidence
    }

    /// Return where the successful bytes came from.
    pub fn origin(&self) -> &ReadOrigin {
        &self.origin
    }

    /// Consume this read into its source evidence and byte origin.
    pub fn into_evidence_and_origin(self) -> (ReadRecord, ReadOrigin) {
        (self.evidence, self.origin)
    }
}

impl ReadRecord {
    /// Construct evidence from a locator and the bytes returned by the read.
    #[inline]
    pub fn new(locator: ReadLocator, bytes: &[u8]) -> Self {
        Self {
            locator,
            digest: ContentDigest::of(bytes),
        }
    }

    /// Return the stable source identity.
    #[inline]
    pub fn locator(&self) -> &ReadLocator {
        &self.locator
    }

    /// Return the digest captured at read time.
    #[inline]
    pub fn digest(&self) -> ContentDigest {
        self.digest
    }

    /// Whether this evidence has a persistent logical locator.
    ///
    /// For provider-backed reads, consult the provider owner before reusing
    /// the bytes; a persistent locator alone is not a freshness guarantee.
    #[inline]
    pub fn is_persistent(&self) -> bool {
        self.locator.is_persistent()
    }
}

/// A value loaded from a file together with its read evidence.
#[derive(Debug, Clone)]
pub(crate) struct Loaded<T> {
    pub(crate) value: T,
    pub(crate) read: FileRead,
}

/// Result of one resolver operation together with every input it observed.
#[derive(Debug, Clone)]
pub(crate) struct ReadAttempt<T> {
    pub(crate) result: FileResult<T>,
    pub(crate) read: Option<FileRead>,
    pub(crate) disk_reads: Vec<DiskReadPath>,
    pub(crate) package_checks: Vec<PackageCheck>,
}

impl<T> ReadAttempt<T> {
    pub(crate) fn bare(result: FileResult<T>) -> Self {
        Self {
            result,
            read: None,
            disk_reads: Vec::new(),
            package_checks: Vec::new(),
        }
    }

    pub(crate) fn err(err: FileError, package_checks: Vec<PackageCheck>) -> Self {
        Self {
            result: Err(err),
            read: None,
            disk_reads: Vec::new(),
            package_checks,
        }
    }

    pub(crate) fn with_loaded<U>(self, res: FileResult<U>) -> ReadAttempt<Loaded<U>> {
        let result = res.and_then(|value| {
            let read = self.read.clone().ok_or_else(|| {
                FileError::Other(Some("successful file value had no read origin".into()))
            })?;
            Ok(Loaded { value, read })
        });
        ReadAttempt {
            result,
            read: self.read,
            disk_reads: self.disk_reads,
            package_checks: self.package_checks,
        }
    }

    pub(crate) fn map_loaded<U>(
        self,
        f: impl FnOnce(T) -> FileResult<U>,
    ) -> ReadAttempt<Loaded<U>> {
        let result = self.result.and_then(f).and_then(|value| {
            let read = self.read.clone().ok_or_else(|| {
                FileError::Other(Some("successful file value had no read origin".into()))
            })?;
            Ok(Loaded { value, read })
        });
        ReadAttempt {
            result,
            read: self.read,
            disk_reads: self.disk_reads,
            package_checks: self.package_checks,
        }
    }

    pub(crate) fn into_deps(self) -> crate::session::AccessedDeps {
        crate::session::AccessedDeps {
            reads: self.read.into_iter().collect(),
            disk_reads: self.disk_reads,
            package_checks: self.package_checks,
        }
    }
}

impl ReadAttempt<std::sync::Arc<[u8]>> {
    pub(crate) fn ok_provider(
        bytes: std::sync::Arc<[u8]>,
        locator: ReadLocator,
        origin: ReadOrigin,
        package_checks: Vec<PackageCheck>,
    ) -> Self {
        let evidence = ReadRecord::new(locator, &bytes);
        Self {
            result: Ok(bytes),
            read: Some(FileRead::new(evidence, origin)),
            disk_reads: Vec::new(),
            package_checks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::hash_length_prefixed;

    fn owned_fields(values: &[&[u8]]) -> Vec<Vec<u8>> {
        values.iter().map(|v| v.to_vec()).collect()
    }

    fn framed_digest(fields: &[Vec<u8>]) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        for field in fields {
            hash_length_prefixed(&mut hasher, field);
        }
        *hasher.finalize().as_bytes()
    }

    #[test]
    fn hash_length_prefixed_no_alias() {
        // Under a u8 length prefix, a 256-byte field would alias an empty field + 255 bytes;
        // 64-bit framing keeps them distinct.
        let mut wide = vec![0xFF_u8];
        wide.extend([0x7A; 255]);
        let tail = vec![0x7A_u8; 255];
        let pairs = [
            (owned_fields(&[]), owned_fields(&[b""])),
            (owned_fields(&[b"a"]), owned_fields(&[b"", b"a"])),
            (owned_fields(&[b"a"]), owned_fields(&[b"a", b""])),
            (owned_fields(&[b"ab"]), owned_fields(&[b"a", b"b"])),
            (owned_fields(&[b"a", b"bc"]), owned_fields(&[b"ab", b"c"])),
            (
                owned_fields(&[wide.as_slice()]),
                owned_fields(&[b"", tail.as_slice()]),
            ),
        ];

        for (lhs, rhs) in pairs {
            assert_ne!(
                framed_digest(&lhs),
                framed_digest(&rhs),
                "{lhs:?} vs {rhs:?}"
            );
        }
    }
}
