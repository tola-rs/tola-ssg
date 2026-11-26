//! Absolute address a site is published at.

use std::fmt;

use thiserror::Error;

/// Absolute `http`/`https` site address keeping origin and base path separate.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SiteOrigin {
    origin: String,
    base_path: Option<String>,
}

/// Invalid site origin or address.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SiteOriginError {
    #[error("starts or ends with whitespace")]
    SurroundingWhitespace,
    #[error("does not start with `http://` or `https://`")]
    NotHttp,
    #[error("is not a complete `http://` or `https://` URL with a valid host")]
    Malformed,
    #[error("uses the scheme `{0}`; use `http` or `https`")]
    UnsupportedScheme(String),
    #[error("names no host")]
    MissingHost,
    #[error("contains credentials")]
    Credentials,
    #[error("contains a query string")]
    Query,
    #[error("contains a fragment")]
    Fragment,
}

impl SiteOrigin {
    /// Parse an absolute `http`/`https` site address without credentials, query, or fragment.
    pub fn parse(raw: &str) -> Result<Self, SiteOriginError> {
        if raw.trim() != raw {
            return Err(SiteOriginError::SurroundingWhitespace);
        }
        let Some((scheme, rest)) = raw.split_once("://") else {
            return Err(SiteOriginError::NotHttp);
        };
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            return Err(SiteOriginError::UnsupportedScheme(scheme.to_lowercase()));
        }
        let url = url::Url::parse(raw).map_err(|_| SiteOriginError::Malformed)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(SiteOriginError::UnsupportedScheme(
                url.scheme().to_lowercase(),
            ));
        }
        if url.host_str().is_none() {
            return Err(SiteOriginError::MissingHost);
        }
        // Check authority text for `@` too because `url::Url` allows an empty username (`https://@host`).
        let auth_end = rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len());
        let auth = &rest[..auth_end];
        if auth.contains('@') || !url.username().is_empty() || url.password().is_some() {
            return Err(SiteOriginError::Credentials);
        }
        if url.query().is_some() {
            return Err(SiteOriginError::Query);
        }
        if url.fragment().is_some() {
            return Err(SiteOriginError::Fragment);
        }
        let raw_path = &rest[auth_end..];
        let base_path = match raw_path {
            "" | "/" => None,
            _ => Some(rooted_path(url.path())),
        };
        Ok(Self {
            origin: url.origin().ascii_serialization(),
            base_path,
        })
    }

    /// Canonical origin, `scheme://host[:port]` without a trailing slash.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Encoded deployment path ending with `/`, or `None` when none was named.
    pub fn base_path(&self) -> Option<&str> {
        self.base_path.as_deref()
    }
}

fn rooted_path(path: &str) -> String {
    if path.ends_with('/') {
        path.to_owned()
    } else {
        format!("{path}/")
    }
}

impl fmt::Display for SiteOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.origin)?;
        if let Some(base_path) = &self.base_path {
            f.write_str(base_path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_and_base_path() {
        for (raw, origin, base_path) in [
            ("https://example.com", "https://example.com", None),
            ("https://example.com/", "https://example.com", None),
            ("https://example.com:8080", "https://example.com:8080", None),
            ("https://EXAMPLE.com:443/", "https://example.com", None),
            (
                "http://example.com/docs",
                "http://example.com",
                Some("/docs/"),
            ),
            (
                "https://example.com/%E6%96%87%E6%A1%A3/",
                "https://example.com",
                Some("/%E6%96%87%E6%A1%A3/"),
            ),
            ("https://example.com/.", "https://example.com", Some("/")),
            ("https://example.com/%2e", "https://example.com", Some("/")),
        ] {
            let addr = SiteOrigin::parse(raw).unwrap();
            assert_eq!(addr.origin(), origin, "{raw}");
            assert_eq!(addr.base_path(), base_path, "{raw}");
        }
    }
}
