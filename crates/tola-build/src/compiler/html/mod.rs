//! Frame styles and raw-text payloads of native HTML Bundle documents.

mod frames;
mod payloads;

pub(crate) use frames::align_inline_frames;
pub(crate) use payloads::minify_generated_payloads;
