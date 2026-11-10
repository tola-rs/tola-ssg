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

/// The `sys.inputs` member holding module records keyed by the project alias.
pub const MODULES_MEMBER: &str = "__tola_modules";

/// The `sys.inputs` member holding the ordered source records.
pub const SOURCE_RECORDS_MEMBER: &str = "__tola_source_records";

/// The `sys.inputs` member holding lexical source file views keyed by source path.
pub const SOURCE_RECORDS_BY_FILE_MEMBER: &str = "__tola_source_records_by_file";

/// The `sys.inputs` member holding the final configured asset URLs.
pub const ASSET_URLS_MEMBER: &str = "__tola_asset_urls";

/// Site-root sources of exact-file asset declarations, keyed by declared URL.
pub const ASSET_ORIGINS_MEMBER: &str = "__tola_asset_origins";

/// Each source's origin, keyed by source path: `source`, plus the declaration's `file` and `range`.
pub const SOURCE_ORIGINS_MEMBER: &str = "__tola_source_origins";

/// The virtual path naming the capability observation one source-analysis read records.
pub const CAPABILITY_OBSERVATION_PATH: &str = ".tola-capability";

/// The virtual directory holding one observation per discovered source.
pub const SOURCE_OBSERVATION_DIRECTORY: &str = ".tola-source";

/// The virtual directory holding one observation per configured icon file, and one tracked name
/// list per configured collection as `.names` beside its icons.
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
