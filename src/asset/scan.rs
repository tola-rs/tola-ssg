//! Asset scanning functions (pure, no side effects).

use std::path::Path;

use crate::config::SiteConfig;
use crate::core::{ContentKind, UrlPath};
use crate::page::PageRoute;

use super::{AssetKind, AssetRoute};

/// Scan global assets directory.
///
/// Returns all assets found in the configured nested asset directories
/// with their computed URLs and output paths.
///
/// # Move-type Flatten Files
///
/// Files configured as flatten with `type = "move"` (the default) are
/// **skipped** during nested scanning. They will only be output to the
/// flatten location. Files with `type = "copy"` are included here and
/// also in `scan_flatten_assets`.
///
/// # Pure Function
///
/// This function only reads the filesystem and returns data.
/// It does not modify any state.
pub fn scan_global_assets(config: &SiteConfig) -> Vec<AssetRoute> {
    let output_root = config.paths().output_dir();
    let assets_config = &config.build.assets;
    let mut results = Vec::new();

    // Scan each nested directory
    for entry in &assets_config.nested {
        let assets_dir = entry.source();
        if !assets_dir.exists() {
            continue;
        }

        let prefix = entry.output_name();
        scan_global_recursive(
            &mut results,
            assets_dir,
            assets_dir,
            &output_root,
            prefix,
            assets_config,
        );
    }

    results
}

/// Recursive helper for scanning global assets.
///
/// Skips move-type flatten files to avoid duplicate output.
fn scan_global_recursive(
    results: &mut Vec<AssetRoute>,
    dir: &Path,
    base: &Path,
    output_root: &Path,
    prefix: &str,
    assets_config: &crate::config::AssetsConfig,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_global_recursive(results, &path, base, output_root, prefix, assets_config);
        } else {
            // Skip flatten files (they only output to root directory)
            if assets_config.is_flatten(&path) {
                continue;
            }

            let rel = path.strip_prefix(base).unwrap_or(&path);
            let url = UrlPath::from_asset(&format!("/{}/{}", prefix, rel.display()));
            let output = output_root.join(prefix).join(rel);

            results.push(AssetRoute {
                source: path,
                url,
                output,
                kind: AssetKind::Global,
            });
        }
    }
}

/// Scan flatten assets (individual files copied to output root).
///
/// Returns all flatten assets with their computed URLs and output paths.
/// Flatten files are copied directly to the output root directory.
///
/// # Example
///
/// ```toml
/// [build.assets]
/// flatten = [
///     "CNAME",                                    # → output/CNAME
///     { file = "icons/fav.ico", as = "favicon.ico" }, # → output/favicon.ico
/// ]
/// ```
///
/// # Pure Function
///
/// This function only reads the filesystem and returns data.
pub fn scan_flatten_assets(config: &SiteConfig) -> Vec<AssetRoute> {
    let output_root = config.paths().output_dir();
    let mut results = Vec::new();

    for entry in &config.build.assets.flatten {
        let source = entry.source();
        if !source.exists() {
            continue;
        }

        // Flatten files go directly to output root
        let output_name = entry.output_name();
        let url = UrlPath::from_asset(&format!("/{}", output_name));
        let output = output_root.join(output_name);

        results.push(AssetRoute {
            source: source.to_path_buf(),
            url,
            output,
            kind: AssetKind::Global, // Flatten files are treated as global
        });
    }

    results
}

/// Scan colocated assets for a single page.
///
/// Colocated assets are files that live alongside a content file:
/// ```text
/// content/posts/
/// ├── hello.typ           → page
/// └── hello/              → colocated_dir
///     ├── image.png       → colocated asset
///     └── assets/
///         └── logo.svg    → nested colocated asset
/// ```
///
/// # Arguments
///
/// * `colocated_dir` - Directory containing colocated assets
/// * `route` - Page route (for URL and output path computation)
///
/// # Pure Function
///
/// This function only reads the filesystem and returns data.
pub fn scan_colocated_assets(colocated_dir: &Path, route: &PageRoute) -> Vec<AssetRoute> {
    if !colocated_dir.exists() {
        return vec![];
    }

    let mut results = Vec::new();
    scan_colocated_recursive(
        &mut results,
        colocated_dir,
        colocated_dir,
        route.permalink.as_str(),
        &route.output_dir,
        route.is_index,
    );
    results
}

