//! CSS changes to already compiled frames without changing DOM or introspection structure.

use typst::diag::SourceResult;
use typst::syntax::VirtualPath;
use typst_html::{HtmlDocument, HtmlElement, HtmlFrame, HtmlNode};

use crate::bundle::BundleCancellation;

/// A frame styling operation was cancelled or rejected before any document changed.
#[derive(Debug, thiserror::Error)]
pub enum HtmlFrameStyleError {
    /// The caller cancelled traversal or preparation.
    #[error("HTML frame styling was cancelled")]
    Cancelled,
    /// The caller rejected a frame using native source diagnostics.
    #[error("HTML frame styling failed")]
    Rejected {
        /// Diagnostics retain their original spans and hints.
        diagnostics: Vec<crate::NativeDiagnostic>,
    },
}

impl crate::diagnostic::CancelledError for HtmlFrameStyleError {
    fn from_cancellation() -> Self {
        Self::Cancelled
    }
}

type FrameStyleRule<'a> = dyn FnMut(&VirtualPath, &HtmlElement, &HtmlFrame) -> SourceResult<Vec<(&'static str, String)>>
    + 'a;

pub(crate) fn style_document_frames(
    doc: &HtmlDocument,
    path: &VirtualPath,
    cancel: &BundleCancellation,
    style: &mut FrameStyleRule<'_>,
) -> Result<Option<(HtmlDocument, usize)>, HtmlFrameStyleError> {
    let mut count = 0;
    let Some(root) = style_element_frames(doc.root(), path, cancel, style, &mut count)? else {
        return Ok(None);
    };
    let mut replacement = doc.clone();
    *replacement.root_mut() = root;
    Ok(Some((replacement, count)))
}

fn style_element_frames(
    elem: &HtmlElement,
    doc: &VirtualPath,
    cancel: &BundleCancellation,
    style: &mut FrameStyleRule<'_>,
    count: &mut usize,
) -> Result<Option<HtmlElement>, HtmlFrameStyleError> {
    cancel.ensure_active_as()?;
    let mut replacement = None;
    for (pos, child) in elem.children.iter().enumerate() {
        cancel.ensure_active_as()?;
        let changed = match child {
            HtmlNode::Element(child) => {
                style_element_frames(child, doc, cancel, style, count)?.map(HtmlNode::Element)
            }
            HtmlNode::Frame(frame) => {
                let decls = style(doc, elem, frame);
                cancel.ensure_active_as()?;
                let decls = decls.map_err(|diags| HtmlFrameStyleError::Rejected {
                    diagnostics: diags.into_iter().map(Into::into).collect(),
                })?;
                apply_frame_css(frame, decls, count)
            }
            HtmlNode::Text(..) | HtmlNode::Tag(_) => None,
        };
        if let Some(child) = changed {
            // Copy each changed ancestor once; untouched subtrees remain shared.
            replacement
                .get_or_insert_with(|| elem.clone())
                .children
                .make_mut()[pos] = child;
        }
    }
    cancel.ensure_active_as()?;
    Ok(replacement)
}

fn apply_frame_css(
    frame: &HtmlFrame,
    decls: Vec<(&'static str, String)>,
    count: &mut usize,
) -> Option<HtmlNode> {
    let mut css = frame.css.clone();
    for (name, val) in decls {
        if !css
            .iter()
            .any(|prop| prop.name == name && prop.value.as_str() == val)
        {
            css.push(name, val);
        }
    }
    if css == frame.css {
        return None;
    }
    let mut replacement = frame.clone();
    replacement.css = css;
    *count += 1;
    Some(HtmlNode::Frame(replacement))
}
