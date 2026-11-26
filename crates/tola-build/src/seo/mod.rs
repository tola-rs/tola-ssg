//! Feed and sitemap outputs declared by the final Typst Bundle.

use std::sync::Arc;
use tola_address::SiteUrlMount;

use crate::cancellation::BuildCancellation;
use crate::config::{ReferenceLevel, SiteConfig};

mod date;
mod declaration;
pub(crate) mod feed;
mod sitemap;

#[cfg(test)]
mod tests;

pub(crate) use declaration::SeoDeclarationError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum RenderError {
    #[error(transparent)]
    Declaration(#[from] SeoDeclarationError),
    #[error(transparent)]
    Cancelled(#[from] crate::cancellation::BuildCancelled),
    #[error(transparent)]
    HtmlExport(tola_typst::HtmlFragmentError),
    #[error(transparent)]
    Compiler(tola_typst::CompileError),
}

impl From<tola_typst::CompileError> for RenderError {
    fn from(error: tola_typst::CompileError) -> Self {
        match error {
            tola_typst::CompileError::Cancelled => {
                Self::Cancelled(crate::cancellation::BuildCancelled)
            }
            error => Self::Compiler(error),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RenderedOutputs {
    pub(crate) outputs: Vec<RenderedOutput>,
    pub(crate) warnings: Vec<tola_typst::NativeDiagnostic>,
}

#[derive(Debug, Clone)]
pub(crate) struct RenderedOutput {
    pub(crate) producer: &'static str,
    pub(crate) url: tola_address::UrlPath,
    pub(crate) declaration: crate::output::semantics::OutputDeclaration,
    pub(crate) bytes: Arc<[u8]>,
}

/// Complete immutable SEO outputs and diagnostics from a successful rendering pass.
#[derive(Debug)]
pub(crate) struct SeoCompilation {
    compilation: Arc<tola_typst::BundleCompilation>,
    configuration: SeoConfiguration,
    rendered: RenderedOutputs,
}

impl SeoCompilation {
    pub(crate) fn prepare(
        config: &SiteConfig,
        compilation: &Arc<tola_typst::BundleCompilation>,
        cancellation: &BuildCancellation,
        previous: Option<&Arc<Self>>,
    ) -> Result<Arc<Self>, RenderError> {
        cancellation.ensure_active()?;
        let prepared = if let Some(previous) = previous
            && Arc::ptr_eq(&previous.compilation, compilation)
            && previous.configuration.matches(config)
        {
            Arc::clone(previous)
        } else {
            let rendered = render_outputs(config, compilation, cancellation)?;
            Arc::new(Self {
                compilation: Arc::clone(compilation),
                configuration: SeoConfiguration::capture(config),
                rendered,
            })
        };
        cancellation.ensure_active()?;
        Ok(prepared)
    }

    pub(crate) fn outputs(&self) -> &[RenderedOutput] {
        &self.rendered.outputs
    }

    pub(crate) fn warnings(&self) -> &[tola_typst::NativeDiagnostic] {
        &self.rendered.warnings
    }
}

/// Keep in sync with configuration reads in the SEO renderers.
#[derive(Debug, PartialEq, Eq)]
struct SeoConfiguration {
    origin: Option<String>,
    base_path: SiteUrlMount,
    navigation: ReferenceLevel,
    resources: ReferenceLevel,
}

impl SeoConfiguration {
    fn capture(config: &SiteConfig) -> Self {
        Self {
            origin: config.site.origin.clone(),
            base_path: config.site.base_path.clone(),
            navigation: config.build.references.navigation,
            resources: config.build.references.resources,
        }
    }

    fn matches(&self, config: &SiteConfig) -> bool {
        *self == Self::capture(config)
    }
}

pub(crate) fn render_outputs(
    config: &SiteConfig,
    compilation: &tola_typst::BundleCompilation,
    cancellation: &BuildCancellation,
) -> Result<RenderedOutputs, RenderError> {
    cancellation.ensure_active()?;
    let rendered = (|| {
        let mut rendered = feed::render_outputs(config, compilation, cancellation)?;
        let sitemaps = sitemap::render_outputs(config, compilation, cancellation)?;
        rendered.outputs.extend(sitemaps.outputs);
        rendered.warnings.extend(sitemaps.warnings);
        Ok(rendered)
    })();
    cancellation.ensure_active()?;
    rendered
}
