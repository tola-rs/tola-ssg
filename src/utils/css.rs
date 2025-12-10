//! CSS utilities: auto-enhance generation and Tailwind integration.
//!
//! This module provides:
//! - Auto-enhanced CSS generation for SVG theme adaptation
//! - Tailwind CSS build integration

use crate::config::SiteConfig;
use crate::utils::exec::FilterRule;
use crate::exec;
use anyhow::{anyhow, Result};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

// ============================================================================
// Auto-enhance CSS
// ============================================================================

/// CSS content for SVG theme adaptation and dark mode support.
/// Loaded from `src/embed/css/enhance.css` at compile time.
const ENHANCE_CSS: &str = include_str!("../embed/css/enhance.css");

/// Compute the hash for the enhance CSS content.
fn compute_hash() -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    ENHANCE_CSS.hash(&mut hasher);
    format!("{:x}", hasher.finish())[..8].to_string()
}

/// Get the enhance CSS filename (hidden file with hash).
///
/// Returns a filename like `.enhance-a1b2c3d4.css`.
pub fn enhance_css_filename() -> String {
    format!(".enhance-{}.css", compute_hash())
}

/// Generate and write the auto-enhance CSS file to the output directory.
///
/// Returns the relative path to the generated file.
pub fn generate_enhance_css(output_dir: &Path) -> Result<PathBuf> {
    let filename = enhance_css_filename();
    let path = output_dir.join(&filename);

    // Write CSS file
    let mut file = fs::File::create(&path)?;
    file.write_all(ENHANCE_CSS.as_bytes())?;

    Ok(PathBuf::from(filename))
}

/// Clean up old enhance CSS files (files matching `.enhance-*.css` pattern).
///
/// Keeps only the current version based on hash.
pub fn cleanup_old_enhance_css(output_dir: &Path) -> Result<()> {
    let current_filename = enhance_css_filename();

    for entry in fs::read_dir(output_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Match pattern: .enhance-{hash}.css but not current file
        if name_str.starts_with(".enhance-")
            && name_str.ends_with(".css")
            && name_str != current_filename
        {
            fs::remove_file(entry.path())?;
        }
    }

    Ok(())
}

// ============================================================================
// Tailwind CSS
// ============================================================================

/// Tailwind filter: skip version banner in output.
pub static TAILWIND_FILTER: FilterRule = FilterRule::new(&["≈ tailwindcss"]);

/// Check if a path is the Tailwind input file.
pub fn is_tailwind_input(path: &Path, config: &SiteConfig) -> bool {
    config.build.css.tailwind.enable
        && config
            .build
            .css
            .tailwind
            .input
            .as_ref()
            .is_some_and(|input| path.canonicalize().ok().as_ref() == Some(input))
}

/// Run Tailwind CSS build for the input file.
pub fn run_tailwind(input: &Path, output: &Path, config: &SiteConfig) -> Result<()> {
    exec!(
        pty=true;
        filter=&TAILWIND_FILTER;
        config.get_root();
        &config.build.css.tailwind.command;
        "-i", input, "-o", output,
        if config.build.minify { "--minify" } else { "" }
    )?;
    Ok(())
}

/// Rebuild Tailwind CSS using configured input path.
///
/// Used by watch mode to rebuild when source files change.
pub fn rebuild_tailwind(
    config: &'static SiteConfig,
    get_output_path: impl FnOnce(&Path) -> Result<PathBuf>,
) -> Result<()> {
    let input = config
        .build
        .css
        .tailwind
        .input
        .as_ref()
        .ok_or_else(|| anyhow!("Tailwind input path not configured"))?;

    let output = get_output_path(input)?;
    run_tailwind(input, &output, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_enhance_css_filename_format() {
        let filename = enhance_css_filename();
        assert!(filename.starts_with(".enhance-"));
        assert!(filename.ends_with(".css"));
        assert_eq!(filename.len(), ".enhance-12345678.css".len());
    }

    #[test]
    fn test_enhance_css_filename_stable() {
        // Same content should produce same hash
        let f1 = enhance_css_filename();
        let f2 = enhance_css_filename();
        assert_eq!(f1, f2);
    }

    #[test]
    fn test_generate_enhance_css() {
        let dir = tempdir().unwrap();
        let result = generate_enhance_css(dir.path()).unwrap();

        // Check filename format
        let filename = result.to_string_lossy();
        assert!(filename.starts_with(".enhance-"));
        assert!(filename.ends_with(".css"));

        // Check file exists and has content
        let content = fs::read_to_string(dir.path().join(&result)).unwrap();
        assert!(content.contains("typst-text"));
        assert!(content.contains("currentColor"));
    }

    #[test]
    fn test_cleanup_old_enhance_css() {
        let dir = tempdir().unwrap();

        // Create some old files
        fs::write(dir.path().join(".enhance-old1.css"), "old").unwrap();
        fs::write(dir.path().join(".enhance-old2.css"), "old").unwrap();

        // Generate current file
        generate_enhance_css(dir.path()).unwrap();

        // Cleanup
        cleanup_old_enhance_css(dir.path()).unwrap();

        // Only current file should remain
        let files: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".enhance-"))
            .collect();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name().to_string_lossy(), enhance_css_filename());
    }
}
