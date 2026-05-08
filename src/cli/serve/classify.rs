use std::path::{Path, PathBuf};

use crate::address::Resource;
use crate::address::SiteIndex;
use crate::config::SiteConfig;
use crate::core::UrlPath;
use crate::utils::path::normalize_path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ServedOutputKind {
    PageHtml { source: PathBuf },
    RedirectHtml,
    NotFoundHtml,
    Asset,
    GeneratedHtml,
    UnknownHtml,
}

pub(super) fn classify_served_output(
    request_url: &str,
    output_path: &Path,
    config: &SiteConfig,
    state: &SiteIndex,
) -> ServedOutputKind {
    let output_path = normalize_path(output_path);

    if !is_html_path(&output_path) {
        return ServedOutputKind::Asset;
    }

    if is_not_found_output(&output_path, config) {
        return ServedOutputKind::NotFoundHtml;
    }

    let url = UrlPath::from_browser(request_url);
    if let Some(kind) = classify_page_output(&url, &output_path, state) {
        return kind;
    }

    if is_alias_redirect_output(&url, &output_path, config, state) {
        return ServedOutputKind::RedirectHtml;
    }

    ServedOutputKind::GeneratedHtml
}

fn is_html_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("html" | "htm")
    )
}

fn is_not_found_output(output_path: &Path, config: &SiteConfig) -> bool {
    normalize_path(&config.build.output.join("404.html")) == output_path
}

fn classify_page_output(
    url: &UrlPath,
    output_path: &Path,
    state: &SiteIndex,
) -> Option<ServedOutputKind> {
    let space = state.address().read();
    let resource = space.get_by_url(url)?;

    match resource {
        Resource::Page { route, .. } => {
            let route_output = normalize_path(&route.output_file);
            if route_output != output_path {
                return None;
            }

            if route.is_404 {
                Some(ServedOutputKind::NotFoundHtml)
            } else {
                Some(ServedOutputKind::PageHtml {
                    source: normalize_path(&route.source),
                })
            }
        }
        Resource::Asset { .. } => Some(ServedOutputKind::Asset),
    }
}

fn is_alias_redirect_output(
    url: &UrlPath,
    output_path: &Path,
    config: &SiteConfig,
    state: &SiteIndex,
) -> bool {
    let output_dir = config.paths().output_dir();

    state.pages().get_pages().into_iter().any(|page| {
        page.meta.aliases.iter().any(|alias| {
            let alias_url = UrlPath::from_page(alias);
            alias_url == *url
                && normalize_path(&alias_url.output_html_path(&output_dir)) == output_path
        })
    })
}
