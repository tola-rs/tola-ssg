//! Font loading and caching.
//!
//! Share a [`FontStore`] across compilations with the same font configuration.
//!
//! # Font Sources
//!
//! Fonts are searched in order:
//! 1. Custom paths provided at initialization (e.g., site fonts)
//! 2. System fonts (if enabled)
//! 3. Embedded fonts (if enabled via `embed-fonts` feature)

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use typst::diag::FileError;
use typst::foundations::Bytes;
use typst::text::{Font, FontInfo};
use typst_kit::fonts::{self, FontSource, FontStore as KitFontStore};

use crate::BundleCancellation;
use crate::world::file::ContentDigest;

#[cfg(feature = "embed-fonts")]
mod embedded;

/// Bound read-buffer allocation and cancellation latency during font I/O.
const FONT_READ_CHUNK_BYTES: usize = 64 * 1024;

/// Options for font initialization.
///
/// # Example
///
/// ```ignore
/// use tola_typst::{FontOptions, FontStore};
/// use std::path::Path;
///
/// let options = FontOptions::new()
///     .with_system_fonts(true)
///     .with_embedded_fonts(true)
///     .with_custom_paths(&[
///         Path::new("assets/fonts"),
///         Path::new("content/fonts"),
///     ]);
///
/// let fonts = FontStore::with_options(options);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontOptions {
    /// Whether to include system fonts.
    pub include_system_fonts: bool,
    /// Whether to include embedded fonts (New Computer Modern, etc.).
    /// Only effective when `embed-fonts` feature is enabled.
    pub include_embedded_fonts: bool,
    /// Custom font directories to search.
    pub custom_paths: Vec<PathBuf>,
    /// Physical restrictions applied before discovery and every lazy font read.
    pub source_boundary: crate::world::SourceBoundary,
}

impl FontOptions {
    /// Enable system fonts and, with `embed-fonts`, embedded fonts; no custom paths.
    pub fn new() -> Self {
        Self {
            include_system_fonts: true,
            include_embedded_fonts: true,
            custom_paths: Vec::new(),
            source_boundary: crate::world::SourceBoundary::default(),
        }
    }

    /// Set whether to include system fonts.
    ///
    /// Skipping system discovery can speed up initialization when configured fonts suffice.
    pub fn with_system_fonts(mut self, include: bool) -> Self {
        self.include_system_fonts = include;
        self
    }

    /// Set whether to include embedded fonts.
    ///
    /// Embedded fonts include New Computer Modern Math and other default fonts.
    /// Only effective when `embed-fonts` feature is enabled.
    pub fn with_embedded_fonts(mut self, include: bool) -> Self {
        self.include_embedded_fonts = include;
        self
    }

    /// Set custom font paths to search.
    pub fn with_custom_paths(mut self, paths: &[&Path]) -> Self {
        self.custom_paths = paths.iter().map(|p| p.to_path_buf()).collect();
        self
    }

    /// Restrict physical font sources without excluding embedded fonts.
    pub fn with_source_boundary(mut self, boundary: crate::world::SourceBoundary) -> Self {
        self.source_boundary = boundary;
        self
    }

    /// Add a single custom font path.
    pub fn add_path(mut self, path: impl AsRef<Path>) -> Self {
        self.custom_paths.push(path.as_ref().to_path_buf());
        self
    }
}

impl Default for FontOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// Failure to load or preserve one generation's font resources.
#[derive(Debug, Clone, thiserror::Error)]
pub enum FontLoadError {
    /// A configured directory or font file could not be read.
    #[error("could not read the font file `{}`", path.display())]
    File {
        /// Absolute source path.
        path: PathBuf,
        /// Structured filesystem or decoding failure.
        #[source]
        error: FileError,
    },
    /// A lazy font read no longer matches the store that selected it.
    #[error("a font file changed while Tola read it")]
    Changed {
        /// Changed font path.
        path: PathBuf,
    },
    /// The caller cancelled font resource preparation.
    #[error("font loading cancelled")]
    Cancelled,
}

