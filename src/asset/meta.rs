//! Asset path and URL resolution.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::config::SiteConfig;
use crate::core::UrlPath;

use super::{AssetKind, AssetRoute};



/// Create an `AssetRoute` from a source path in a nested assets directory.
///
/// This is for global assets only. Use `scan::scan_colocated_assets` for colocated assets.
///
/// # Errors
///
/// Returns an error if the source path is not within any configured assets directory.
pub fn route_from_source(source: PathBuf, config: &SiteConfig) -> Result<AssetRoute> {
    let output_dir = config.paths().output_dir();

    // Find which nested directory contains this source
    for entry in &config.build.assets.nested {
        let assets_dir = entry.source();
        if let Ok(relative) = source.strip_prefix(assets_dir) {
            let prefix = entry.output_name();
            let url = UrlPath::from_asset(&format!("/{}/{}", prefix, relative.display()));
            let output = output_dir.join(prefix).join(relative);

            return Ok(AssetRoute {
                source,
                url,
                output,
                kind: AssetKind::Global,
            });
        }
    }

    Err(anyhow!(
        "File is not in any configured assets directory: {}",
        source.display()
    ))
}

/// Get relative path from assets directory (for logging).
pub fn relative_path(source: &Path, config: &SiteConfig) -> String {
    // Try each nested directory
    for entry in &config.build.assets.nested {
        let assets_dir = entry.source();
        if let Ok(rel) = source.strip_prefix(assets_dir) {
            return rel.display().to_string();
        }
    }
    source.display().to_string()
}



/// Generate a URL path from an output file path.
///
/// Handles path prefix stripping and cross-platform separators.
///
/// # Errors
///
/// Returns an error if the path is not within the output directory.
pub fn url_from_output_path(path: &Path, config: &SiteConfig) -> Result<String> {
    let output_root = &config.build.output;

    // Strip output root
    let rel_to_output = path
        .strip_prefix(output_root)
        .map_err(|_| anyhow!("Path is not in output directory: {}", path.display()))?;

    // Convert to string and ensure forward slashes
    let path_str = rel_to_output.to_string_lossy().replace('\\', "/");

    // Ensure it starts with /
    let url = if path_str.starts_with('/') {
        path_str
    } else {
        format!("/{path_str}")
    };

    Ok(url)
}



/// Compute href for an asset path (relative to site root).
///
/// The path in config (e.g., `build.header.styles`) should be the **full physical path**
/// relative to site root, like `"vendor/static/style.css"`.
///
/// # Example
///
/// ```toml
/// [build.assets]
/// nested = [{ dir = "vendor/static", as = "lib" }]
/// flatten = [{ file = "icons/fav.ico", as = "favicon.ico" }]
///
/// [build.header]
/// styles = ["vendor/static/style.css"]  # Full path relative to site root
/// icon = "icons/fav.ico"                # Full path relative to site root
/// ```
///
/// This generates URLs: `/lib/style.css` and `/favicon.ico`
///
/// # Errors
///
/// Returns an error if the asset path is not within any configured asset entry.
pub fn compute_asset_href(asset_path: &Path, config: &SiteConfig) -> Result<String> {
    // Strip common prefixes: "./"
    let normalized = asset_path.strip_prefix("./").unwrap_or(asset_path);

    // Convert to absolute path for comparison (entries are normalized to absolute)
    let root = config.get_root();
    let abs_path = crate::utils::path::normalize_path(&root.join(normalized));

    // Try flatten entries first (exact file match)
    for entry in &config.build.assets.flatten {
        if abs_path == entry.source() {
            // Flatten files go to output root
            return Ok(format!("/{}", entry.output_name()));
        }
    }

    // Try nested entries (directory prefix match)
    for entry in &config.build.assets.nested {
        let source = entry.source();
        if abs_path.starts_with(source) {
            let relative = abs_path.strip_prefix(source).unwrap_or(&abs_path);
            let rel_str = relative.to_string_lossy();
            let rel_clean = rel_str.trim_start_matches('/');
            let output_name = entry.output_name();

            if rel_clean.is_empty() {
                return Ok(format!("/{}/", output_name));
            } else {
                return Ok(format!("/{}/{}", output_name, rel_clean));
            }
        }
    }

    // Not found - error with helpful message
    Err(anyhow!(
        "Asset path '{}' is not in any configured asset entry. \
         Path should be relative to site root. \
         Configured nested: {:?}, flatten: {:?}",
        asset_path.display(),
        config.build.assets.nested.iter()
            .map(|e| e.source().strip_prefix(root).unwrap_or(e.source()).display().to_string())
            .collect::<Vec<_>>(),
        config.build.assets.flatten.iter()
            .map(|e| e.source().strip_prefix(root).unwrap_or(e.source()).display().to_string())
            .collect::<Vec<_>>()
    ))
}



