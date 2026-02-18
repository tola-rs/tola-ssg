//! Asset processing with side effects (copying, minification).

use std::fs;
use std::path::Path;

use anyhow::{Result, anyhow};

use crate::config::SiteConfig;
use crate::core::ContentKind;
use crate::freshness::is_newer_than;
use crate::hooks::css;
use crate::log;
use crate::page::PageRoute;

use super::meta::{relative_path, route_from_source};

/// Process an asset file from the assets directory.
///
/// Copies the asset to the output directory, respecting freshness checks.
/// Skips CSS processor input (handled centrally).
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

/// Process an asset file from the content directory (non-.typ files).
///
/// These are files in the content directory that aren't pages.
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

/// Copy colocated assets from source directory to output directory.
///
/// Colocated assets are files that live alongside a content file:
/// ```text
/// content/posts/
/// ├── hello.typ           → public/posts/hello/index.html
/// └── hello/              → colocated_dir
///     ├── image.png       → public/posts/hello/image.png
///     └── assets/
///         └── logo.svg    → public/posts/hello/assets/logo.svg
/// ```
///
/// For index files, all non-content files in the same directory are colocated assets.
///
/// Returns the number of files copied.
pub fn copy_colocated_assets(route: &PageRoute, clean: bool) -> Result<usize> {
    let colocated_dir = match &route.colocated_dir {
        Some(dir) => dir,
        None => return Ok(0),
    };

    if !colocated_dir.exists() {
        return Ok(0);
    }

    let mut count = 0;
    copy_dir_recursive(
        colocated_dir,
        &route.output_dir,
        route.is_index,
        clean,
        &mut count,
    )?;
    Ok(count)
}

/// Recursively copy directory contents, skipping content files for index pages.
fn copy_dir_recursive(
    src_dir: &Path,
    dest_dir: &Path,
    is_index: bool,
    clean: bool,
    count: &mut usize,
) -> Result<()> {
    for entry in fs::read_dir(src_dir)? {
        let entry = entry?;
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dest_dir.join(&file_name);

        if src_path.is_dir() {
            // Recursively copy subdirectories
            copy_dir_recursive(&src_path, &dest_path, false, clean, count)?;
        } else {
            // For index files, skip content files (.typ, .md) in the same directory
            if is_index && ContentKind::from_path(&src_path).is_some() {
                continue;
            }

            // Skip if destination is fresh (use mtime comparison for assets)
            // Note: is_fresh is designed for HTML with embedded hashes, so we use
            // is_newer_than for binary assets which compares modification times.
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

/// Process flatten assets (files that go to output root).
///
/// Returns the number of files processed.
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

/// Generate CNAME file if needed.
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
    use crate::core::UrlPath;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_route(
        colocated_dir: Option<PathBuf>,
        output_dir: PathBuf,
        is_index: bool,
    ) -> PageRoute {
        PageRoute {
            source: PathBuf::from("test.typ"),
            is_index,
            is_404: false,
            colocated_dir,
            permalink: UrlPath::from_page("/test/"),
            output_file: output_dir.join("index.html"),
            output_dir,
            full_url: "https://example.com/test/".to_string(),
            relative: "test".to_string(),
        }
    }

    #[test]
    fn test_copy_colocated_assets_none() {
        let dir = TempDir::new().unwrap();
        let route = make_route(None, dir.path().to_path_buf(), false);
        let count = copy_colocated_assets(&route, true).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_copy_colocated_assets_non_existent() {
        let dir = TempDir::new().unwrap();
        let route = make_route(
            Some(PathBuf::from("/nonexistent/path")),
            dir.path().to_path_buf(),
            false,
        );
        let count = copy_colocated_assets(&route, true).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_copy_colocated_assets_simple() {
        let dir = TempDir::new().unwrap();

        // Create colocated assets directory with files
        let colocated_dir = dir.path().join("hello");
        fs::create_dir_all(&colocated_dir).unwrap();
        fs::write(colocated_dir.join("image.png"), "fake png").unwrap();
        fs::write(colocated_dir.join("style.css"), "body {}").unwrap();

        // Create output directory
        let output_dir = dir.path().join("public/posts/hello");
        fs::create_dir_all(&output_dir).unwrap();

        let route = make_route(Some(colocated_dir), output_dir.clone(), false);
        let count = copy_colocated_assets(&route, true).unwrap();

        assert_eq!(count, 2);
        assert!(output_dir.join("image.png").exists());
        assert!(output_dir.join("style.css").exists());
    }

    #[test]
    fn test_copy_colocated_assets_nested() {
        let dir = TempDir::new().unwrap();

        // Create nested structure
        let colocated_dir = dir.path().join("hello");
        let assets_dir = colocated_dir.join("assets");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::write(colocated_dir.join("image.png"), "fake png").unwrap();
        fs::write(assets_dir.join("logo.svg"), "<svg></svg>").unwrap();

        let output_dir = dir.path().join("public/posts/hello");
        fs::create_dir_all(&output_dir).unwrap();

        let route = make_route(Some(colocated_dir), output_dir.clone(), false);
        let count = copy_colocated_assets(&route, true).unwrap();

        assert_eq!(count, 2);
        assert!(output_dir.join("image.png").exists());
        assert!(output_dir.join("assets/logo.svg").exists());
    }

    #[test]
    fn test_copy_colocated_assets_index_skips_content() {
        let dir = TempDir::new().unwrap();

        // Content files (.typ, .md) should always be skipped - they are pages, not assets
        let colocated_dir = dir.path().join("posts");
        fs::create_dir_all(&colocated_dir).unwrap();
        fs::write(colocated_dir.join("index.typ"), "= Index").unwrap();
        fs::write(colocated_dir.join("other.md"), "# Other").unwrap();
        fs::write(colocated_dir.join("image.png"), "fake png").unwrap();

        let output_dir = dir.path().join("public/posts");
        fs::create_dir_all(&output_dir).unwrap();

        let route = make_route(Some(colocated_dir), output_dir.clone(), true);
        let count = copy_colocated_assets(&route, true).unwrap();

        // Only image.png should be copied, not .typ or .md files
        assert_eq!(count, 1);
        assert!(output_dir.join("image.png").exists());
        assert!(!output_dir.join("index.typ").exists());
        assert!(!output_dir.join("other.md").exists());
    }

    #[test]
    fn test_copy_colocated_assets_incremental() {
        use std::thread;
        use std::time::Duration;

        let dir = TempDir::new().unwrap();

        let colocated_dir = dir.path().join("hello");
        fs::create_dir_all(&colocated_dir).unwrap();
        fs::write(colocated_dir.join("image.png"), "fake png").unwrap();

        let output_dir = dir.path().join("public/posts/hello");
        fs::create_dir_all(&output_dir).unwrap();

        let route = make_route(Some(colocated_dir.clone()), output_dir.clone(), false);

        // First copy (clean mode)
        let count = copy_colocated_assets(&route, true).unwrap();
        assert_eq!(count, 1);

        // Second copy (incremental mode) - should skip since dest is newer
        let count = copy_colocated_assets(&route, false).unwrap();
        assert_eq!(count, 0);

        // Wait a bit and modify source file
        thread::sleep(Duration::from_millis(10));
        fs::write(colocated_dir.join("image.png"), "modified png").unwrap();

        // Third copy (incremental mode) - should copy since source is newer
        let count = copy_colocated_assets(&route, false).unwrap();
        assert_eq!(count, 1);
    }
}
