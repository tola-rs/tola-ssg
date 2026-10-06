//! Source-field selection and JSON presentation for the inspect command.

use anyhow::Result;
use serde_json::{Map, Value};

pub(super) struct SourceFormat {
    pub(super) raw: bool,
    pub(super) pretty: bool,
    pub(super) filter_empty: bool,
    pub(super) fields: Option<Vec<String>>,
}

pub(super) fn sources(
    sources: &tola_build::inspect::InspectedSources,
    options: &SourceFormat,
) -> Result<String> {
    let rows = source_rows(sources, options)?;
    encode(&rows, options)
}

/// The rows the `sources` projection shows, in the order the command prints them.
pub(super) fn source_rows(
    sources: &tola_build::inspect::InspectedSources,
    options: &SourceFormat,
) -> Result<Vec<Value>> {
    let fields = options
        .fields
        .as_ref()
        .map(|fields| normalize_fields(fields));
    sources
        .sources()
        .iter()
        .filter_map(|source| source.metadata().map(|metadata| (source.path(), metadata)))
        .map(|(path, metadata)| {
            let metadata = metadata_json(&metadata.to_typst_dict(), fields.as_deref())?;
            Ok(selected_fields(
                &path.to_string_lossy(),
                metadata,
                fields.as_deref(),
                options.filter_empty,
            ))
        })
        .collect()
}

/// The rows one projection prints as JSON, in the encoding the reader asked for.
pub(super) fn encode(rows: &[Value], options: &SourceFormat) -> Result<String> {
    let rows = Value::Array(rows.to_vec());
    let rows = if options.raw { rows } else { plain_text(&rows) };
    Ok(if options.pretty {
        serde_json::to_string_pretty(&rows)?
    } else {
        serde_json::to_string(&rows)?
    })
}

/// The rows of an array `value` that `keep` holds, in projection order.
pub(super) fn filtered(value: &Value, keep: impl Fn(&Value) -> bool) -> Value {
    let Value::Array(rows) = value else {
        return value.clone();
    };
    Value::Array(rows.iter().filter(|row| keep(row)).cloned().collect())
}

/// Reduce Typst content objects in serialized metadata to the text they render.
fn plain_text(json: &Value) -> Value {
    match json {
        Value::Object(object) if object.contains_key("func") => {
            let mut text = String::new();
            append_content_text(json, &mut text);
            Value::String(text)
        }
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), plain_text(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(plain_text).collect()),
        other => other.clone(),
    }
}

fn append_content_text(json: &Value, output: &mut String) {
    match json {
        Value::Object(object) => {
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                output.push_str(text);
            } else if let Some(body) = object.get("body") {
                append_content_text(body, output);
            } else if let Some(children) = object.get("children").filter(|value| value.is_array()) {
                append_content_text(children, output);
            } else if let Some(child) = object.get("child") {
                append_content_text(child, output);
            }
        }
        Value::Array(values) => {
            for value in values {
                append_content_text(value, output);
            }
        }
        Value::String(value) => output.push_str(value),
        _ => {}
    }
}

fn metadata_json(
    metadata: &typst::foundations::Dict,
    fields: Option<&[String]>,
) -> serde_json::Result<Value> {
    let selected = match fields {
        Some(fields) => fields
            .iter()
            .filter(|field| field.as_str() != "path")
            .filter_map(|field| {
                metadata
                    .get(field.as_str())
                    .ok()
                    .map(|value| (field.as_str().into(), value.clone()))
            })
            .collect(),
        None => metadata
            .iter()
            .filter(|(field, _)| field.as_str() != "path")
            .map(|(field, value)| (field.clone(), value.clone()))
            .collect(),
    };
    serde_json::to_value(typst::foundations::Value::Dict(selected))
}

fn selected_fields(
    path: &str,
    metadata: Value,
    fields: Option<&[String]>,
    filter_empty: bool,
) -> Value {
    let mut row = Map::new();
    row.insert("path".into(), Value::String(path.into()));
    if let Value::Object(metadata) = metadata {
        if let Some(fields) = fields {
            for field in fields {
                if field == "path" {
                    continue;
                }
                match metadata.get(field) {
                    Some(value) if !filter_empty || !is_empty(value) => {
                        row.insert(field.clone(), value.clone());
                    }
                    None if !filter_empty => {
                        row.insert(field.clone(), Value::Null);
                    }
                    _ => {}
                }
            }
        } else {
            row.extend(
                metadata.into_iter().filter(|(field, value)| {
                    field != "path" && (!filter_empty || !is_empty(value))
                }),
            );
        }
    }
    Value::Object(row)
}

fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.is_empty(),
        Value::Array(values) => values.is_empty(),
        _ => false,
    }
}