impl crate::diagnostic::CancelledError for FontLoadError {
    fn from_cancellation() -> Self {
        Self::Cancelled
    }
}

impl FontLoadError {
    /// File or directory associated with this failure.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::File { path, .. } | Self::Changed { path } => Some(path),
            Self::Cancelled => None,
        }
    }

    /// Whether the caller cancelled preparation or validation.
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

/// Shared fonts and the source evidence belonging to one resource generation.
///
/// Configured directories retain complete membership and byte digests, and a used font from one
/// is checked against that metadata. System discovery is fixed for this store's lifetime and its
/// fonts are compared by identity alone, since no configured digest describes them. Bytes are
/// read lazily, when Typst requests a selected face.
pub struct FontStore {
    options: FontOptions,
    fonts: OnceLock<LoadedFonts>,
    loading: Mutex<()>,
}

impl FontStore {
    /// Configure system and optional embedded fonts without loading them.
    pub fn new() -> Self {
        Self::with_options(FontOptions::new())
    }

    /// Configure a new independent font-resource generation.
    pub fn with_options(options: FontOptions) -> Self {
        Self {
            options,
            fonts: OnceLock::new(),
            loading: Mutex::new(()),
        }
    }

    /// Configure additional font directories without loading them.
    pub fn with_paths(paths: &[&Path]) -> Self {
        Self::with_options(FontOptions::new().with_custom_paths(paths))
    }

    /// Options assigned to this resource generation.
    pub fn options(&self) -> &FontOptions {
        &self.options
    }

    /// Load the fonts with cancellation around directory and file reads.
    ///
    /// Cancelled or failed preparation does not initialize the store. Official
    /// system-font discovery is checked before and after its indivisible call.
    pub fn load(&self, cancellation: &BundleCancellation) -> Result<&KitFontStore, FontLoadError> {
        cancellation.ensure_active_as()?;
        if self.fonts.get().is_none() {
            let _loading = self.loading.lock();
            cancellation.ensure_active_as()?;
            if self.fonts.get().is_none() {
                let fonts = load_fonts(&self.options, cancellation)?;
                cancellation.ensure_active_as()?;
                assert!(
                    self.fonts.set(fonts).is_ok(),
                    "the font store has one loader"
                );
            }
        }
        if let Some(error) = self.read_failure() {
            return Err(error);
        }
        Ok(self.prepared())
    }

    /// Load the fonts now while leaving individual fonts lazy.
    pub fn preload(self) -> Result<Self, FontLoadError> {
        self.load(&BundleCancellation::new())?;
        Ok(self)
    }

    /// Whether this store has loaded its fonts.
    pub fn is_loaded(&self) -> bool {
        self.fonts.get().is_some()
    }

    /// Recheck configured directories and actual reads of selected system fonts.
    ///
    /// This does not rediscover system fonts. A different store is a
    /// different generation even when its font names and metadata compare equal.
    pub fn is_current(&self, cancellation: &BundleCancellation) -> Result<bool, FontLoadError> {
        cancellation.ensure_active_as()?;
        let Some(fonts) = self.fonts.get() else {
            return Ok(false);
        };
        if self.read_failure().is_some() {
            return Ok(false);
        }
        let current = match scan_configured_fonts(
            &self.options,
            cancellation,
            &fonts.configured.files,
            |_, _| Ok(()),
        ) {
            Ok(current) => current,
            Err(FontLoadError::Cancelled) => return Err(FontLoadError::Cancelled),
            Err(_) => return Ok(false),
        };
        if current != fonts.configured {
            return Ok(false);
        }
        for path in &fonts.system_paths {
            let file = &fonts.files[path];
            if file.expected.is_some() {
                continue;
            }
            let Some(Ok(observed)) = file.bytes.get() else {
                continue;
            };
            // No configuration describes a system font, so its bytes are the only evidence.
            if observed
                .identity
                .as_ref()
                .is_some_and(|identity| identity.describes(path))
            {
                continue;
            }
            check_font_source(&self.options.source_boundary, path)?;
            match read_font_bytes(path, Some(cancellation)) {
                Ok(current) if current.digest == observed.digest => {}
                Err(FontLoadError::Cancelled) => return Err(FontLoadError::Cancelled),
                _ => return Ok(false),
            }
        }
        cancellation.ensure_active_as()?;
        Ok(self.read_failure().is_none())
    }

