//! Result of resolving one decoded site-root route.

use std::sync::Arc;
use tola_address::UrlPath;

#[derive(Debug, Clone)]
pub enum AddressResolution {
    Found {
        url: UrlPath,
    },

    NotFound,

    FragmentNotFound {
        fragment: String,
        /// Shared by all misses on this page.
        available: Arc<[String]>,
    },

    DocumentNotAllowed {
        /// Decoded route that named the document.
        reference: String,
        document: UrlPath,
    },
}
