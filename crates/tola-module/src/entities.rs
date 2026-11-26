//! Query entity records, retaining native source identities.

use std::collections::HashSet;

use anyhow::{Result, bail, ensure};
use serde_json::{Value as Json, json};
use typst::foundations::Value;
use typst::syntax::{FileId, RootedPath, VirtualRoot};

/// One entity in the model, with an optional native source and an independent URL.
#[derive(Debug, Clone)]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub source: Option<FileId>,
    pub url: Option<String>,
}

pub fn parse(value: &Value) -> Result<Vec<Entity>> {
    let Value::Array(items) = value else {
        bail!("the query must return an array of entities")
    };
    let mut entities = Vec::with_capacity(items.len());
    let mut ids = HashSet::new();
    for (idx, item) in items.iter().enumerate() {
        let Value::Dict(fields) = item else {
            bail!("entity {idx} is not a dictionary")
        };
        for (key, _) in fields.iter() {
            ensure!(
                ["id", "name", "description", "source", "url"].contains(&key.as_str()),
                "entity {idx} has the unknown field `{key}`"
            );
        }
        let required = |key: &str| -> Result<String> {
            match fields.get(key) {
                Ok(Value::Str(val)) => Ok(val.to_string()),
                _ => bail!("entity {idx} needs a string `{key}`"),
            }
        };
        let optional = |key: &str| -> Result<Option<String>> {
            match fields.get(key) {
                Err(_) | Ok(Value::None) => Ok(None),
                Ok(Value::Str(val)) => Ok(Some(val.to_string())),
                _ => bail!("`{key}` in entity {idx} must be a string or `none`"),
            }
        };
        let id = required("id")?;
        ensure!(!id.is_empty(), "entity {idx} has an empty `id`");
        ensure!(
            ids.insert(id.clone()),
            "entity {idx} repeats the `id` `{id}`"
        );
        let source = match fields.get("source") {
            Err(_) | Ok(Value::None) => None,
            Ok(val) => Some(
                val.clone()
                    .cast::<RootedPath>()
                    .map_err(|_| {
                        anyhow::anyhow!("`source` in entity {idx} must be a native path or `none`")
                    })?
                    .intern(),
            ),
        };
        entities.push(Entity {
            id,
            name: required("name")?,
            description: optional("description")?,
            source,
            url: optional("url")?,
        });
    }
    Ok(entities)
}

/// The CLI representation preserves virtual root identity instead of formatting a Typst value.
pub fn json(entities: &[Entity]) -> Json {
    Json::Array(
        entities
            .iter()
            .map(|e| {
                json!({
                    "id": e.id,
                    "name": e.name,
                    "description": e.description,
                    "source": e.source.map(path_json),
                    "url": e.url,
                })
            })
            .collect(),
    )
}

/// Preserve a native path's virtual root when writing a query result as JSON.
pub(crate) fn path_json(id: FileId) -> Json {
    let root = match id.root() {
        VirtualRoot::Project => "project".to_owned(),
        VirtualRoot::Package(package) => package.to_string(),
    };
    let path = id.vpath().get_without_slash();
    json!({ "root": root, "path": path })
}
