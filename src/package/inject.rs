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

use super::{Phase, TolaPackage};

/// Typed specification for base virtual-package injection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InjectSpec {
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
            include_format: false,
        }
    }

    /// Toggle site payload.
    pub const fn with_site(mut self, include_site: bool) -> Self {
        self.include_site = include_site;
        self
    }

    /// Toggle pages payload.
    pub const fn with_pages(mut self, include_pages: bool) -> Self {
        self.include_pages = include_pages;
        self
    }

    /// Toggle format helper.
    pub const fn with_format(mut self, include_format: bool) -> Self {
        self.include_format = include_format;
        self
    }
}

/// Build base inputs (`site/pages/phase/format`) without file-specific `current` context.
pub fn build_base_inputs(
    config: &SiteConfig,
    store: &StoredPageMap,
    spec: InjectSpec,
) -> Result<typst_batch::Inputs> {
    let mut combined = serde_json::Map::new();

    if spec.include_site {
        let site_info_json = serde_json::to_value(&config.site.info)
            .unwrap_or(serde_json::Value::Object(Default::default()));
        combined.insert(TolaPackage::Site.input_key(), site_info_json);
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
    .map_err(|e| anyhow!("failed to build virtual-package inputs: {}", e))
}

/// Merge `@tola/current` payload into existing inputs.
pub fn merge_current_context(
    inputs: &mut typst_batch::Inputs,
    store: &StoredPageMap,
    permalink: &UrlPath,
    source_rel: Option<&str>,
) -> Result<()> {
    let current_context = store.build_current_context(permalink, source_rel);
    inputs
        .merge_json(&current_context)
        .map_err(|e| anyhow!("failed to merge @tola/current inputs: {}", e))
}

/// Build inputs for a specific source file, including `@tola/current`.
pub fn build_inputs_for_source(
    config: &SiteConfig,
    store: &StoredPageMap,
    source_path: &Path,
    spec: InjectSpec,
) -> Result<typst_batch::Inputs> {
    let mut inputs = build_base_inputs(config, store, spec)?;
    let normalized = normalize_path(source_path);

    // Resolve permalink from source mapping first. If absent, derive from route.
    let permalink = if let Some(url) = store.get_permalink_by_source(source_path) {
        url
    } else {
        let page =
            crate::compiler::page::CompiledPage::from_paths(&normalized, config).map_err(|e| {
                anyhow!(
                    "failed to derive permalink for {}: {}",
                    source_path.display(),
                    e
                )
            })?;
        let url = page.route.permalink.clone();
        store.insert_source_mapping(normalized.clone(), url.clone());
        url
    };

    let content_dir = normalize_path(&config.build.content);
    let source_rel = normalized
        .strip_prefix(&content_dir)
        .ok()
        .map(|p| p.to_string_lossy().to_string());

    merge_current_context(&mut inputs, store, &permalink, source_rel.as_deref())?;
    Ok(inputs)
}
