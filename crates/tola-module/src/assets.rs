//! Asset URL, prefix, selection, and declaration values shared by modules and site config.

use crate::options::OptionPath;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tola_address::{OutputPath, OutputPathError, UrlPath, UrlPathError};
use tola_input::{FilePatterns, FileSelection};

use crate::fragment::{FragmentError, read_fields, read_fragment};

/// A configured asset URL could not establish a portable output identity.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AssetUrlError {
    #[error(transparent)]
    Url(#[from] UrlPathError),
    #[error(transparent)]
    Output(#[from] OutputPathError),
    #[error("asset output `{output}` is reserved for Tola")]
    Reserved { output: OutputPath },
}

/// A validated site-root prefix for an asset tree, with a canonical trailing slash.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(try_from = "String", into = "String")]
pub struct AssetUrlPrefix {
    url: UrlPath,
    output: Option<OutputPath>,
}

impl AssetUrlPrefix {
    pub fn parse(raw: &str) -> Result<Self, AssetUrlError> {
        let url = UrlPath::parse(raw)?;
        // A prefix names a directory, so `/assets` and `/assets/` are one prefix.
        let url = if url.as_str().trim_matches('/').is_empty() {
            url
        } else {
            let normalized = format!("/{}/", url.as_str().trim_matches('/'));
            UrlPath::from_decoded(&normalized)?
        };
        let output = if url.as_str() != "/" {
            let output = OutputPath::parse(url.as_str().trim_matches('/'))?;
            require_asset_output(&output)?;
            Some(output)
        } else {
            None
        };
        Ok(Self { url, output })
    }

    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    pub fn url_path(&self) -> &UrlPath {
        &self.url
    }

    /// The member named by an owner-relative resource key under this prefix.
    pub fn member_for<'a>(&self, name: &'a str) -> Option<&'a str> {
        match &self.output {
            Some(prefix) => name.strip_prefix(prefix.as_str())?.strip_prefix('/'),
            None => Some(name),
        }
    }

    pub fn output_for(&self, member: &OutputPath) -> Result<OutputPath, AssetUrlError> {
        match &self.output {
            Some(prefix) => Ok(prefix.join(member)),
            None => {
                require_asset_output(member)?;
                Ok(member.clone())
            }
        }
    }
}

impl TryFrom<String> for AssetUrlPrefix {
    type Error = AssetUrlError;
    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::parse(&raw)
    }
}

impl From<AssetUrlPrefix> for String {
    fn from(prefix: AssetUrlPrefix) -> Self {
        let encoded = prefix.url.to_encoded();
        if encoded == "/" {
            encoded
        } else {
            encoded.trim_end_matches('/').to_owned()
        }
    }
}

impl std::fmt::Display for AssetUrlPrefix {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One validated site-root asset URL and the logical output file it maps to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct AssetUrl {
    url: UrlPath,
    output: OutputPath,
}

impl AssetUrl {
    pub fn parse(raw: &str) -> Result<Self, AssetUrlError> {
        let url = UrlPath::parse(raw)?;
        let output = tola_address::asset_output_from_url(&url)?;
        require_asset_output(&output)?;
        Ok(Self { url, output })
    }

    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    pub fn url_path(&self) -> &UrlPath {
        &self.url
    }

    pub fn output_path(&self) -> &OutputPath {
        &self.output
    }
}

impl TryFrom<String> for AssetUrl {
    type Error = AssetUrlError;
    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::parse(&raw)
    }
}

impl From<AssetUrl> for String {
    fn from(url: AssetUrl) -> Self {
        url.url.to_encoded()
    }
}

impl std::fmt::Display for AssetUrl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn require_asset_output(output: &OutputPath) -> Result<(), AssetUrlError> {
    if output.is_reserved_for_non_system_output() {
        Err(AssetUrlError::Reserved {
            output: output.clone(),
        })
    } else {
        Ok(())
    }
}

/// Asset declarations relative to the site or module that owns them.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Assets {
    pub trees: Vec<AssetTreeDeclaration>,
    pub files: Vec<AssetFileDeclaration>,
}

impl Assets {
    /// Check declarations without inspecting sources that a hook may generate.
    pub fn validate(&self) -> anyhow::Result<()> {
        for source in self
            .trees
            .iter()
            .map(|entry| &entry.source)
            .chain(self.files.iter().map(|entry| &entry.source))
        {
            tola_input::validate_relative_path(source)
                .map_err(|error| anyhow::anyhow!("an asset's `source` {error}"))?;
        }
        Ok(())
    }
}

/// Whether the declaration publishes under its owner's effective options.
pub fn is_enabled(
    enable: bool,
    enabled_by: Option<&OptionPath>,
    options: &toml::Table,
    owner: &str,
    source: &Path,
) -> anyhow::Result<bool> {
    crate::options::enabled_by(enable, enabled_by, options)
        .map_err(|error| anyhow::anyhow!("{owner} asset `{}`: {error}", source.display()))
}

/// Selected members of one directory published below a URL prefix within its owner.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AssetTreeDeclaration {
    /// Logical source directory. TOML supplies a site-relative path; runtime
    /// configuration holds its logical absolute path without following symlinks.
    pub source: PathBuf,

    /// Site-root URL prefix for descendants of `source`.
    #[serde(rename = "url-prefix")]
    pub url_prefix: AssetUrlPrefix,
    /// Follow selected symbolic links to regular files while scanning this tree.
    #[serde(rename = "follow-file-symlinks")]
    pub follow_file_symlinks: bool,
    /// Whether this declaration may publish.
    pub enable: bool,
    /// TOML key path to a boolean in the owner's options.
    #[serde(rename = "enabled-by", skip_serializing_if = "Option::is_none")]
    pub enabled_by: Option<OptionPath>,
    /// Whether the site may minify the published bytes.
    pub minify: bool,
    #[serde(flatten, serialize_with = "serialize_tree_patterns")]
    selection: Arc<FileSelection>,
}

