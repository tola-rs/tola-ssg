//! Host-to-package `sys.inputs` keys and virtual observation paths.
//!
//! Observation paths are virtual files emitted by the engine and read as tracked Typst world inputs.

/// `sys.inputs` key for the module re-exported by `@tola/host` (underscores avoid author keys).
pub const HOST_MODULE: &str = "__tola";

/// `sys.inputs` key for module records keyed by project alias.
pub const MODULES_MEMBER: &str = "__tola_modules";

/// `sys.inputs` key for ordered source records.
pub const SOURCE_RECORDS_MEMBER: &str = "__tola_source_records";

/// `sys.inputs` key for lexical source file views keyed by source path.
pub const SOURCE_RECORDS_BY_FILE_MEMBER: &str = "__tola_source_records_by_file";

/// `sys.inputs` key for final configured asset URLs.
pub const ASSET_URLS_MEMBER: &str = "__tola_asset_urls";

/// `sys.inputs` key for site-root paths of exact-file asset declarations, keyed by declared URL.
pub const ASSET_ORIGINS_MEMBER: &str = "__tola_asset_origins";

/// `sys.inputs` key for each source's origin (`source`, `file`, `range`), keyed by source path.
pub const SOURCE_ORIGINS_MEMBER: &str = "__tola_source_origins";

/// Virtual path for the capability observation read during source analysis.
pub const CAPABILITY_OBSERVATION_PATH: &str = ".tola-capability";

/// Virtual directory for per-source observations.
pub const SOURCE_OBSERVATION_DIRECTORY: &str = ".tola-source";

/// Virtual directory for icon files and `.names` suggestion lists.
pub const ICON_OBSERVATION_DIRECTORY: &str = ".tola-icon";

/// Virtual directory prefix for published icon requests (`<namespace><name>` appended after `/`).
pub const ICON_REQUEST_OBSERVATION_DIRECTORY: &str = ".tola-icon-request/";

/// Virtual directory prefix for image derivative requests (descriptor appended after `/`).
pub const IMAGE_REQUEST_OBSERVATION_DIRECTORY: &str = ".tola-image-request/";

/// File extension for image derivative observation paths, including the leading dot.
pub const IMAGE_REQUEST_OBSERVATION_EXTENSION: &str = ".json";
