//! Icon namespaces a site configures and the collections they prepared.

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::cancellation::BuildCancellation;
use crate::config::ResolvedSiteConfig;
use crate::config::section::IconCollectionSource;
use crate::filesystem::display_path;
use crate::resources::BuildResources;

/// Project every configured icon namespace, and one namespace's icon names when `namespace` selects it.
///
/// Collections are prepared through the same cache a build uses, so remote bytes are fetched only
/// when they are not cached yet. A selected namespace adds `names`, in lexical order; each name is
/// an icon or one of its aliases.
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
        let mut row = Map::new();
        row.insert("namespace".to_owned(), json!(name));
        row.insert("source".to_owned(), source_json(source, root));
        row.insert("prefix".to_owned(), json!(collection.source_prefix()));
        row.insert("icons".to_owned(), json!(collection.len()));
        if namespace.is_some() {
            row.insert(
                "names".to_owned(),
                json!(collection.names().collect::<Vec<_>>()),
            );
        }
        rows.push(Value::Object(row));
    }
    Ok(Value::Array(rows))
}

/// One namespace's declared source, with a preset's release and pinned digest resolved.
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
            sha256,
        } => match preset {
            Some(preset) => {
                let resolved = crate::icon::iconify::resolve(preset, version.as_deref())
                    .expect("a loaded configuration indexes every preset");
                json!({
                    "type": "remote-json",
                    "preset": preset,
                    "version": resolved.version,
                    "sha256": resolved.sha256.to_string(),
                })
            }
            None => json!({
                "type": "remote-json",
                "url": url.as_deref(),
                "sha256": sha256.map(|digest| digest.to_string()),
            }),
        },
    }
}

/// Every Iconify collection this Tola release indexes, and the releases it pins.
///
/// The index is part of the executable, so this projection reads no configuration and needs no
/// site. Rows are in ascending preset order, and each one's `releases` start with its `newest`.
pub fn presets() -> Value {
    Value::Array(
        crate::icon::iconify::collections()
            .iter()
            .map(|collection| {
                json!({
                    "preset": collection.name,
                    "newest": collection.newest().version,
                    "releases": collection.versions().collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_project_their_icons() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("brand")).unwrap();
        std::fs::write(
            root.join("brand/mark.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="currentColor" d="M0 0h24v24H0z"/></svg>"#,
        )
        .unwrap();
        std::fs::write(
            root.join("ui.json"),
            br#"{"prefix":"upstream","icons":{"home":{"body":"<path fill='currentColor' d='M0 0h16v16H0z'/>"}}}"#,
        )
        .unwrap();
        let config = crate::config::tests::load_test_config(
            root,
            r#"
            [icons.collections.brand]
            source-type = "local-svg-dir"
            path = "brand"
            [icons.collections.ui]
            source-type = "local-json"
            path = "ui.json"
            "#,
        );
        let cancellation = BuildCancellation::new();
        let resources = BuildResources::new();

        assert_eq!(
            icons(&config, &resources, &cancellation, None).unwrap(),
            json!([
                {
                    "namespace": "brand",
                    "source": { "type": "local-svg-dir", "path": "brand" },
                    "prefix": null,
                    "icons": 1,
                },
                {
                    "namespace": "ui",
                    "source": { "type": "local-json", "path": "ui.json" },
                    "prefix": "upstream",
                    "icons": 1,
                },
            ])
        );

        assert_eq!(
            icons(&config, &resources, &cancellation, Some("ui")).unwrap(),
            json!([
                {
                    "namespace": "ui",
                    "source": { "type": "local-json", "path": "ui.json" },
                    "prefix": "upstream",
                    "icons": 1,
                    "names": ["home"],
                },
            ])
        );

        assert_eq!(
            icons(&config, &resources, &cancellation, Some("missing")).unwrap(),
            json!([])
        );
    }

    #[test]
    fn presets_project_the_index_in_name_order() {
        let Value::Array(rows) = presets() else {
            panic!("the preset projection is a row array")
        };
        assert!(!rows.is_empty(), "the index pins at least one collection");

        let mut names = Vec::new();
        for row in &rows {
            let name = row["preset"].as_str().expect("every row names a preset");
            let newest = row["newest"].as_str().expect("every row names a release");
            let releases = row["releases"]
                .as_array()
                .expect("every row lists releases");
            assert_eq!(releases.first().and_then(Value::as_str), Some(newest));
            assert!(releases.iter().all(|release| release.is_string()));
            names.push(name);
        }
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "presets are in ascending name order");
    }
}
