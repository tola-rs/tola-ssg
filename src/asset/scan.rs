//! Asset scanning functions (pure, no side effects).

use std::path::Path;

use crate::config::SiteConfig;
use crate::core::ContentKind;

use super::AssetRoute;
use super::route::{
    route_from_content_source, route_from_flatten_source, route_from_nested_source,
};

/// Scan global assets directory
///
/// Returns all assets found in the configured nested asset directories
/// with their computed URLs and output paths
///
/// # Flatten Files
///
/// Files configured as flatten are skipped during nested scanning. They are
/// emitted only by `scan_flatten_assets`, so one source file has one output
/// route.
///
/// # Pure Function
///
/// This function only reads the filesystem and returns data
/// It does not modify any state
pub fn scan_global_assets(config: &SiteConfig) -> Vec<AssetRoute> {
    let assets_config = &config.build.assets;
    let mut results = Vec::new();

    // Scan each nested directory
    for entry in &assets_config.nested {
        let assets_dir = entry.source();
        if !assets_dir.exists() {
            continue;
        }

        scan_global_recursive(&mut results, assets_dir, config, assets_config);
    }

    results
}

/// Recursive helper for scanning global assets
///
/// Skips flatten files to avoid duplicate output
fn scan_global_recursive(
    results: &mut Vec<AssetRoute>,
    dir: &Path,
    config: &SiteConfig,
    assets_config: &crate::config::AssetsConfig,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_global_recursive(results, &path, config, assets_config);
        } else {
            // Skip flatten files (they only output to root directory)
            if assets_config.is_flatten(&path) {
                continue;
            }

            if let Some(route) = route_from_nested_source(&path, config) {
                results.push(route);
            }
        }
    }
}

/// Scan flatten assets (individual files copied to output root)
///
/// Returns all flatten assets with their computed URLs and output paths
/// Flatten files are copied directly to the output root directory
///
/// # Example
///
/// ```toml
/// [build.assets]
/// flatten = [
///     "CNAME",                                    # -> output/CNAME
///     { file = "icons/fav.ico", as = "favicon.ico" }, # -> output/favicon.ico
/// ]
/// ```
///
/// # Pure Function
///
/// This function only reads the filesystem and returns data
pub fn scan_flatten_assets(config: &SiteConfig) -> Vec<AssetRoute> {
    let mut results = Vec::new();

    for entry in &config.build.assets.flatten {
        let source = entry.source();
        if !source.is_file() {
            continue;
        }

        if let Some(route) = route_from_flatten_source(source, config) {
            results.push(route);
        }
    }

    results
}

/// Scan content assets (non-page files in content directory)
///
/// Returns all non-page files found in the content directory
/// with their computed URLs and output paths.
///
/// ```text
/// content/
/// ├── index.typ           -> (page, skipped)
/// ├── about.typ           -> (page, skipped)
/// ├── about/
/// │   └── photo.png       -> /about/photo.png
/// └── posts/
///     ├── hello.typ       -> (page, skipped)
///     └── hello/
///         └── image.png   -> /posts/hello/image.png
/// ```
///
/// # Pure Function
///
/// This function only reads the filesystem and returns data
pub fn scan_content_assets(config: &SiteConfig) -> Vec<AssetRoute> {
    let content_dir = &config.build.content;

    if !config.build.assets.colocated || !content_dir.exists() {
        return vec![];
    }

    let mut results = Vec::new();
    scan_content_recursive(&mut results, content_dir, config);
    results
}

