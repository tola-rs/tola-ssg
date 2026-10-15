//! Interpretation of native document outputs and their final HTML inventories.

use std::sync::Arc;

use anyhow::Result;

use tola_address::OutputPath;

#[derive(Clone)]
pub(crate) struct RealizedPage {
    pub(crate) page: crate::site::HtmlPage,
    pub(crate) inventory: Arc<tola_typst::HtmlDocumentInventory>,
}

pub(crate) fn from_compilation(
    compilation: &tola_typst::BundleCompilation,
    cancellation: &tola_typst::BundleCancellation,
) -> Result<Vec<RealizedPage>> {
    let mut documents = Vec::new();
    for document in compilation.documents() {
        cancellation.ensure_active()?;
        let Some(inventory) = document.html_inventory(cancellation)? else {
            continue;
        };
        let output = output_path_for_virtual(document.path())?;
        let permalink = tola_address::route_for_output(&output);
        let mut sources = document.source_ids();
        for source in document.rendered_source_ids(cancellation)? {
            if !sources.contains(&source) {
                sources.push(source);
            }
        }
        documents.push(RealizedPage {
            page: crate::site::HtmlPage {
                permalink,
                output,
                properties: document.info().clone(),
                sources,
            },
            inventory,
        });
    }
    Ok(documents)
}

pub(crate) fn output_path_for_virtual(path: &typst::syntax::VirtualPath) -> Result<OutputPath> {
    OutputPath::parse(path.get_without_slash()).map_err(Into::into)
}
