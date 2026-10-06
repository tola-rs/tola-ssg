//! One module per `tola.toml` section a site configures. A section owns its keys,
//! their defaults, and the validation of the values an author wrote.

pub mod assets;
pub mod build;
pub mod icons;
pub(crate) mod path;
pub mod site;
pub mod typst;
pub mod vendor;

pub use assets::{
    AssetFileDeclaration, AssetTreeDeclaration, AssetUrl, AssetUrlError, AssetUrlPrefix,
    AssetsConfig,
};
pub use build::BuildSectionConfig;
pub use icons::{IconCollectionSource, IconsConfig, Sha256Digest};
pub use site::{SiteAuthor, SiteSectionConfig};
pub use typst::{FontsConfig, TypstSectionConfig};
pub use vendor::VendorConfig;
