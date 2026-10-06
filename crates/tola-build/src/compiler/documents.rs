//! Interpretation of native document outputs and their final HTML inventories.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;

use tola_address::OutputPath;

pub(crate) struct RealizedSite {
    pub(crate) documents: Vec<crate::site::HtmlPage>,
    pub(crate) html_inventories: BTreeMap<OutputPath, Arc<tola_typst::HtmlDocumentInventory>>,
}

pub(crate) fn from_compilation(
    compilation: &tola_typst::BundleCompilation,
    cancellation: &tola_typst::BundleCancellation,
) -> Result<RealizedSite> {
    let mut documents = Vec::new();
    let mut html_inventories = BTreeMap::new();
    for document in compilation.documents() {
        cancellation.ensure_active().map_err(anyhow::Error::new)?;
        let Some(inventory) = document.html_inventory(cancellation)? else {
            continue;
        };
        let output = output_path_for_virtual(document.path())?;
        let permalink = tola_address::route_for_output(&output);
        html_inventories.insert(output.clone(), inventory);
        documents.push(crate::site::HtmlPage {
            permalink,
            output,
            properties: document.info().clone(),
            sources: document.source_ids(),
        });
    }
    Ok(RealizedSite {
        documents,
        html_inventories,
    })
}

pub(crate) fn output_path_for_virtual(path: &typst::syntax::VirtualPath) -> Result<OutputPath> {
    OutputPath::parse(path.get_without_slash()).map_err(Into::into)
}
