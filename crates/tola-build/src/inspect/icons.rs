//! Icon namespaces a site configures and the collections they prepared.

use std::path::Path;

use serde_json::{Value, json};

use crate::cancellation::BuildCancellation;
use crate::config::ResolvedSiteConfig;
use crate::config::section::IconCollectionSource;
use crate::resources::BuildResources;
use tola_diagnostic::display_path;

/// Project configured icon namespaces, or each name in the selected namespace.
///
/// Collections are prepared through the same cache a build uses, so remote bytes are fetched only
/// when they are not cached yet. Selected names include aliases and are in lexical order.
pub fn icons(
    config: &ResolvedSiteConfig,
    resources: &BuildResources,
    cancellation: &BuildCancellation,
    namespace: Option<&str>,
) -> anyhow::Result<Value> {
    let inputs = crate::package::prepare_package_inputs(
        config,
        resources,
        cancellation,
        &mut None,
        &Default::default(),
    )?;
    let collections = inputs.icons.collections();
    let root = config.get_root();
    let mut rows = Vec::new();
    for (name, source) in &config.icons.collections {
        if namespace.is_some_and(|selected| selected != name) {
            continue;
        }
        let collection = collections
            .collection(name)
            .expect("every configured icon namespace prepared a collection");
        if namespace.is_some() {
            rows.extend(collection.names().map(|icon| {
                json!({
                    "namespace": name,
                    "name": icon,
                })
            }));
        } else {
            rows.push(json!({
                "namespace": name,
                "source": source_json(source, root),
                "prefix": collection.source_prefix(),
                "icons": collection.len(),
            }));
        }
    }
    Ok(Value::Array(rows))
}

/// One namespace's declared source.
fn source_json(source: &IconCollectionSource, root: &Path) -> Value {
    match source {
        IconCollectionSource::LocalSvgDir { path } => {
            json!({ "type": "local-svg-dir", "path": display_path(path, root) })
        }
        IconCollectionSource::LocalJson { path } => {
            json!({ "type": "local-json", "path": display_path(path, root) })
        }
        IconCollectionSource::RemoteJson {
            preset,
            version,
            url,
            mirrors,
        } => {
            let mut row = json!({
                "type": "remote-json",
                "preset": preset.as_deref(),
                "version": version.as_deref(),
                "url": url.as_deref(),
            });
            if let Some(mirrors) = mirrors {
                row["mirrors"] = json!(mirrors);
            }
            row
        }
    }
}

#[cfg(test)]
mod tests {}
