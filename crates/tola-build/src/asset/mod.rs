//! Configured asset declarations: the inventory they resolve to, the bytes their
//! render publishes, and the output identity and browser URLs those bytes hold.
//!
//! Declarations alone select the configured asset outputs. A derivative a helper
//! derives from a declared source — a resized image, for example — is a separate
//! output; the declared original stays published.

mod inventory;
pub(crate) mod minify;
mod output;
mod urls;

pub(crate) use inventory::{ConfiguredAssetInventory, render_configured_asset_inventory};
pub(crate) use minify::AssetMinifyWarning;
pub use urls::{AssetOrigin, AssetUrls, PublishedAsset};

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use super::urls::AssetUrls;
    use crate::config::section::{AssetFileDeclaration, AssetUrl};

    pub(crate) fn published_url(urls: &AssetUrls, declared_url: &str) -> Option<String> {
        urls.to_typst_dict()
            .get(declared_url)
            .ok()
            .and_then(|value| value.clone().cast::<typst::foundations::Str>().ok())
            .map(|value| value.as_str().to_owned())
    }

    pub(crate) fn file_declaration(
        source: impl Into<PathBuf>,
        url: impl AsRef<str>,
    ) -> AssetFileDeclaration {
        AssetFileDeclaration::new(source, AssetUrl::parse(url.as_ref()).unwrap())
    }
}
