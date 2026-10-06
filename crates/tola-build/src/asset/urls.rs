//! Final browser URLs of the site's declared `assets` entries.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use typst::foundations::{Dict, IntoValue};

use crate::config::ResolvedSiteConfig;

/// The browser URL of one published asset.
///
/// The address is the site-root URL of the file with the deployment mount
/// applied. A site that asked `[assets] cache-busting` for identified URLs
/// names the identity of the published bytes in a query, so changed bytes are
/// a changed address while the published name stays what the declaration says.
/// A caller that rendered no bytes passes no identity and gets the plain
/// address.
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
///
/// A declaration is stable; the address a browser fetches may not be, because
/// `[assets] cache-busting` appends the identity of the published bytes to
/// the URL. Only a build knows those bytes, so a build derives the dictionary
/// from an inventory that has already rendered them, while a read-only check
/// derives it from the declarations and the tree's members alone and so never
/// has an identity.
///
/// A key absent from this dictionary names something the site does not publish.
/// Nothing here discovers files, and nothing invents a URL: every value comes
/// from one declaration's own output path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssetUrls {
    /// Site-root URL -> mounted browser URL. A `assets.files` declaration
    /// contributes its `url`; a `assets.trees` declaration contributes its
    /// `url-prefix` joined with the path of every member it publishes, such as
    /// `/assets/images/logo.svg`. A member a `files` declaration owns is the
    /// declaration's, not the tree's. A build appends `?h=<identity>` to the
    /// value of a file it identified; the key never has one.
    urls: BTreeMap<String, String>,
    /// Where each key's file is declared, when the view was built from the site's declarations
    /// rather than from a build inventory.
    origins: BTreeMap<String, AssetOrigin>,
}

/// The declaration that publishes one asset URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetOrigin {
    /// An `assets.files` entry publishes its own `source` file at this URL.
    File { source: PathBuf },
    /// An `assets.trees` source publishes this member at this URL.
    TreeMember { source: PathBuf, member: PathBuf },
}

/// One published asset URL: what its author writes, where a browser fetches it, and the declaration
/// that publishes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedAsset<'a> {
    pub declared: &'a str,
    pub address: &'a str,
    pub origin: Option<&'a AssetOrigin>,
}

impl AssetUrls {
    /// Resolve every declaration to the address of the file it names.
    ///
    /// Read-only checks use this: they must not render or minify configured
    /// assets, so every declaration resolves to the mounted address of the name
    /// the configuration gives it, and a site with cache busting enabled
    /// resolves to the same address without an identity, because a check knows
    /// no bytes. Tree members are enumerated from their source directory, by the
    /// same rules and the same exclusions a build applies, without reading,
    /// rendering, or minifying them.
    pub fn for_check(
        config: &ResolvedSiteConfig,
        cancellation: &crate::cancellation::BuildCancellation,
    ) -> Result<Self> {
        let mount = config.url_mount();
        let mut published = Vec::new();
        for declaration in &config.assets.files {
            let url = declaration.url().url_path();
            published.push((
                declaration.url().as_str().to_owned(),
                browser_url(&mount, url, None),
                AssetOrigin::File {
                    source: declaration.source().to_path_buf(),
                },
            ));
        }
        for declaration in &config.assets.trees {
            let source = declaration.source();
            let members =
                super::inventory::enumerate_tree_members(config, declaration, cancellation)?;
            for member in members {
                let output = super::output::output_for_configured_tree_member(
                    source,
                    &member,
                    declaration.url_prefix(),
                    config.get_root(),
                )?;
                let url = tola_address::asset_url_from_output(&output.output);
                let browser = browser_url(&mount, &url, None);
                published.push((
                    url.as_str().to_owned(),
                    browser,
                    AssetOrigin::TreeMember {
                        source: source.to_path_buf(),
                        member: member.to_path_buf(),
                    },
                ));
            }
        }
        Ok(Self::from_published(published))
    }

