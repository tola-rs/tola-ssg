//! Views derived from one realized Bundle: compiled documents, HTML indexes,
//! fragments, frame styling, and raw-text rewriting.

use crate::diagnostic::CompileError;
use crate::extract::{Heading, HeadingExtractor, Link, LinkExtractor, extract_elements};
use crate::introspection::{
    MetadataCardinalityError, MetadataDeclaration, all_values, first_value, labeled_declarations,
    unique_metadata_value,
};
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use typst::foundations::{Content, Value};
use typst::introspection::Introspector;
use typst::syntax::FileId;
use typst_bundle::{BundleDocument, BundleFile};

use super::*;

/// Recheck cancellation while waiting for another HTML index operation.
const HTML_INDEX_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Introspection access for one compiled document in a bundle.
///
/// Preserves native Typst values without prescribing serialization or publication conventions.
#[derive(Clone)]
pub struct CompiledBundleDocument<'a> {
    pub(super) path: &'a typst::syntax::VirtualPath,
    pub(super) document: &'a BundleDocument,
    pub(super) compilation: &'a BundleCompilation,
}

impl std::fmt::Debug for CompiledBundleDocument<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompiledBundleDocument")
            .field("path", self.path)
            .field("kind", &self.kind())
            .finish_non_exhaustive()
    }
}

