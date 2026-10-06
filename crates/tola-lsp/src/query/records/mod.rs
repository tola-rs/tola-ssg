//! The records one chain of reads addresses: where their declared shape comes from, what shape
//! they answer, and the value a parameter provably takes from the site's calls.

pub(in crate::query) mod declared;
pub(in crate::query) mod origin;
pub(in crate::query) mod parameters;