#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_route_from_source() {
        let dir = TempDir::new().unwrap();

        // Create assets directory
        let assets_dir = dir.path().join("assets");
        fs::create_dir_all(&assets_dir).unwrap();
        let source = assets_dir.join("images/logo.png");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "fake png").unwrap();

        let mut config = SiteConfig::default();
        config.build.assets.nested = vec![
            crate::config::section::build::assets::NestedEntry::Simple(assets_dir),
        ];
        config.build.output = dir.path().join("public");

        let route = route_from_source(source.clone(), &config).unwrap();

        assert_eq!(route.source, source);
        assert_eq!(route.url.as_str(), "/assets/images/logo.png");
        assert_eq!(route.output, dir.path().join("public/assets/images/logo.png"));
        assert_eq!(route.kind, AssetKind::Global);
    }

    #[test]
    fn test_route_from_source_not_in_assets() {
        let dir = TempDir::new().unwrap();

        let mut config = SiteConfig::default();
        config.build.assets.nested = vec![
            crate::config::section::build::assets::NestedEntry::Simple(dir.path().join("assets")),
        ];

        let source = dir.path().join("other/file.txt");
        let result = route_from_source(source, &config);

        assert!(result.is_err());
    }

    #[test]
    fn test_url_from_output_path() {
        let dir = TempDir::new().unwrap();

        let mut config = SiteConfig::default();
        config.build.output = dir.path().join("public");

        let path = dir.path().join("public/assets/logo.png");
        let url = url_from_output_path(&path, &config).unwrap();

        assert_eq!(url, "/assets/logo.png");
    }

    #[test]
    fn test_compute_asset_href() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();

        // Create assets directory
        let assets_dir = root.join("assets");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::write(assets_dir.join("style.css"), "body {}").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(&root);
        config.build.assets.nested = vec![
            crate::config::section::build::assets::NestedEntry::Simple(assets_dir),
        ];
        config.build.output = root.join("public");

        // Path relative to site root
        let href = compute_asset_href(Path::new("assets/style.css"), &config).unwrap();
        assert_eq!(href, "/assets/style.css");
    }

    #[test]
    fn test_compute_asset_href_with_as() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();

        // Create vendor/static directory (multi-level)
        let vendor_dir = root.join("vendor/static");
        fs::create_dir_all(&vendor_dir).unwrap();
        fs::write(vendor_dir.join("app.js"), "// js").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(&root);
        // dir = "vendor/static", as = "lib"
        config.build.assets.nested = vec![
            crate::config::section::build::assets::NestedEntry::Full {
                dir: vendor_dir,
                output_as: Some("lib".to_string()),
            },
        ];
        config.build.output = root.join("public");

        // Full physical path relative to site root
        let href = compute_asset_href(Path::new("vendor/static/app.js"), &config).unwrap();
        assert_eq!(href, "/lib/app.js");

        // Using "lib/app.js" (the output name) should NOT work
        let result = compute_asset_href(Path::new("lib/app.js"), &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_compute_asset_href_flatten() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().unwrap();

        // Create flatten file
        let icons_dir = root.join("icons");
        fs::create_dir_all(&icons_dir).unwrap();
        fs::write(icons_dir.join("fav.ico"), "icon").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(&root);
        config.build.assets.flatten = vec![
            crate::config::section::build::assets::FlattenEntry::Full {
                file: icons_dir.join("fav.ico"),
                output_as: Some("favicon.ico".to_string()),
            },
        ];
        config.build.output = root.join("public");

        // Full physical path relative to site root
        let href = compute_asset_href(Path::new("icons/fav.ico"), &config).unwrap();
        assert_eq!(href, "/favicon.ico");
    }
}