impl CompiledBundleDocument<'_> {
    /// The normalized virtual output path assigned by Typst.
    pub fn path(&self) -> &typst::syntax::VirtualPath {
        self.path
    }

    /// Export kind selected for this document.
    pub fn kind(&self) -> BundleDocumentKind {
        document_kind(self.document)
    }

    /// Native metadata configured through Typst's `document` element.
    pub fn info(&self) -> &typst::model::DocumentInfo {
        self.document.info()
    }

    /// First matching metadata value in this document's native order.
    pub fn metadata_first(&self, label: &str) -> Option<Value> {
        first_value(self.metadata_declarations(label))
    }

    /// One metadata value, distinguishing an absent label from duplicate declarations.
    pub fn metadata_unique(&self, label: &str) -> Result<Option<Value>, MetadataCardinalityError> {
        unique_metadata_value(label, self.metadata_all(label))
    }

    /// Query all metadata values by label in document order.
    pub fn metadata_all(&self, label: &str) -> Vec<Value> {
        all_values(self.metadata_declarations(label))
    }

    /// Query all labeled metadata declarations in document order.
    ///
    /// Unlike [`Self::metadata_all`], retains declaration spans for reporting
    /// malformed protocol values without custom Typst elements.
    pub fn metadata_declarations(&self, label: &str) -> Vec<MetadataDeclaration> {
        labeled_declarations(self.introspector(), label)
    }

    /// Links realized in this document, excluding all sibling documents.
    pub fn links(&self) -> Vec<Link> {
        extract_elements(self.elements(), LinkExtractor::new())
    }

    /// Semantic headings realized in this document, excluding all siblings.
    ///
    /// HTML IDs are excluded: show rules can make DOM headings differ from
    /// Typst headings.
    pub fn headings(&self) -> Vec<Heading> {
        let mut headings = extract_elements(self.elements(), HeadingExtractor::new());
        for heading in &mut headings {
            heading.fragment = heading
                .location
                .and_then(|location| self.compilation.introspector().anchor(location))
                .map(ToString::to_string)
                .filter(|fragment| !fragment.is_empty());
        }
        headings
    }

    /// Borrow the official HTML DOM without exporting or copying it.
    ///
    /// Paged documents have no HTML root. The root belongs to this compilation;
    /// host transformations must use the compilation's transactional mutation APIs.
    pub fn html_root(&self) -> Option<&typst_html::HtmlElement> {
        match self.document {
            BundleDocument::Html(document) => Some(document.root()),
            BundleDocument::Paged(..) => None,
        }
    }

    /// Inspect this document's final Typst HTML DOM.
    ///
    /// One inventory is built per compilation and shared by its readers; a fork builds its own.
    /// Returns `None` for paged Bundle documents.
    pub fn html_inventory(
        &self,
        cancel: &BundleCancellation,
    ) -> Result<Option<Arc<crate::html::HtmlDocumentInventory>>, CompileError> {
        cancel.ensure_active()?;
        let BundleDocument::Html(doc) = self.document else {
            return Ok(None);
        };
        let inventory = self.compilation.cached_html_index(
            self.path,
            cancel,
            |slot| &mut slot.inventory,
            || {
                crate::html::HtmlDocumentInventory::from_bundle_document(
                    self.path,
                    doc,
                    self.compilation.introspector(),
                    cancel,
                )
            },
        )?;
        Ok(Some(inventory))
    }

    /// Export a body or uniquely identified HTML element with its style dependencies.
    ///
    /// Native late links use this document's own Bundle introspector. The complete
    /// document is not recompiled, and the original DOM remains unchanged.
    pub fn html_fragment(
        &self,
        selection: &crate::html::HtmlFragmentSelection,
        options: &crate::html::HtmlFragmentOptions,
        cancel: &BundleCancellation,
    ) -> Result<crate::html::HtmlFragmentExport, crate::html::HtmlFragmentError> {
        self.html_fragment_with_links(selection, options, cancel, &mut |url, _| Ok(url.to_owned()))
    }

    /// Export a compiled HTML subtree while the host rewrites its URL references.
    ///
    /// Structural encoding remains owned by Typst. The callback supplies only
    /// reference policy, including references inside native SVG frames.
    pub fn html_fragment_with_links(
        &self,
        selection: &crate::html::HtmlFragmentSelection,
        options: &crate::html::HtmlFragmentOptions,
        cancel: &BundleCancellation,
        rewrite: &mut dyn FnMut(
            &str,
            crate::html::HtmlReferenceUse,
        ) -> Result<String, crate::html::HtmlFragmentError>,
    ) -> Result<crate::html::HtmlFragmentExport, crate::html::HtmlFragmentError> {
        if cancel.is_cancelled() {
            return Err(crate::html::HtmlFragmentError::Cancelled);
        }
        let BundleDocument::Html(doc) = self.document else {
            return Err(crate::html::HtmlFragmentError::NotHtmlDocument {
                path: self.path.clone(),
            });
        };
        let index = self.compilation.cached_html_index(
            self.path,
            cancel,
            |slot| &mut slot.fragments,
            || crate::html::HtmlFragmentIndex::new(doc, cancel),
        )?;
        crate::html::export_fragment(doc, self, &index, selection, options, cancel, rewrite)
    }

    pub(crate) fn link_resolver(&self) -> typst::model::LateLinkResolver<'_> {
        typst::model::LateLinkResolver::new(Some(self.path), self.compilation.introspector())
    }

    /// File identities of the original body of this native Bundle
    /// document, before target-specific realization.
    ///
    /// These IDs describe source provenance, not a one-to-one mapping from
    /// source files to output documents.
    pub fn source_ids(&self) -> Vec<FileId> {
        let original_body = self
            .compilation
            .original_bodies()
            .get(self.path)
            .expect("compiled Bundle document retains its original body");
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        let mut root = true;
        let _ = original_body.traverse(&mut |content: Content| {
            if root {
                root = false;
                return ControlFlow::<()>::Continue(());
            }
            if let Some(id) = content.span().id()
                && seen.insert(id)
            {
                ids.push(id);
            }
            ControlFlow::Continue(())
        });
        if ids.is_empty()
            && let Some(id) = original_body.span().id()
        {
            ids.push(id);
        }
        ids
    }

    /// Source identities actually represented by this document's realized HTML nodes.
    ///
    /// Deferred content may not occur in the original body. These IDs describe rendered
    /// contribution, not page ownership or a source-to-element position mapping.
    pub fn rendered_source_ids(
        &self,
        cancellation: &BundleCancellation,
    ) -> Result<Vec<FileId>, CompileError> {
        let Some(root) = self.html_root() else {
            return Ok(Vec::new());
        };
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        let mut pending = vec![root];
        while let Some(element) = pending.pop() {
            cancellation.ensure_active()?;
            if let Some(id) = element.span.id()
                && seen.insert(id)
            {
                ids.push(id);
            }
            for child in &element.children {
                cancellation.ensure_active()?;
                if let Some(id) = child.span().id()
                    && seen.insert(id)
                {
                    ids.push(id);
                }
            }
            pending.extend(
                element
                    .children
                    .iter()
                    .rev()
                    .filter_map(|child| match child {
                        typst_html::HtmlNode::Element(element) => Some(element),
                        _ => None,
                    }),
            );
        }
        Ok(ids)
    }

    fn elements(&self) -> Box<dyn Iterator<Item = &typst::foundations::Content> + '_> {
        // The shared trait object answers selectors only; each document kind's own introspector
        // is the one that enumerates its realized elements.
        match self.document {
            BundleDocument::Paged(document, _) => {
                Box::new(document.introspector().elements().all())
            }
            BundleDocument::Html(document) => Box::new(document.introspector().elements().all()),
        }
    }

    fn introspector(&self) -> &dyn Introspector {
        match self.document {
            BundleDocument::Paged(document, _) => document.introspector().as_ref(),
            BundleDocument::Html(document) => document.introspector().as_ref(),
        }
    }
}

