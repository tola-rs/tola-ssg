//! Asset processing with side effects (copying, minification).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::config::SiteConfig;
use crate::freshness::is_newer_than;

use super::AssetKind;
use super::route::{AssetRoute, relative_path, route_from_source};
use crate::logger;

/// Summary for a batch asset processing pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssetProcessSummary {
    /// Routes returned by the scanner and considered by the processor.
    pub scanned: usize,
    /// Routes that wrote or rewrote an output file.
    pub written: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssetRouteOutcome {
    Skipped,
    Written,
}

/// Process a configured asset file.
///
/// Copies the asset to the output directory, respecting freshness checks
pub fn process_asset(
    asset_path: &Path,
    config: &SiteConfig,
    clean: bool,
    log_file: bool,
) -> Result<()> {
    let route = route_from_source(asset_path.to_path_buf(), config)?;

    process_asset_route(&route, config, clean, log_file).map(|_| ())
}

fn process_asset_route(
    route: &AssetRoute,
    config: &SiteConfig,
    clean: bool,
    log_file: bool,
) -> Result<AssetRouteOutcome> {
    if skip_asset_route(route, config, clean) {
        return Ok(AssetRouteOutcome::Skipped);
    }

    write_asset_route(route, config, log_file)?;
    Ok(AssetRouteOutcome::Written)
}

fn skip_asset_route(route: &AssetRoute, config: &SiteConfig, clean: bool) -> bool {
    is_atomic_css_output_path(&route.output, config)
        || (!clean && route.output.exists() && !is_newer_than(&route.source, &route.output))
}

fn is_atomic_css_output_path(path: &Path, config: &SiteConfig) -> bool {
    crate::css::build::output_asset(config).is_some_and(|asset| {
        crate::utils::path::normalize_path(path) == crate::utils::path::normalize_path(&asset.path)
    })
}

fn write_asset_route(route: &AssetRoute, config: &SiteConfig, log_file: bool) -> Result<()> {
    let asset_path = &route.source;

    if log_file {
        match route.kind {
            AssetKind::Global => logger::log(
                "assets",
                format_args!("{}", relative_path(asset_path, config)),
            ),
            AssetKind::Content => logger::log(
                "content",
                format_args!("{}", relative_path(asset_path, config)),
            ),
        }
    }

    if let Some(parent) = route.output.parent() {
        fs::create_dir_all(parent)?;
    }

    let ext = asset_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();

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

/// Process configured nested assets.
///
/// This uses `scan_nested_assets` so build, serve, validation, and conflict
/// detection share the same source -> URL -> output routing rules.
pub fn process_nested_assets(
    config: &SiteConfig,
    clean: bool,
    log_file: bool,
) -> Result<AssetProcessSummary> {
    let mut summary = AssetProcessSummary::default();

    for route in super::scan_nested_assets(config) {
        summary.scanned += 1;
        match process_asset_route(&route, config, clean, log_file)
            .with_context(|| format!("asset {}", relative_path(&route.source, config)))?
        {
            AssetRouteOutcome::Skipped => {}
            AssetRouteOutcome::Written => summary.written += 1,
        }
    }

    Ok(summary)
}

/// Process every asset source owned by `build.assets`.
pub fn process_configured_assets(
    config: &SiteConfig,
    clean: bool,
    log_file: bool,
) -> Result<AssetProcessSummary> {
    let mut summary = process_nested_assets(config, clean, log_file)?;

    summary.scanned += super::scan_flatten_assets(config).len();
    summary.written += process_flatten_assets(config, clean, log_file)?;

    summary.scanned += super::scan_content_assets(config).len();
    summary.written += process_content_assets(config, clean)?;

    Ok(summary)
}

/// Process all non-page files in the content directory.
///
/// Copies all files that are not pages to the output directory,
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
    let mut count = 0;

    for route in super::scan_content_assets(config) {
        if is_atomic_css_output_path(&route.output, config) {
            continue;
        }

        if !clean && route.output.exists() && !is_newer_than(&route.source, &route.output) {
            continue;
        }

        if let Some(parent) = route.output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&route.source, &route.output)?;
        count += 1;
    }

    Ok(count)
}

