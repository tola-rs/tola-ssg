//! Configuration section definitions.
//!
//! Each module corresponds to a section in `tola.toml`:
//!
//! | Module     | TOML Section   | Purpose                           |
//! |------------|----------------|-----------------------------------|
//! | `build`    | `[build]`      | Build paths, assets, svg, css     |
//! | `deploy`   | `[deploy]`     | Deployment settings               |
//! | `serve`    | `[serve]`      | Development server                |
//! | `site`     | `[site]`       | Site info, nav, preload           |
//! | `theme`    | `[theme]`      | Theme settings (recolor)          |
//! | `validate` | `[validate]`   | Link/asset validation             |

pub mod build;
mod deploy;
mod serve;
pub mod site;
pub mod theme;
mod validate;

// Re-export section configs
pub use build::{
    AssetsConfig, BuildSectionConfig, FeedConfig, FeedFormat, SlugCase, SlugConfig, SlugMode,
    SvgConverter, SvgFormat,
};
pub use deploy::DeployConfig;
pub use serve::ServeConfig;
pub use site::SiteSectionConfig;
pub use theme::ThemeSectionConfig;
pub use validate::{ValidateConfig, ValidateLevel};