impl<'a> CompiledBundleDocument<'a> {
    /// The document `file` names, when it is one.
    pub(super) fn of(
        path: &'a typst::syntax::VirtualPath,
        file: &'a BundleFile,
        compilation: &'a BundleCompilation,
    ) -> Option<Self> {
        let BundleFile::Document(document) = file else {
            return None;
        };
        Some(Self {
            path,
            document,
            compilation,
        })
    }
}

/// One rewritten HTML document and the page count it replaced.
type RewrittenDocument = (typst::syntax::VirtualPath, Box<typst_html::HtmlDocument>);

impl BundleCompilation {
    /// Every HTML document's replacement, built before anything is installed.
    fn rewrite_documents<E: crate::diagnostic::CancelledError>(
        &self,
        cancellation: &BundleCancellation,
        mut rewrite: impl FnMut(
            &typst::syntax::VirtualPath,
            &typst_html::HtmlDocument,
        ) -> Result<Option<(Box<typst_html::HtmlDocument>, usize)>, E>,
    ) -> Result<(Vec<RewrittenDocument>, usize), E> {
        cancellation.ensure_active_as()?;
        let mut replacements = Vec::new();
        let mut count = 0;
        for (path, file) in self.bundle.files.iter() {
            cancellation.ensure_active_as()?;
            let BundleFile::Document(BundleDocument::Html(document)) = file else {
                continue;
            };
            if let Some((replacement, changed)) = rewrite(path, document)? {
                count += changed;
                replacements.push((path.clone(), replacement));
            }
        }
        cancellation.ensure_active_as()?;
        Ok((replacements, count))
    }

    fn cached_html_index<T, E: crate::diagnostic::CancelledError>(
        &self,
        path: &typst::syntax::VirtualPath,
        cancel: &BundleCancellation,
        slot_field: impl Fn(&mut HtmlDocumentIndexes) -> &mut Option<Arc<T>>,
        build: impl FnOnce() -> Result<T, E>,
    ) -> Result<Arc<T>, E> {
        let mut indexes = self.html_indexes(cancel)?;
        if let Some(existing) = indexes
            .get_mut(path)
            .and_then(|slot| slot_field(slot).as_ref().map(Arc::clone))
        {
            return Ok(existing);
        }
        // Hold the index lock only for map operations, never for the DOM walk.
        drop(indexes);
        let built = Arc::new(build()?);
        cancel.ensure_active_as()?;
        let mut indexes = self.html_indexes(cancel)?;
        let slot = slot_field(indexes.entry(path.clone()).or_default());
        Ok(Arc::clone(slot.get_or_insert_with(|| Arc::clone(&built))))
    }