/// Recursive helper for scanning colocated assets.
fn scan_colocated_recursive(
    results: &mut Vec<AssetRoute>,
    dir: &Path,
    base_dir: &Path,
    base_permalink: &str,
    base_output: &Path,
    _is_index: bool,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            // Skip directories that are colocated dirs for other pages
            // A directory is another page's colocated dir if there's a .typ or .md file
            // with the same name in the parent directory
            // e.g., content/test_colocated/ is skipped if content/test_colocated.typ exists
            let dir_name = path.file_name().unwrap_or_default();
            let parent = path.parent().unwrap_or(Path::new(""));
            let has_typ = parent
                .join(format!("{}.typ", dir_name.to_string_lossy()))
                .exists();
            let has_md = parent
                .join(format!("{}.md", dir_name.to_string_lossy()))
                .exists();
            if has_typ || has_md {
                continue;
            }

            // Compute relative path from base for nested directories
            let rel_from_base = path.strip_prefix(base_dir).unwrap_or(&path);
            let nested_output = base_output.join(rel_from_base);

            scan_colocated_recursive(
                results,
                &path,
                base_dir,
                base_permalink,
                &nested_output,
                false, // Nested dirs are never index
            );
            continue;
        }

        // Always skip content files (.typ, .md) - they are pages, not assets
        // This applies to both index and non-index colocated directories
        if ContentKind::from_path(&path).is_some() {
            continue;
        }

        // Compute URL and output path
        let rel_path = path.strip_prefix(base_dir).unwrap_or(&path);
        let url = UrlPath::from_asset(&format!(
            "{}/{}",
            base_permalink.trim_end_matches('/'),
            rel_path.display()
        ));
        let output = base_output.join(rel_path);

        results.push(AssetRoute {
            source: path,
            url,
            output,
            kind: AssetKind::Colocated,
        });
    }
}

