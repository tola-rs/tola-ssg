//! Configuration utility types.
//!
//! | Module   | Purpose                                      |
//! |----------|----------------------------------------------|
//! | `error`  | Configuration error types                    |
//! | `handle` | Global configuration handle (thread-safe)    |
//! | `path`   | Path resolution utilities                    |
//! | `status` | Field status validation                      |

mod error;
mod field;
pub mod handle;
mod path;
mod status;

pub use error::{ConfigDiagnostics, ConfigError};
pub use field::FieldPath;
pub use handle::{cfg, clear_clean_flag, init_config, reload_config};
pub use path::PathResolver;
pub use status::{check_section_status, FieldStatus};
