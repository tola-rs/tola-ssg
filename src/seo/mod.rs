//! SEO utilities for static site output.
//!
//! Generates auxiliary files and injects metadata for search engines and social media:
//!
//! - **Feed**: RSS/Atom feeds for blog readers (`rss.xml`, `atom.xml`)
//! - **Sitemap**: Search engine indexing (`sitemap.xml`)
//! - **OG Tags**: Open Graph meta tags for social media sharing
//!
//! All generators use pre-collected `PageMeta` from the build pipeline,
//! avoiding redundant filesystem scans or re-compilation.

pub mod extract;
pub mod feed;
pub mod og;
pub mod sitemap;

use crate::{address::SiteIndex, config::SiteConfig};
use anyhow::Result;

/// Build configured SEO output files from the current page index.
pub fn build_outputs(config: &SiteConfig, state: &SiteIndex) -> Result<()> {
    let (feed_result, sitemap_result) = rayon::join(
        || state.with_pages(|pages| feed::build_feed(config, pages)),
        || state.with_pages(|pages| sitemap::build_sitemap(config, pages)),
    );

    feed_result?;
    sitemap_result?;
    Ok(())
}
