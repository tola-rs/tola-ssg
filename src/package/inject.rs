//! Typed virtual-package input injection helpers.
//!
//! This module centralizes `sys.inputs` construction for `@tola/*` packages
//! to keep behavior consistent across build/query/serve/validate paths.

use std::path::Path;

use anyhow::{Result, anyhow};

use crate::config::SiteConfig;
use crate::core::UrlPath;
use crate::page::StoredPageMap;
use crate::utils::path::normalize_path;
use crate::utils::path::route::strip_path_prefix_from_page_url;

use super::{Phase, TolaPackage};

/// Typed specification for base virtual-package injection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InjectSpec {
    /// Virtual package phase (`filter` or `visible`).
    pub phase: Phase,
    /// Include `@tola/site` payload.
    pub include_site: bool,
    /// Include `@tola/pages` payload.
    pub include_pages: bool,
    /// Include `format = "html"` helper flag.
    pub include_format: bool,
}

impl InjectSpec {
    /// Default visible-phase injection used by compile/query paths.
    pub const fn visible() -> Self {
        Self {
            phase: Phase::Visible,
            include_site: true,
            include_pages: true,
            include_format: true,
        }
    }

    /// Default filter-phase injection used by lightweight scan/filter paths.
    pub const fn filter() -> Self {
        Self {
            phase: Phase::Filter,
            include_site: false,
            include_pages: false,
            // Scan/filter phase can still execute user templates; keep `format`
            // available so `sys.inputs.at("format", ...)` and legacy
            // `sys.inputs.format` checks don't fail.
            include_format: true,
        }
    }

    /// Toggle site payload.
    pub const fn with_site(mut self, include_site: bool) -> Self {
        self.include_site = include_site;
        self
    }

    /// Toggle pages payload.
    #[allow(dead_code)]
    const fn with_pages(mut self, include_pages: bool) -> Self {
        self.include_pages = include_pages;
        self
    }

    /// Toggle format helper.
    #[allow(dead_code)]
    const fn with_format(mut self, include_format: bool) -> Self {
        self.include_format = include_format;
        self
    }
}

fn validate_spec(spec: InjectSpec, needs_current: bool) -> Result<()> {
    match spec.phase {
        Phase::Visible => {
            if !spec.include_site || !spec.include_pages {
                anyhow::bail!(
                    "invalid visible injection contract: @tola/site and @tola/pages are required"
                );
            }
        }
        Phase::Filter => {
            if needs_current {
                anyhow::bail!(
                    "invalid filter injection contract: @tola/current is not available in filter phase"
                );
            }
        }
    }
    Ok(())
}

fn path_prefix(config: &SiteConfig) -> String {
    config.paths().prefix().to_string_lossy().into_owned()
}

fn site_payload(config: &SiteConfig) -> serde_json::Value {
    let mut site = serde_json::to_value(&config.site.info)
        .unwrap_or(serde_json::Value::Object(Default::default()));

    let root = match path_prefix(config) {
        prefix if prefix.is_empty() => "/".to_string(),
        prefix => format!("/{}/", prefix.trim_matches('/')),
    };

    if let Some(obj) = site.as_object_mut() {
        obj.insert("root".to_string(), serde_json::Value::String(root));
    }

    site
}

fn strip_permalink_in_page_object(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    prefix: &str,
) {
    if let Some(permalink_value) = obj.get_mut("permalink")
        && let Some(permalink) = permalink_value.as_str()
    {
        *permalink_value =
            serde_json::Value::String(strip_path_prefix_from_page_url(permalink, prefix));
    }
}

fn strip_pages_permalinks(value: &mut serde_json::Value, prefix: &str) {
    let Some(arr) = value.as_array_mut() else {
        return;
    };
    for page in arr {
        if let Some(obj) = page.as_object_mut() {
            strip_permalink_in_page_object(obj, prefix);
        }
    }
}