    /// Sorted physical paths of completed lazy font-read attempts.
    ///
    /// Failed attempts are included so a host can observe their recovery.
    /// This is not an inventory of every installed system font or directory.
    pub fn read_paths(&self) -> Vec<PathBuf> {
        self.fonts
            .get()
            .into_iter()
            .flat_map(|fonts| &fonts.files)
            .filter(|(_, file)| file.bytes.get().is_some())
            .map(|(path, _)| path.clone())
            .collect()
    }

    /// Number of faces in the loaded fonts.
    pub fn font_count(&self) -> Option<usize> {
        self.fonts.get().map(|fonts| {
            (0..)
                .take_while(|&index| fonts.native.book().info(index).is_some())
                .count()
        })
    }

    /// Number of families in the loaded fonts.
    pub fn family_count(&self) -> Option<usize> {
        self.fonts
            .get()
            .map(|fonts| fonts.native.book().families().count())
    }

    /// Whether a path has a font format supported by the native font scanner.
    pub fn accepts_path(path: &Path) -> bool {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                ["ttf", "otf", "ttc", "otc"]
                    .iter()
                    .any(|supported| extension.eq_ignore_ascii_case(supported))
            })
    }

    pub(crate) fn prepared(&self) -> &KitFontStore {
        &self
            .fonts
            .get()
            .expect("world construction loads its fonts")
            .native
    }

    pub(crate) fn read_failure(&self) -> Option<FontLoadError> {
        let fonts = self.fonts.get()?;
        fonts
            .files
            .values()
            .find_map(|file| file.failure.get().cloned())
    }
}

impl Default for FontStore {
    fn default() -> Self {
        Self::new()
    }
}

struct LoadedFonts {
    native: KitFontStore,
    configured: FontInventory,
    files: BTreeMap<PathBuf, Arc<FontFile>>,
    system_paths: BTreeSet<PathBuf>,
}

#[derive(PartialEq, Eq)]
struct FontInventory {
    roots: Vec<(PathBuf, Option<PathBuf>)>,
    files: BTreeMap<PathBuf, FontFileIdentity>,
}

/// What one configured font file was when its digest was computed.
///
/// Revalidating a generation compares this identity first: an unchanged file
/// cannot have changed its content digest, so the bytes are not read again.
#[derive(PartialEq, Eq)]
struct FontFileIdentity {
    length: u64,
    modified: Option<std::time::SystemTime>,
    digest: ContentDigest,
}

impl FontFileIdentity {
    /// The identity of `path` as it is on disk now, describing `digest`.
    fn of(path: &Path, digest: ContentDigest) -> Result<Self, FontLoadError> {
        let metadata = fs::metadata(path).map_err(|error| font_io_error(path, error))?;
        Ok(Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            digest,
        })
    }

    /// Whether the file on disk is still the file this identity describes.
    fn describes(&self, path: &Path) -> bool {
        match fs::metadata(path) {
            Ok(metadata) => {
                metadata.len() == self.length && metadata.modified().ok() == self.modified
            }
            Err(_) => false,
        }
    }

    /// This identity as the file is now, refusing one that changed while it was validated.
    fn reobserved(&self, path: &Path) -> Result<Self, FontLoadError> {
        let observed = Self::of(path, self.digest)?;
        if observed.length != self.length || observed.modified != self.modified {
            return Err(FontLoadError::Changed {
                path: path.to_path_buf(),
            });
        }
        Ok(observed)
    }
}

struct FontFile {
    path: PathBuf,
    expected: Option<ContentDigest>,
    bytes: OnceLock<Result<FontFileBytes, FontLoadError>>,
    failure: OnceLock<FontLoadError>,
    boundary: crate::world::SourceBoundary,
}

struct FontFileBytes {
    bytes: Bytes,
    digest: ContentDigest,
    identity: Option<FontFileIdentity>,
}

