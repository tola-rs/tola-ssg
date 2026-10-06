//! Validated SVG icons shared by CSS generation and document producers.
//!
//! Parses caller-supplied bytes into immutable, self-contained SVG values without I/O or caching.
//! An Iconify source prefix is metadata; the caller chooses the collection namespace.
//!
//! ```
//! use tola_icons::{IconCollections, IconCollection, IconPaint};
//!
//! let mut brand = IconCollection::new();
//! brand.insert_svg("mark", br#"<svg viewBox="0 0 24 12">
//!   <path fill="currentColor" d="M0 0h24v12H0z"/>
//! </svg>"#)?;
//! let mut icons = IconCollections::new();
//! icons.mount("brand", brand)?;
//! let mark = icons.get("brand", "mark").unwrap();
//! assert_eq!(mark.aspect_ratio(), 2.0);
//! assert_eq!(mark.paint(), IconPaint::CurrentColor);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod collection;
mod collections;
mod identity;
mod svg;

pub use collection::{IconCollection, IconLicense, InvalidCollection};
pub use collections::{IconCollections, InvalidIconNamespace};
pub use identity::{IconCollectionName, IconId, IconName, InvalidIconName};
pub use svg::{IconPaint, InvalidSvg, SvgIcon, ViewBox};
