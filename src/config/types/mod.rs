//! Configuration utility types.
//!
//! | Module   | Purpose                                      |
//! |----------|----------------------------------------------|
//! | `error`  | Configuration error types                    |
//! | `handle` | Global configuration handle (thread-safe)    |
//! | `output` | Generated public output paths                |
//! | `path`   | Path resolution utilities                    |
//! | `status` | Field status validation                      |

mod error;
mod field;
pub mod handle;
mod output;
mod path;
pub(crate) mod status;

pub use error::{ConfigDiagnostics, ConfigError};
pub use field::FieldPath;
pub use handle::{ConfigHandle, config_handle, init_config};
pub use output::GeneratedOutputPath;
pub use path::PathResolver;
pub use status::{ConfigPresence, FieldStatus};
