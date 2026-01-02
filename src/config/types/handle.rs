//! Global config with atomic reload support.
//!
//! Uses `arc-swap` for lock-free reads and atomic config replacement.
//! This enables hot-reloading of `tola.toml` during watch mode.

use crate::config::SiteConfig;
use anyhow::Result;
use arc_swap::ArcSwap;
use std::sync::{Arc, LazyLock};

/// Global config storage.
pub static CONFIG: LazyLock<ArcSwap<SiteConfig>> =
    LazyLock::new(|| ArcSwap::from_pointee(SiteConfig::default()));

/// Global hash of the current config file content.
static CONFIG_HASH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[inline]
pub fn cfg() -> Arc<SiteConfig> {
    CONFIG.load_full()
}

/// Reload config from disk if content changed.
///
/// Returns `Ok(true)` if config was updated, `Ok(false)` if unchanged.
pub fn reload_config() -> Result<bool> {
    use std::fs;

    let c = cfg();
    let cli = c.cli.expect("CLI should be set during initialization");

    let content = fs::read_to_string(&c.config_path)?;
    let new_hash = crate::utils::hash::compute(content.as_bytes());

    let old_hash = CONFIG_HASH.load(std::sync::atomic::Ordering::Relaxed);
    if new_hash == old_hash {
        return Ok(false);
    }

    let new_config = SiteConfig::load(cli)?;
    CONFIG.store(Arc::new(new_config));
    CONFIG_HASH.store(new_hash, std::sync::atomic::Ordering::Relaxed);

    Ok(true)
}

#[inline]
pub fn init_config(config: SiteConfig) -> Arc<SiteConfig> {
    use std::fs;

    if config.config_path.exists()
        && let Ok(content) = fs::read_to_string(&config.config_path)
    {
        let hash = crate::utils::hash::compute(content.as_bytes());
        CONFIG_HASH.store(hash, std::sync::atomic::Ordering::Relaxed);
    }

    let arc = Arc::new(config);
    CONFIG.store(Arc::clone(&arc));
    arc
}

/// Clear the clean flag after initial build.
pub fn clear_clean_flag() {
    let mut config = (*cfg()).clone();
    config.build.clean = false;
    CONFIG.store(Arc::new(config));
}
