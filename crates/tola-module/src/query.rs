//! Read one compiled model query: its value or entities, with read evidence.

use anyhow::{Context as _, Result, bail, ensure};
use serde_json::{Map, Number, Value as Json};
use typst::World;
use typst::foundations::Value;

use crate::declaration::QueryReturns;

/// The model value and entity facts one compiled query produced.
pub struct QueryOutcome {
    /// The projected model value, when the request asked to materialize it.
    pub data: Option<Json>,
    pub entities: Vec<super::entities::Entity>,
    pub source_reads: Vec<typst::syntax::Source>,
    pub accessed: tola_typst::AccessedDeps,
    /// A site-author problem; the query produced no data.
    pub failure: Option<String>,
}

/// Everything one compiled query needs; no configuration, session, or build state.
pub struct QueryRequest<'a> {
    pub name: &'a str,
    pub label: &'a str,
    pub returns: QueryReturns,
    pub materialize: bool,
    pub world: &'a tola_typst::TypstWorld,
    pub bundle: &'a tola_typst::BundleCompilation,
    pub cancellation: &'a tola_typst::BundleCancellation,
}

/// Read one compiled model's value or entities, with the evidence its reads produced.
pub fn run_query(request: QueryRequest<'_>) -> Result<QueryOutcome> {
    let declarations = request.bundle.metadata_declarations(request.label);
    if declarations.len() != 1 {
        return Ok(QueryOutcome {
            data: None,
            entities: Vec::new(),
            source_reads: Vec::new(),
            accessed: tola_typst::AccessedDeps::default(),
            failure: Some(format!(
                "query {} requires one <{}> metadata declaration, found {}",
                request.name,
                request.label,
                declarations.len(),
            )),
        });
    }
    let mut outcome = QueryOutcome {
        data: None,
        entities: Vec::new(),
        source_reads: Vec::new(),
        accessed: tola_typst::AccessedDeps::default(),
        failure: None,
    };
    let query_value = match request.returns {
        QueryReturns::Json => json_value(declarations[0].value(), "data"),
        QueryReturns::Entities => {
            super::entities::parse(declarations[0].value()).and_then(|parsed| {
                let (source_reads, accessed) =
                    read_entity_sources(&parsed, request.world, request.cancellation)?;
                outcome.source_reads = source_reads;
                outcome.accessed = accessed;
                let entities_json = if request.materialize {
                    super::entities::json(&parsed)?
                } else {
                    Json::Null
                };
                outcome.entities = parsed;
                Ok(entities_json)
            })
        }
    };
    match query_value {
        Ok(query_json) => {
            outcome.data = request.materialize.then_some(query_json);
        }
        Err(error) => outcome.failure = Some(format!("query {}: {error}", request.name)),
    }
    Ok(outcome)
}

/// Read each entity's declared source in this query's world and keep that evidence.
///
/// An entity names a source the model compiled, so the bytes read here must be the ones that
/// compile saw; a mismatch means the source changed while the query ran.
fn read_entity_sources(
    entities: &[super::entities::Entity],
    world: &tola_typst::TypstWorld,
    cancellation: &tola_typst::BundleCancellation,
) -> Result<(Vec<typst::syntax::Source>, tola_typst::AccessedDeps)> {
    let mut reads = Vec::new();
    let mut accessed = tola_typst::AccessedDeps::default();
    for entity in entities {
        cancellation.ensure_active()?;
        let Some(source) = entity.source else {
            continue;
        };
        let (bytes, mut read_accessed) = world.read_file_with_evidence(source);
        accessed.reads.append(&mut read_accessed.reads);
        accessed.disk_reads.append(&mut read_accessed.disk_reads);
        accessed
            .package_checks
            .append(&mut read_accessed.package_checks);
        let id = &entity.id;
        let unreadable = || format!("Tola could not read the source named by entity `{id}`");
        let bytes = bytes.map_err(|error| anyhow::Error::new(error).context(unreadable()))?;
        let read = world
            .source(source)
            .map_err(|error| anyhow::Error::new(error).context(unreadable()))?;
        ensure!(
            bytes.as_slice() == read.text().as_bytes(),
            "the source named by entity `{id}` changed while the query ran"
        );
        reads.push(read);
    }
    Ok((reads, accessed))
}

pub fn json_value(value: &Value, at: &str) -> Result<Json> {
    Ok(match value {
        Value::None => Json::Null,
        Value::Bool(value) => Json::Bool(*value),
        Value::Int(value) => Json::Number((*value).into()),
        Value::Float(value) => Json::Number(
            Number::from_f64(*value)
                .with_context(|| format!("{at} must contain a finite number"))?,
        ),
        Value::Str(value) => Json::String(value.to_string()),
        Value::Array(values) => Json::Array(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| json_value(value, &format!("{at}[{index}]")))
                .collect::<Result<_>>()?,
        ),
        Value::Dict(values) => {
            let mut object = Map::new();
            for (key, value) in values {
                object.insert(key.to_string(), json_value(value, &format!("{at}.{key}"))?);
            }
            Json::Object(object)
        }
        other => bail!(
            "{at} contains {}; project it to a JSON value explicitly",
            other.ty()
        ),
    })
}