/// Scan all assets for a collection of pages.
///
/// This combines global assets, flatten assets, and colocated assets from all pages
/// into a single list. Useful for conflict detection and address space building.
///
/// # Arguments
///
/// * `pages` - All pages (for colocated assets)
/// * `config` - Site configuration (for global and flatten assets)
///
/// # Pure Function
///
/// This function only reads the filesystem and returns data.
#[allow(dead_code)]
pub fn scan_all_assets(
    pages: &[crate::page::CompiledPage],
    config: &SiteConfig,
) -> Vec<AssetRoute> {
    let mut results = scan_global_assets(config);
    results.extend(scan_flatten_assets(config));

    for page in pages {
        if let Some(dir) = &page.route.colocated_dir {
            results.extend(scan_colocated_assets(dir, &page.route));
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_route(
        colocated_dir: Option<PathBuf>,
        output_dir: PathBuf,
        permalink: &str,
        _is_index: bool,
    ) -> PageRoute {
        PageRoute {
            source: PathBuf::from("test.typ"),
            is_index: _is_index,
            is_404: false,
            colocated_dir,
            permalink: UrlPath::from_page(permalink),
            output_file: output_dir.join("index.html"),
            output_dir,
            full_url: format!("https://example.com{}", permalink),
            relative: "test".to_string(),
        }
    }

    #[test]
    fn test_scan_colocated_empty() {
        let dir = TempDir::new().unwrap();
        let route = make_route(None, dir.path().to_path_buf(), "/test/", false);

        // Non-existent directory
        let assets = scan_colocated_assets(Path::new("/nonexistent"), &route);
        assert!(assets.is_empty());
    }

    #[test]
    fn test_scan_colocated_simple() {
        let dir = TempDir::new().unwrap();

        // Create colocated directory with files
        let colocated = dir.path().join("hello");
        fs::create_dir_all(&colocated).unwrap();
        fs::write(colocated.join("image.png"), "fake png").unwrap();
        fs::write(colocated.join("style.css"), "body {}").unwrap();

        let output_dir = dir.path().join("public/posts/hello");
        let route = make_route(
            Some(colocated.clone()),
            output_dir.clone(),
            "/posts/hello/",
            false,
        );

        let assets = scan_colocated_assets(&colocated, &route);

        assert_eq!(assets.len(), 2);
        assert!(assets.iter().any(|a| a.url == "/posts/hello/image.png"));
        assert!(assets.iter().any(|a| a.url == "/posts/hello/style.css"));
        assert!(assets.iter().all(|a| a.kind == AssetKind::Colocated));
    }

    #[test]
    fn test_scan_colocated_nested() {
        let dir = TempDir::new().unwrap();

        // Create nested structure
        let colocated = dir.path().join("hello");
        let nested = colocated.join("assets");
        fs::create_dir_all(&nested).unwrap();
        fs::write(colocated.join("image.png"), "fake png").unwrap();
        fs::write(nested.join("logo.svg"), "<svg></svg>").unwrap();

        let output_dir = dir.path().join("public/posts/hello");
        let route = make_route(
            Some(colocated.clone()),
            output_dir.clone(),
            "/posts/hello/",
            false,
        );

        let assets = scan_colocated_assets(&colocated, &route);

        assert_eq!(assets.len(), 2);
        assert!(assets.iter().any(|a| a.url == "/posts/hello/image.png"));
        assert!(
            assets
                .iter()
                .any(|a| a.url == "/posts/hello/assets/logo.svg")
        );
    }

    #[test]
    fn test_scan_colocated_skips_content_files() {
        let dir = TempDir::new().unwrap();

        // Content files (.typ, .md) should always be skipped - they are pages, not assets
        let colocated = dir.path().join("posts");
        fs::create_dir_all(&colocated).unwrap();
        fs::write(colocated.join("index.typ"), "= Index").unwrap();
        fs::write(colocated.join("other.md"), "# Other").unwrap();
        fs::write(colocated.join("image.png"), "fake png").unwrap();

        // Test with is_index = true
        let output_dir = dir.path().join("public/posts");
        let route = make_route(Some(colocated.clone()), output_dir.clone(), "/posts/", true);
        let assets = scan_colocated_assets(&colocated, &route);
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].url, "/posts/image.png");

        // Test with is_index = false - should still skip content files
        let route = make_route(
            Some(colocated.clone()),
            output_dir.clone(),
            "/posts/",
            false,
        );
        let assets = scan_colocated_assets(&colocated, &route);
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].url, "/posts/image.png");
    }

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
    fn test_scan_all_assets_no_duplicates() {
        let dir = TempDir::new().unwrap();

        // Create assets directory
        let assets_dir = dir.path().join("assets");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::write(assets_dir.join("logo.png"), "fake png").unwrap();
        fs::write(assets_dir.join("CNAME"), "example.com").unwrap();

        let mut config = SiteConfig::default();
        config.build.assets.nested =
            vec![crate::config::section::build::assets::NestedEntry::Simple(
                assets_dir.clone(),
            )];
        config.build.assets.flatten =
            vec![crate::config::section::build::assets::FlattenEntry::Simple(
                assets_dir.join("CNAME"),
            )];
        config.build.output = dir.path().join("public");

        // scan_all_assets should have no duplicates
        let all_assets = scan_all_assets(&[], &config);

        // Count occurrences of the CNAME source file
        let cname_source = assets_dir.join("CNAME");
        let cname_count = all_assets
            .iter()
            .filter(|a| a.source == cname_source)
            .count();

        // CNAME should appear exactly once (from flatten, not from nested)
        assert_eq!(cname_count, 1);

        // And it should be at the flatten URL
        let cname_asset = all_assets
            .iter()
            .find(|a| a.source == cname_source)
            .unwrap();
        assert_eq!(cname_asset.url, "/CNAME");
    }
}
