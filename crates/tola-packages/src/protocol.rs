//! The contract between the build engine and the builtin packages.
//!
//! The engine injects values into `sys.inputs`, and the packages read them back by name. These
//! keys are an internal protocol between Tola and its own packages, not a trust boundary. The
//! observation paths name virtual files: the engine produces them, and the packages' natives
//! read them as ordinary tracked world input.

/// The `sys.inputs` member holding the module `@tola/host` re-exports.
///
/// The leading underscores keep it clear of the keys an author sets.
pub const HOST_MODULE: &str = "__tola";

/// The `sys.inputs` member holding the ordered source records.
pub const SOURCE_RECORDS_MEMBER: &str = "__tola_source_records";

/// The `sys.inputs` member holding lexical source file views keyed by source path.
pub const SOURCE_RECORDS_BY_FILE_MEMBER: &str = "__tola_source_records_by_file";

/// The `sys.inputs` member holding the final configured asset URLs.
pub const ASSET_URLS_MEMBER: &str = "__tola_asset_urls";

/// Site-root sources of exact-file asset declarations, keyed by declared URL.
pub const ASSET_ORIGINS_MEMBER: &str = "__tola_asset_origins";

/// The `sys.inputs` member holding each source's origin, keyed by the source path the records
/// carry.
///
/// A value is `(file: <site-root path>, range: none | (start, end))`: the file the source lives in
/// and the byte range of its `<tola-meta>` declaration, or `none` when it declares no metadata.
/// A value cannot carry the position it was written at, so the engine resolves both from one
/// source of the same immutable snapshot and passes them here; a diagnostic then takes its file
/// and its range from this one entry rather than pairing an independent file with a path.
pub const SOURCE_ORIGINS_MEMBER: &str = "__tola_source_origins";

/// The virtual path naming the capability observation one source-analysis read records.
pub const CAPABILITY_OBSERVATION_PATH: &str = ".tola-capability";

/// The virtual directory holding one observation per discovered source.
pub const SOURCE_OBSERVATION_DIRECTORY: &str = ".tola-source";

/// The virtual directory holding one observation per configured icon file.
pub const ICON_OBSERVATION_DIRECTORY: &str = ".tola-icon";

/// The virtual directory holding one observation per published icon request.
///
/// A namespace and the icon name it publishes are joined directly after the trailing slash.
pub const ICON_REQUEST_OBSERVATION_DIRECTORY: &str = ".tola-icon-request/";

/// The virtual directory prefix holding one observation per image derivative request.
///
/// A descriptor name is joined directly after the trailing slash.
pub const IMAGE_REQUEST_OBSERVATION_DIRECTORY: &str = ".tola-image-request/";

/// The extension every image derivative observation carries, including its leading dot.
pub const IMAGE_REQUEST_OBSERVATION_EXTENSION: &str = ".json";