/// The face a font file held when this store discovered it.
///
/// A lazy read accepts the file only while it still parses to the face this identity was taken
/// from: bytes that parse to that face keep it, a face that changed does not. The identity is
/// BLAKE3 over the face's own [`Hash`] encoding, which has a face in 32 bytes instead of a
/// second copy of it, whose unicode coverage dominates its size, and which covers a field Typst
/// adds to [`FontInfo`] without a list here to keep in step.
///
/// A variation axis is compared by bit pattern, where [`FontInfo`] equality compares floats.
///
/// This identity lives in the process that computed it: std's [`Hash`] encoding fixes integer
/// width and byte order for this host's own machine code, not the length-prefixed framing
/// [`hash_length_prefixed`](crate::hash_length_prefixed) fixes for persisted digests. It is never
/// written down and never compared against another host's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FaceIdentity(ContentDigest);

impl FaceIdentity {
    /// The identity of the face `info` describes.
    fn of(info: &FontInfo) -> Self {
        let mut hasher = Blake3Hasher::default();
        info.hash(&mut hasher);
        Self(hasher.digest())
    }
}

/// [`Hasher`] that absorbs a value's own [`Hash`] encoding into BLAKE3.
#[derive(Default)]
struct Blake3Hasher(blake3::Hasher);

impl Blake3Hasher {
    /// The digest of everything absorbed so far.
    fn digest(&self) -> ContentDigest {
        ContentDigest::from_bytes(*self.0.finalize().as_bytes())
    }
}

impl Hasher for Blake3Hasher {
    /// The [`Hasher`] contract's completion value. [`Hash`] implementations reach this bridge only
    /// through its `write*` methods, which never call it, so [`Blake3Hasher::digest`] has the
    /// identity.
    fn finish(&self) -> u64 {
        u64::from_le_bytes(
            self.0.finalize().as_bytes()[..8]
                .try_into()
                .expect("a BLAKE3 digest holds 32 bytes"),
        )
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
}

struct FontFileSource {
    file: Arc<FontFile>,
    index: u32,
    /// The face the file held when discovery parsed it.
    face: FaceIdentity,
}

impl FontSource for FontFileSource {
    fn load(&self) -> Option<Font> {
        let observed = self.file.bytes.get_or_init(|| {
            check_font_source(&self.file.boundary, &self.file.path)?;
            let bytes = read_font_bytes_with_identity(&self.file.path, None)?;
            if self
                .file
                .expected
                .is_some_and(|expected| expected != bytes.digest)
            {
                return Err(FontLoadError::Changed {
                    path: self.file.path.clone(),
                });
            }
            Ok(bytes)
        });
        let bytes = match observed {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = self.file.failure.set(error.clone());
                return None;
            }
        };
        let parsed_face = FontInfo::new(bytes.bytes.as_slice(), self.index)
            .as_ref()
            .map(FaceIdentity::of);
        if parsed_face != Some(self.face) {
            let _ = self.file.failure.set(FontLoadError::Changed {
                path: self.file.path.clone(),
            });
            return None;
        }
        let font = Font::new(bytes.bytes.clone(), self.index);
        if font.is_none() {
            let _ = self.file.failure.set(FontLoadError::Changed {
                path: self.file.path.clone(),
            });
        }
        font
    }
}

