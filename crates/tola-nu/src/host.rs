use nu_engine::CallExt;
use nu_protocol::{
    PipelineData, ShellError, Signature, Span, SyntaxShape, Type, Value,
    engine::{Call, Command, CommandType, EngineState, Stack},
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
pub type SubmitDiagnostics = Arc<dyn Fn(Json) -> Result<(), String> + Send + Sync>;
pub type RenderDiagnostics = Arc<dyn Fn(&Path, Json, bool) -> Result<String, String> + Send + Sync>;
/// Read the configured assets matching source paths in the current build.
pub type GetAssets =
    Arc<dyn Fn(Option<String>, Vec<String>, Vec<String>) -> Result<Json, String> + Send + Sync>;
/// Read the digest of the paths selected by the hook's rerun-on declaration.
pub type RerunOnDigest = Arc<dyn Fn() -> Result<Option<String>, String> + Send + Sync>;

#[derive(Default, Clone)]
pub struct Host {
    pub context: Option<Json>,
    pub site: Option<Json>,
    pub modules: BTreeMap<String, Json>,
    pub module: Option<String>,
    pub hook: Option<Json>,
    pub init: Option<Json>,
    pub query: Option<Query>,
    pub submit_diagnostics: Option<SubmitDiagnostics>,
    pub render_diagnostics: Option<RenderDiagnostics>,
    pub get_assets: Option<GetAssets>,
    pub rerun_on_digest: Option<RerunOnDigest>,
    /// The invocation's SVG fonts, or a configuration error reported when an SVG needs them.
    pub svg_fonts: Option<Result<Arc<tola_typst::FontStore>, String>>,
}

#[derive(Clone, Copy)]
pub(super) enum Operation {
    Context,
    Site,
    Module,
    Hook,
    Assets,
    Init,
    Query,
    Submit,
    Render,
}

#[derive(Clone)]
pub(super) struct HostCommand(pub Operation, pub Arc<OnceLock<Host>>);

impl Command for HostCommand {
    fn command_type(&self) -> CommandType {
        match self.0 {
            Operation::Context | Operation::Submit | Operation::Render => CommandType::Custom,
            _ => CommandType::Builtin,
        }
    }

    fn name(&self) -> &str {
        match self.0 {
            Operation::Context => "get-context",
            Operation::Site => "get-site",
            Operation::Module => "get-module",
            Operation::Hook => "get-hook",
            Operation::Assets => "get-assets",
            Operation::Init => "get-init",
            Operation::Query => "query",
            Operation::Submit => "submit",
            Operation::Render => "render-diagnostics",
        }
    }

    fn description(&self) -> &str {
        match self.0 {
            Operation::Context => "Read the context Tola supplied for this invocation.",
            Operation::Site => "Read the current site's settings and paths.",
            Operation::Module => "Read the current or named module instance.",
            Operation::Hook => "Read the current hook's stage and directories.",
            Operation::Assets => "Read configured assets in this hook's input snapshot.",
            Operation::Init => "Read the current initializer's source and directories.",
            Operation::Query => "Evaluate a site query.",
            Operation::Submit => "Submit one complete batch from a Tola hook or module command.",
            Operation::Render => "Render a list of diagnostics as terminal text.",
        }
    }

    fn extra_description(&self) -> &str {
        match self.0 {
            Operation::Submit => {
                "A second submission is rejected, even if the first batch was empty. An error fails the host operation after the script finishes."
            }
            Operation::Assets => {
                "Each row has a source path, an output path relative to the hook input directory, and a media type. Patterns are relative to the site root or the selected module root."
            }
            _ => "",
        }
    }

    fn signature(&self) -> Signature {
        let sig = Signature::build(self.name());
        match self.0 {
            Operation::Context => sig.switch("json", "Return the context as JSON.", None),
            Operation::Module => sig.optional("name", SyntaxShape::String, "Module alias."),
            Operation::Hook => sig.switch(
                "rerun-on-digest",
                "Hash the declared rerun-on files instead of returning the hook record.",
                None,
            ),
            Operation::Assets => sig
                .input_output_types(vec![(Type::Nothing, Type::list(Type::record()))])
                .named(
                    "module",
                    SyntaxShape::String,
                    "Read sources inside this module.",
                    None,
                )
                .named(
                    "include",
                    SyntaxShape::List(Box::new(SyntaxShape::String)),
                    "Include source paths or globs. Defaults to all.",
                    None,
                )
                .named(
                    "exclude",
                    SyntaxShape::List(Box::new(SyntaxShape::String)),
                    "Exclude source paths or globs.",
                    None,
                ),
            Operation::Query => sig
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
            Operation::Submit => {
                sig.input_output_types(vec![(Type::list(Type::record()), Type::Nothing)])
            }
            Operation::Render => {
                sig.input_output_types(vec![(Type::list(Type::record()), Type::String)])
            }
            _ => sig,
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
        let err = |msg: String| {
            ShellError::Generic(GenericError::new(self.description().to_owned(), msg, span))
        };
        engine.signals().check(&span)?;
        let empty = Host::default();
        let host = self.1.get().unwrap_or(&empty);
        let val = match self.0 {
            Operation::Context => {
                let ctx = host.context.as_ref().ok_or_else(|| {
                    err("no Tola invocation context; run `tola help @tola-scripts/context` for details".into())
                })?;
                if call.has_flag(engine, stack, "json")? {
                    let out = serde_json::to_string(ctx).map_err(|cause| err(cause.to_string()))?;
                    return Ok(PipelineData::value(Value::string(out, span), None));
                }
                ctx.clone()
            }
            Operation::Site => host
                .site
                .clone()
                .ok_or_else(|| err("get-site requires a loaded site".into()))?,
            Operation::Module => {
                if host.site.is_none() {
                    return Err(err("get-module requires a loaded site".into()));
                }
                let name: Option<String> = call.opt(engine, stack, 0)?;
                let name = name.as_ref().or(host.module.as_ref()).ok_or_else(|| {
                    err("get-module requires a module alias or a current module".into())
                })?;
                host.modules
                    .get(name)
                    .cloned()
                    .ok_or_else(|| err(format!("no configured module is named `{name}`")))?
            }
            Operation::Hook => {
                if call.has_flag(engine, stack, "rerun-on-digest")? {
                    let digest = host.rerun_on_digest.as_ref().ok_or_else(|| {
                        err("get-hook --rerun-on-digest needs a Nu build hook".into())
                    })?;
                    digest().map_err(err)?.map_or(Json::Null, Json::String)
                } else {
                    host.hook
                        .clone()
                        .ok_or_else(|| err("get-hook can only be called in a hook".into()))?
                }
            }
            Operation::Assets => {
                let assets = host.get_assets.as_ref().ok_or_else(|| {
                    err("get-assets needs a hook that runs after asset discovery".into())
                })?;
                let module = call.get_flag(engine, stack, "module")?;
                let include = call
                    .get_flag(engine, stack, "include")?
                    .unwrap_or_else(|| vec!["**".into()]);
                let exclude = call.get_flag(engine, stack, "exclude")?.unwrap_or_default();
                assets(module, include, exclude).map_err(err)?
            }
            Operation::Init => host
                .init
                .clone()
                .ok_or_else(|| err("get-init can only be called in an initializer".into()))?,
            Operation::Query => {
                let target: String = call.req(engine, stack, 0)?;
                let cfg: Option<String> = call.get_flag(engine, stack, "config")?;
                let label: Option<String> = call.get_flag(engine, stack, "label")?;
                let query = host
                    .query
                    .as_ref()
                    .ok_or_else(|| err("query requires a loaded site".into()))?;
                query(
                    engine.cwd(Some(stack))?.as_ref(),
                    &target,
                    cfg.map(PathBuf::from).as_deref(),
                    label.as_deref(),
                )
                .map_err(err)?
            }
            Operation::Submit => {
                let err = |msg: String| -> ShellError {
                    GenericError::new("cannot submit diagnostics", msg, span).into()
                };
                let submit = host.submit_diagnostics.as_ref().ok_or_else(|| {
                    err("use `diagnostic submit` in a Tola hook or module command".into())
                })?;
                let diags = into_json(input.into_value(span)?).map_err(err)?;
                submit(diags).map_err(err)?;
                return Ok(PipelineData::empty());
            }
            Operation::Render => {
                let render = host
                    .render_diagnostics
                    .as_ref()
                    .ok_or_else(|| err("run the script with Tola to render diagnostics".into()))?;
                let diags = into_json(input.into_value(span)?).map_err(err)?;
                let color = stack.get_config(engine).use_ansi_coloring.get(engine);
                let rendered =
                    render(engine.cwd(Some(stack))?.as_ref(), diags, color).map_err(err)?;
                return Ok(PipelineData::value(Value::string(rendered, span), None));
            }
        };
        engine.signals().check(&span)?;
        Ok(PipelineData::value(from_json(val, span), None))
    }
}

fn from_json(val: Json, span: Span) -> Value {
    match val {
        Json::Null => Value::nothing(span),
        Json::Bool(b) => Value::bool(b, span),
        Json::Number(n) => n.as_i64().map_or_else(
            || Value::float(n.as_f64().expect("JSON number"), span),
            |n| Value::int(n, span),
        ),
        Json::String(s) => Value::string(s, span),
        Json::Array(vals) => {
            Value::list(vals.into_iter().map(|v| from_json(v, span)).collect(), span)
        }
        Json::Object(fields) => Value::record(
            fields
                .into_iter()
                .map(|(k, v)| (k, from_json(v, span)))
                .collect(),
            span,
        ),
    }
}

fn into_json(val: Value) -> Result<Json, String> {
    match val {
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
            .map(|(k, v)| Ok((k.clone(), into_json(v.clone())?)))
            .collect::<Result<_, _>>()
            .map(Json::Object),
        val => Err(format!(
            "diagnostic fields cannot contain {}",
            val.get_type()
        )),
    }
}
