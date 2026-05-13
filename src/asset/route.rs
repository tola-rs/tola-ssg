//! Asset route: source -> URL -> output mapping.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::config::SiteConfig;
use crate::core::ContentKind;
use crate::core::UrlPath;
use crate::utils::path::normalize_path;

use super::AssetKind;

/// Reserved directory for system-generated assets.
pub const SYSTEM_ASSET_DIR: &str = ".tola";

/// Route information for a static asset
///
/// This is the single source of truth for asset path mapping
/// Used by both scanning and address space registration
#[derive(Debug, Clone)]
pub struct AssetRoute {
    /// Source file path (absolute)
    pub source: PathBuf,
    /// URL path (e.g., "/assets/logo.png" or "/posts/hello/image.png")
    pub url: UrlPath,
    /// Output file path (absolute)
    pub output: PathBuf,
    /// Asset kind (Global or Content)
    pub kind: AssetKind,
}

/// Create an `AssetRoute` from a configured asset source path.
///
/// The route URL is site-root-relative and does not include `path_prefix`.
/// The output path always includes `path_prefix` through `config.paths()`.
pub fn route_from_source(source: PathBuf, config: &SiteConfig) -> Result<AssetRoute> {
    if let Some(route) = route_from_flatten_source(&source, config) {
        return Ok(route);
    }

    if let Some(route) = route_from_nested_source(&source, config) {
        return Ok(route);
    }

    if let Some(route) = route_from_content_source(&source, config) {
        return Ok(route);
    }

    Err(anyhow!(
        "File is not in any configured asset entry: {}",
        source.display()
    ))
}

/// Create a route for a nested global asset source.
pub fn route_from_nested_source(source: &Path, config: &SiteConfig) -> Option<AssetRoute> {
    let output_dir = config.paths().output_dir();

    for entry in &config.build.assets.nested {
        let base = entry.source();
        let Ok(relative) = source.strip_prefix(base) else {
            continue;
        };
        let rel_url = path_to_url(relative);
        let url = UrlPath::from_asset(&join_url(entry.output_name(), &rel_url));
        let output = output_dir.join(entry.output_name()).join(relative);

        return Some(AssetRoute {
            source: source.to_path_buf(),
            url,
            output,
            kind: AssetKind::Global,
        });
    }

    None
}

/// Create a route for a flatten global asset source.
pub fn route_from_flatten_source(source: &Path, config: &SiteConfig) -> Option<AssetRoute> {
    let output_dir = config.paths().output_dir();

    for entry in &config.build.assets.flatten {
        if source == entry.source() {
            let output_name = entry.output_name();
            return Some(AssetRoute {
                source: source.to_path_buf(),
                url: UrlPath::from_asset(output_name),
                output: output_dir.join(output_name),
                kind: AssetKind::Global,
            });
        }
    }

    None
}

/// Create a route for an allowed colocated content asset source.
pub fn route_from_content_source(source: &Path, config: &SiteConfig) -> Option<AssetRoute> {
    if ContentKind::from_path(source).is_some()
        || !config
            .build
            .assets
            .contains_colocated_source(source, &config.build.content)
    {
        return None;
    }

    let relative = source.strip_prefix(&config.build.content).ok()?;
    let rel_url = path_to_url(relative);

    Some(AssetRoute {
        source: source.to_path_buf(),
        url: UrlPath::from_asset(&rel_url),
        output: config.paths().output_dir().join(relative),
        kind: AssetKind::Content,
    })
}

/// Get relative path from the asset owner directory for logging.
pub fn relative_path(source: &Path, config: &SiteConfig) -> String {
    for entry in &config.build.assets.flatten {
        if source == entry.source() {
            return entry.output_name().to_string();
        }
    }

    for entry in &config.build.assets.nested {
        if let Ok(rel) = source.strip_prefix(entry.source()) {
            return path_to_url(rel);
        }
    }

    if let Ok(rel) = source.strip_prefix(&config.build.content) {
        return path_to_url(rel);
    }

    source.display().to_string()
}

