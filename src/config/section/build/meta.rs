//! Metadata extraction configuration.

use serde::{Deserialize, Serialize};

/// Default metadata label for Typst files
pub const TOLA_META_LABEL: &str = "tola-meta";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MetaConfig {
    /// Label name for metadata extraction in Typst files.
    pub label: String,
}

impl Default for MetaConfig {
    fn default() -> Self {
        Self {
            label: TOLA_META_LABEL.into(),
        }
    }
}
