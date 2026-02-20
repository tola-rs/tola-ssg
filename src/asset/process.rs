//! Asset processing with side effects (copying, minification).

use std::fs;
use std::path::Path;

use anyhow::{Result, anyhow};

use crate::config::SiteConfig;
use crate::core::ContentKind;
use crate::freshness::is_newer_than;
use crate::hooks::css;
use crate::log;

use super::meta::{relative_path, route_from_source};

/// Process an asset file from the assets directory
///
/// Copies the asset to the output directory, respecting freshness checks
/// Skips CSS processor input (handled centrally)
pub fn process_asset(
    asset_path: &Path,
    config: &SiteConfig,
    clean: bool,
    log_file: bool,
) -> Result<()> {
    let route = route_from_source(asset_path.to_path_buf(), config)?;

    // Skip if up-to-date (use mtime comparison for assets, not hash markers)
    if !clean && route.output.exists() && !is_newer_than(asset_path, &route.output) {
        return Ok(());
    }

    if log_file {
        log!("assets"; "{}", relative_path(asset_path, config));
    }

    if let Some(parent) = route.output.parent() {
        fs::create_dir_all(parent)?;
    }

    let ext = asset_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();

    // Skip CSS processor input (handled centrally)
    if ext == "css" && css::is_css_input(asset_path, config) {
        return Ok(());
    }

    // Minify JS/CSS (skip already minified .min.js/.min.css)
    let stem = asset_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let is_minified = stem.ends_with(".min");
    if !is_minified && (ext == "js" || ext == "css") {
        let source = fs::read_to_string(&route.source)?;
        let minified =
            super::minify::minify_by_ext(&route.source, &source).unwrap_or_else(|| source.clone());
        fs::write(&route.output, minified)?;
    } else {
        fs::copy(&route.source, &route.output)?;
    }
    Ok(())
}

/// Process an asset file from the content directory (non-.typ files)
///
/// These are files in the content directory that aren't pages
pub fn process_rel_asset(
    path: &Path,
    config: &SiteConfig,
    clean: bool,
    log_file: bool,
) -> Result<()> {
    let content = &config.build.content;
    let output = config.paths().output_dir();

    let rel_path = path
        .strip_prefix(content)?
        .to_str()
        .ok_or_else(|| anyhow!("Invalid path"))?;

    let output_path = output.join(rel_path);

    // Relative assets don't depend on templates/config, use mtime comparison
    if !clean && output_path.exists() && !is_newer_than(path, &output_path) {
        return Ok(());
    }

    if log_file {
        log!("content"; "{}", rel_path);
    }

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::copy(path, output_path)?;
    Ok(())
}

/// Process all non-content files in the content directory
///
/// Copies all files that are not pages (.typ, .md) to the output directory,
/// preserving the directory structure.
///
/// ```text
/// content/
/// ├── index.typ           -> (page, skipped)
/// ├── about.typ           -> (page, skipped)
/// ├── about/
/// │   └── photo.png       -> public/about/photo.png
/// └── posts/
///     ├── hello.typ       -> (page, skipped)
///     └── hello/
///         └── image.png   -> public/posts/hello/image.png
/// ```
///
/// Returns the number of files copied
pub fn process_content_assets(config: &SiteConfig, clean: bool) -> Result<usize> {
    let content_dir = &config.build.content;
    let output_dir = config.paths().output_dir();

    if !content_dir.exists() {
        return Ok(0);
    }

    let mut count = 0;
    copy_content_assets_recursive(content_dir, content_dir, &output_dir, clean, &mut count)?;
    Ok(count)
}

/// Recursively copy non-content files from content directory to output
fn copy_content_assets_recursive(
    dir: &Path,
    content_root: &Path,
    output_root: &Path,
    clean: bool,
    count: &mut usize,
) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };

    for entry in entries.flatten() {
        let src_path = entry.path();

        if src_path.is_dir() {
            // Recursively process subdirectories
            copy_content_assets_recursive(&src_path, content_root, output_root, clean, count)?;
        } else {
            // Skip content files (.typ, .md) - they are pages, not assets
            if ContentKind::from_path(&src_path).is_some() {
                continue;
            }

            // Compute output path: content/a/b/file.png -> output/a/b/file.png
            let rel_path = src_path.strip_prefix(content_root).unwrap_or(&src_path);
            let dest_path = output_root.join(rel_path);

            // Skip if destination is fresh
            if !clean && dest_path.exists() && !is_newer_than(&src_path, &dest_path) {
                continue;
            }

            // Create parent directory and copy file
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&src_path, &dest_path)?;
            *count += 1;
        }
    }

    Ok(())
}