/// Recursive helper for scanning content assets
fn scan_content_recursive(results: &mut Vec<AssetRoute>, dir: &Path, config: &SiteConfig) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            scan_content_recursive(results, &path, config);
        } else {
            // Skip page sources; they are compiled, not copied as assets.
            if ContentKind::from_path(&path).is_some() {
                continue;
            }

            if let Some(route) = route_from_content_source(&path, config) {
                results.push(route);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::AssetKind;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_scan_global_empty() {
        let dir = TempDir::new().unwrap();
        let mut config = SiteConfig::default();
        config.build.assets.nested =
            vec![crate::config::section::build::assets::NestedEntry::Simple(
                dir.path().join("nonexistent"),
            )];

        let assets = scan_global_assets(&config);
        assert!(assets.is_empty());
    }

    #[test]
    fn test_scan_global_simple() {
        let dir = TempDir::new().unwrap();

        // Create assets directory
        let assets_dir = dir.path().join("assets");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::write(assets_dir.join("logo.png"), "fake png").unwrap();
        fs::write(assets_dir.join("style.css"), "body {}").unwrap();

        let mut config = SiteConfig::default();
        config.build.assets.nested =
            vec![crate::config::section::build::assets::NestedEntry::Simple(
                assets_dir,
            )];
        config.build.output = dir.path().join("public");

        let assets = scan_global_assets(&config);

        assert_eq!(assets.len(), 2);
        assert!(assets.iter().any(|a| a.url == "/assets/logo.png"));
        assert!(assets.iter().any(|a| a.url == "/assets/style.css"));
        assert!(assets.iter().all(|a| a.kind == AssetKind::Global));
    }

    #[test]
    fn test_scan_global_nested() {
        let dir = TempDir::new().unwrap();

        // Create nested assets
        let assets_dir = dir.path().join("assets");
        let images_dir = assets_dir.join("images");
        fs::create_dir_all(&images_dir).unwrap();
        fs::write(assets_dir.join("logo.png"), "fake png").unwrap();
        fs::write(images_dir.join("photo.jpg"), "fake jpg").unwrap();

        let mut config = SiteConfig::default();
        config.build.assets.nested =
            vec![crate::config::section::build::assets::NestedEntry::Simple(
                assets_dir,
            )];
        config.build.output = dir.path().join("public");

        let assets = scan_global_assets(&config);

        assert_eq!(assets.len(), 2);
        assert!(assets.iter().any(|a| a.url == "/assets/logo.png"));
        assert!(assets.iter().any(|a| a.url == "/assets/images/photo.jpg"));
    }

    #[test]
    fn test_scan_flatten_empty() {
        let dir = TempDir::new().unwrap();

        let mut config = SiteConfig::default();
        config.build.assets.flatten = vec![];
        config.build.output = dir.path().join("public");

        let assets = scan_flatten_assets(&config);
        assert!(assets.is_empty());
    }

    #[test]
    fn test_scan_flatten_simple() {
        let dir = TempDir::new().unwrap();

        // Create flatten files
        fs::write(dir.path().join("CNAME"), "example.com").unwrap();
        fs::write(dir.path().join("robots.txt"), "User-agent: *").unwrap();

        let mut config = SiteConfig::default();
        config.build.assets.flatten = vec![
            crate::config::section::build::assets::FlattenEntry::Simple(dir.path().join("CNAME")),
            crate::config::section::build::assets::FlattenEntry::Simple(
                dir.path().join("robots.txt"),
            ),
        ];
        config.build.output = dir.path().join("public");

        let assets = scan_flatten_assets(&config);

        assert_eq!(assets.len(), 2);
        assert!(assets.iter().any(|a| a.url == "/CNAME"));
        assert!(assets.iter().any(|a| a.url == "/robots.txt"));
        assert!(
            assets
                .iter()
                .all(|a| a.output.parent().unwrap() == dir.path().join("public"))
        );
    }

    #[test]
    fn test_scan_flatten_with_as() {
        let dir = TempDir::new().unwrap();

        // Create flatten file in subdirectory
        let icons_dir = dir.path().join("icons");
        fs::create_dir_all(&icons_dir).unwrap();
        fs::write(icons_dir.join("fav.ico"), "icon data").unwrap();

        let mut config = SiteConfig::default();
        config.build.assets.flatten =
            vec![crate::config::section::build::assets::FlattenEntry::Full {
                file: icons_dir.join("fav.ico"),
                output_as: Some("favicon.ico".to_string()),
            }];
        config.build.output = dir.path().join("public");

        let assets = scan_flatten_assets(&config);

        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].url, "/favicon.ico");
        assert_eq!(assets[0].output, dir.path().join("public/favicon.ico"));
    }

    #[test]
    fn test_scan_flatten_nonexistent() {
        let dir = TempDir::new().unwrap();

        let mut config = SiteConfig::default();
        config.build.assets.flatten =
            vec![crate::config::section::build::assets::FlattenEntry::Simple(
                dir.path().join("does_not_exist.txt"),
            )];
        config.build.output = dir.path().join("public");

        let assets = scan_flatten_assets(&config);
        assert!(assets.is_empty()); // Nonexistent files are skipped
    }

    #[test]
    fn test_scan_flatten_directory_is_not_an_asset_file() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("icons");
        fs::create_dir_all(&source).unwrap();

        let mut config = SiteConfig::default();
        config.build.assets.flatten =
            vec![crate::config::section::build::assets::FlattenEntry::Simple(
                source,
            )];
        config.build.output = dir.path().join("public");

        let assets = scan_flatten_assets(&config);

        assert!(assets.is_empty());
    }

    #[test]
    fn test_scan_global_skips_flatten_files() {
        let dir = TempDir::new().unwrap();

        // Create assets directory with multiple files
        let assets_dir = dir.path().join("assets");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::write(assets_dir.join("logo.png"), "fake png").unwrap();
        fs::write(assets_dir.join("CNAME"), "example.com").unwrap();
        fs::write(assets_dir.join("robots.txt"), "User-agent: *").unwrap();

        let mut config = SiteConfig::default();
        // Configure nested to scan the assets directory
        config.build.assets.nested =
            vec![crate::config::section::build::assets::NestedEntry::Simple(
                assets_dir.clone(),
            )];
        // Configure flatten for CNAME (should be skipped in nested scan)
        config.build.assets.flatten =
            vec![crate::config::section::build::assets::FlattenEntry::Simple(
                assets_dir.join("CNAME"),
            )];
        config.build.output = dir.path().join("public");

        // Scan global assets - should NOT include CNAME
        let global_assets = scan_global_assets(&config);
        assert_eq!(global_assets.len(), 2); // logo.png + robots.txt
        assert!(global_assets.iter().any(|a| a.url == "/assets/logo.png"));
        assert!(global_assets.iter().any(|a| a.url == "/assets/robots.txt"));
        assert!(!global_assets.iter().any(|a| a.url == "/assets/CNAME")); // CNAME excluded

        // Scan flatten assets - should include CNAME
        let flatten_assets = scan_flatten_assets(&config);
        assert_eq!(flatten_assets.len(), 1);
        assert_eq!(flatten_assets[0].url, "/CNAME");
    }

    #[test]
    fn scan_content_assets_default_disabled() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();
        fs::write(content_dir.join("index.typ"), "= Home").unwrap();
        fs::write(content_dir.join("logo.png"), "image").unwrap();

        let mut config = SiteConfig::default();
        config.build.content = content_dir;
        config.build.output = dir.path().join("public");

        assert!(scan_content_assets(&config).is_empty());
    }

    #[test]
    fn scan_content_assets_enabled_with_prefix_output() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(content_dir.join("posts/hello")).unwrap();
        fs::write(content_dir.join("posts/hello.typ"), "= Hello").unwrap();
        fs::write(content_dir.join("posts/hello/image.png"), "image").unwrap();

        let mut config = SiteConfig::default();
        config.build.content = content_dir.clone();
        config.build.output = dir.path().join("public");
        config.build.path_prefix = "docs/blog".into();
        config.build.assets.colocated = true;

        let assets = scan_content_assets(&config);

        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].url, "/posts/hello/image.png");
        assert_eq!(
            assets[0].output,
            dir.path().join("public/docs/blog/posts/hello/image.png")
        );
    }
}
