//! Configured asset declarations and the inventory, bytes, and URLs they resolve to.

mod inventory;
pub(crate) mod minify;
mod output;
mod urls;

pub(crate) use inventory::{ConfiguredAssetInventory, render_configured_asset_inventory};
pub(crate) use minify::AssetMinifyWarning;
pub use urls::{AssetOrigin, AssetUrls, ModuleAsset, ModuleAssetInput, PublishedAsset};

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
