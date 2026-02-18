//! Platform utilities for cross-architecture cache safety
//!
//! Provides compile-time architecture fingerprints for cache file naming.
//! This ensures rkyv-serialized data is not shared across incompatible architectures.

use std::path::PathBuf;

/// Compile-time architecture fingerprint (zero runtime overhead)
///
/// Used to ensure rkyv cache files are only loaded on compatible architectures
/// The fingerprint is embedded in cache filenames to prevent accidental cross-arch use
///
/// # Examples
///
/// - `x86_64_macos` on Intel Mac
/// - `aarch64_macos` on Apple Silicon Mac
/// - `x86_64_linux` on Linux x64
/// - `aarch64_linux` on Linux ARM64
pub const ARCH_FINGERPRINT: &str = {
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    {
        "x86_64_linux"
    }

    #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
    {
        "x86_64_macos"
    }

    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    {
        "aarch64_macos"
    }

    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    {
        "aarch64_linux"
    }

    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    {
        "x86_64_windows"
    }

    #[cfg(all(target_arch = "aarch64", target_os = "windows"))]
    {
        "aarch64_windows"
    }

    // Fallback for other architectures
    #[cfg(not(any(
        all(target_arch = "x86_64", target_os = "linux"),
        all(target_arch = "x86_64", target_os = "macos"),
        all(target_arch = "aarch64", target_os = "macos"),
        all(target_arch = "aarch64", target_os = "linux"),
        all(target_arch = "x86_64", target_os = "windows"),
        all(target_arch = "aarch64", target_os = "windows"),
    )))]
    {
        concat!(
            env!("CARGO_CFG_TARGET_ARCH"),
            "_",
            env!("CARGO_CFG_TARGET_OS")
        )
    }
};

/// Generate a cache path with architecture fingerprint
///
/// Creates a path like `{base}/{name}_{arch}.rkyv`
///
/// # Arguments
///
/// * `base` - Base directory for cache files
/// * `name` - Cache entry name (without extension)
///
/// # Examples
///
/// ```
/// use tola::utils::platform::cache_path;
///
/// let path = cache_path(".cache", "index");
/// // On Apple Silicon Mac: .cache/index_aarch64_macos.rkyv
/// // On Linux x64: .cache/index_x86_64_linux.rkyv
/// ```
#[allow(dead_code)]
pub fn cache_path(base: &str, name: &str) -> PathBuf {
    PathBuf::from(base).join(format!("{}_{}.rkyv", name, ARCH_FINGERPRINT))
}

/// Check if a cache path has a valid architecture fingerprint
///
/// Returns `true` if the filename contains the current architecture fingerprint
/// This is used to determine if a cache file can be safely loaded
#[allow(dead_code)]
pub fn is_cache_valid_for_arch(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.contains(ARCH_FINGERPRINT))
        .unwrap_or(false)
}

/// Generate a hash-based cache key from a path
///
/// Uses the path's string representation for consistent hashing
#[allow(dead_code)]
pub fn path_to_cache_hash(path: &std::path::Path) -> u64 {
    // Use our deterministic hash module
    crate::utils::hash::compute(path.to_string_lossy().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arch_fingerprint_format() {
        // Should be in format "arch_os"
        assert!(ARCH_FINGERPRINT.contains('_'));
        let parts: Vec<_> = ARCH_FINGERPRINT.split('_').collect();
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn test_cache_path() {
        let path = cache_path(".cache", "test");
        let path_str = path.to_string_lossy();

        assert!(path_str.starts_with(".cache"));
        assert!(path_str.ends_with(".rkyv"));
        assert!(path_str.contains(ARCH_FINGERPRINT));
    }

    #[test]
    fn test_is_cache_valid() {
        let valid_path = cache_path(".cache", "test");
        assert!(is_cache_valid_for_arch(&valid_path));

        let invalid_path = PathBuf::from(".cache/test_wrong_arch.rkyv");
        assert!(!is_cache_valid_for_arch(&invalid_path));
    }

    #[test]
    fn test_path_hash() {
        let path1 = PathBuf::from("content/index.typ");
        let path2 = PathBuf::from("content/index.typ");
        let path3 = PathBuf::from("content/other.typ");

        assert_eq!(path_to_cache_hash(&path1), path_to_cache_hash(&path2));
        assert_ne!(path_to_cache_hash(&path1), path_to_cache_hash(&path3));
    }
}
