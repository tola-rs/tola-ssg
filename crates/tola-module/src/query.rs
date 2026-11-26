//! Read one compiled model query: its value or entities, with read evidence.

use anyhow::{Context as _, Result, bail, ensure};
use serde_json::{Map, Number, Value as Json};
use typst::World;
use typst::foundations::Value;

/// The model value and entity facts one compiled query produced.
pub struct QueryOutput {
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
    pub entities: bool,
    pub materialize: bool,
    pub world: &'a tola_typst::TypstWorld,
    pub bundle: &'a tola_typst::BundleCompilation,
    pub cancellation: &'a tola_typst::BundleCancellation,
}

/// Read one compiled model's value or entities, with the evidence its reads produced.
pub fn run_query(req: QueryRequest<'_>) -> Result<QueryOutput> {
    let decls = req.bundle.metadata_declarations(req.label);
    if decls.len() != 1 {
        return Ok(QueryOutput {
            data: None,
            entities: Vec::new(),
            source_reads: Vec::new(),
            accessed: tola_typst::AccessedDeps::default(),
            failure: Some(format!(
                "query {} requires one <{}> metadata declaration, found {}",
                req.name,
                req.label,
                decls.len(),
            )),
        });
    }
    let mut out = QueryOutput {
        data: None,
        entities: Vec::new(),
        source_reads: Vec::new(),
        accessed: tola_typst::AccessedDeps::default(),
        failure: None,
    };
    let val = if req.entities {
        super::entities::parse(decls[0].value()).and_then(|parsed| {
            out.source_reads =
                read_entity_sources(&parsed, req.world, req.cancellation, &mut out.accessed)?;
            let json = if req.materialize {
                super::entities::json(&parsed)
            } else {
                Json::Null
            };
            out.entities = parsed;
            Ok(json)
        })
    } else {
        json_value(decls[0].value(), "data")
    };
    match val {
        Ok(json) => out.data = req.materialize.then_some(json),
        Err(err) => out.failure = Some(format!("query {}: {err}", req.name)),
    }
    Ok(out)
}

/// Read each entity's declared source and verify its bytes match what compile saw.
fn read_entity_sources(
    entities: &[super::entities::Entity],
    world: &tola_typst::TypstWorld,
    cancel: &tola_typst::BundleCancellation,
    accessed: &mut tola_typst::AccessedDeps,
) -> Result<Vec<typst::syntax::Source>> {
    let mut reads = Vec::new();
    for entity in entities {
        cancel.ensure_active()?;
        let Some(src) = entity.source else {
            continue;
        };
        let (bytes, mut read_deps) = world.read_file_with_evidence(src);
        accessed.reads.append(&mut read_deps.reads);
        accessed.disk_reads.append(&mut read_deps.disk_reads);
        accessed
            .package_checks
            .append(&mut read_deps.package_checks);
        let id = &entity.id;
        let unreadable = || format!("Tola could not read the source named by entity `{id}`");
        let bytes = bytes.map_err(|err| anyhow::Error::new(err).context(unreadable()))?;
        let read = world
            .source(src)
            .map_err(|err| anyhow::Error::new(err).context(unreadable()))?;
        ensure!(
            bytes.as_slice() == read.text().as_bytes(),
            "the source named by entity `{id}` changed while the query ran"
        );
        reads.push(read);
    }
    Ok(reads)
}

pub fn json_value(value: &Value, at: &str) -> Result<Json> {
    if let Ok(path) = value.clone().cast::<typst::syntax::RootedPath>() {
        return Ok(super::entities::path_json(path.intern()));
    }
    Ok(match value {
        Value::None => Json::Null,
        Value::Bool(val) => Json::Bool(*val),
        Value::Int(val) => Json::Number((*val).into()),
        Value::Float(val) => Json::Number(
            Number::from_f64(*val).with_context(|| format!("{at} must contain a finite number"))?,
        ),
        Value::Str(val) => Json::String(val.to_string()),
        Value::Array(vals) => Json::Array(
            vals.iter()
                .enumerate()
                .map(|(idx, val)| json_value(val, &format!("{at}[{idx}]")))
                .collect::<Result<_>>()?,
        ),
        Value::Dict(vals) => {
            let mut obj = Map::new();
            for (key, val) in vals {
                obj.insert(key.to_string(), json_value(val, &format!("{at}.{key}"))?);
            }
            Json::Object(obj)
        }
        other => bail!(
            "{at} contains {}; project it to a JSON value explicitly",
            other.ty()
        ),
    })
}
