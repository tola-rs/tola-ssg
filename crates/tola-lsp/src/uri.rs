//! Source URL conversion at the RFC 3986 protocol boundary.
//!
//! Encoding turns a site path into the URI an editor opens; decoding turns an editor URI back
//! into the path every other site path is compared with, so one rule decides which file a URI
//! means.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lsp_types::Uri;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::Deserialize;
use serde::de::value::{Error, StringDeserializer};
use url::Url;

// WHATWG URL paths permit bytes that RFC 3986 does not allow in a path.
// Preserve existing escapes and path separators; never encode the authority.
const URI_PATH_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// The local path an editor URI names, in the editor's own spelling.
///
/// The spelling matters: a document URI Tola publishes back to the editor keeps the identity the
/// editor opened, so paths compared with each other are normalized separately.
pub(super) fn to_file_path(uri: &str) -> Result<PathBuf> {
    Url::parse(uri)
        .context("the editor URI is not valid")?
        .to_file_path()
        .map_err(|()| anyhow::anyhow!("the editor URI must name a local file"))
}

/// The site identity an editor URI names, normalized as every other site path is.
///
/// One file has several spellings (symlinks, `.` and `..`, escapes), so an identity compared or
/// cached anywhere in the crate goes through this rule.
pub(super) fn to_site_path(uri: &str) -> Result<PathBuf> {
    Ok(tola_build::filesystem::normalize_existing_prefix(
        &to_file_path(uri)?,
    ))
}

/// The two spellings of one site root: the path Tola resolves and the one the editor named.
///
/// Normalizing a root is a filesystem walk, so a reply that re-addresses many files — a workspace
/// symbol list, a rename's edit keys — normalizes once and addresses each file from that.
#[derive(Debug, Clone)]
pub(super) struct ClientRoot {
    resolved: PathBuf,
    spelled: PathBuf,
}

impl ClientRoot {
    /// The root a client spelled, with the form the filesystem resolves it to derived here.
    pub(super) fn new(spelled: impl Into<PathBuf>) -> Self {
        let spelled = spelled.into();
        let resolved = tola_build::filesystem::normalize_existing_prefix(&spelled);
        Self::with_resolved(spelled, resolved)
    }

    /// The root a client spelled, and the resolved form that root denotes.
    pub(super) fn with_resolved(spelled: impl Into<PathBuf>, resolved: impl Into<PathBuf>) -> Self {
        Self {
            resolved: resolved.into(),
            spelled: spelled.into(),
        }
    }

    /// The root as the filesystem resolves it: the path every site path is compared against.
    pub(super) fn resolved(&self) -> &Path {
        &self.resolved
    }

    /// The site path in the editor's own spelling, without encoding.
    ///
    /// A path outside the resolved root has no suffix to rejoin, so it stays as it is.
    pub(super) fn spell(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.resolved)
            .map(|suffix| self.spelled.join(suffix))
            .unwrap_or_else(|_| path.to_path_buf())
    }

    /// The editor's address for one site path: the resolved root is stripped, the spelling the
    /// editor named is rejoined, and the result is encoded.
    pub(super) fn address(&self, path: &Path) -> Result<Uri> {
        from_file_path(&self.spell(path))
    }
}

pub(super) fn from_file_path(path: &Path) -> Result<Uri> {
    let url = Url::from_file_path(path).map_err(|_| {
        anyhow::anyhow!("Tola could not address this site file as an editor document")
    })?;
    from_url(url)
}

pub(super) fn from_url(mut url: Url) -> Result<Uri> {
    let path: Cow<'_, str> = utf8_percent_encode(url.path(), URI_PATH_ENCODE_SET).into();
    if let Cow::Owned(path) = path {
        url.set_path(&path);
    }
    Uri::deserialize(StringDeserializer::<Error>::new(url.into()))
        .context("source URL is not a valid protocol URI")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_uris_preserve_path_characters() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source [draft] %25 ^正文.typ");
        let uri = from_file_path(&source).unwrap();
        assert_eq!(
            url::Url::parse(uri.as_str())
                .unwrap()
                .to_file_path()
                .unwrap(),
            source
        );
    }

    /// The resolved root a path is read through is not the root the editor named, so the address
    /// goes back to the editor's own spelling.
    #[test]
    fn alias_root_spelling_addresses_resolved_file() {
        let base = std::env::current_dir().unwrap();
        let spelled = base.join("client/site");
        let resolved = base.join("resolved/site");
        let root = ClientRoot::with_resolved(&spelled, &resolved);
        assert_eq!(
            root.address(&resolved.join("content/document.typ"))
                .unwrap(),
            from_file_path(&spelled.join("content/document.typ")).unwrap()
        );
    }
}
