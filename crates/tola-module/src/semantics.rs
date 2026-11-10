//! Declarative roles for exports of a module's public Typst entry.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::Deserialize;

use crate::declaration::{Query, QueryReturns};

/// A parameter selected by position or by its declared name.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged, deny_unknown_fields)]
pub enum SemanticParameter {
    Positional { positional: usize },
    Named { named: String },
}

/// The relationship between an argument and an entity.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SemanticKind {
    Reference,
    Definition,
}

/// A role names an export of the public library, not a private source file.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SemanticRole {
    pub export: String,
    pub parameter: SemanticParameter,
    pub role: SemanticKind,
    pub query: String,
}

/// A semantic role a module declares but the site cannot use.
#[derive(Debug, thiserror::Error)]
pub enum SemanticsError {
    #[error("semantics requires a public Typst entry")]
    MissingEntry,
    #[error("semantic export must name a public export path")]
    Export,
    #[error("semantic parameter name is invalid")]
    Parameter,
    #[error("semantic query {query:?} must name an entities query")]
    Query { query: String },
}

pub(super) fn validate(
    roles: &[SemanticRole],
    has_entry: bool,
    queries: &BTreeMap<String, Query>,
) -> Result<(), SemanticsError> {
    for role in roles {
        if !has_entry {
            return Err(SemanticsError::MissingEntry);
        }
        if !role.export.split('.').all(typst::syntax::is_ident) {
            return Err(SemanticsError::Export);
        }
        if let SemanticParameter::Named { named } = &role.parameter
            && !typst::syntax::is_ident(named)
        {
            return Err(SemanticsError::Parameter);
        }
        if !queries
            .get(&role.query)
            .is_some_and(|query| query.returns == QueryReturns::Entities)
        {
            return Err(SemanticsError::Query {
                query: role.query.clone(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {}