    /// The published view one caller walked: every declared URL, the address it resolves to, and
    /// the declaration that publishes it.
    pub(super) fn from_published(
        published: impl IntoIterator<Item = (String, String, AssetOrigin)>,
    ) -> Self {
        let mut urls = BTreeMap::new();
        let mut origins = BTreeMap::new();
        for (declared, address, origin) in published {
            urls.insert(declared.clone(), address);
            origins.insert(declared, origin);
        }
        Self { urls, origins }
    }

    /// The address a browser fetches for one declared asset URL, when this site publishes it.
    pub fn address(&self, declared: &str) -> Option<&str> {
        self.urls.get(declared).map(String::as_str)
    }

    /// The declaration that publishes one declared asset URL, when this view knows it.
    pub fn origin(&self, declared: &str) -> Option<&AssetOrigin> {
        self.origins.get(declared)
    }

    /// The declarations that publish exactly one file, keyed by the declared URL.
    ///
    /// A tree member and an asset without an origin are absent: neither names one file, so a caller
    /// comparing a resolved file against a declaration has nothing to compare for them.
    pub(crate) fn origins(&self) -> impl Iterator<Item = (&str, &AssetOrigin)> {
        self.origins
            .iter()
            .map(|(declared, origin)| (declared.as_str(), origin))
    }

    /// Every published asset URL with its address, and its declaration when the view knows one.
    pub fn published(&self) -> impl Iterator<Item = PublishedAsset<'_>> {
        self.urls.iter().map(|(declared, address)| PublishedAsset {
            declared,
            address,
            origin: self.origins.get(declared),
        })
    }

    /// Whether every declaration this dictionary resolves still resolves the same way.
    ///
    /// A compilation that read a URL read it from the dictionary it was bound to,
    /// so it is reusable only while the next dictionary answers every declaration
    /// this one names with the same address. A declaration the next dictionary
    /// adds or drops is not an address a compilation read, so it neither refreshes
    /// nor invalidates one. An address follows the published bytes only when the
    /// site asked for cache busting, so without it a declaration is a function of
    /// the configuration and this holds until that declaration changes.
    pub(crate) fn values_match(&self, next: &Self) -> bool {
        self.urls
            .iter()
            .all(|(declared, browser)| next.urls.get(declared).is_none_or(|next| next == browser))
    }

    /// The `asset-url()` view of this site's published declarations.
    pub(crate) fn to_typst_dict(&self) -> Dict {
        self.urls
            .iter()
            .map(|(declared, browser)| (declared.clone().into(), browser.clone().into_value()))
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

        let urls = AssetUrls::for_check(&config, &Default::default()).unwrap();

        assert_eq!(
            published_url(&urls, "/app.js").as_deref(),
            Some("/blog/app.js")
        );
        assert_eq!(published_url(&urls, "/undeclared.js"), None);

        // A tree member an exact declaration owns belongs to the declaration, and a
        // check appends no identity to any value an editor resolves.
        let directory = tempfile::TempDir::new().unwrap();
        let assets = directory.path().join("assets");
        std::fs::create_dir_all(assets.join("images")).unwrap();
        std::fs::write(assets.join("images/logo.svg"), b"<svg></svg>").unwrap();
        std::fs::write(assets.join("app.js"), "const answer = 40 + 2;").unwrap();
        let mut tree_config = crate::config::tests::load_test_config(
            directory.path(),
            "[site]\nbase-path = \"/blog/\"",
        );
        tree_config.assets.cache_busting = true;
        tree_config.assets.files = vec![file_declaration(assets.join("app.js"), "/app.js")];
        tree_config.assets.trees = vec![crate::config::section::AssetTreeDeclaration::new(
            &assets,
            crate::config::section::AssetUrlPrefix::parse("/assets").unwrap(),
        )];
        let tree_urls = AssetUrls::for_check(&tree_config, &Default::default()).unwrap();
        assert_eq!(published_url(&tree_urls, "/assets/app.js"), None);
        assert_eq!(
            published_url(&tree_urls, "/assets/images/logo.svg").as_deref(),
            Some("/blog/assets/images/logo.svg")
        );
        for value in tree_urls.urls.values() {
            assert!(!value.contains('?'), "{value}");
        }

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

        let urls = AssetUrls::for_check(&config, &Default::default()).unwrap();

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
