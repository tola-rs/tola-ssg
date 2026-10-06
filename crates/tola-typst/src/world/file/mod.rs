//! Typst file resolution, pure file providers, and read provenance.
//!
//! A [`FileProvider`] may supply in-memory bytes or map a complete Typst file
//! identity to a disk path. No provider touches the filesystem: [`FileResolver`]
//! owns all real reads, so successful byte origins and failed disk attempts
//! follow the same typed path through compilation and caching.

mod cache;
mod candidate;
mod evidence;
mod local_cache;
mod provider;
mod read;
mod resolver;

pub use cache::SharedFileCache;
pub use candidate::CandidateFileSnapshot;
pub use evidence::{
    ContentDigest, DiskReadPath, FileRead, ReadEvidence, ReadLocator, ReadOrigin,
    hash_length_prefixed,
};
pub(crate) use evidence::{Loaded, ReadAttempt};
pub(crate) use local_cache::LocalFileCache;
pub use provider::{EmptyFiles, FileMap, FileProvider, FileTarget};
pub use read::{EMPTY_ID, STDIN_ID, decode_utf8, file_id, file_id_from_path, virtual_file_id};
pub use resolver::FileResolver;
