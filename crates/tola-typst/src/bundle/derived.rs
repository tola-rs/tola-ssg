//! Views derived from one realized Bundle: compiled documents, HTML indexes,
//! fragments, frame styling, and raw-text rewriting.

use crate::diagnostic::CompileError;
use crate::extract::{Heading, HeadingExtractor, Link, LinkExtractor, extract_elements};
use crate::introspection::{
    MetadataCardinalityError, MetadataDeclaration, all_values, first_value, label_selector,
    metadata_declaration, unique_metadata_value,
};
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use typst::foundations::{Content, Selector, Value};
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
        let Some(label) = label_selector(label) else {
            return Vec::new();
        };
        self.introspector()
            .query(&Selector::Label(label))
            .iter()
            .filter_map(metadata_declaration)
            .collect()
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
        cancellation: &BundleCancellation,
    ) -> Result<Option<Arc<crate::html::HtmlDocumentInventory>>, CompileError> {
        cancellation.ensure_active()?;
        let BundleDocument::Html(document) = self.document else {
            return Ok(None);
        };
        let indexes = self.compilation.html_indexes(cancellation)?;
        if let Some(inventory) = indexes
            .get(self.path)
            .and_then(|indexes| indexes.inventory.as_ref())
        {
            return Ok(Some(Arc::clone(inventory)));
        }
        // Hold the index lock only for map operations, never for the DOM walk.
        drop(indexes);
        let inventory = Arc::new(crate::html::HtmlDocumentInventory::from_bundle_document(
            self.path,
            document,
            self.compilation.introspector(),
            cancellation,
        )?);
        cancellation.ensure_active()?;
        let mut indexes = self.compilation.html_indexes(cancellation)?;
        let slot = indexes.entry(self.path.clone()).or_default();
        if let Some(existing) = &slot.inventory {
            return Ok(Some(Arc::clone(existing)));
        }
        slot.inventory = Some(Arc::clone(&inventory));
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
        cancellation: &BundleCancellation,
    ) -> Result<crate::html::HtmlFragmentExport, crate::html::HtmlFragmentError> {
        self.html_fragment_with_links(selection, options, cancellation, &mut |url, _| {
            Ok(url.to_owned())
        })
    }

    /// Export a compiled HTML subtree while the host rewrites its URL references.
    ///
    /// Structural encoding remains owned by Typst. The callback supplies only
    /// reference policy, including references inside native SVG frames.
    pub fn html_fragment_with_links(
        &self,
        selection: &crate::html::HtmlFragmentSelection,
        options: &crate::html::HtmlFragmentOptions,
        cancellation: &BundleCancellation,
        rewrite: &mut dyn FnMut(
            &str,
            crate::html::HtmlReferenceUse,
        ) -> Result<String, crate::html::HtmlFragmentError>,
    ) -> Result<crate::html::HtmlFragmentExport, crate::html::HtmlFragmentError> {
        if cancellation.is_cancelled() {
            return Err(crate::html::HtmlFragmentError::Cancelled);
        }
        let BundleDocument::Html(document) = self.document else {
            return Err(crate::html::HtmlFragmentError::NotHtmlDocument {
                path: self.path.clone(),
            });
        };
        let index = self
            .compilation
            .html_fragment_index(self.path, document, cancellation)?;
        crate::html::export_fragment(
            document,
            self,
            &index,
            selection,
            options,
            cancellation,
            rewrite,
        )
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

    fn elements(&self) -> Box<dyn Iterator<Item = &typst::foundations::Content> + '_> {
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

impl BundleCompilation {
    fn html_fragment_index(
        &self,
        path: &typst::syntax::VirtualPath,
        document: &typst_html::HtmlDocument,
        cancellation: &BundleCancellation,
    ) -> Result<Arc<crate::html::HtmlFragmentIndex>, crate::html::HtmlFragmentError> {
        let indexes = self
            .html_indexes(cancellation)
            .map_err(|_| crate::html::HtmlFragmentError::Cancelled)?;
        if let Some(index) = indexes
            .get(path)
            .and_then(|indexes| indexes.fragments.as_ref())
        {
            return Ok(Arc::clone(index));
        }
        drop(indexes);
        let index = Arc::new(crate::html::HtmlFragmentIndex::new(document, cancellation)?);
        if cancellation.is_cancelled() {
            return Err(crate::html::HtmlFragmentError::Cancelled);
        }
        let mut indexes = self
            .html_indexes(cancellation)
            .map_err(|_| crate::html::HtmlFragmentError::Cancelled)?;
        let slot = indexes.entry(path.clone()).or_default();
        if let Some(existing) = &slot.fragments {
            return Ok(Arc::clone(existing));
        }
        slot.fragments = Some(Arc::clone(&index));
        Ok(index)
    }

    fn html_indexes(
        &self,
        cancellation: &BundleCancellation,
    ) -> Result<
        parking_lot::MutexGuard<'_, HashMap<typst::syntax::VirtualPath, HtmlDocumentIndexes>>,
        CompileError,
    > {
        loop {
            cancellation.ensure_active()?;
            if let Some(indexes) = self
                .html_indexes
                .try_lock_for(HTML_INDEX_LOCK_POLL_INTERVAL)
            {
                cancellation.ensure_active()?;
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
        cancellation.ensure_active_as()?;
        let mut replacements = Vec::new();
        let mut count = 0;
        for (path, file) in self.bundle.files.iter() {
            cancellation.ensure_active_as()?;
            let BundleFile::Document(BundleDocument::Html(document)) = file else {
                continue;
            };
            if let Some((replacement, changed)) =
                crate::html::style_document_frames(document, path, cancellation, &mut style)?
            {
                count += changed;
                replacements.push((path.clone(), Box::new(replacement)));
            }
        }
        cancellation.ensure_active_as()?;
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
        cancellation.ensure_active_as()?;
        let mut replacements = Vec::new();
        let mut count = 0;
        for (path, file) in self.bundle.files.iter() {
            cancellation.ensure_active_as()?;
            let BundleFile::Document(BundleDocument::Html(document)) = file else {
                continue;
            };
            let changes =
                crate::html::collect_raw_text(document, path, cancellation, &mut rewrite)?;
            if changes.is_empty() {
                continue;
            }
            count += changes.len();
            let mut replacement = document.clone();
            crate::html::apply_raw_text(&mut replacement, changes, cancellation)?;
            replacements.push((path.clone(), replacement));
        }
        cancellation.ensure_active_as()?;
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
    fn raw_text_rewrite_replaces_only_payloads() {
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
        let cancellation = BundleCancellation::default();
        let mut observed = Vec::new();
        let rewritten = compilation
            .rewrite_html_raw_text(&cancellation, |path, kind, span, text| {
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
        // Every payload reports the text it was written from, so a caller that cannot
        // rewrite one can locate it in the source file.
        assert_eq!(
            observed
                .iter()
                .map(|(_, _, span, _)| {
                    span.id().map(|id| id.vpath().get_with_slash().to_owned())
                })
                .collect::<Vec<_>>(),
            vec![
                Some("/main.typ".to_owned()),
                Some("/main.typ".to_owned()),
                Some("/main.typ".to_owned()),
            ]
        );
        let entries = compilation
            .export(&BundleOptions::default(), &cancellation, None)
            .unwrap();
        let html = String::from_utf8(
            entry_from(entries.entries(), "/index.html")
                .bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("<style>.a{color:red}</style>"), "{html}");
        assert!(
            html.contains("<script>function answer(){return 42}</script>"),
            "{html}"
        );
        assert!(html.contains("export const answer = 6 + 6"), "{html}");
        assert!(html.contains(r#"id="kept""#), "{html}");
        assert!(html.contains(r#"class="body""#), "{html}");
        assert!(html.contains(r#"{"a": 1}"#), "{html}");
    }
    #[test]
    fn cancelled_raw_text_rewrite_is_inert() {
        let mut compilation =
            compile_without_export(r#"#document("index.html")[#html.style(".a { color: red; }")]"#);
        let cancellation = BundleCancellation::default();
        cancellation.cancel();
        let error = compilation
            .rewrite_html_raw_text(&cancellation, |_, _, _, _| Some(String::new()))
            .unwrap_err();
        assert!(matches!(error, crate::html::HtmlRawTextError::Cancelled));

        let cancellation = BundleCancellation::default();
        let entries = compilation
            .export(&BundleOptions::default(), &cancellation, None)
            .unwrap();
        let html = String::from_utf8(
            entry_from(entries.entries(), "/index.html")
                .bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains(".a { color: red; }"), "{html}");
    }
    #[test]
    fn frame_css_changes_apply_once() {
        let mut compilation = compile_without_export(
            r#"#document("page.html")[
  #html.div[
    #html.span(id: "frame")[#box(html.frame(rect(width: 2pt, height: 3pt)))]
    #html.div[
      #html.span(id: "nested")[#box(html.frame(rect(width: 2pt, height: 3pt)))]
      #html.span(id: "level")[#box(html.frame(rect(width: 2pt, height: 3pt)))]
    ]
  ]
]"#,
        );
        let path = typst::syntax::VirtualPath::new("page.html").unwrap();
        let cancellation = BundleCancellation::new();
        let inventory = compilation
            .document(&path)
            .unwrap()
            .html_inventory(&cancellation)
            .unwrap()
            .unwrap();
        let style = |_: &typst::syntax::VirtualPath,
                     parent: &typst_html::HtmlElement,
                     _: &typst_html::HtmlFrame| {
            if parent
                .attrs
                .get(typst_html::attr::id)
                .is_some_and(|id| id == "level")
            {
                return Ok(Vec::new());
            }
            Ok(vec![
                ("vertical-align", "1em".to_owned()),
                ("vertical-align", "-0.25em".to_owned()),
            ])
        };
        assert_eq!(
            compilation.style_html_frames(&cancellation, style).unwrap(),
            2
        );
        let updated = compilation
            .document(&path)
            .unwrap()
            .html_inventory(&cancellation)
            .unwrap()
            .unwrap();
        assert!(!Arc::ptr_eq(&inventory, &updated));
        assert!(
            updated
                .fragments()
                .iter()
                .any(|fragment| fragment.value() == "frame")
        );
        for id in ["frame", "nested", "level"] {
            let html = compilation
                .document(&path)
                .unwrap()
                .html_fragment(
                    &crate::html::HtmlFragmentSelection::Id(id.into()),
                    &crate::html::HtmlFragmentOptions::default(),
                    &cancellation,
                )
                .unwrap()
                .html;
            assert_eq!(
                html.contains("vertical-align: -0.25em"),
                id != "level",
                "{html}"
            );
            assert!(!html.contains("vertical-align: 1em"), "{html}");
        }
        assert_eq!(
            compilation.style_html_frames(&cancellation, style).unwrap(),
            0
        );
        let repeated = compilation
            .document(&path)
            .unwrap()
            .html_inventory(&cancellation)
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&updated, &repeated));
    }
    #[test]
    fn rejected_frame_style_installs_nothing() {
        for cancel in [false, true] {
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
            let cancellation = BundleCancellation::new();
            let mut seen = 0;
            let error = compilation
                .style_html_frames(&cancellation, |_, _, frame| {
                    seen += 1;
                    if seen == 2 {
                        if cancel {
                            cancellation.cancel();
                        }
                        typst::diag::bail!(frame.span, "declined frame style");
                    }
                    Ok(vec![("vertical-align", "-1em".to_owned())])
                })
                .unwrap_err();
            assert_eq!(seen, 2);
            assert!(matches!(
                (&error, cancel),
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
    #[cfg(feature = "parallel")]
    #[test]
    fn concurrent_inventory_publishes_one_index() {
        let compilation = compile_without_export(
            r#"#document("first.html")[#html.a(href: "/first")[First]]
#document("second.html")[#html.a(href: "/second")[Second]]"#,
        );
        let first_path = typst::syntax::VirtualPath::new("first.html").unwrap();
        let second_path = typst::syntax::VirtualPath::new("second.html").unwrap();
        let cancellation = BundleCancellation::new();
        fn inventory(
            compilation: &BundleCompilation,
            path: &typst::syntax::VirtualPath,
            cancellation: &BundleCancellation,
        ) -> Arc<crate::html::HtmlDocumentInventory> {
            compilation
                .document(path)
                .unwrap()
                .html_inventory(cancellation)
                .unwrap()
                .unwrap()
        }

        let (first, second) = rayon::join(
            || inventory(&compilation, &first_path, &cancellation),
            || inventory(&compilation, &second_path, &cancellation),
        );
        assert_eq!(first.references()[0].destination(), "/first");
        assert_eq!(second.references()[0].destination(), "/second");
        assert!(!Arc::ptr_eq(&first, &second));

        let (first_again, second_again) = rayon::join(
            || inventory(&compilation, &first_path, &cancellation),
            || inventory(&compilation, &second_path, &cancellation),
        );
        assert!(Arc::ptr_eq(&first, &first_again));
        assert!(Arc::ptr_eq(&second, &second_again));
    }
    #[test]
    fn cancelled_inventory_keeps_cache() {
        let compilation = compile_without_export(
            r#"#document("page.html")[
  #link("/first/")[First]
  #link("/second/")[Second]
]"#,
        );
        let path = typst::syntax::VirtualPath::new("page.html").unwrap();
        let document = compilation.document(&path).unwrap();
        let cancelled = BundleCancellation::new();
        cancelled.cancel();
        assert!(
            document
                .html_inventory(&cancelled)
                .unwrap_err()
                .is_cancelled()
        );

        let cancellation = BundleCancellation::new();
        let inventory = document.html_inventory(&cancellation).unwrap().unwrap();
        assert_eq!(
            inventory
                .references()
                .iter()
                .filter(|reference| reference.tag() == "a")
                .count(),
            2
        );
        assert!(
            document
                .html_inventory(&cancelled)
                .unwrap_err()
                .is_cancelled()
        );
        let repeated = document.html_inventory(&cancellation).unwrap().unwrap();
        assert!(Arc::ptr_eq(&inventory, &repeated));
    }
}