/// Compute a browser href for a configured user asset path.
///
/// The input path is a physical path relative to the site root, matching
/// `site.header.icon`, `site.header.styles`, and similar config fields.
pub fn compute_asset_href(asset_path: &Path, config: &SiteConfig) -> Result<String> {
    let normalized = asset_path.strip_prefix("./").unwrap_or(asset_path);
    let abs_path = normalize_path(&config.get_root().join(normalized));
    let route = route_from_source(abs_path, config)?;
    Ok(href_for_route(&route, config))
}

/// Convert an asset route URL into a browser href, applying `path_prefix`.
pub fn href_for_route(route: &AssetRoute, config: &SiteConfig) -> String {
    config.paths().url_for_site_path(route.url.as_str())
}

/// Resolve a site-root link as an asset browser href.
///
/// Returns `None` when the URL is not owned by user assets or the reserved
/// generated asset namespace. Query strings and fragments are preserved.
pub fn resolve_asset_href(value: &str, config: &SiteConfig) -> Option<String> {
    let (path, suffix) = split_url_suffix(value);
    let path = path.trim_start_matches('/');
    let matched = asset_url_match(path, config)?;

    if matched.had_prefix {
        Some(format!("/{}{}", path, suffix))
    } else {
        Some(format!(
            "{}{}",
            config.paths().url_for_site_path(&matched.logical),
            suffix
        ))
    }
}

/// Returns true if a site-root URL points at an asset namespace.
pub fn is_asset_url(value: &str, config: &SiteConfig) -> bool {
    let (path, _) = split_url_suffix(value);
    asset_url_match(path.trim_start_matches('/'), config).is_some()
}

/// Returns true when a site-root URL is a generated asset namespace or an
/// existing configured user asset.
pub fn asset_url_exists(value: &str, config: &SiteConfig) -> bool {
    let (path, _) = split_url_suffix(value);
    let matched = match asset_url_match(path.trim_start_matches('/'), config) {
        Some(matched) => matched,
        None => return false,
    };

    if is_system_asset_url(&matched.logical) {
        return true;
    }

    source_for_logical_asset_url(&matched.logical, config).is_some_and(|source| source.exists())
}

struct AssetUrlMatch {
    logical: String,
    had_prefix: bool,
}

fn asset_url_match(path: &str, config: &SiteConfig) -> Option<AssetUrlMatch> {
    if is_logical_asset_url(path, config) {
        return Some(AssetUrlMatch {
            logical: path.to_string(),
            had_prefix: false,
        });
    }

    let prefix = path_prefix_to_url(&config.build.path_prefix);
    if prefix.is_empty() {
        return None;
    }

    let stripped = strip_url_prefix(path, &prefix)?;
    if is_logical_asset_url(stripped, config) {
        return Some(AssetUrlMatch {
            logical: stripped.to_string(),
            had_prefix: true,
        });
    }

    None
}

fn is_logical_asset_url(path: &str, config: &SiteConfig) -> bool {
    is_system_asset_url(path)
        || is_nested_asset_url(path, config)
        || is_flatten_asset_url(path, config)
        || is_colocated_asset_url(path, config)
}

