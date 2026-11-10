//! Build-side query entities wire value.

use std::sync::Arc;

use tola_module::entities::Entity;

use crate::check::SourceRevision;

/// A query's entities and the exact revision whose world resolved their source identities.
pub struct EntityQueryResult {
    pub result: super::query::QueryResult,
    pub entities: Vec<Entity>,
    /// The model's world and complete read attempts, including failed model inputs.
    pub revision: Option<Arc<SourceRevision>>,
    /// Sources opened to validate entity paths, beyond the model compilation's own reads.
    pub source_reads: Vec<typst::syntax::Source>,
    /// Read attempts, physical identities and package selections for the entity sources.
    pub source_accessed: tola_typst::AccessedDeps,
}
