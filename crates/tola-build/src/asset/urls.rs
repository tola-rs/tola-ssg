//! Final browser URLs of the site's declared `assets` entries.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use typst::foundations::{Dict, IntoValue};

use crate::config::ResolvedSiteConfig;

/// The site-root URL of one published asset with the deployment mount and identity applied.
pub(super) fn browser_url(
    mount: &tola_address::SiteUrlMount,
    url: &tola_address::UrlPath,
    identity: Option<&str>,
) -> String {
    let mut browser = mount.browser_path(url);
    if let Some(identity) = identity {
        browser.push_str("?h=");
        browser.push_str(identity);
    }
    browser
}

/// Every URL the site publishes, keyed by the site-root URL its author writes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssetUrls {
    prepared: bool,
    /// Site-root URL -> mounted browser URL; a build appends `?h=<identity>` when busting.
    urls: BTreeMap<String, (String, AssetOrigin, Option<ModuleAsset>)>,
}

/// The declaration that publishes one asset URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetOrigin {
    /// An `assets.files` entry publishes its own `source` file at this URL.
    File { source: PathBuf },
    /// An `assets.trees` source publishes this member at this URL.
    TreeMember { source: PathBuf, member: PathBuf },
}

/// One published asset URL and the declaration that publishes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedAsset<'a> {
    pub declared: &'a str,
    pub address: &'a str,
    pub origin: Option<&'a AssetOrigin>,
}

/// Explicit ownership and input of one published module asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleAsset {
    pub alias: String,
    pub name: String,
    pub input: ModuleAssetInput,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModuleAssetInput {
    Module(PathBuf),
    Project(PathBuf),
}

impl AssetUrls {
    /// Read the project declarations’ mounted URLs without rendering their bytes.
    pub fn for_check_project(
        config: &ResolvedSiteConfig,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> Result<Self> {
        Self::for_check_with(
            config,
            crate::modules::prepared::AppliedAssets::project_only(config),
            cancellation,
        )
    }

    pub(crate) fn for_check_with(
        config: &ResolvedSiteConfig,
        assets: crate::modules::prepared::AppliedAssets<'_>,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> Result<Self> {
        super::inventory::check_asset_urls(config, assets, cancellation)
    }

    /// The selected module assets and the mounted URLs this build publishes.
    pub fn module_assets(&self) -> impl Iterator<Item = (&ModuleAsset, &str)> {
        self.urls
            .values()
            .filter(move |_| self.prepared)
            .filter_map(|(address, _, asset)| asset.as_ref().map(|asset| (asset, address.as_str())))
    }

    /// Index published addresses and their declaration owners.
    pub(super) fn from_published(
        published: impl IntoIterator<Item = (String, String, AssetOrigin, Option<ModuleAsset>)>,
    ) -> Self {
        Self {
            prepared: true,
            urls: published
                .into_iter()
                .map(|(declared, address, origin, module)| (declared, (address, origin, module)))
                .collect(),
        }
    }

    /// Index unrendered addresses for editor previews.
    pub(super) fn from_preview(
        addresses: impl IntoIterator<Item = (String, String, AssetOrigin, Option<ModuleAsset>)>,
    ) -> Self {
        Self {
            prepared: false,
            ..Self::from_published(addresses)
        }
    }

    pub(crate) fn is_prepared(&self) -> bool {
        self.prepared
    }

    /// The address a browser fetches for one declared asset URL, when this site publishes it.
    pub fn address(&self, declared: &str) -> Option<&str> {
        self.urls
            .get(declared)
            .map(|(address, _, _)| address.as_str())
    }

    /// The declaration that publishes one declared asset URL, when this site publishes it.
    pub fn origin(&self, declared: &str) -> Option<&AssetOrigin> {
        self.urls.get(declared).map(|(_, origin, _)| origin)
    }

    /// Declarations in declared-URL order.
    pub(crate) fn origins(&self) -> impl Iterator<Item = (&str, &AssetOrigin)> {
        self.urls
            .iter()
            .map(|(declared, (_, origin, _))| (declared.as_str(), origin))
    }

    /// Every published asset URL with its address and publishing declaration.
    pub fn published(&self) -> impl Iterator<Item = PublishedAsset<'_>> {
        self.urls
            .iter()
            .map(|(declared, (address, origin, _))| PublishedAsset {
                declared,
                address,
                origin: Some(origin),
            })
    }

    /// Whether every declaration this dictionary resolved still resolves the same way.
    pub(crate) fn values_match(&self, next: &Self) -> bool {
        self.urls.iter().all(|(declared, (browser, _, _))| {
            next.address(declared).is_none_or(|next| next == browser)
        })
    }