fn strip_current_context_permalinks(value: &mut serde_json::Value, prefix: &str) {
    let key = TolaPackage::Current.input_key();
    let Some(current) = value.get_mut(key).and_then(|v| v.as_object_mut()) else {
        return;
    };

    if let Some(v) = current.get_mut("current-permalink")
        && let Some(s) = v.as_str()
    {
        *v = serde_json::Value::String(strip_path_prefix_from_page_url(s, prefix));
    }

    if let Some(v) = current.get_mut("parent-permalink")
        && let Some(s) = v.as_str()
    {
        *v = serde_json::Value::String(strip_path_prefix_from_page_url(s, prefix));
    }

    if let Some(links_to) = current.get_mut("links_to") {
        strip_pages_permalinks(links_to, prefix);
    }
    if let Some(linked_by) = current.get_mut("linked_by") {
        strip_pages_permalinks(linked_by, prefix);
    }
}

fn build_base_inputs_impl(
    config: &SiteConfig,
    store: &StoredPageMap,
    spec: InjectSpec,
) -> Result<typst_batch::Inputs> {
    validate_spec(spec, false)?;

    let mut combined = serde_json::Map::new();

    if spec.include_site {
        combined.insert(TolaPackage::Site.input_key(), site_payload(config));
    }

    if spec.include_pages {
        let mut pages_payload = store.pages_to_json_value_with_drafts();
        strip_pages_permalinks(&mut pages_payload, &path_prefix(config));
        combined.insert(TolaPackage::Pages.input_key(), pages_payload);
    }

    combined.insert(
        Phase::input_key().to_string(),
        serde_json::json!(spec.phase.as_str()),
    );

    if spec.include_format {
        combined.insert("format".to_string(), serde_json::json!("html"));
    }

    typst_batch::Inputs::from_json_with_content(
        &serde_json::Value::Object(combined),
        config.get_root(),
    )
    .map_err(|e| anyhow!("failed to build virtual-package inputs: {}", e))
}

/// Merge `@tola/current` payload into existing inputs.
fn merge_current_context(
    config: &SiteConfig,
    inputs: &mut typst_batch::Inputs,
    store: &StoredPageMap,
    permalink: &UrlPath,
    path_rel: Option<&str>,
) -> Result<()> {
    let mut current_context = store.build_current_context(permalink, path_rel);
    strip_current_context_permalinks(&mut current_context, &path_prefix(config));
    inputs
        .merge_json(&current_context)
        .map_err(|e| anyhow!("failed to merge @tola/current inputs: {}", e))
}

fn resolve_source_context(
    config: &SiteConfig,
    store: &StoredPageMap,
    file_path: &Path,
) -> Result<(UrlPath, Option<String>)> {
    let normalized = normalize_path(file_path);

    // Resolve permalink from source mapping first. If absent, derive from route.
    let permalink = if let Some(url) = store.get_permalink_by_source(file_path) {
        url
    } else {
        let page =
            crate::compiler::page::CompiledPage::from_paths(&normalized, config).map_err(|e| {
                anyhow!(
                    "failed to derive permalink for {}: {}",
                    file_path.display(),
                    e
                )
            })?;
        let url = page.route.permalink.clone();
        store.insert_source_mapping(normalized.clone(), url.clone());
        url
    };

    let content_dir = normalize_path(&config.build.content);
    let path_rel = normalized
        .strip_prefix(&content_dir)
        .ok()
        .map(|p| p.to_string_lossy().to_string());

    Ok((permalink, path_rel))
}

fn build_inputs_for_source_impl(
    config: &SiteConfig,
    store: &StoredPageMap,
    file_path: &Path,
    spec: InjectSpec,
) -> Result<typst_batch::Inputs> {
    validate_spec(spec, true)?;

    let mut inputs = build_base_inputs_impl(config, store, spec)?;
    let (permalink, path_rel) = resolve_source_context(config, store, file_path)?;

    merge_current_context(config, &mut inputs, store, &permalink, path_rel.as_deref())?;
    Ok(inputs)
}

