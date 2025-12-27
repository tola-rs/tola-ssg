//! Global config with atomic reload support.
//!
//! Uses `arc-swap` for lock-free reads and atomic config replacement.
//! This enables hot-reloading of `tola.toml` during watch mode.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    CONFIG (ArcSwap)                         │
//! │                                                             │
//! │  ┌─────────────┐     ┌─────────────┐     ┌─────────────┐    │
//! │  │  Reader 1   │     │  Reader 2   │     │   Writer    │    │
//! │  │  (rayon)    │     │  (rayon)    │     │  (watch)    │    │
//! │  └──────┬──────┘     └──────┬──────┘     └──────┬──────┘    │
//! │         │                   │                   │           │
//! │         ▼                   ▼                   ▼           │
//! │       cfg()              cfg()           reload_config()    │
//! │    (lock-free)         (lock-free)      (atomic replace)    │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use crate::config::cfg;
//!
//! let c = cfg();
//! build_site(&c)?;  // Arc auto-derefs to &SiteConfig
//! ```

use super::SiteConfig;
use arc_swap::ArcSwap;
use std::sync::{Arc, LazyLock};

// =============================================================================
// Global State
// =============================================================================

/// Global config storage with atomic replacement support.
///
/// Initialized with default config, then replaced with loaded config in main.
/// During watch mode, can be atomically replaced when tola.toml changes.
pub static CONFIG: LazyLock<ArcSwap<SiteConfig>> =
    LazyLock::new(|| ArcSwap::from_pointee(SiteConfig::default()));

// =============================================================================
// Public API
// =============================================================================

/// Get current config as `Arc<SiteConfig>`.
///
/// Returns an `Arc` that keeps the config alive. Thread-safe and wait-free.
/// The Arc auto-derefs to `&SiteConfig`, making it ergonomic to use:
///
/// ```ignore
/// let c = cfg();
/// some_function(&c);  // Works directly, no extra & needed
/// ```
///
/// # Performance
///
/// Lock-free read via atomic load. Suitable for hot paths in rayon parallel contexts.
#[inline]
pub fn cfg() -> Arc<SiteConfig> {
    CONFIG.load_full()
}

/// Replace config atomically (called when tola.toml changes).
///
/// Reads CLI from current config to reload, ensuring consistent access.
/// The old config remains valid for any readers that loaded it before this call.
/// New readers will see the updated config.
///
/// # Errors
///
/// Returns error if tola.toml parsing fails.
pub fn reload_config() -> anyhow::Result<()> {
    let cli = cfg()
        .cli
        .expect("CLI should be set in config during initialization");
    let new_config = SiteConfig::load(cli)?;
    CONFIG.store(Arc::new(new_config));
    Ok(())
}

/// Initialize global config (called once at startup).
///
/// This replaces the default config with the loaded one.
#[inline]
pub fn init_config(config: SiteConfig) {
    CONFIG.store(Arc::new(config));
}

