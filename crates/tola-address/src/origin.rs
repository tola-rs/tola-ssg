//! The absolute address a site is published at.

use std::fmt;

use thiserror::Error;

/// One absolute `http`/`https` site address.
///
/// A site stores its origin and the deployment path below it as separate settings, so this
/// value keeps them apart: [`Self::origin`] is the canonical `scheme://host[:port]`, and
/// [`Self::base_path`] is the rooted path the address named, when it named one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SiteOrigin {
    origin: String,
    base_path: Option<String>,
}

/// Why one value cannot be a site address.
///
/// A message reads as the rest of a sentence about the address, so a caller renders it as
/// `site.origin <reason>` or `the site URL <reason>`.
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
    /// Parse one absolute site address.
    ///
    /// The address names an `http` or `https` host and has no credentials, query string,
    /// or fragment. It may name the deployment path the site is published below.
    pub fn parse(raw: &str) -> Result<Self, SiteOriginError> {
        if raw.trim() != raw {
            return Err(SiteOriginError::SurroundingWhitespace);
        }
        let Some((scheme, remainder)) = raw.split_once("://") else {
            return Err(SiteOriginError::NotHttp);
        };
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            return Err(SiteOriginError::UnsupportedScheme(scheme.to_lowercase()));
        }
        let parsed = url::Url::parse(raw).map_err(|_| SiteOriginError::Malformed)?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(SiteOriginError::UnsupportedScheme(
                parsed.scheme().to_lowercase(),
            ));
        }
        if parsed.host_str().is_none() {
            return Err(SiteOriginError::MissingHost);
        }
        // The authority decides whether the written address has credentials, because the
        // URL parser accepts an empty user name that no site may configure.
        let authority_end = remainder
            .find(['/', '\\', '?', '#'])
            .unwrap_or(remainder.len());
        let authority = &remainder[..authority_end];
        if authority.contains('@') || !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(SiteOriginError::Credentials);
        }
        if parsed.query().is_some() {
            return Err(SiteOriginError::Query);
        }
        if parsed.fragment().is_some() {
            return Err(SiteOriginError::Fragment);
        }
        let written_path = &remainder[authority_end..];
        // A path the URL parser normalized away — `/` and `/.` — is still a written path: a
        // site origin names none, and a site address that names `/` names the root below it.
        let base_path = match written_path {
            "" => None,
            "/" => None,
            _ => Some(rooted_path(parsed.path())),
        };
        Ok(Self {
            origin: parsed.origin().ascii_serialization(),
            base_path,
        })
    }

    /// Canonical origin, `scheme://host[:port]` without a trailing slash.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Encoded deployment path ending with `/`, or `None` when the address names none.
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
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.origin)?;
        if let Some(base_path) = &self.base_path {
            formatter.write_str(base_path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_address_keeps_origin_apart() {
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
            let address = SiteOrigin::parse(raw).unwrap();
            assert_eq!(address.origin(), origin, "{raw}");
            assert_eq!(address.base_path(), base_path, "{raw}");
        }
    }

    #[test]
    fn refusals_never_leak_rust_types() {
        for raw in [
            " https://example.com",
            "https://example.com ",
            "example.com",
            "https:example.com",
            "ftp://example.com",
            "https://",
            "https://user@example.com",
            "https://@example.com",
            "https://user:secret@example.com",
            "https://example.com?preview=1",
            "https://example.com#preview",
        ] {
            let error = SiteOrigin::parse(raw).unwrap_err();
            assert!(
                !error.to_string().contains("url::"),
                "the reason names no Rust type: {error}"
            );
        }
    }
}
