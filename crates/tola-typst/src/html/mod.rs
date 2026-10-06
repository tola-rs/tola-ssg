//! Typed inspection and reference binding of native HTML documents.

mod document;
mod fragment;
mod frame;
mod inventory;
mod raw_text;
mod reference;

pub use document::HtmlDocument;
pub use fragment::{
    HtmlFragmentError, HtmlFragmentExport, HtmlFragmentOptions, HtmlFragmentSelection,
};
pub(crate) use fragment::{HtmlFragmentIndex, export_fragment};
pub use frame::HtmlFrameStyleError;
pub(crate) use frame::style_document_frames;
pub use inventory::{
    HtmlBaseHref, HtmlDocumentInventory, HtmlFragment, HtmlFragmentKind, HtmlReference,
};
pub use raw_text::{HtmlRawText, HtmlRawTextError};
pub(crate) use raw_text::{apply_raw_text, collect_raw_text};
pub use reference::{HtmlReferenceUse, is_javascript_mime_type};
