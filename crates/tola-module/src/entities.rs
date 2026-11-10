//! Version 1 entity records, retaining native source identities.

use std::collections::HashSet;

use anyhow::{Result, bail, ensure};
use serde_json::{Value as Json, json};
use typst::foundations::Value;
use typst::syntax::{FileId, RootedPath, VirtualRoot};

/// One entity in the model, with an optional native source and an independent URL.
#[derive(Debug, Clone)]
pub struct Entity {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
    pub source: Option<FileId>,
    pub url: Option<String>,
}

pub fn parse(value: &Value) -> Result<Vec<Entity>> {
    let Value::Array(items) = value else {
        bail!("the query must return an array of entities")
    };
    let mut entities = Vec::with_capacity(items.len());
    let mut ids = HashSet::new();
    for (index, item) in items.iter().enumerate() {
        let Value::Dict(fields) = item else {
            bail!("entity {index} is not a dictionary")
        };
        for (key, _) in fields.iter() {
            ensure!(
                ["id", "label", "detail", "source", "url"].contains(&key.as_str()),
                "entity {index} has the unknown field `{key}`"
            );
        }
        let required = |key: &str| -> Result<String> {
            match fields.get(key) {
                Ok(Value::Str(value)) => Ok(value.to_string()),
                _ => bail!("entity {index} needs a string `{key}`"),
            }
        };
        let optional = |key: &str| -> Result<Option<String>> {
            match fields.get(key) {
                Err(_) | Ok(Value::None) => Ok(None),
                Ok(Value::Str(value)) => Ok(Some(value.to_string())),
                _ => bail!("`{key}` in entity {index} must be a string or `none`"),
            }
        };
        let id = required("id")?;
        ensure!(!id.is_empty(), "entity {index} has an empty `id`");
        ensure!(
            ids.insert(id.clone()),
            "entity {index} repeats the `id` `{id}`"
        );
        let source = match fields.get("source") {
            Err(_) | Ok(Value::None) => None,
            Ok(value) => Some(
                value
                    .clone()
                    .cast::<RootedPath>()
                    .map_err(|_| {
                        anyhow::anyhow!(
                            "`source` in entity {index} must be a native path or `none`"
                        )
                    })?
                    .intern(),
            ),
        };
        entities.push(Entity {
            id,
            label: required("label")?,
            detail: optional("detail")?,
            source,
            url: optional("url")?,
        });
    }
    Ok(entities)
}

/// The CLI representation preserves virtual root identity instead of formatting a Typst value.
pub fn json(entities: &[Entity]) -> Result<Json> {
    let mut records = Vec::with_capacity(entities.len());
    for entity in entities {
        let source = entity
            .source
            .map(|id| {
                let root = match id.root() {
                    VirtualRoot::Project => "project".to_owned(),
                    VirtualRoot::Package(package) => package.to_string(),
                };
                let path = id.vpath().get_without_slash();
                Ok::<_, anyhow::Error>(json!({ "root": root, "path": path }))
            })
            .transpose()?;
        records.push(json!({ "id": entity.id, "label": entity.label, "detail": entity.detail, "source": source, "url": entity.url }));
    }
    Ok(Json::Array(records))
}
