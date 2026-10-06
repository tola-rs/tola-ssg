//! The upstream panics Tola can explain to a site author.

use std::any::Any;

/// An upstream panic Tola recognizes, in the site author's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplainedPanic {
    /// What Tola could not do in the author's site.
    pub message: String,
    /// The concrete next action, when there is one.
    pub help: Option<String>,
    /// The consequence the author should know about.
    pub note: Option<String>,
}

/// Explain a panic payload Tola recognizes, or `None` for one it cannot.
///
/// The HTML exporter finishes a document by inserting its equation stylesheet into the document's
/// head, and panics when a document contains math but has no head; a site author reaches that by
/// writing their own `html.html`/`html.body` without an `html.head`. The panic has no source
/// location, so the explanation names the shape of the document instead.
pub fn explained_panic(payload: &(dyn Any + Send)) -> Option<ExplainedPanic> {
    let message = payload.downcast_ref::<&str>().copied();
    let message = message.or_else(|| payload.downcast_ref::<String>().map(String::as_str))?;
    let message = message.trim();
    if !message.contains("head to be present in document output") {
        return None;
    }
    Some(ExplainedPanic {
        message: "could not export a document whose `html.html` has no `html.head`".to_owned(),
        help: Some("Add `html.head[...]` (it may be empty)".to_owned()),
        note: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_panics_are_not_explained() {
        assert!(explained_panic(&"something else went wrong").is_none());
        assert!(explained_panic(&7u32).is_none());
    }
}
