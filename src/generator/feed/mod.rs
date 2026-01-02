//! Feed generation (RSS, Atom).
//!
//! Generates syndication feeds from compiled page metadata:
//!
//! - **RSS 2.0**: Standard feed format (`rss.xml`)
//! - **Atom 1.0**: Modern feed format (`atom.xml`)

use crate::config::{FeedFormat, SiteConfig};
use anyhow::Result;

pub mod atom;
mod common;
pub mod rss;

/// Build feed if enabled in config (RSS or Atom based on format setting).
pub fn build_feed(config: &SiteConfig) -> Result<()> {
    if config.build.feed.enable {
        match config.build.feed.format {
            FeedFormat::Rss => rss::build_rss(config)?,
            FeedFormat::Atom => atom::build_atom(config)?,
        }
    }
    Ok(())
}