fn load_fonts(
    options: &FontOptions,
    cancellation: &BundleCancellation,
) -> Result<LoadedFonts, FontLoadError> {
    let mut native = KitFontStore::new();
    let mut files = BTreeMap::<PathBuf, Arc<FontFile>>::new();
    let configured =
        scan_configured_fonts(options, cancellation, &BTreeMap::new(), |path, observed| {
            let bytes = observed.bytes.as_slice();
            let digest = observed.digest;
            let file = files.entry(path.to_path_buf()).or_insert_with(|| {
                Arc::new(FontFile {
                    path: path.to_path_buf(),
                    expected: Some(digest),
                    bytes: OnceLock::new(),
                    failure: OnceLock::new(),
                    boundary: options.source_boundary.clone(),
                })
            });
            let faces = ttf_parser::fonts_in_collection(bytes).unwrap_or(1);
            for index in 0..faces {
                cancellation.ensure_active_as()?;
                if let Some(info) = FontInfo::new(bytes, index) {
                    native.push((
                        FontFileSource {
                            file: Arc::clone(file),
                            index,
                            face: FaceIdentity::of(&info),
                        },
                        info,
                    ));
                }
            }
            Ok(())
        })?;
    let mut system_paths = BTreeSet::new();
    if options.include_system_fonts {
        cancellation.ensure_active_as()?;
        let system = fonts::system();
        cancellation.ensure_active_as()?;
        for (source, info) in system {
            cancellation.ensure_active_as()?;
            let path = crate::world::normalize_path(&source.path);
            check_font_source(&options.source_boundary, &path)?;
            system_paths.insert(path.clone());
            let file = files.entry(path.clone()).or_insert_with(|| {
                Arc::new(FontFile {
                    path,
                    expected: None,
                    bytes: OnceLock::new(),
                    failure: OnceLock::new(),
                    boundary: options.source_boundary.clone(),
                })
            });
            native.push((
                FontFileSource {
                    file: Arc::clone(file),
                    index: source.index,
                    face: FaceIdentity::of(&info),
                },
                info,
            ));
        }
    }
    #[cfg(feature = "embed-fonts")]
    if options.include_embedded_fonts {
        cancellation.ensure_active_as()?;
        native.extend(embedded::fonts());
    }
    Ok(LoadedFonts {
        native,
        configured,
        files,
        system_paths,
    })
}

/// Scan the configured font directories.
///
/// `known` has the identities a previous scan established; files those still describe are
/// not read again and `observe` is not called for them.
fn scan_configured_fonts(
    options: &FontOptions,
    cancellation: &BundleCancellation,
    known: &BTreeMap<PathBuf, FontFileIdentity>,
    mut observe: impl FnMut(&Path, &FontFileBytes) -> Result<(), FontLoadError>,
) -> Result<FontInventory, FontLoadError> {
    let mut roots = Vec::new();
    let mut files = BTreeMap::new();
    for path in &options.custom_paths {
        cancellation.ensure_active_as()?;
        check_font_source(&options.source_boundary, path)?;
        let logical = std::path::absolute(path).map_err(|error| font_io_error(path, error))?;
        let canonical = match fs::canonicalize(&logical) {
            Ok(canonical) => Some(canonical),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(font_io_error(&logical, error)),
        };
        if let Some(root) = &canonical {
            let mut visited = BTreeSet::new();
            walk_font_directory(
                root,
                cancellation,
                &options.source_boundary,
                &mut visited,
                &mut |path| {
                    if let Some(known) = known.get(path).filter(|known| known.describes(path)) {
                        files.insert(path.to_path_buf(), known.reobserved(path)?);
                        return Ok(());
                    }
                    check_font_source(&options.source_boundary, path)?;
                    let observed = read_font_bytes(path, Some(cancellation))?;
                    observe(path, &observed)?;
                    files.insert(
                        path.to_path_buf(),
                        FontFileIdentity::of(path, observed.digest)?,
                    );
                    Ok(())
                },
            )?;
            if fs::canonicalize(&logical).ok().as_ref() != Some(root) {
                return Err(FontLoadError::Changed { path: logical });
            }
        }
        roots.push((logical, canonical));
    }
    Ok(FontInventory { roots, files })
}

