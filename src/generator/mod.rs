//! Content generators for static site output.
//!
//! Generates auxiliary files from compiled page metadata:
//!
//! - **RSS**: Feed for blog readers (`rss.xml`)
//! - **Sitemap**: Search engine indexing (`sitemap.xml`)
//!
//! Both generators use pre-collected `PageMeta` from the build pipeline,
//! avoiding redundant filesystem scans or re-compilation.

pub mod rss;
pub mod sitemap;