/// Build visible-phase base inputs.
pub fn build_visible_inputs(
    config: &SiteConfig,
    store: &StoredPageMap,
) -> Result<typst_batch::Inputs> {
    build_base_inputs_impl(config, store, InjectSpec::visible())
}

/// Build filter-phase base inputs with site payload.
pub fn build_filter_inputs_with_site(
    config: &SiteConfig,
    store: &StoredPageMap,
) -> Result<typst_batch::Inputs> {
    build_base_inputs_impl(config, store, InjectSpec::filter().with_site(true))
}

/// Build visible-phase inputs for a specific source, including `@tola/current`.
pub fn build_visible_inputs_for_source(
    config: &SiteConfig,
    store: &StoredPageMap,
    file_path: &Path,
) -> Result<typst_batch::Inputs> {
    build_inputs_for_source_impl(config, store, file_path, InjectSpec::visible())
}

/// Build visible-phase `@tola/current` payload for a specific source.
pub fn build_visible_current_context_for_source(
    config: &SiteConfig,
    store: &StoredPageMap,
    file_path: &Path,
) -> Result<serde_json::Value> {
    validate_spec(InjectSpec::visible(), true)?;
    let (permalink, path_rel) = resolve_source_context(config, store, file_path)?;
    let mut current = store.build_current_context(&permalink, path_rel.as_deref());
    strip_current_context_permalinks(&mut current, &path_prefix(config));
    Ok(current)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use tempfile::TempDir;

    use crate::page::StoredPageMap;

    #[test]
    fn test_visible_spec_requires_site_and_pages() {
        let spec = InjectSpec::visible().with_site(false);
        assert!(validate_spec(spec, false).is_err());
    }

    #[test]
    fn test_filter_spec_rejects_current_context() {
        let spec = InjectSpec::filter();
        assert!(validate_spec(spec, true).is_err());
    }

    #[test]
    fn test_site_payload_exposes_root_from_path_prefix() {
        let mut config = SiteConfig::default();
        config.build.path_prefix = std::path::PathBuf::from("docs/blog");

        let payload = site_payload(&config);

        assert_eq!(payload["root"], "/docs/blog/");
        assert!(payload.get("title").is_some());
    }

    #[test]
    fn test_build_visible_current_context_for_source_includes_path_and_filename() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content_dir = root.join("content");
        fs::create_dir_all(&content_dir).unwrap();

        let file_path = content_dir.join("post.typ");
        fs::write(&file_path, "= Hello").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(root);
        config.build.content = content_dir.clone();

        let store = StoredPageMap::new();
        let current =
            build_visible_current_context_for_source(&config, &store, &file_path).unwrap();

        let key = TolaPackage::Current.input_key();
        let path = current
            .get(&key)
            .and_then(|v| v.get("path"))
            .and_then(|v| v.as_str());
        let filename = current
            .get(&key)
            .and_then(|v| v.get("filename"))
            .and_then(|v| v.as_str());
        let current_permalink = current
            .get(&key)
            .and_then(|v| v.get("current-permalink"))
            .and_then(|v| v.as_str());

        assert_eq!(path, Some("post.typ"));
        assert_eq!(filename, Some("post.typ"));
        assert_eq!(current_permalink, Some("/post/"));
    }

    #[test]
    fn test_build_visible_current_context_for_source_strips_configured_prefix() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let content_dir = root.join("content");
        fs::create_dir_all(&content_dir).unwrap();

        let file_path = content_dir.join("post.typ");
        fs::write(&file_path, "= Hello").unwrap();

        let mut config = SiteConfig::default();
        config.set_root(root);
        config.build.content = content_dir.clone();
        config.build.path_prefix = std::path::PathBuf::from("blog");

        let store = StoredPageMap::new();
        let current =
            build_visible_current_context_for_source(&config, &store, &file_path).unwrap();

        let key = TolaPackage::Current.input_key();
        let current_permalink = current
            .get(&key)
            .and_then(|v| v.get("current-permalink"))
            .and_then(|v| v.as_str());

        assert_eq!(current_permalink, Some("/post/"));
    }
}