fn walk_font_directory(
    directory: &Path,
    cancellation: &BundleCancellation,
    boundary: &crate::world::SourceBoundary,
    visited: &mut BTreeSet<PathBuf>,
    observe: &mut impl FnMut(&Path) -> Result<(), FontLoadError>,
) -> Result<(), FontLoadError> {
    cancellation.ensure_active_as()?;
    check_font_source(boundary, directory)?;
    if !visited.insert(directory.to_path_buf()) {
        return Ok(());
    }
    let mut entries = fs::read_dir(directory)
        .map_err(|error| font_io_error(directory, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| font_io_error(directory, error))?;
    entries.sort_unstable_by_key(|entry| entry.file_name());
    for entry in entries {
        cancellation.ensure_active_as()?;
        let logical = entry.path();
        check_font_source(boundary, &logical)?;
        let path = match fs::canonicalize(&logical) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_)
                if fs::symlink_metadata(&logical)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink()) =>
            {
                continue;
            }
            Err(error) => return Err(font_io_error(&logical, error)),
        };
        let metadata = fs::metadata(&path).map_err(|error| font_io_error(&path, error))?;
        if metadata.is_dir() {
            walk_font_directory(&path, cancellation, boundary, visited, observe)?;
        } else if metadata.is_file()
            && FontStore::accepts_path(&path)
            && visited.insert(path.clone())
        {
            observe(&path)?;
        }
    }
    Ok(())
}