    /// The `asset-url()` view of this site's published declarations.
    pub(crate) fn to_typst_dict(&self) -> Dict {
        self.urls
            .iter()
            .map(|(declared, (browser, _, _))| {
                (declared.clone().into(), browser.clone().into_value())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::asset::tests::{file_declaration, published_url};
    use crate::config::section::AssetFileDeclaration;

    fn config_with(
        declarations: impl IntoIterator<Item = AssetFileDeclaration>,
    ) -> (tempfile::TempDir, Arc<ResolvedSiteConfig>) {
        let directory = tempfile::TempDir::new().unwrap();
        let mut config = crate::config::tests::load_test_config(
            directory.path(),
            "[site]\nbase-path = \"/blog/\"",
        );
        config.assets.files = declarations.into_iter().collect();
        (directory, Arc::new(config))
    }

    #[test]
    fn declarations_resolve_under_the_mount() {
        let (_directory, config) = config_with([
            file_declaration("assets/app.js", "/app.js"),
            file_declaration("assets/%E5%9B%BE.svg", "/%E5%9B%BE.svg"),
        ]);

        let urls = AssetUrls::for_check_with(
            &config,
            crate::modules::prepared::AppliedAssets::project_only(&config),
            &Default::default(),
        )
        .unwrap();

        assert_eq!(
            published_url(&urls, "/app.js").as_deref(),
            Some("/blog/app.js")
        );
        assert_eq!(published_url(&urls, "/undeclared.js"), None);

        // The key is the decoded URL the declaration was written with, while the
        // value is the browser URL: it has the deployment mount and is percent
        // encoded, so a non-ASCII path is spelled the way a browser fetches it.
        let dict = urls.to_typst_dict();
        assert_eq!(dict.len(), 2);
        for (declared, browser) in [
            ("/app.js", "/blog/app.js"),
            ("/图.svg", "/blog/%E5%9B%BE.svg"),
        ] {
            let value = dict.get(declared).unwrap();
            assert_eq!(
                value
                    .clone()
                    .cast::<typst::foundations::Str>()
                    .unwrap()
                    .as_str(),
                browser
            );
        }
    }

    /// A check knows no bytes, so cache busting adds no identity.
    #[test]
    fn checks_resolve_no_identity() {
        let directory = tempfile::TempDir::new().unwrap();
        let assets = directory.path().join("assets");
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::write(assets.join("app.css"), "body { color: red; }").unwrap();
        let mut config = crate::config::tests::load_test_config(
            directory.path(),
            "[assets]\ncache-busting = true",
        );
        config.assets.trees = vec![crate::config::section::AssetTreeDeclaration::new(
            &assets,
            crate::config::section::AssetUrlPrefix::parse("/assets").unwrap(),
        )];

        let urls = AssetUrls::for_check_with(
            &config,
            crate::modules::prepared::AppliedAssets::project_only(&config),
            &Default::default(),
        )
        .unwrap();

        assert_eq!(
            published_url(&urls, "/assets/app.css").as_deref(),
            Some("/assets/app.css")
        );
        for asset in urls.published() {
            assert!(!asset.address.contains('?'), "{}", asset.address);
        }
    }

    /// A published asset names the declaration that publishes it.
    #[test]
    fn published_assets_name_their_declaration() {
        let directory = tempfile::TempDir::new().unwrap();
        let assets = directory.path().join("assets");
        std::fs::create_dir_all(assets.join("images")).unwrap();
        std::fs::write(assets.join("images/logo.svg"), b"<svg></svg>").unwrap();
        let mut config = crate::config::tests::load_test_config(directory.path(), "");
        config.assets.files = vec![file_declaration(
            directory.path().join("brand.svg"),
            "/brand/logo.svg",
        )];
        config.assets.trees = vec![crate::config::section::AssetTreeDeclaration::new(
            &assets,
            crate::config::section::AssetUrlPrefix::parse("/assets").unwrap(),
        )];

        let urls = AssetUrls::for_check_with(
            &config,
            crate::modules::prepared::AppliedAssets::project_only(&config),
            &Default::default(),
        )
        .unwrap();

        assert_eq!(urls.address("/brand/logo.svg"), Some("/brand/logo.svg"));
        assert_eq!(
            urls.origin("/brand/logo.svg"),
            Some(&AssetOrigin::File {
                source: directory.path().join("brand.svg"),
            })
        );
        assert_eq!(
            urls.origin("/assets/images/logo.svg"),
            Some(&AssetOrigin::TreeMember {
                source: assets.clone(),
                member: PathBuf::from("images/logo.svg"),
            })
        );
        // A URL this site does not publish names no declaration at all.
        assert_eq!(urls.address("/nope.svg"), None);
        assert_eq!(urls.origin("/nope.svg"), None);
    }

    #[test]
    fn resolved_addresses_stay_reusable() {
        let declared = |pairs: &[(&str, &str)]| {
            AssetUrls::from_published(pairs.iter().map(|(declared, browser)| {
                (
                    (*declared).to_owned(),
                    (*browser).to_owned(),
                    AssetOrigin::File {
                        source: PathBuf::from("assets/app.js"),
                    },
                    None,
                )
            }))
        };
        let first = declared(&[
            ("/app.js", "/app.js?h=one"),
            ("/site.css", "/site.css?h=two"),
        ]);

        // A declaration this dictionary resolved keeps its address, so a build
        // that adds a URL a compilation never read stays reusable.
        let grown = declared(&[
            ("/app.js", "/app.js?h=one"),
            ("/site.css", "/site.css?h=two"),
            ("/added.svg", "/added.svg?h=three"),
        ]);
        assert!(first.values_match(&grown));

        // A declaration that disappears leaves no address a compilation read
        // contradicting, so it stays reusable too.
        let narrowed = declared(&[("/app.js", "/app.js?h=one")]);
        assert!(first.values_match(&narrowed));

        // Changed bytes are a changed address, so the compilation that read the
        // old one cannot be reused.
        let changed = declared(&[
            ("/app.js", "/app.js?h=changed"),
            ("/site.css", "/site.css?h=two"),
        ]);
        assert!(!first.values_match(&changed));

        // The comparison is directional: the dictionary a compilation read is
        // the one whose declarations have to keep their addresses.
        assert!(narrowed.values_match(&first));
    }
}
