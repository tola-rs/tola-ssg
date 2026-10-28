//! Font loading and caching.
//!
//! Share a [`FontStore`] across compilations with the same font configuration.
//!
//! # Precedence
//!
//! Faces join one store in search order: configured directories, system fonts, then the fonts
//! this binary carries. An equal match keeps the earlier face, so a site's own font wins over a
//! same-named carried one and the carried set serves as the fallback.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use typst::diag::FileError;
use typst::foundations::Bytes;
use typst::text::{Font, FontAxis, FontInfo, FontVariant};
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
    /// Whether to include the four text families this binary carries: Libertinus Serif, New
    /// Computer Modern, New Computer Modern Math, and DejaVu Sans Mono.
    ///
    /// Only effective with the `embed-fonts` feature.
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

    /// Set whether to include the fonts this binary carries.
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
    /// A configured directory or one of the font files below it could not be read.
    #[error("could not read the font file `{}`", path.display())]
    File {
        /// Absolute source path.
        path: PathBuf,
        /// Structured filesystem or decoding failure.
        #[source]
        error: FileError,
    },
    /// A configured font input is not a directory.
    #[error("`{}` is not a font directory", path.display())]
    NotDirectory {
        /// Configured path that names something other than a directory.
        path: PathBuf,
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
            Self::File { path, .. } | Self::Changed { path } | Self::NotDirectory { path } => {
                Some(path)
            }
            Self::Cancelled => None,
        }
    }

    /// Whether the caller cancelled preparation or validation.
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

/// One face the compiler fonts offer, with the input it is read from.
#[derive(Debug, Clone, PartialEq)]
pub struct CompilerFont {
    /// The family name, spelled as the face declares it.
    pub family: String,
    /// The style, weight, and stretch a selection matches this face by.
    pub variant: FontVariant,
    /// The variation axes the face declares; empty for a static face.
    pub axes: Vec<FontAxis>,
    /// The input the face's bytes come from.
    pub input: CompilerFontInput,
}

/// The input one compiler font face is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompilerFontInput {
    /// A face file below one configured directory.
    ConfiguredDirectory {
        /// The configured directory, as the host passed it.
        directory: PathBuf,
        /// The file inside it the face is read from.
        file: PathBuf,
    },
    /// A face file system font discovery reported.
    SystemFile {
        /// The face file.
        file: PathBuf,
    },
    /// A face this binary carries.
    Embedded,
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
            |_, _, _| Ok(()),
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

    /// Every face the loaded fonts offer, in the order the compiler indexes them.
    ///
    /// A store that has not loaded its fonts offers nothing: discovery has not run.
    pub fn compiler_fonts(&self) -> Vec<CompilerFont> {
        let Some(fonts) = self.fonts.get() else {
            return Vec::new();
        };
        let book = fonts.native.book();
        fonts
            .inputs
            .iter()
            .enumerate()
            .filter_map(|(index, input)| {
                let info = book.info(index)?;
                Some(CompilerFont {
                    family: info.family.clone(),
                    variant: info.variant,
                    axes: info.axes.clone(),
                    input: input.clone(),
                })
            })
            .collect()
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
    /// The input that provided each face, in the native store's own index order.
    inputs: Vec<CompilerFontInput>,
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
    let mut inputs = Vec::new();
    let configured = scan_configured_fonts(
        options,
        cancellation,
        &BTreeMap::new(),
        |directory, path, observed| {
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
                    inputs.push(CompilerFontInput::ConfiguredDirectory {
                        directory: directory.to_path_buf(),
                        file: path.to_path_buf(),
                    });
                }
            }
            Ok(())
        },
    )?;
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
            inputs.push(CompilerFontInput::SystemFile {
                file: file.path.clone(),
            });
        }
    }
    #[cfg(feature = "embed-fonts")]
    if options.include_embedded_fonts {
        for face in embedded::fonts() {
            native.push(face);
            inputs.push(CompilerFontInput::Embedded);
        }
    }
    Ok(LoadedFonts {
        native,
        configured,
        files,
        system_paths,
        inputs,
    })
}

/// Scan the configured font directories.
///
/// `known` has the identities a previous scan established; files those still describe are
/// not read again and `observe` is not called for them.
///
/// `observe` receives the configured directory being scanned, the file below it, and that file's
/// bytes.
fn scan_configured_fonts(
    options: &FontOptions,
    cancellation: &BundleCancellation,
    known: &BTreeMap<PathBuf, FontFileIdentity>,
    mut observe: impl FnMut(&Path, &Path, &FontFileBytes) -> Result<(), FontLoadError>,
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
        if canonical.as_ref().is_some_and(|root| !root.is_dir()) {
            return Err(FontLoadError::NotDirectory { path: logical });
        }
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
                    observe(&logical, path, &observed)?;
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

    #[cfg(feature = "embed-fonts")]
    #[test]
    fn configured_font_file_is_refused() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("font.ttf");
        fs::write(&path, b"not a font").unwrap();
        let store = FontStore::with_options(isolated_options().with_custom_paths(&[&path]));

        assert!(matches!(
            store.load(&BundleCancellation::new()),
            Err(FontLoadError::NotDirectory { .. })
        ));
    }

    #[cfg(feature = "embed-fonts")]
    #[cfg(unix)]
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

    /// A face's identity ignores bytes the file gained after the bytes the face was read from, so
    /// a rewritten file still names the font it holds.
    #[cfg(feature = "embed-fonts")]
    #[test]
    fn font_identity_ignores_trailing_bytes() {
        let (font, _) = embedded::fonts().next().unwrap();
        let original = font.data().as_slice();
        let mut changed = original.to_vec();
        changed.extend_from_slice(b"additional bytes");
        assert_eq!(FontInfo::new(original, 0), FontInfo::new(&changed, 0));
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
    #[cfg(unix)]
    #[test]
    fn narrowed_coverage_changes_face_identity() {
        use typst::text::Coverage;

        let (_, info) = embedded::fonts().next().unwrap();
        let mut narrowed = info.clone();
        narrowed.coverage = Coverage::from_vec(vec![0x41]);
        assert_ne!(FaceIdentity::of(&info), FaceIdentity::of(&narrowed));
    }
}