/// Process flatten assets (files that go to output root)
///
/// Returns the number of files processed
pub fn process_flatten_assets(config: &SiteConfig, clean: bool, log_file: bool) -> Result<usize> {
    let assets = super::scan_flatten_assets(config);
    let mut count = 0;

    for route in assets {
        // Skip if up-to-date (use mtime comparison for assets)
        if !clean && route.output.exists() && !is_newer_than(&route.source, &route.output) {
            continue;
        }

        if log_file {
            log!("assets"; "{}", relative_path(&route.source, config));
        }

        if let Some(parent) = route.output.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::copy(&route.source, &route.output)?;
        count += 1;
    }

    Ok(count)
}

/// Generate CNAME file if needed
///
/// Auto-generates CNAME from `site.url` domain when:
/// 1. `site.url` is defined with a custom domain
/// 2. No flatten entry outputs as "CNAME", or the source file doesn't exist
pub fn process_cname(config: &SiteConfig) -> Result<bool> {
    use super::generated::should_generate_cname;

    let domain = should_generate_cname(
        config.site.info.url.as_deref(),
        &config.build.assets.flatten,
        config.get_root(),
    );

    if let Some(domain) = domain {
        let output_dir = config.paths().output_dir();
        let cname_path = output_dir.join("CNAME");
        fs::write(&cname_path, &domain)?;
        crate::debug!("assets"; "generated CNAME: {}", domain);
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_process_content_assets_empty() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();

        let mut config = SiteConfig::default();
        config.build.content = content_dir;
        config.build.output = dir.path().join("public");

        let count = process_content_assets(&config, true).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_process_content_assets_simple() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let about_dir = content_dir.join("about");
        fs::create_dir_all(&about_dir).unwrap();

        // Create content file (should be skipped)
        fs::write(content_dir.join("about.typ"), "= About").unwrap();
        // Create asset files (should be copied)
        fs::write(about_dir.join("photo.png"), "fake png").unwrap();
        fs::write(about_dir.join("style.css"), "body {}").unwrap();

        let output_dir = dir.path().join("public");
        let mut config = SiteConfig::default();
        config.build.content = content_dir;
        config.build.output = output_dir.clone();

        let count = process_content_assets(&config, true).unwrap();
        assert_eq!(count, 2);
        assert!(output_dir.join("about/photo.png").exists());
        assert!(output_dir.join("about/style.css").exists());
        assert!(!output_dir.join("about.typ").exists());
    }

    #[test]
    fn test_process_content_assets_nested() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        let posts_dir = content_dir.join("posts");
        let hello_dir = posts_dir.join("hello");
        fs::create_dir_all(&hello_dir).unwrap();

        // Create content files (should be skipped)
        fs::write(posts_dir.join("hello.typ"), "= Hello").unwrap();
        // Create asset files (should be copied)
        fs::write(hello_dir.join("image.png"), "fake png").unwrap();

        let output_dir = dir.path().join("public");
        let mut config = SiteConfig::default();
        config.build.content = content_dir;
        config.build.output = output_dir.clone();

        let count = process_content_assets(&config, true).unwrap();
        assert_eq!(count, 1);
        assert!(output_dir.join("posts/hello/image.png").exists());
    }

    #[test]
    fn test_process_content_assets_skips_content_files() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();

        // Create various files
        fs::write(content_dir.join("index.typ"), "= Home").unwrap();
        fs::write(content_dir.join("about.md"), "# About").unwrap();
        fs::write(content_dir.join("logo.png"), "fake png").unwrap();

        let output_dir = dir.path().join("public");
        let mut config = SiteConfig::default();
        config.build.content = content_dir;
        config.build.output = output_dir.clone();

        let count = process_content_assets(&config, true).unwrap();
        assert_eq!(count, 1); // Only logo.png
        assert!(output_dir.join("logo.png").exists());
        assert!(!output_dir.join("index.typ").exists());
        assert!(!output_dir.join("about.md").exists());
    }

    #[test]
    fn test_process_content_assets_incremental() {
        use std::thread;
        use std::time::Duration;

        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();
        fs::write(content_dir.join("image.png"), "fake png").unwrap();

        let output_dir = dir.path().join("public");
        let mut config = SiteConfig::default();
        config.build.content = content_dir.clone();
        config.build.output = output_dir;

        // First copy (clean mode)
        let count = process_content_assets(&config, true).unwrap();
        assert_eq!(count, 1);

        // Second copy (incremental mode) - should skip since dest is newer
        let count = process_content_assets(&config, false).unwrap();
        assert_eq!(count, 0);

        // Wait a bit and modify source file
        thread::sleep(Duration::from_millis(10));
        fs::write(content_dir.join("image.png"), "modified png").unwrap();

        // Third copy (incremental mode) - should copy since source is newer
        let count = process_content_assets(&config, false).unwrap();
        assert_eq!(count, 1);
    }
}
