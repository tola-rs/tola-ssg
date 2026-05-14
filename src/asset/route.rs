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
    let source = normalize_source_path(&source);

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

/// Create an `AssetRoute` from a config field that stores a physical source path.
///
/// Config asset fields may be root-relative (`assets/app.css`) or already
/// normalized to an absolute path. The route itself is still owned by
/// `build.assets`.
pub fn route_from_config_source(path: &Path, config: &SiteConfig) -> Result<AssetRoute> {
    route_from_source(config_source_path(path, config), config)
}

/// Create a route for a nested global asset source.
pub fn route_from_nested_source(source: &Path, config: &SiteConfig) -> Option<AssetRoute> {
    let source = normalize_source_path(source);
    let output_dir = config.paths().output_dir();

    for entry in &config.build.assets.nested {
        let base = normalize_source_path(entry.source());
        let Ok(relative) = source.strip_prefix(&base) else {
            continue;
        };
        let rel_url = path_to_url(relative);
        let url = UrlPath::from_asset(&join_url(entry.output_name(), &rel_url));
        let output = output_dir.join(entry.output_name()).join(relative);

        return Some(AssetRoute {
            source: source.clone(),
            url,
            output,
            kind: AssetKind::Global,
        });
    }

    None
}

/// Create a route for a flatten global asset source.
pub fn route_from_flatten_source(source: &Path, config: &SiteConfig) -> Option<AssetRoute> {
    let source = normalize_source_path(source);
    let output_dir = config.paths().output_dir();

    for entry in &config.build.assets.flatten {
        if source == normalize_source_path(entry.source()) {
            let output_name = entry.output_name();
            return Some(AssetRoute {
                source: source.clone(),
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
    let source = normalize_source_path(source);
    let content = normalize_source_path(&config.build.content);
    let relative = source.strip_prefix(&content).ok()?.to_path_buf();
    if ContentKind::from_path(source.as_path()).is_some()
        || !config.build.assets.colocated
        || relative.as_os_str().is_empty()
    {
        return None;
    }

    let rel_url = path_to_url(&relative);

    Some(AssetRoute {
        source,
        url: UrlPath::from_asset(&rel_url),
        output: config.paths().output_dir().join(&relative),
        kind: AssetKind::Content,
    })
}

/// Get relative path from the asset owner directory for logging.
pub fn relative_path(source: &Path, config: &SiteConfig) -> String {
    let source = normalize_source_path(source);

    for entry in &config.build.assets.flatten {
        if source == normalize_source_path(entry.source()) {
            return entry.output_name().to_string();
        }
    }

    for entry in &config.build.assets.nested {
        let base = normalize_source_path(entry.source());
        if let Ok(rel) = source.strip_prefix(base) {
            return path_to_url(rel);
        }
    }

    if let Ok(rel) = source.strip_prefix(normalize_source_path(&config.build.content)) {
        return path_to_url(rel);
    }

    source.display().to_string()
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

    source_for_logical_asset_url(&matched.logical, config).is_some_and(|source| source.is_file())
}

/// Resolve a site-root asset URL back to its configured source file.
///
/// Query strings, fragments, and `path_prefix` are accepted. Generated system
/// assets do not have user source files and return `None`.
pub fn source_for_asset_url(value: &str, config: &SiteConfig) -> Option<PathBuf> {
    if !value.starts_with('/') || value.starts_with("//") {
        return None;
    }

    let (path, _) = split_url_suffix(value);
    let matched = asset_url_match(path.trim_start_matches('/'), config)?;
    if is_system_asset_url(&matched.logical) {
        return None;
    }

    source_for_logical_asset_url(&matched.logical, config)
}

/// Suggest a likely fix for a missing site-root asset URL.
pub fn asset_url_hint(value: &str, config: &SiteConfig) -> Option<String> {
    if !value.starts_with('/') || value.starts_with("//") {
        return None;
    }

    let (path, suffix) = split_url_suffix(value);
    let logical = path.trim_start_matches('/');
    if logical.is_empty() {
        return None;
    }

    let root = normalize_path(config.get_root());
    let source = normalize_path(&root.join(Path::new(logical)));
    if !source.is_file() {
        return None;
    }

    if let Ok(route) = route_from_source(source.clone(), config) {
        let suggested = format!("{}{}", route.url.as_str(), suffix);
        if suggested != value {
            return Some(format!(
                "did you mean `{suggested}`? `{}` is a source path; configured assets are linked by their output URL",
                path.trim_start_matches('/')
            ));
        }
    }

    unconfigured_source_hint(&source, &root)
}

/// Suggest a likely fix for a config field that points at an unowned asset
/// source path.
pub fn asset_source_hint(path: &Path, config: &SiteConfig) -> Option<String> {
    let source = config_source_path(path, config);
    if !source.is_file() || route_from_source(source.clone(), config).is_ok() {
        return None;
    }

    unconfigured_source_hint(&source, &normalize_path(config.get_root()))
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
        .any(|entry| nested_asset_rest(path, entry.output_name()).is_some())
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
    if route_from_content_source(&source, config).is_some_and(|route| route.source.is_file()) {
        return true;
    }

    has_asset_like_extension(Path::new(path))
        && ContentKind::from_path(Path::new(path)).is_none()
        && config.build.assets.colocated
}

fn source_for_logical_asset_url(path: &str, config: &SiteConfig) -> Option<PathBuf> {
    for entry in &config.build.assets.flatten {
        if path == entry.output_name() {
            return Some(normalize_source_path(entry.source()));
        }
    }

    for entry in &config.build.assets.nested {
        let output_name = entry.output_name();
        if let Some(rest) = nested_asset_rest(path, output_name) {
            return Some(normalize_source_path(&entry.source().join(rest)));
        }
    }

    let content_source = config.build.content.join(path);
    route_from_content_source(&content_source, config).map(|route| route.source)
}

fn config_source_path(path: &Path, config: &SiteConfig) -> PathBuf {
    let path = path.strip_prefix("./").unwrap_or(path);
    if path.is_absolute() {
        normalize_source_path(path)
    } else {
        normalize_source_path(&config.get_root().join(path))
    }
}

fn normalize_source_path(path: &Path) -> PathBuf {
    if path.exists() {
        return normalize_path(path);
    }

    let Some(parent) = path.parent() else {
        return normalize_path(path);
    };

    if parent == path {
        return normalize_path(path);
    }

    let parent = normalize_source_path(parent);
    match path.file_name() {
        Some(name) => parent.join(name),
        None => normalize_path(path),
    }
}

fn unconfigured_source_hint(source: &Path, root: &Path) -> Option<String> {
    let rel = source
        .strip_prefix(root)
        .ok()
        .map(path_to_url)
        .unwrap_or_else(|| source.display().to_string());

    let hint = match Path::new(&rel).parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            let top = Path::new(&rel)
                .components()
                .next()?
                .as_os_str()
                .to_string_lossy();
            format!(
                "file exists at `{rel}`, but it is not covered by `build.assets.nested`; add `nested = [\"{top}\"]` or move it under a configured asset directory"
            )
        }
        _ => format!(
            "file exists at `{rel}`, but it is not covered by `build.assets.flatten`; add `flatten = [\"{rel}\"]` or move it under a configured asset directory"
        ),
    };

    Some(hint)
}

fn nested_asset_rest<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(prefix)?.strip_prefix('/')?;
    (!rest.is_empty()).then_some(rest)
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

        assert_eq!(route.source, normalize_path(&source));
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

    #[test]
    fn nested_asset_namespace_root_is_not_an_asset_file() {
        let dir = TempDir::new().unwrap();
        let assets_dir = dir.path().join("assets");
        fs::create_dir_all(&assets_dir).unwrap();

        let mut config = SiteConfig::default();
        config.build.assets.nested = vec![NestedEntry::Simple(assets_dir)];

        assert!(!is_asset_url("/assets", &config));
        assert!(!asset_url_exists("/assets", &config));
    }

    #[test]
    fn colocated_directory_url_is_not_an_asset_file() {
        let dir = TempDir::new().unwrap();
        let content_dir = dir.path().join("content");
        fs::create_dir_all(content_dir.join("posts")).unwrap();

        let mut config = SiteConfig::default();
        config.build.content = content_dir;
        config.build.assets.colocated = true;

        assert!(!is_asset_url("/posts", &config));
        assert!(!asset_url_exists("/posts", &config));
    }

    #[test]
    fn asset_url_hint_suggests_nested_output_url_for_source_shaped_url() {
        let dir = TempDir::new().unwrap();
        let images_dir = dir.path().join("assets/images");
        fs::create_dir_all(&images_dir).unwrap();
        fs::write(images_dir.join("logo.png"), "image").unwrap();

        let mut config = SiteConfig::default();
        config.root = dir.path().to_path_buf();
        config.build.assets.nested = vec![NestedEntry::Simple(crate::utils::path::normalize_path(
            &images_dir,
        ))];

        let hint = asset_url_hint("/assets/images/logo.png", &config).unwrap();

        assert!(hint.contains("`/images/logo.png`"));
        assert!(hint.contains("source path"));
    }

    #[test]
    fn asset_url_hint_suggests_nested_config_for_existing_unconfigured_file() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("images")).unwrap();
        fs::write(dir.path().join("images/logo.png"), "image").unwrap();

        let mut config = SiteConfig::default();
        config.root = dir.path().to_path_buf();
        config.build.assets.nested = vec![NestedEntry::Simple(crate::utils::path::normalize_path(
            &dir.path().join("assets"),
        ))];

        let hint = asset_url_hint("/images/logo.png", &config).unwrap();

        assert!(hint.contains("not covered by `build.assets.nested`"));
        assert!(hint.contains("nested = [\"images\"]"));
    }

    #[test]
    fn source_for_asset_url_uses_nested_output_namespace() {
        let dir = TempDir::new().unwrap();
        let images_dir = dir.path().join("assets/images");
        let source_raw = images_dir.join("logo.png");
        fs::create_dir_all(&images_dir).unwrap();
        fs::write(&source_raw, "image").unwrap();
        let source = crate::utils::path::normalize_path(&source_raw);

        let mut config = SiteConfig::default();
        config.root = dir.path().to_path_buf();
        config.build.assets.nested = vec![NestedEntry::Simple(crate::utils::path::normalize_path(
            &images_dir,
        ))];

        assert_eq!(
            source_for_asset_url("/images/logo.png?v=1", &config),
            Some(source)
        );
        assert_eq!(
            source_for_asset_url("/assets/images/logo.png", &config),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn route_from_config_source_normalizes_missing_file_parent() {
        let dir = TempDir::new().unwrap();
        let real_root = dir.path().join("site");
        let link_root = dir.path().join("site-link");
        let assets_dir = real_root.join("assets");
        fs::create_dir_all(&assets_dir).unwrap();
        std::os::unix::fs::symlink(&real_root, &link_root).unwrap();

        let mut config = SiteConfig::default();
        config.root = link_root;
        config.build.output = real_root.join("public");
        config.build.assets.nested = vec![NestedEntry::Simple(normalize_path(&assets_dir))];

        let route = route_from_config_source(Path::new("assets/app.css"), &config).unwrap();

        assert_eq!(route.source, normalize_path(&assets_dir).join("app.css"));
        assert_eq!(route.url, "/assets/app.css");
    }
}
