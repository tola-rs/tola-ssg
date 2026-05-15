//! Public URL paths.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use super::{ConfigDiagnostics, FieldPath, PathResolver};
use crate::core::UrlPath;

/// Site-root public URL for a generated or referenced file.
///
/// Public URLs start with `/`, do not include `path_prefix`, and never include
/// the output directory name. The filesystem output path is derived from the
/// URL when a Tola subsystem owns the generated file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PublicUrl(String);

impl PublicUrl {
    pub fn new(url: impl Into<String>) -> Self {
        Self(url.into())
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Slash-separated URL path without the leading slash.
    pub fn logical_path(&self) -> String {
        self.0
            .strip_prefix('/')
            .unwrap_or(self.as_str())
            .to_string()
    }

    /// Site-root URL path, before `path_prefix` is applied.
    pub fn url_path(&self) -> UrlPath {
        UrlPath::from_asset(self.as_str())
    }

    /// Browser href with `path_prefix` applied.
    pub fn href(&self, paths: PathResolver<'_>) -> String {
        paths.url_for_site_path(self.logical_path())
    }

    /// Filesystem path under `PathResolver::output_dir()`.
    pub fn output_path(&self, paths: PathResolver<'_>) -> PathBuf {
        paths.output_dir().join(self.logical_path())
    }

    pub fn canonical_url(&self, base_url: Option<&str>) -> String {
        self.url_path().canonical_url(base_url)
    }

    pub fn validate(&self, field: FieldPath, diag: &mut ConfigDiagnostics) {
        validate_public_file_url(self.as_str(), field, None, diag);
    }

    pub fn validate_indexed(
        &self,
        field: FieldPath,
        idx: usize,
        total: usize,
        diag: &mut ConfigDiagnostics,
    ) {
        let label = (total > 1).then(|| format!("[{idx}]"));
        validate_public_file_url(self.as_str(), field, label, diag);
    }
}

impl fmt::Display for PublicUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for PublicUrl {
    fn from(url: &str) -> Self {
        Self::new(url)
    }
}

impl From<String> for PublicUrl {
    fn from(url: String) -> Self {
        Self::new(url)
    }
}

fn validate_public_file_url(
    url: &str,
    field: FieldPath,
    label: Option<String>,
    diag: &mut ConfigDiagnostics,
) {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        diag.error(field, labeled_message(label, "URL must not be empty"));
        return;
    }

    if trimmed != url {
        diag.error(
            field,
            labeled_message(
                label,
                format!("URL '{url}' must not contain surrounding whitespace"),
            ),
        );
        return;
    }

    if !trimmed.starts_with('/') {
        diag.error(
            field,
            labeled_message(label, format!("URL '{trimmed}' must start with `/`")),
        );
        return;
    }

    if trimmed.starts_with("//") {
        diag.error(
            field,
            labeled_message(label, format!("URL '{trimmed}' must not start with `//`")),
        );
        return;
    }

    if trimmed.contains(['?', '#']) {
        diag.error(
            field,
            labeled_message(
                label,
                format!("URL '{trimmed}' must not include query strings or fragments"),
            ),
        );
        return;
    }

    if trimmed.contains('\\') {
        diag.error(
            field,
            labeled_message(label, format!("URL '{trimmed}' must use `/` separators")),
        );
        return;
    }

    if trimmed.ends_with('/') {
        diag.error(
            field,
            labeled_message(label, format!("URL '{trimmed}' must reference a file")),
        );
        return;
    }

    let logical = trimmed.trim_start_matches('/');
    if logical.split('/').any(str::is_empty) {
        diag.error(
            field,
            labeled_message(
                label,
                format!("URL '{trimmed}' must not contain empty path segments"),
            ),
        );
        return;
    }

    if logical == "public" || logical.starts_with("public/") {
        diag.error(
            field,
            labeled_message(
                label,
                format!("URL '{trimmed}' must not include the output directory name"),
            ),
        );
        return;
    }

    for comp in Path::new(logical).components() {
        let reason = match comp {
            Component::CurDir => Some("current directory '.' not allowed"),
            Component::ParentDir => Some("parent directory '..' not allowed"),
            Component::Prefix(_) | Component::RootDir => Some("absolute paths not allowed"),
            _ => None,
        };
        if let Some(reason) = reason {
            diag.error(
                field,
                labeled_message(label.clone(), format!("URL '{trimmed}': {reason}")),
            );
        }
    }
}

fn labeled_message(label: Option<String>, message: impl fmt::Display) -> String {
    match label {
        Some(label) => format!("{label} {message}"),
        None => message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIELD: FieldPath = FieldPath::new("test.url");

    fn error_messages(diag: &ConfigDiagnostics) -> Vec<&str> {
        diag.errors()
            .iter()
            .map(|error| error.message.as_str())
            .collect()
    }

    #[test]
    fn rejects_url_without_leading_slash() {
        let url = PublicUrl::from("styles/site.css");
        let mut diag = ConfigDiagnostics::new();

        url.validate(FIELD, &mut diag);

        assert!(
            error_messages(&diag)
                .iter()
                .any(|message| message.contains("must start with `/`"))
        );
    }

    #[test]
    fn rejects_output_directory_prefix() {
        let url = PublicUrl::from("/public/styles/site.css");
        let mut diag = ConfigDiagnostics::new();

        url.validate(FIELD, &mut diag);

        assert!(
            error_messages(&diag)
                .iter()
                .any(|message| message.contains("output directory name"))
        );
    }

    #[test]
    fn rejects_double_slash_url() {
        let url = PublicUrl::from("//styles/site.css");
        let mut diag = ConfigDiagnostics::new();

        url.validate(FIELD, &mut diag);

        assert!(
            error_messages(&diag)
                .iter()
                .any(|message| message.contains("must not start with `//`"))
        );
    }

    #[test]
    fn rejects_non_canonical_url_path_segments() {
        for (raw, expected) in [
            ("/styles\\site.css", "must use `/` separators"),
            ("/styles//site.css", "empty path segments"),
            ("/./styles/site.css", "current directory"),
        ] {
            let url = PublicUrl::from(raw);
            let mut diag = ConfigDiagnostics::new();

            url.validate(FIELD, &mut diag);

            assert!(
                error_messages(&diag)
                    .iter()
                    .any(|message| message.contains(expected)),
                "{raw} should fail with {expected:?}, got {:?}",
                error_messages(&diag)
            );
        }
    }

    #[test]
    fn derives_href_and_output_path_from_site_root_url() {
        let url = PublicUrl::from("/styles/site.css");
        let paths = PathResolver::new(Path::new("/site/public"), Path::new("docs"));

        assert_eq!(url.logical_path(), "styles/site.css");
        assert_eq!(url.href(paths), "/docs/styles/site.css");
        assert_eq!(
            url.output_path(paths),
            PathBuf::from("/site/public/docs/styles/site.css")
        );
    }
}
