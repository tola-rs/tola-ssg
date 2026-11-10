use nu_engine::CallExt;
use nu_protocol::{
    PipelineData, ShellError, Signature, Span, SyntaxShape, Value,
    engine::{Call, Command, EngineState, Stack},
    shell_error::generic::GenericError,
};
use serde_json::Value as Json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

pub type Query =
    Arc<dyn Fn(&Path, &str, Option<&Path>, Option<&str>) -> Result<Json, String> + Send + Sync>;
pub type Diagnostic = Arc<dyn Fn(Json) -> Result<(), String> + Send + Sync>;

#[derive(Default, Clone)]
pub struct Host {
    pub site: Option<Json>,
    pub modules: BTreeMap<String, Json>,
    pub module: Option<String>,
    pub hook: Option<Json>,
    pub init: Option<Json>,
    pub query: Option<Query>,
    pub diagnostic: Option<Diagnostic>,
}

#[derive(Clone, Copy)]
pub(super) enum Operation {
    Site,
    Module,
    Hook,
    Init,
    Query,
    Submit,
}

#[derive(Clone)]
pub(super) struct HostCommand(pub Operation, pub Arc<OnceLock<Host>>);

impl Command for HostCommand {
    fn name(&self) -> &str {
        match self.0 {
            Operation::Site => "get-site",
            Operation::Module => "get-module",
            Operation::Hook => "get-hook",
            Operation::Init => "get-init",
            Operation::Query => "query",
            Operation::Submit => "__tola_diagnostic_submit",
        }
    }

    fn description(&self) -> &str {
        match self.0 {
            Operation::Site => "Read the current site's settings and paths.",
            Operation::Module => "Read the current or named module instance.",
            Operation::Hook => "Read the current hook's stage and directories.",
            Operation::Init => "Read the current initializer's source and directories.",
            Operation::Query => "Evaluate a site query.",
            Operation::Submit => "Submit diagnostics to the current invocation.",
        }
    }

    fn signature(&self) -> Signature {
        let signature = Signature::build(self.name());
        match self.0 {
            Operation::Module => signature.optional("name", SyntaxShape::String, "Module alias."),
            Operation::Query => signature
                .required("target", SyntaxShape::String, "Query name or Typst file.")
                .named(
                    "config",
                    SyntaxShape::Filepath,
                    "Configuration override.",
                    None,
                )
                .named(
                    "label",
                    SyntaxShape::String,
                    "Metadata label for a Typst file.",
                    None,
                ),
            _ => signature,
        }
    }

    fn run(
        &self,
        engine: &EngineState,
        stack: &mut Stack,
        call: &Call,
        input: PipelineData,
    ) -> Result<PipelineData, ShellError> {
        let span = call.head;
        let error = |message: String| {
            ShellError::Generic(GenericError::new(
                self.description().to_owned(),
                message,
                span,
            ))
        };
        engine.signals().check(&span)?;
        let empty = Host::default();
        let host = self.1.get().unwrap_or(&empty);
        let value = match self.0 {
            Operation::Site => host
                .site
                .clone()
                .ok_or_else(|| error("get-site requires a loaded site".into()))?,
            Operation::Module => {
                if host.site.is_none() {
                    return Err(error("get-module requires a loaded site".into()));
                }
                let name: Option<String> = call.opt(engine, stack, 0)?;
                let name = name.as_ref().or(host.module.as_ref()).ok_or_else(|| {
                    error("get-module requires a module alias or a current module".into())
                })?;
                host.modules
                    .get(name)
                    .cloned()
                    .ok_or_else(|| error(format!("no configured module is named `{name}`")))?
            }
            Operation::Hook => host
                .hook
                .clone()
                .ok_or_else(|| error("get-hook can only be called in a hook".into()))?,
            Operation::Init => host
                .init
                .clone()
                .ok_or_else(|| error("get-init can only be called in an initializer".into()))?,
            Operation::Query => {
                let target: String = call.req(engine, stack, 0)?;
                let config: Option<String> = call.get_flag(engine, stack, "config")?;
                let label: Option<String> = call.get_flag(engine, stack, "label")?;
                let query = host
                    .query
                    .as_ref()
                    .ok_or_else(|| error("query requires a loaded site".into()))?;
                query(
                    engine.cwd(Some(stack))?.as_ref(),
                    &target,
                    config.map(PathBuf::from).as_deref(),
                    label.as_deref(),
                )
                .map_err(error)?
            }
            Operation::Submit => {
                let submit = host
                    .diagnostic
                    .as_ref()
                    .ok_or_else(|| error("this invocation has no diagnostic collector".into()))?;
                let diagnostics = into_json(input.into_value(span)?).map_err(&error)?;
                submit(diagnostics).map_err(error)?;
                return Ok(PipelineData::empty());
            }
        };
        engine.signals().check(&span)?;
        Ok(PipelineData::value(from_json(value, span), None))
    }
}

fn from_json(value: Json, span: Span) -> Value {
    match value {
        Json::Null => Value::nothing(span),
        Json::Bool(value) => Value::bool(value, span),
        Json::Number(value) => value.as_i64().map_or_else(
            || Value::float(value.as_f64().expect("JSON number"), span),
            |value| Value::int(value, span),
        ),
        Json::String(value) => Value::string(value, span),
        Json::Array(values) => Value::list(
            values
                .into_iter()
                .map(|value| from_json(value, span))
                .collect(),
            span,
        ),
        Json::Object(fields) => Value::record(
            fields
                .into_iter()
                .map(|(name, value)| (name, from_json(value, span)))
                .collect(),
            span,
        ),
    }
}

fn into_json(value: Value) -> Result<Json, String> {
    match value {
        Value::Nothing { .. } => Ok(Json::Null),
        Value::Bool { val, .. } => Ok(val.into()),
        Value::Int { val, .. } => Ok(val.into()),
        Value::Float { val, .. } => Ok(val.into()),
        Value::String { val, .. } => Ok(val.into()),
        Value::List { vals, .. } => vals
            .into_iter()
            .map(into_json)
            .collect::<Result<Vec<_>, _>>()
            .map(Json::Array),
        Value::Record { val, .. } => val
            .iter()
            .map(|(name, value)| Ok((name.clone(), into_json(value.clone())?)))
            .collect::<Result<_, _>>()
            .map(Json::Object),
        value => Err(format!(
            "diagnostic fields cannot contain {}",
            value.get_type()
        )),
    }
}
