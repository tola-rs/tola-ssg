use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::page::PageMeta;

/// Metadata that can be either normalized or raw
#[derive(Debug)]
pub enum QueryMeta {
    Normalized(Box<PageMeta>),
    Raw(JsonValue),
}

impl QueryMeta {
    /// Check if this is a draft.
    pub(super) fn is_draft(&self) -> bool {
        match self {
            QueryMeta::Normalized(meta) => meta.draft,
            QueryMeta::Raw(json) => json.get("draft").and_then(|v| v.as_bool()).unwrap_or(false),
        }
    }
}

impl Serialize for QueryMeta {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            QueryMeta::Normalized(meta) => meta.serialize(serializer),
            QueryMeta::Raw(json) => json.serialize(serializer),
        }
    }
}

/// Result for a single queried page
#[derive(Debug, Serialize)]
pub struct PageQueryResult {
    pub path: String,
    pub url: String,
    #[serde(flatten)]
    pub meta: QueryMeta,
}

/// Result for batch query
#[derive(Debug, Serialize)]
#[serde(transparent)]
pub struct QueryResult {
    pub pages: Vec<PageQueryResult>,
}