fn read_font_bytes(
    path: &Path,
    cancellation: Option<&BundleCancellation>,
) -> Result<FontFileBytes, FontLoadError> {
    if let Some(cancellation) = cancellation {
        cancellation.ensure_active_as()?;
    }
    ensure_font_path_identity(path)?;
    let mut file = fs::File::open(path).map_err(|error| font_io_error(path, error))?;
    let mut bytes = Vec::new();
    let mut buffer = [0; FONT_READ_CHUNK_BYTES];
    loop {
        if let Some(cancellation) = cancellation {
            cancellation.ensure_active_as()?;
        }
        let count = file
            .read(&mut buffer)
            .map_err(|error| font_io_error(path, error))?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    ensure_font_path_identity(path)?;
    let digest = ContentDigest::of(&bytes);
    Ok(FontFileBytes {
        bytes: Bytes::new(bytes),
        digest,
        identity: None,
    })
}

/// Read a font file and describe the file those bytes came from.
fn read_font_bytes_with_identity(
    path: &Path,
    cancellation: Option<&BundleCancellation>,
) -> Result<FontFileBytes, FontLoadError> {
    let mut observed = read_font_bytes(path, cancellation)?;
    observed.identity = Some(FontFileIdentity::of(path, observed.digest)?);
    Ok(observed)
}

fn ensure_font_path_identity(path: &Path) -> Result<(), FontLoadError> {
    if fs::canonicalize(path).map_err(|error| font_io_error(path, error))? != path {
        return Err(FontLoadError::Changed {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn check_font_source(
    boundary: &crate::world::SourceBoundary,
    path: &Path,
) -> Result<(), FontLoadError> {
    boundary.check(path).map_err(|error| FontLoadError::File {
        path: path.to_path_buf(),
        error,
    })
}

fn font_io_error(path: &Path, error: std::io::Error) -> FontLoadError {
    FontLoadError::File {
        path: path.to_path_buf(),
        error: FileError::from_io(error, path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_options() -> FontOptions {
        FontOptions::new()
            .with_system_fonts(false)
            .with_embedded_fonts(false)
    }

    /// A store holding one lazily read face of `font` from `path`, with the `expected: None` of a
    /// discovered system font: no configured digest describes its bytes.
    #[cfg(feature = "embed-fonts")]
    fn system_face_store(path: &Path, font: &Font) -> (FontStore, Arc<FontFile>) {
        let file = Arc::new(FontFile {
            path: path.to_path_buf(),
            expected: None,
            bytes: OnceLock::new(),
            failure: OnceLock::new(),
            boundary: crate::world::SourceBoundary::default(),
        });
        let mut native = KitFontStore::new();
        native.push((
            FontFileSource {
                file: Arc::clone(&file),
                index: font.index(),
                face: FaceIdentity::of(font.info()),
            },
            font.info().clone(),
        ));
        let store = FontStore::with_options(isolated_options());
        assert!(
            store
                .fonts
                .set(LoadedFonts {
                    native,
                    configured: FontInventory {
                        roots: Vec::new(),
                        files: BTreeMap::new()
                    },
                    files: BTreeMap::from([(path.to_path_buf(), Arc::clone(&file))]),
                    system_paths: BTreeSet::from([path.to_path_buf()]),
                })
                .is_ok()
        );
        (store, file)
    }

    #[test]
    fn one_store_reuses_its_loaded_fonts() {
        let store = FontStore::with_options(isolated_options());
        let cancellation = BundleCancellation::new();
        let first = store.load(&cancellation).unwrap();
        let second = store.load(&cancellation).unwrap();
        assert!(std::ptr::eq(first, second));
        assert_eq!(store.font_count(), Some(0));
        assert_eq!(store.family_count(), Some(0));
        assert!(store.is_current(&cancellation).unwrap());
    }

    #[test]
    fn cancelled_preparation_can_be_retried() {
        let store = FontStore::with_options(isolated_options());
        let cancellation = BundleCancellation::new();
        cancellation.cancel();
        assert!(matches!(
            store.load(&cancellation),
            Err(FontLoadError::Cancelled)
        ));
        assert!(!store.is_loaded());
        assert!(store.load(&BundleCancellation::new()).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn font_aliases_respect_source_limits() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path().join("site");
        let fonts = root.join("fonts");
        fs::create_dir_all(&fonts).unwrap();
        let outside = directory.path().join("external.ttf");
        fs::write(&outside, b"host font bytes").unwrap();
        std::os::unix::fs::symlink(&outside, fonts.join("linked.ttf")).unwrap();
        let store = FontStore::with_options(
            isolated_options()
                .with_custom_paths(&[&fonts])
                .with_source_boundary(crate::SourceBoundary::new(&root, true)),
        );
        assert!(matches!(
            store.load(&BundleCancellation::new()),
            Err(FontLoadError::File { .. })
        ));
    }

    #[test]
    fn configured_font_change_marks_stale() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("font.ttf");
        fs::write(&path, b"not yet a font").unwrap();
        let store =
            FontStore::with_options(isolated_options().with_custom_paths(&[directory.path()]));
        let cancellation = BundleCancellation::new();
        store.load(&cancellation).unwrap();
        assert!(store.is_current(&cancellation).unwrap());
        fs::write(&path, b"new font source bytes").unwrap();
        assert!(!store.is_current(&cancellation).unwrap());
    }

    #[cfg(feature = "embed-fonts")]
    #[test]
    fn unchanged_configured_font_is_not_reread() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("font.ttf");
        let (font, _) = embedded::fonts().next().unwrap();
        fs::write(&path, font.data().as_slice()).unwrap();
        let store =
            FontStore::with_options(isolated_options().with_custom_paths(&[directory.path()]));
        let cancellation = BundleCancellation::new();
        store.load(&cancellation).unwrap();
        let configured = &store.fonts.get().unwrap().configured.files;
        let identity = configured.values().next().expect("one configured font");

        assert!(identity.describes(&path));
        assert!(store.is_current(&cancellation).unwrap());

        let mut changed = font.data().as_slice().to_vec();
        changed.push(b'x');
        fs::write(&path, changed).unwrap();
        assert!(!identity.describes(&path));
        assert!(!store.is_current(&cancellation).unwrap());
    }

    #[cfg(feature = "embed-fonts")]
    #[test]
    fn changed_font_bytes_fail_lazy_read() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("font.ttf");
        let (font, _) = embedded::fonts().next().unwrap();
        let original = font.data().as_slice();
        fs::write(&path, original).unwrap();
        let store =
            FontStore::with_options(isolated_options().with_custom_paths(&[directory.path()]));
        let cancellation = BundleCancellation::new();
        store.load(&cancellation).unwrap();
        let fonts = store.fonts.get().unwrap();
        assert!(fonts.files.values().all(|file| file.bytes.get().is_none()));
        let mut changed = original.to_vec();
        changed.extend_from_slice(b"additional bytes");
        assert_eq!(FontInfo::new(original, 0), FontInfo::new(&changed, 0));
        fs::write(&path, changed).unwrap();

        assert!(store.prepared().font(0).is_none());
        assert_eq!(store.read_paths(), [path.canonicalize().unwrap()]);
        assert!(matches!(
            store.read_failure(),
            Some(FontLoadError::Changed { .. })
        ));
        assert!(!store.is_current(&cancellation).unwrap());
    }

    #[cfg(feature = "embed-fonts")]
    #[test]
    fn system_font_bytes_read_lazily() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("system.ttf");
        let (font, _) = embedded::fonts().next().unwrap();
        let original = font.data().as_slice();
        fs::write(&path, original).unwrap();
        let path = path.canonicalize().unwrap();
        let (store, file) = system_face_store(&path, &font);
        assert!(file.bytes.get().is_none());
        assert!(store.read_paths().is_empty());
        let mut changed = original.to_vec();
        changed.extend_from_slice(b"new raw font bytes");
        fs::write(&path, &changed).unwrap();

        let loaded = store.prepared().font(0).unwrap();
        assert_eq!(store.read_paths(), std::slice::from_ref(&path));
        assert_eq!(loaded.data().as_slice(), changed.as_slice());
        assert!(store.is_current(&BundleCancellation::new()).unwrap());
        fs::write(&path, original).unwrap();
        assert!(!store.is_current(&BundleCancellation::new()).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn hidden_fonts_count_with_broken_links() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::TempDir::new().unwrap();
        let hidden = directory.path().join(".hidden");
        fs::create_dir(&hidden).unwrap();
        let font = hidden.join(".font.ttf");
        fs::write(&font, b"font source").unwrap();
        symlink("self", directory.path().join("self")).unwrap();
        symlink("missing", directory.path().join("broken")).unwrap();
        symlink(directory.path(), hidden.join("ancestor")).unwrap();
        let store =
            FontStore::with_options(isolated_options().with_custom_paths(&[directory.path()]));
        let cancellation = BundleCancellation::new();
        store.load(&cancellation).unwrap();
        assert!(store.is_current(&cancellation).unwrap());

        fs::write(font, b"changed source").unwrap();
        assert!(!store.is_current(&cancellation).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn retargeted_font_symlink_is_stale() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::TempDir::new().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        for root in [&first, &second] {
            fs::create_dir(root).unwrap();
            fs::write(root.join("font.ttf"), b"same source bytes").unwrap();
        }
        let alias = directory.path().join("alias");
        symlink(&first, &alias).unwrap();
        let cancellation = BundleCancellation::new();
        let store = FontStore::with_options(isolated_options().with_custom_paths(&[&alias]));
        store.load(&cancellation).unwrap();
        fs::remove_file(&alias).unwrap();
        symlink(&second, &alias).unwrap();
        assert!(!store.is_current(&cancellation).unwrap());

        let root = directory.path().join("fonts");
        fs::create_dir(&root).unwrap();
        let nested = root.join("nested");
        symlink(&first, &nested).unwrap();
        let store = FontStore::with_options(isolated_options().with_custom_paths(&[&root]));
        store.load(&cancellation).unwrap();
        fs::remove_file(&nested).unwrap();
        symlink(&second, &nested).unwrap();
        assert!(!store.is_current(&cancellation).unwrap());
    }

    #[cfg(feature = "embed-fonts")]
    #[test]
    fn replaced_font_face_fails_lazy_read() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("system.ttf");
        let mut carried = embedded::fonts();
        let (font, _) = carried.next().unwrap();
        let (replacement, _) = carried.next().unwrap();
        fs::write(&path, font.data().as_slice()).unwrap();
        let path = path.canonicalize().unwrap();
        let (store, _) = system_face_store(&path, &font);

        fs::write(&path, replacement.data().as_slice()).unwrap();

        assert!(store.prepared().font(0).is_none());
        assert_eq!(store.read_paths(), std::slice::from_ref(&path));
        assert!(matches!(
            store.read_failure(),
            Some(FontLoadError::Changed { .. })
        ));
    }

    #[cfg(feature = "embed-fonts")]
    #[test]
    fn narrowed_coverage_changes_face_identity() {
        use typst::text::Coverage;

        let (_, info) = embedded::fonts().next().unwrap();
        let mut narrowed = info.clone();
        narrowed.coverage = Coverage::from_vec(vec![0x41]);
        assert_ne!(FaceIdentity::of(&info), FaceIdentity::of(&narrowed));
    }
}
