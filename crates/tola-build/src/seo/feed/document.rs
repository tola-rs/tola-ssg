//! Feed bodies selected from the final official Bundle HTML DOM.

use std::collections::BTreeMap;
use std::sync::Arc;

use tola_typst::{
    BundleCompilation, HtmlFragmentError, HtmlFragmentOptions, HtmlFragmentSelection,
    HtmlReferenceUse,
};
use typst::foundations::Dict;

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::config::ResolvedSiteConfig;
use crate::seo::RenderError;
use crate::seo::declaration::{SeoDeclaration, field_path, resolve_target};
use tola_address::OutputPath;

/// One rendering pass shares an exported selection across entries and feed formats.
/// The compiled Bundle separately owns its reusable structural DOM indexes.
pub(super) struct DocumentContents<'a> {
    config: &'a ResolvedSiteConfig,
    compilation: &'a BundleCompilation,
    bases: BTreeMap<OutputPath, url::Url>,
    selections: BTreeMap<(OutputPath, HtmlFragmentSelection), Arc<str>>,
    warnings: Vec<tola_typst::NativeDiagnostic>,
}

impl<'a> DocumentContents<'a> {
    pub(super) fn new(config: &'a ResolvedSiteConfig, compilation: &'a BundleCompilation) -> Self {
        Self {
            config,
            compilation,
            bases: BTreeMap::new(),
            selections: BTreeMap::new(),
            warnings: Vec::new(),
        }
    }

    pub(super) fn into_warnings(self) -> Vec<tola_typst::NativeDiagnostic> {
        self.warnings
    }

    pub(super) fn load(
        &mut self,
        declaration: &SeoDeclaration,
        fields: &Dict,
        field: &str,
        feed_output: &OutputPath,
        cancellation: &BuildCancellation,
    ) -> Result<Arc<str>, RenderError> {
        cancellation.ensure_active()?;
        declaration.check_fields(fields, &["document", "id"], field)?;
        let document_field = field_path(field, "document");
        let target = resolve_target(
            declaration,
            declaration.required(fields, "document", field)?,
            &document_field,
            feed_output,
            self.config,
            self.compilation,
            cancellation,
        )?;
        let selection = match declaration.optional_string(fields, "id", field)? {
            None => HtmlFragmentSelection::Body,
            Some(id) => {
                if id.is_empty() {
                    return Err(declaration
                        .invalid(
                            &field_path(field, "id"),
                            "HTML element id must not be empty",
                        )
                        .into());
                }
                HtmlFragmentSelection::Id(id)
            }
        };
        let key = (target.output.clone(), selection);
        if let Some(html) = self.selections.get(&key) {
            return Ok(Arc::clone(html));
        }
        let path =
            typst::syntax::VirtualPath::new(target.output.as_str()).expect("validated output path");
        let document = self
            .compilation
            .document(&path)
            .expect("resolved Bundle target exists");
        let base = if let Some(base) = self.bases.get(&target.output) {
            base.clone()
        } else {
            let mut canonical = target.url;
            canonical.set_fragment(None);
            canonical.set_query(None);
            let base = document
                .html_inventory(&cancellation.bundle_cancellation())?
                .and_then(|inventory| inventory.base_href().map(|base| base.value().to_owned()))
                .and_then(|base| canonical.join(&base).ok())
                .filter(|base| {
                    !base.cannot_be_a_base() && !matches!(base.scheme(), "data" | "javascript")
                })
                .unwrap_or(canonical);
            self.bases.insert(target.output.clone(), base.clone());
            base
        };
        let policy = cancellation.bundle_cancellation();
        let fragment = document.html_fragment_with_links(
            &key.1,
            &HtmlFragmentOptions { preserve_ancestors: true, ..Default::default() },
            &policy,
            &mut |destination, reference_use| {
                cancellation.ensure_active().map_err(|_| HtmlFragmentError::Cancelled)?;
                match base.join(destination) {
                    Ok(url) => Ok(url.to_string()),
                    Err(_) => {
                        let level = match reference_use {
                            HtmlReferenceUse::Navigation => self.config.build.references.navigation,
                            _ => self.config.build.references.resources,
                        };
                        match level {
                            crate::config::ReferenceLevel::Error => Err(HtmlFragmentError::InvalidReference {
                                message: format!("cannot resolve the feed link `{destination}` against `{base}`"),
                            }),
                            crate::config::ReferenceLevel::Warn => {
                                self.warnings.push(declaration.reference_warning(field,
                                    format!("cannot resolve the feed link `{destination}` against `{base}`")));
                                Ok(destination.to_owned())
                            }
                        }
                    }
                }
            },
        ).map_err(|error| {
            if error.is_cancelled() {
                RenderError::Cancelled(BuildCancelled)
            } else if error.raw_diagnostics().is_some() {
                RenderError::HtmlExport(error)
            } else {
                let reason = match &error {
                    HtmlFragmentError::MissingId { .. } => {
                        "no HTML element in the target document has this `content` id".to_owned()
                    }
                    HtmlFragmentError::RepeatedId { .. } => {
                        "multiple elements in the target document share this `content` id".to_owned()
                    }
                    HtmlFragmentError::MissingBody | HtmlFragmentError::RepeatedBody => {
                        "the target document does not have exactly one HTML body".to_owned()
                    }
                    HtmlFragmentError::NotHtmlDocument { .. } => {
                        "the feed content target is not an HTML document".to_owned()
                    }
                    HtmlFragmentError::InvalidReference { message } => message.clone(),
                    HtmlFragmentError::Cancelled | HtmlFragmentError::Export { .. } => {
                        "the feed content could not be selected".to_owned()
                    }
                };
                declaration.invalid(field, reason).into()
            }
        })?;
        cancellation.ensure_active()?;
        let mut html = fragment.styles.concat();
        html.push_str(&fragment.html);
        let html: Arc<str> = html.into();
        self.selections.insert(key, Arc::clone(&html));
        Ok(html)
    }
}