    fn html_indexes<E: crate::diagnostic::CancelledError>(
        &self,
        cancel: &BundleCancellation,
    ) -> Result<
        parking_lot::MutexGuard<'_, HashMap<typst::syntax::VirtualPath, HtmlDocumentIndexes>>,
        E,
    > {
        loop {
            cancel.ensure_active_as()?;
            if let Some(indexes) = self
                .html_indexes
                .try_lock_for(HTML_INDEX_LOCK_POLL_INTERVAL)
            {
                cancel.ensure_active_as()?;
                return Ok(indexes);
            }
        }
    }

    /// Change CSS on selected compiled frames, without modifying DOM structure or native anchors.
    ///
    /// The callback observes the document path, immediate containing element, and immutable
    /// frame, then returns CSS properties to set. Unmentioned properties are retained;
    /// an empty vector leaves a frame unchanged. Property names are static, as required
    /// by Typst's public CSS setter. Repeated property names use their final value.
    /// All callbacks and changed document copies complete before installation. An error or
    /// cancellation leaves the original Bundle and its derived indexes unchanged.
    /// Returns the number of frames whose CSS changed.
    pub fn style_html_frames(
        &mut self,
        cancellation: &BundleCancellation,
        mut style: impl FnMut(
            &typst::syntax::VirtualPath,
            &typst_html::HtmlElement,
            &typst_html::HtmlFrame,
        ) -> typst::diag::SourceResult<Vec<(&'static str, String)>>,
    ) -> Result<usize, crate::html::HtmlFrameStyleError> {
        let (replacements, count) = self.rewrite_documents(cancellation, |path, document| {
            crate::html::style_document_frames(document, path, cancellation, &mut style)
                .map(|found| found.map(|(replacement, changed)| (Box::new(replacement), changed)))
        })?;
        install_html_documents(self, replacements);
        Ok(count)
    }

    /// Rewrite the text payloads of compiled raw-text elements, without modifying DOM structure.
    ///
    /// The callback observes the document path, the payload kind, the span the payload reports, and
    /// the current text, then returns replacement text; `None` keeps the payload unchanged. Element
    /// structure, attributes, and every other node are never modified. All callbacks and changed
    /// document copies complete before installation, so an error or cancellation leaves the
    /// original Bundle and its derived indexes unchanged. Returns the number of rewritten payloads.
    pub fn rewrite_html_raw_text(
        &mut self,
        cancellation: &BundleCancellation,
        mut rewrite: impl FnMut(
            &typst::syntax::VirtualPath,
            crate::html::HtmlRawText,
            typst::syntax::Span,
            &str,
        ) -> Option<String>,
    ) -> Result<usize, crate::html::HtmlRawTextError> {
        let (replacements, count) = self.rewrite_documents(cancellation, |path, document| {
            let changes =
                crate::html::collect_raw_text(document, path, cancellation, &mut rewrite)?;
            if changes.is_empty() {
                return Ok(None);
            }
            let changed = changes.len();
            let mut replacement = document.clone();
            crate::html::apply_raw_text(&mut replacement, changes, cancellation)?;
            Ok(Some((Box::new(replacement), changed)))
        })?;
        install_html_documents(self, replacements);
        Ok(count)
    }
}

/// Install prepared HTML documents together, shared by every derived-DOM rewrite.
pub(super) fn install_html_documents(
    compilation: &mut BundleCompilation,
    replacements: Vec<(typst::syntax::VirtualPath, Box<typst_html::HtmlDocument>)>,
) {
    if replacements.is_empty() {
        return;
    }
    let files = Arc::make_mut(&mut compilation.bundle.files);
    for (path, replacement) in replacements {
        let Some(BundleFile::Document(BundleDocument::Html(document))) = files.get_mut(&path)
        else {
            unreachable!("HTML document replacement preserves the document set")
        };
        *document = replacement;
        compilation.html_indexes.get_mut().remove(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{compile_without_export, entry_from};
    use super::*;
    use typst_bundle::BundleOptions;
    #[test]
    fn raw_text_rewrite_payloads() {
        use crate::html::HtmlRawText;

        let mut compilation = compile_without_export(
            r#"#document("index.html")[
  #html.style(".a { color: red; }")
  #html.script("function answer() { return 6 * 7; }")
  #html.elem("script", attrs: (type: "module"))[export const answer = 6 + 6]
  #html.elem("script", attrs: (type: "application/json"))[#("{\"a\": 1}")]
  #html.elem("p", attrs: (id: "kept", class: "body"))[Body]
]"#,
        );
        let cancel = BundleCancellation::default();
        let mut observed = Vec::new();
        let rewritten = compilation
            .rewrite_html_raw_text(&cancel, |path, kind, span, text| {
                observed.push((
                    path.get_with_slash().to_string(),
                    kind,
                    span,
                    text.to_owned(),
                ));
                match kind {
                    HtmlRawText::Stylesheet => Some(".a{color:red}".to_owned()),
                    HtmlRawText::Script => Some("function answer(){return 42}".to_owned()),
                    HtmlRawText::Module => None,
                }
            })
            .unwrap();

        assert_eq!(rewritten, 2);
        assert_eq!(
            observed
                .iter()
                .map(|(path, kind, _, _)| (path.as_str(), *kind))
                .collect::<Vec<_>>(),
            vec![
                ("/index.html", HtmlRawText::Stylesheet),
                ("/index.html", HtmlRawText::Script),
                ("/index.html", HtmlRawText::Module),
            ]
        );
        assert_eq!(
            observed
                .iter()
                .map(|(_, _, span, _)| {
                    span.id().map(|fid| fid.vpath().get_with_slash().to_owned())
                })
                .collect::<Vec<_>>(),
            vec![
                Some("/main.typ".to_owned()),
                Some("/main.typ".to_owned()),
                Some("/main.typ".to_owned()),
            ]
        );
        let entries = compilation
            .export(&BundleOptions::default(), &cancel, None)
            .unwrap();
        let html = String::from_utf8(
            entry_from(entries.entries(), "/index.html")
                .bytes()
                .to_vec(),
        )
        .unwrap();
        for payload in [
            "<style>.a{color:red}</style>",
            "<script>function answer(){return 42}</script>",
            "export const answer = 6 + 6",
        ] {
            assert!(html.contains(payload), "{payload}: {html}");
        }
    }

    #[test]
    fn frame_style_atomic_rollback() {
        for cancel_flag in [false, true] {
            let mut compilation = compile_without_export(
                r#"
#document("a.html")[#box(html.frame(rect(width: 2pt, height: 3pt)))]
#document("b.html")[#box(html.frame(rect(width: 3pt, height: 4pt)))]
"#,
            );
            let fresh = BundleCancellation::new();
            let before = compilation
                .export_entries(&BundleOptions::default(), &fresh, None)
                .unwrap();
            let path = typst::syntax::VirtualPath::new("a.html").unwrap();
            let inventory = compilation
                .document(&path)
                .unwrap()
                .html_inventory(&fresh)
                .unwrap()
                .unwrap();
            let cancel = BundleCancellation::new();
            let mut seen = 0;
            let err = compilation
                .style_html_frames(&cancel, |_, _, frame| {
                    seen += 1;
                    if seen == 2 {
                        if cancel_flag {
                            cancel.cancel();
                        }
                        typst::diag::bail!(frame.span, "declined frame style");
                    }
                    Ok(vec![("vertical-align", "-1em".to_owned())])
                })
                .unwrap_err();
            assert_eq!(seen, 2);
            assert!(matches!(
                (&err, cancel_flag),
                (crate::html::HtmlFrameStyleError::Cancelled, true)
                    | (crate::html::HtmlFrameStyleError::Rejected { .. }, false)
            ));
            let after = compilation
                .export_entries(&BundleOptions::default(), &fresh, None)
                .unwrap();
            for (before, after) in before.iter().zip(after.iter()) {
                assert_eq!(before.bytes().as_slice(), after.bytes().as_slice());
            }
            let retained = compilation
                .document(&path)
                .unwrap()
                .html_inventory(&fresh)
                .unwrap()
                .unwrap();
            assert!(Arc::ptr_eq(&inventory, &retained));
        }
    }
}