#[derive(Default, Deserialize)]
struct AssetTreeFields {
    source: Option<PathBuf>,
    #[serde(rename = "url-prefix")]
    url_prefix: Option<AssetUrlPrefix>,
    #[serde(rename = "follow-file-symlinks")]
    follow_file_symlinks: Option<bool>,
    enable: Option<bool>,
    #[serde(rename = "enabled-by")]
    enabled_by: Option<OptionPath>,
    minify: Option<bool>,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
}

fn serialize_tree_patterns<S: serde::Serializer>(
    selection: &FileSelection,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    selection.patterns().serialize(serializer)
}

impl TryFrom<AssetTreeFields> for AssetTreeDeclaration {
    type Error = String;

    fn try_from(fields: AssetTreeFields) -> Result<Self, Self::Error> {
        let source = fields.source.ok_or("declare `source` for the asset tree")?;
        let url_prefix = fields
            .url_prefix
            .ok_or("declare `url-prefix` for the asset tree")?;
        let selection = FileSelection::new(&FilePatterns {
            include: fields.include.unwrap_or_else(|| vec![".".to_owned()]),
            exclude: fields.exclude.unwrap_or_default(),
        })
        .map_err(|error| error.at("assets.trees").to_string())?;
        Ok(Self {
            source,
            url_prefix,
            follow_file_symlinks: fields.follow_file_symlinks.unwrap_or_default(),
            enable: fields.enable.unwrap_or(true),
            enabled_by: fields.enabled_by,
            minify: fields.minify.unwrap_or(true),
            selection: Arc::new(selection),
        })
    }
}

impl<'de> Deserialize<'de> for AssetTreeDeclaration {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::try_from(read_fields::<AssetTreeFields, _>(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

impl AssetTreeDeclaration {
    /// Check the fields written in one asset tree declaration.
    pub fn validate_fragment(value: &toml::Value) -> Result<(), FragmentError> {
        read_fragment::<AssetTreeFields>(value).map(|_| ())
    }

    /// Declare a source tree with an already validated URL prefix.
    pub fn new(source: impl Into<PathBuf>, url_prefix: AssetUrlPrefix) -> Self {
        AssetTreeFields {
            source: Some(source.into()),
            url_prefix: Some(url_prefix),
            ..AssetTreeFields::default()
        }
        .try_into()
        .expect("the entire asset tree is a valid selection")
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn url_prefix(&self) -> &AssetUrlPrefix {
        &self.url_prefix
    }

    /// The compiled include and exclude patterns relative to the site or module root.
    pub fn selection(&self) -> &FileSelection {
        &self.selection
    }

    pub fn read_source(&mut self, root: &Path) {
        self.source = tola_input::lexical_path_identity(&root.join(&self.source));
    }
}

/// One file published at an exact URL within its owner.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AssetFileDeclaration {
    /// Logical source file. TOML supplies a site-relative path; runtime configuration
    /// holds its logical absolute path without following symlinks.
    pub source: PathBuf,

    /// Exact site-root URL of this file. The URL is an address only: its extension
    /// does not transcode the source bytes.
    pub url: AssetUrl,
    /// Whether this declaration may publish.
    pub enable: bool,
    /// TOML key path to a boolean in the owner's options.
    #[serde(rename = "enabled-by", skip_serializing_if = "Option::is_none")]
    pub enabled_by: Option<OptionPath>,
    /// Whether the site may minify the published bytes.
    pub minify: bool,
}

#[derive(Deserialize)]
struct AssetFileFields {
    source: Option<PathBuf>,
    url: Option<AssetUrl>,
    enable: Option<bool>,
    #[serde(rename = "enabled-by")]
    enabled_by: Option<OptionPath>,
    minify: Option<bool>,
}

impl TryFrom<AssetFileFields> for AssetFileDeclaration {
    type Error = &'static str;

    fn try_from(fields: AssetFileFields) -> Result<Self, Self::Error> {
        Ok(Self {
            source: fields.source.ok_or("declare `source` for the asset file")?,
            url: fields.url.ok_or("declare `url` for the asset file")?,
            enable: fields.enable.unwrap_or(true),
            enabled_by: fields.enabled_by,
            minify: fields.minify.unwrap_or(true),
        })
    }
}

impl<'de> Deserialize<'de> for AssetFileDeclaration {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::try_from(read_fields::<AssetFileFields, _>(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

impl AssetFileDeclaration {
    /// Check the fields written in one asset file declaration.
    pub fn validate_fragment(value: &toml::Value) -> Result<(), FragmentError> {
        read_fragment::<AssetFileFields>(value).map(|_| ())
    }

    /// Declare a source file with an already validated URL and output identity.
    pub fn new(source: impl Into<PathBuf>, url: AssetUrl) -> Self {
        Self {
            source: source.into(),
            url,
            enable: true,
            enabled_by: None,
            minify: true,
        }
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn url(&self) -> &AssetUrl {
        &self.url
    }

    pub fn read_source(&mut self, root: &Path) {
        self.source = tola_input::lexical_path_identity(&root.join(&self.source));
    }
}
