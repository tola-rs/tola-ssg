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

/// Global hash of the current config file content.
static CONFIG_HASH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Replace config atomically (called when tola.toml changes).
///
/// Reads CLI from current config to reload, ensuring consistent access.
/// The old config remains valid for any readers that loaded it before this call.
/// New readers will see the updated config.
///
/// Returns `true` if config was actually updated, `false` if content matches last load.
///
/// # Errors
///
/// Returns error if tola.toml parsing fails.
pub fn reload_config() -> anyhow::Result<bool> {
    use std::fs;

    let c = cfg();
    let cli = c.cli.expect("CLI should be set in config during initialization");

    // Read raw content to check for changes
    // Using config_path form current config which is absolute path
    // If reading fails, bubble up error (file might be deleted temporarily)
    let content = fs::read_to_string(&c.config_path)?;

    // Compute hash
    let new_hash = crate::utils::hash::compute(content.as_bytes());

    // Check against cached hash
    let old_hash = CONFIG_HASH.load(std::sync::atomic::Ordering::Relaxed);
    if new_hash == old_hash {
        return Ok(false);
    }

    // Parse and update
    // Note: SiteConfig::load internally reads the file again, which is acceptable
    // given config size is small and reload is infrequent event.
    // Optimizing to pass content would require changing SiteConfig::load API.
    let new_config = SiteConfig::load(cli)?;

    CONFIG.store(Arc::new(new_config));
    CONFIG_HASH.store(new_hash, std::sync::atomic::Ordering::Relaxed);

    Ok(true)
}

/// Initialize global config (called once at startup).
///
/// This replaces the default config with the loaded one.
#[inline]
pub fn init_config(config: SiteConfig) {
    use std::fs;

    // Initialize hash if file exists
    if config.config_path.exists() {
        if let Ok(content) = fs::read_to_string(&config.config_path) {
            let hash = crate::utils::hash::compute(content.as_bytes());
            CONFIG_HASH.store(hash, std::sync::atomic::Ordering::Relaxed);
        }
    }

    CONFIG.store(Arc::new(config));
}

