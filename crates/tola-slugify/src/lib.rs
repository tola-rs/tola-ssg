//! Turning text into a slug, and nothing else.
//!
//! A slug is the name a reader sees, distinguishes, and stores for a piece of text; it neither
//! encodes URLs nor chooses output paths, which belong to `tola-address`. A name's characters,
//! case, separator, and pronunciations are one [`NamingRules`] value.
//!
//! The tables that name Han text are the `han-tables` feature, on by default.
//! [`HanPronunciations`] and [`NamingRules::pronunciations`] are available in every build.
//! Without the feature, the pronunciation choice is unused and Han text takes the
//! character map, as any text no table names does.
//!
//! ```
//! use tola_slugify::{NamingRules, SlugMode, slugify_segment};
//!
//! let ascii = NamingRules {
//!     mode: SlugMode::Ascii,
//!     ..Default::default()
//! };
//! assert_eq!(slugify_segment("北京 Café", ascii).as_deref(), Some("bei-jing-cafe"));
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod romanization;
mod slug;

pub use slug::{
    HanPronunciations, NamingRules, SlugCase, SlugMode, SlugSeparator, slugify_segment,
};
