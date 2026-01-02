//! Build mode configuration for production/development builds.

/// Build mode configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildMode {
    /// Whether to emit `data-tola-id` attributes on elements.
    /// Used for VDOM diffing and hot reload.
    pub emit_ids: bool,

    /// Whether to cache indexed VDOM for hot reload diffs.
    pub cache_vdom: bool,
}

impl BuildMode {
    /// Production mode: optimized output without debug metadata.
    pub const PRODUCTION: Self = Self {
        emit_ids: false,
        cache_vdom: false,
    };

    /// Development mode: includes hot reload support.
    pub const DEVELOPMENT: Self = Self {
        emit_ids: true,
        cache_vdom: true,
    };

    /// Check if this is development mode.
    #[inline]
    #[allow(dead_code)]
    pub const fn is_dev(&self) -> bool {
        self.emit_ids
    }
}
