//! Asset scanning functions (pure, no side effects).

use std::path::Path;

use crate::config::SiteConfig;
use crate::core::ContentKind;

use super::AssetRoute;
use super::route::{
    route_from_content_source, route_from_flatten_source, route_from_nested_source,
};

/// Scan all nested asset routes.
pub fn scan_nested_assets(config: &SiteConfig) -> Vec<AssetRoute> {
    let mut results = Vec::new();

    for entry in &config.build.assets.nested {
        let source = entry.source();
        if source.exists() {
            scan_dir_recursive(&mut results, source, config);
        }
    }

    results
}

fn scan_dir_recursive(results: &mut Vec<AssetRoute>, dir: &Path, config: &SiteConfig) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir_recursive(results, &path, config);
        } else if let Some(route) = route_from_nested_source(&path, config) {
            results.push(route);
        }
    }
}

/// Scan all file asset routes.
pub fn scan_flatten_assets(config: &SiteConfig) -> Vec<AssetRoute> {
    config
        .build
        .assets
        .flatten
        .iter()
        .filter(|entry| entry.source().is_file())
        .filter_map(|entry| route_from_flatten_source(entry.source(), config))
        .collect()
}

pub fn scan_content_assets(config: &SiteConfig) -> Vec<AssetRoute> {
    let content_dir = &config.build.content;

    if !config.build.assets.colocated || !content_dir.exists() {
        return vec![];
    }

    let mut results = Vec::new();
    scan_content_recursive(&mut results, content_dir, config);
    results
}

fn scan_content_recursive(results: &mut Vec<AssetRoute>, dir: &Path, config: &SiteConfig) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            scan_content_recursive(results, &path, config);
        } else if ContentKind::from_path(&path).is_none()
            && let Some(route) = route_from_content_source(&path, config)
        {
            results.push(route);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::AssetKind;
    use crate::config::section::build::assets::{FlattenEntry, NestedEntry};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn scan_nested_assets_from_directory() {
        let dir = TempDir::new().unwrap();
        let images = dir.path().join("assets/images");
        fs::create_dir_all(&images).unwrap();
        fs::write(images.join("logo.png"), "fake png").unwrap();
        fs::write(images.join("style.css"), "body {}").unwrap();

        let mut config = SiteConfig::default();
        config.build.assets.nested = vec![NestedEntry::new(images, "/images")];
        config.build.output = dir.path().join("public");

        let assets = scan_nested_assets(&config);

        assert_eq!(assets.len(), 2);
        assert!(assets.iter().any(|a| a.url == "/images/logo.png"));
        assert!(assets.iter().any(|a| a.url == "/images/style.css"));
        assert!(assets.iter().all(|a| a.kind == AssetKind::Global));
    }

    #[test]
    fn scan_flatten_assets_uses_explicit_urls() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("assets/CNAME");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "example.com").unwrap();

        let mut config = SiteConfig::default();
        config.build.assets.flatten = vec![FlattenEntry::new(source, "/CNAME")];
        config.build.output = dir.path().join("public");

        let assets = scan_flatten_assets(&config);

        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].url, "/CNAME");
        assert_eq!(assets[0].output, dir.path().join("public/CNAME"));
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
