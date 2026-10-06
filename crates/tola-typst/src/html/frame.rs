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
    document: &HtmlDocument,
    path: &VirtualPath,
    cancellation: &BundleCancellation,
    style: &mut FrameStyleRule<'_>,
) -> Result<Option<(HtmlDocument, usize)>, HtmlFrameStyleError> {
    let mut count = 0;
    let Some(root) = style_element_frames(document.root(), path, cancellation, style, &mut count)?
    else {
        return Ok(None);
    };
    let mut replacement = document.clone();
    *replacement.root_mut() = root;
    Ok(Some((replacement, count)))
}

fn style_element_frames(
    element: &HtmlElement,
    document: &VirtualPath,
    cancellation: &BundleCancellation,
    style: &mut FrameStyleRule<'_>,
    count: &mut usize,
) -> Result<Option<HtmlElement>, HtmlFrameStyleError> {
    cancellation.ensure_active_as()?;
    let mut replacement = None;
    for (position, child) in element.children.iter().enumerate() {
        cancellation.ensure_active_as()?;
        let changed = match child {
            HtmlNode::Element(child) => {
                style_element_frames(child, document, cancellation, style, count)?
                    .map(HtmlNode::Element)
            }
            HtmlNode::Frame(frame) => {
                let declarations = style(document, element, frame);
                cancellation.ensure_active_as()?;
                let declarations =
                    declarations.map_err(|diagnostics| HtmlFrameStyleError::Rejected {
                        diagnostics: diagnostics.into_iter().map(Into::into).collect(),
                    })?;
                let mut css = frame.css.clone();
                for (name, value) in declarations {
                    if !css
                        .iter()
                        .any(|property| property.name == name && property.value.as_str() == value)
                    {
                        css.push(name, value);
                    }
                }
                if css == frame.css {
                    None
                } else {
                    let mut replacement = frame.clone();
                    replacement.css = css;
                    *count += 1;
                    Some(HtmlNode::Frame(replacement))
                }
            }
            HtmlNode::Text(..) | HtmlNode::Tag(_) => None,
        };
        if let Some(child) = changed {
            // Copy each changed ancestor once; untouched subtrees remain shared.
            replacement
                .get_or_insert_with(|| element.clone())
                .children
                .make_mut()[position] = child;
        }
    }
    cancellation.ensure_active_as()?;
    Ok(replacement)
}
