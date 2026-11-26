//! Frame styles and raw-text payloads of native HTML Bundle documents.

mod frames;
mod inline;

pub(crate) use frames::align_inline_frames;
pub(crate) use inline::minify_generated_payloads;
