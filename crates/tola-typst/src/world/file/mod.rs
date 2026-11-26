//! Typst file resolution, pure file providers, and read provenance.
//!
//! A [`FileProvider`] may supply in-memory bytes or map a complete Typst file
//! identity to a disk path. No provider touches the filesystem: [`FileStore`]
//! owns all real reads, so successful byte origins and failed disk attempts
//! follow the same typed path through compilation and caching.

mod cache;
mod local_packages;
mod provider;
mod read;
mod reads;
mod resolver;
mod view;

pub use cache::SharedFileCache;
pub use local_packages::{LocalPackage, LocalPackages};
pub use provider::{EmptyFiles, FileMap, FileProvider, FileTarget};
pub use read::{EMPTY_ID, STDIN_ID, decode_utf8, file_id, file_id_from_path, virtual_file_id};
pub use reads::{
    ContentDigest, DiskReadPath, FileRead, ReadLocator, ReadOrigin, ReadRecord,
    hash_length_prefixed,
};
pub(crate) use reads::{Loaded, ReadAttempt};
pub use resolver::FileStore;
pub use view::FileView;

pub(crate) fn cancelled() -> typst::diag::FileError {
    typst::diag::FileError::Other(Some("operation cancelled".into()))
}