fn is_system_asset_url(path: &str) -> bool {
    path == SYSTEM_ASSET_DIR
        || path
            .strip_prefix(SYSTEM_ASSET_DIR)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn is_nested_asset_url(path: &str, config: &SiteConfig) -> bool {
    config
        .build
        .assets
        .nested
        .iter()
        .any(|entry| segment_prefix_matches(path, entry.output_name()))
}

fn is_flatten_asset_url(path: &str, config: &SiteConfig) -> bool {
    config
        .build
        .assets
        .flatten
        .iter()
        .any(|entry| path == entry.output_name())
}

fn is_colocated_asset_url(path: &str, config: &SiteConfig) -> bool {
    let source = config.build.content.join(path);
    if route_from_content_source(&source, config).is_some_and(|route| route.source.exists()) {
        return true;
    }

    has_asset_like_extension(Path::new(path))
        && ContentKind::from_path(Path::new(path)).is_none()
        && config.build.assets.colocated
}

fn source_for_logical_asset_url(path: &str, config: &SiteConfig) -> Option<PathBuf> {
    for entry in &config.build.assets.flatten {
        if path == entry.output_name() {
            return Some(entry.source().to_path_buf());
        }
    }

    for entry in &config.build.assets.nested {
        let output_name = entry.output_name();
        if path == output_name {
            return Some(entry.source().to_path_buf());
        }
        if let Some(rest) = path.strip_prefix(output_name)
            && let Some(rest) = rest.strip_prefix('/')
        {
            return Some(entry.source().join(rest));
        }
    }

    let content_source = config.build.content.join(path);
    route_from_content_source(&content_source, config).map(|route| route.source)
}

fn segment_prefix_matches(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn strip_url_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if path == prefix {
        Some("")
    } else {
        path.strip_prefix(prefix)?.strip_prefix('/')
    }
}

fn path_prefix_to_url(prefix: &Path) -> String {
    path_to_url(prefix)
}

fn split_url_suffix(value: &str) -> (&str, &str) {
    let idx = value.find(['?', '#']).unwrap_or(value.len());
    (&value[..idx], &value[idx..])
}

fn join_url(first: &str, second: &str) -> String {
    match (first.trim_matches('/'), second.trim_matches('/')) {
        ("", "") => String::new(),
        (first, "") => first.to_string(),
        ("", second) => second.to_string(),
        (first, second) => format!("{first}/{second}"),
    }
}

fn path_to_url(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_matches('/')
        .to_string()
}

fn has_asset_like_extension(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SiteConfig;
    use crate::config::section::build::assets::{FlattenEntry, NestedEntry};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn nested_route_keeps_url_site_root_relative_and_output_prefixed() {
        let dir = TempDir::new().unwrap();
        let assets_dir = dir.path().join("assets");
        fs::create_dir_all(&assets_dir).unwrap();
        let source = assets_dir.join("app.css");
        fs::write(&source, "body{}").unwrap();

        let mut config = SiteConfig::default();
        config.build.output = dir.path().join("public");
        config.build.path_prefix = "docs/blog".into();
        config.build.assets.nested = vec![NestedEntry::Simple(assets_dir)];

        let route = route_from_source(source.clone(), &config).unwrap();

        assert_eq!(route.source, source);
        assert_eq!(route.url, "/assets/app.css");
        assert_eq!(
            route.output,
            dir.path().join("public/docs/blog/assets/app.css")
        );
        assert_eq!(href_for_route(&route, &config), "/docs/blog/assets/app.css");
    }

    #[test]
    fn flatten_route_outputs_under_prefixed_root() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("favicon.ico");
        fs::write(&source, "icon").unwrap();

        let mut config = SiteConfig::default();
        config.build.output = dir.path().join("public");
        config.build.path_prefix = "docs/blog".into();
        config.build.assets.flatten = vec![FlattenEntry::Simple(source.clone())];

        let route = route_from_source(source, &config).unwrap();

        assert_eq!(route.url, "/favicon.ico");
        assert_eq!(
            route.output,
            dir.path().join("public/docs/blog/favicon.ico")
        );
        assert_eq!(href_for_route(&route, &config), "/docs/blog/favicon.ico");
    }

    #[test]
    fn content_route_requires_colocated_permission() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(content_dir.join("posts")).unwrap();
        let source = content_dir.join("posts/image.png");
        fs::write(&source, "image").unwrap();

        let mut config = SiteConfig::default();
        config.build.content = content_dir;
        config.build.output = dir.path().join("public");
        config.build.path_prefix = "docs/blog".into();

        assert!(route_from_source(source.clone(), &config).is_err());

        config.build.assets.colocated = true;
        let route = route_from_source(source, &config).unwrap();

        assert_eq!(route.url, "/posts/image.png");
        assert_eq!(
            route.output,
            dir.path().join("public/docs/blog/posts/image.png")
        );
    }

    #[test]
    fn colocated_url_classification_does_not_require_existing_file_for_asset_extensions() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(&content_dir).unwrap();

        let mut config = SiteConfig::default();
        config.build.content = content_dir;
        config.build.assets.colocated = true;

        assert!(is_asset_url("/posts/missing.png", &config));
        assert!(!asset_url_exists("/posts/missing.png", &config));
        assert!(!is_asset_url("/posts/missing", &config));
    }
}