fn normalize_fields(fields: &[String]) -> Vec<String> {
    fields
        .iter()
        .flat_map(|field| field.split(','))
        .flat_map(|field| field.split_whitespace())
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The inspected sources of a site whose `content` holds `documents`.
    fn inspected_sources(documents: &[(&str, &str)]) -> tola_build::inspect::InspectedSources {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("content")).unwrap();
        for (name, body) in documents {
            std::fs::write(root.join("content").join(name), body).unwrap();
        }
        std::fs::write(root.join("site.typ"), "").unwrap();
        let config = tola_build::config::SiteConfigSchema::default()
            .resolve(
                &root.join("tola.toml"),
                tola_typst::PackageLocations::default(),
                &tola_build::config::loading::BuildOverrides::default(),
            )
            .unwrap();
        tola_build::inspect::inspect_sources(
            &[],
            &config,
            &tola_build::BuildResources::default(),
            &tola_build::cancellation::BuildCancellation::new(),
        )
        .unwrap()
    }

    #[test]
    fn path_only_selection_needs_metadata() {
        let inspected = inspected_sources(&[
            (
                "document.typ",
                "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((:))",
            ),
            ("plain.typ", ""),
        ]);
        let encoded = sources(
            &inspected,
            &SourceFormat {
                raw: true,
                pretty: false,
                filter_empty: false,
                fields: Some(vec!["path".into()]),
            },
        )
        .unwrap();

        assert_eq!(
            serde_json::from_str::<Value>(&encoded).unwrap(),
            json!([{"path": "content/document.typ"}])
        );
    }

    #[test]
    fn retired_label_declaration_reports_no_source_metadata() {
        let inspected = inspected_sources(&[(
            "document.typ",
            "#metadata((title: \"Retired\")) <tola-meta>",
        )]);

        assert_eq!(inspected.matched(), 0);
        assert_eq!(
            sources(
                &inspected,
                &SourceFormat {
                    raw: true,
                    pretty: false,
                    filter_empty: false,
                    fields: None,
                },
            )
            .unwrap(),
            "[]"
        );
    }

    #[test]
    fn unrequested_functions_stay_unserialized() {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("source.typ");
        std::fs::write(
            &main,
            "#metadata((title: \"Document\", related: () => ())) <tola-meta>",
        )
        .unwrap();
        let world = tola_typst::TypstWorld::builder(&main, directory.path())
            .with_local_cache()
            .no_fonts()
            .build(&tola_typst::BundleCancellation::default())
            .unwrap();
        let scan = tola_typst::scan_world_with_evidence(&world).unwrap();
        let metadata = scan
            .metadata_unique("tola-meta")
            .unwrap()
            .unwrap()
            .cast::<typst::foundations::Dict>()
            .unwrap();
        let fields = ["title".to_owned()];
        let row = metadata_json(&metadata, Some(&fields)).unwrap();
        assert_eq!(row, json!({"title": "Document"}));
        assert!(matches!(
            metadata.get("related").unwrap(),
            typst::foundations::Value::Func(_)
        ));
    }

    #[test]
    fn field_selection_keeps_the_source_path() {
        let fields = normalize_fields(&["path,title, draft missing".into()]);
        let row = selected_fields(
            "content/post.typ",
            json!({"path":"user value", "title":"", "draft":false}),
            Some(&fields),
            true,
        );
        assert_eq!(row, json!({"path":"content/post.typ", "draft":false}));

        let url = vec!["url".to_owned()];
        let row = selected_fields("content/post.typ", json!({"draft":true}), Some(&url), false);
        assert_eq!(row["path"], "content/post.typ");
        assert_eq!(row["url"], Value::Null);
    }

    #[test]
    fn filtered_keeps_rows_matching_predicate() {
        let rows = json!([{"title": "one"}, {"title": "two"}]);

        assert_eq!(
            filtered(&rows, |row| row["title"] == "two"),
            json!([{"title": "two"}])
        );
        assert_eq!(
            filtered(&json!("not an array"), |_| false),
            json!("not an array")
        );
    }

    #[test]
    fn exported_rows_encode_as_the_writer_prints_them() {
        let inspected = inspected_sources(&[(
            "document.typ",
            "#import \"@tola/source:0.0.0\": tola-meta\n#tola-meta((title: \"Document\"))",
        )]);
        for raw in [false, true] {
            for pretty in [false, true] {
                let options = SourceFormat {
                    raw,
                    pretty,
                    filter_empty: false,
                    fields: None,
                };
                let rows = source_rows(&inspected, &options).unwrap();

                assert_eq!(
                    encode(&rows, &options).unwrap(),
                    sources(&inspected, &options).unwrap()
                );
            }
        }
    }
}