/// Process explicitly configured flatten assets.
pub fn process_flatten_assets(config: &SiteConfig, clean: bool, log_file: bool) -> Result<usize> {
    let assets = super::scan_flatten_assets(config);
    let mut count = 0;

    for route in assets {
        if is_atomic_css_output_path(&route.output, config) {
            continue;
        }

        // Skip if up-to-date (use mtime comparison for assets)
        if !clean && route.output.exists() && !is_newer_than(&route.source, &route.output) {
            continue;
        }

        if log_file {
            logger::log(
                "assets",
                format_args!("{}", relative_path(&route.source, config)),
            );
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
/// 2. No flatten asset outputs as "/CNAME", or the source file doesn't exist
pub fn process_cname(config: &SiteConfig) -> Result<()> {
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
        logger::debug("assets", format_args!("generated CNAME: {}", domain));
    }

    Ok(())
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
        config.build.assets.colocated = true;

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
        config.build.assets.colocated = true;

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
        fs::write(content_dir.join("about.typ"), "= About").unwrap();
        fs::write(content_dir.join("logo.png"), "fake png").unwrap();

        let output_dir = dir.path().join("public");
        let mut config = SiteConfig::default();
        config.build.content = content_dir;
        config.build.output = output_dir.clone();
        config.build.assets.colocated = true;

        let count = process_content_assets(&config, true).unwrap();
        assert_eq!(count, 1); // Only logo.png
        assert!(output_dir.join("logo.png").exists());
        assert!(!output_dir.join("index.typ").exists());
        assert!(!output_dir.join("about.typ").exists());
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
        config.build.assets.colocated = true;

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

    #[test]
    fn test_process_content_assets_default_disabled() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();
        fs::write(content_dir.join("logo.png"), "fake png").unwrap();

        let output_dir = dir.path().join("public");
        let mut config = SiteConfig::default();
        config.build.content = content_dir;
        config.build.output = output_dir.clone();

        let count = process_content_assets(&config, true).unwrap();

        assert_eq!(count, 0);
        assert!(!output_dir.join("logo.png").exists());
    }

    #[test]
    fn process_nested_assets_reports_scanned_and_written_routes_separately() {
        use crate::config::section::build::assets::NestedEntry;

        let dir = TempDir::new().unwrap();
        let assets_dir = dir.path().join("assets");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::write(assets_dir.join("logo.png"), "logo").unwrap();

        let mut config = SiteConfig::default();
        config.build.assets.nested = vec![NestedEntry::new(assets_dir, "/assets")];
        config.build.output = dir.path().join("public");

        let first = process_nested_assets(&config, true, false).unwrap();
        let second = process_nested_assets(&config, false, false).unwrap();

        assert_eq!(
            first,
            AssetProcessSummary {
                scanned: 1,
                written: 1
            }
        );
        assert_eq!(
            second,
            AssetProcessSummary {
                scanned: 1,
                written: 0
            }
        );
    }

    #[test]
    fn process_nested_assets_does_not_overwrite_atomic_css_output() {
        use crate::config::section::build::assets::NestedEntry;

        let dir = TempDir::new().unwrap();
        let assets_dir = dir.path().join("assets/.tola");
        let output_dir = dir.path().join("public");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::create_dir_all(output_dir.join(".tola")).unwrap();
        fs::write(assets_dir.join("atomic.css"), "asset css").unwrap();
        fs::write(output_dir.join(".tola/atomic.css"), "generated css").unwrap();

        let mut config = SiteConfig::default();
        config.build.assets.nested = vec![NestedEntry::new(assets_dir, "/.tola")];
        config.build.output = output_dir.clone();
        config.build.css.atomic.enable = true;

        let summary = process_nested_assets(&config, true, false).unwrap();

        assert_eq!(summary.scanned, 1);
        assert_eq!(summary.written, 0);
        assert_eq!(
            fs::read_to_string(output_dir.join(".tola/atomic.css")).unwrap(),
            "generated css"
        );
    }
}
