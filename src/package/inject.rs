//! Typed virtual-package input injection helpers.
//!
//! This module centralizes `sys.inputs` construction for `@tola/*` packages
//! to keep behavior consistent across build/query/serve/validate paths.

use std::path::Path;

use anyhow::{Result, anyhow};
use typst_batch::codegen::ConvertError;

use crate::config::SiteConfig;
use crate::core::UrlPath;
use crate::page::{PageState, StoredPageMap};
use crate::utils::path::normalize_path;

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

fn tola_meta_field(path: &str) -> Option<String> {
    let pages_key = TolaPackage::Pages.input_key();

    let mut segments = path.trim_start_matches('/').split('/');
    if segments.next()? != pages_key {
        return None;
    }
    let _page_index = segments.next()?;

    let mut field = Vec::new();
    for segment in segments {
        if is_content_path_segment(segment) {
            break;
        }
        field.push(unescape_path_segment(segment));
    }

    (!field.is_empty()).then(|| field.join("."))
}

fn is_content_path_segment(segment: &str) -> bool {
    matches!(
        segment,
        "func" | "children" | "body" | "child" | "text" | "styles"
    )
}

fn unescape_path_segment(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

fn unsupported_content_hint(path: &str) -> Option<&'static str> {
    path.starts_with(&format!("/{}", TolaPackage::Pages.input_key())).then_some(
        "<tola-meta> fields can be shared across files, so they must be portable static metadata.",
    )
}

fn virtual_package_input_error(error: ConvertError, extra_hints: bool) -> anyhow::Error {
    match error {
        ConvertError::Unsupported { path, .. } => {
            let detail = if let Some(field) = tola_meta_field(&path) {
                format!("unsupported contextual content in <tola-meta> field `{field}`")
            } else {
                "unsupported contextual content in <tola-meta>".to_string()
            };
            if extra_hints && let Some(hint) = unsupported_content_hint(&path) {
                anyhow!(
                    "{}\n\n{} {}\n",
                    detail,
                    crate::logger::style_hint("hint:"),
                    hint
                )
            } else {
                anyhow!(detail)
            }
        }
        error => anyhow!("failed to build virtual-package inputs: {}", error),
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
        combined.insert(
            TolaPackage::Pages.input_key(),
            store.pages_to_json_value_with_drafts(),
        );
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
    .map_err(|error| virtual_package_input_error(error, config.build.extra_hints))
}

/// Merge `@tola/current` payload into existing inputs.
fn merge_current_context_value(
    inputs: &mut typst_batch::Inputs,
    current_context: &serde_json::Value,
) -> Result<()> {
    inputs
        .merge_json(current_context)
        .map_err(|e| anyhow!("failed to merge @tola/current inputs: {}", e))
}

fn merge_current_context(
    inputs: &mut typst_batch::Inputs,
    store: &StoredPageMap,
    permalink: &UrlPath,
    path_rel: Option<&str>,
) -> Result<()> {
    let current_context = PageState::new(store).build_current_context(permalink, path_rel);
    merge_current_context_value(inputs, &current_context)
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
        page.route.permalink
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

    merge_current_context(&mut inputs, store, &permalink, path_rel.as_deref())?;
    Ok(inputs)
}

/// Build visible-phase base inputs.
pub fn build_visible_inputs(
    config: &SiteConfig,
    store: &StoredPageMap,
) -> Result<typst_batch::Inputs> {
    build_base_inputs_impl(config, store, InjectSpec::visible())
}

/// Build visible-phase base inputs and merge a caller-provided `@tola/current` payload.
pub fn build_visible_inputs_with_current_context(
    config: &SiteConfig,
    store: &StoredPageMap,
    current_context: &serde_json::Value,
) -> Result<typst_batch::Inputs> {
    let mut inputs = build_base_inputs_impl(config, store, InjectSpec::visible())?;
    merge_current_context_value(&mut inputs, current_context)?;
    Ok(inputs)
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
    Ok(PageState::new(store).build_current_context(&permalink, path_rel.as_deref()))
}

/// Build visible-phase `@tola/current` inputs for a specific source.
pub fn build_visible_current_inputs_for_source(
    config: &SiteConfig,
    store: &StoredPageMap,
    file_path: &Path,
) -> Result<typst_batch::Inputs> {
    let current = build_visible_current_context_for_source(config, store, file_path)?;
    typst_batch::Inputs::from_json(&current)
        .map_err(|e| anyhow!("failed to build @tola/current inputs: {}", e))
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
    fn test_build_visible_current_context_for_source_uses_site_permalink() {
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

    #[test]
    fn test_build_visible_inputs_reports_unsupported_metadata_path() {
        let dir = TempDir::new().unwrap();
        let mut config = SiteConfig::default();
        config.set_root(dir.path());

        let store = StoredPageMap::new();
        store.insert_page(
            UrlPath::from_page("/a/"),
            crate::page::PageMeta {
                summary: Some(serde_json::json!({"func": "context"})),
                ..Default::default()
            },
        );

        let err = match build_visible_inputs(&config, &store) {
            Ok(_) => panic!("context should be rejected"),
            Err(err) => err,
        };
        let message = err.to_string();

        assert!(message.contains("unsupported contextual content in <tola-meta> field `summary`"));
        assert!(message.contains("hint:"));
        assert!(message.contains("portable static metadata"));
    }

    #[test]
    fn test_build_visible_inputs_respects_extra_hints_flag() {
        let dir = TempDir::new().unwrap();
        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.extra_hints = false;

        let store = StoredPageMap::new();
        store.insert_page(
            UrlPath::from_page("/a/"),
            crate::page::PageMeta {
                summary: Some(serde_json::json!({"func": "context"})),
                ..Default::default()
            },
        );

        let err = match build_visible_inputs(&config, &store) {
            Ok(_) => panic!("context should be rejected"),
            Err(err) => err,
        };
        let message = err.to_string();

        assert_eq!(
            message,
            "unsupported contextual content in <tola-meta> field `summary`"
        );
    }

    #[test]
    fn test_build_visible_inputs_reports_nested_metadata_field() {
        let dir = TempDir::new().unwrap();
        let mut config = SiteConfig::default();
        config.set_root(dir.path());
        config.build.extra_hints = false;

        let store = StoredPageMap::new();
        let mut extra = crate::page::JsonMap::new();
        extra.insert(
            "card".to_string(),
            serde_json::json!({"summary": {"func": "context"}}),
        );
        store.insert_page(
            UrlPath::from_page("/a/"),
            crate::page::PageMeta {
                extra,
                ..Default::default()
            },
        );

        let err = match build_visible_inputs(&config, &store) {
            Ok(_) => panic!("context should be rejected"),
            Err(err) => err,
        };

        assert_eq!(
            err.to_string(),
            "unsupported contextual content in <tola-meta> field `card.summary`"
        );
    }
}
