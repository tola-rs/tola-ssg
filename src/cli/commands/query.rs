//! Model queries use the same evaluator for direct and module-declared commands.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use clap::{Args, FromArgMatches};
use serde_json::Value;
use tola_build::BuildResources;
use tola_build::config::ResolvedSiteConfig;
use tola_build::modules::query::QueryResult;
use tola_build::modules::{Query, ResolvedModule};
use tola_diagnostic::DiagnosticError;

use crate::cancellation::Cancellation;
use crate::cli::args::{JsonOutputArgs, QueryArgs};
use crate::cli::output::CommandOutput;

#[derive(Args, Debug, Clone)]
pub(crate) struct QueryOptions {
    /// Include schema, diagnostics, and input evidence around the query value.
    #[arg(long)]
    pub report: bool,
    #[command(flatten)]
    pub display: JsonOutputArgs,
}

pub(in crate::cli) fn run(
    config: Arc<ResolvedSiteConfig>,
    args: &QueryArgs,
    resources: BuildResources,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    super::read::check_output(&args.options.display, output)?;
    let scope = resources.input_scope();
    let target = &args.target;
    let file = Path::new(target).is_absolute()
        || target.starts_with("./")
        || target.starts_with("../")
        || target.starts_with(".\\")
        || target.starts_with("..\\");
    let result = if file {
        let label = args
            .label
            .as_deref()
            .context("a query file requires `--label LABEL`")?;
        ensure!(!label.is_empty(), "a query label must not be empty");
        tola_build::modules::query::run_file(
            config,
            &std::path::absolute(target)?,
            label,
            resources,
            Vec::new(),
            &cancellation.token(),
        )?
    } else {
        ensure!(
            args.label.is_none(),
            "`--label` applies only to an explicit query file"
        );
        let (namespace, local) = target.split_once('.').context(
            "choose a module-qualified query such as `notes.nodes`, or an explicit `./query.typ` file",
        )?;
        let modules =
            tola_build::modules::resolve_modules(config.get_root(), config.modules(), scope)?;
        super::extensions::check_namespaces(&modules)?;
        let module = modules
            .get(namespace)
            .with_context(|| format!("no module is named `{namespace}`"))?;
        let query = module
            .queries
            .get(local)
            .with_context(|| format!("module `{namespace}` declares no query `{local}`"))?;
        tola_build::modules::query::run(
            config,
            module,
            query,
            target,
            resources,
            Vec::new(),
            &cancellation.token(),
        )?
    };
    complete(result, &args.options, &[], scope, output, cancellation)
}

#[expect(
    clippy::too_many_arguments,
    reason = "a declared query's inputs are distinct configuration values"
)]
pub(in crate::cli) fn run_declared(
    config: Arc<ResolvedSiteConfig>,
    module: &ResolvedModule,
    query: &Query,
    name: &str,
    usage: &str,
    projection: Option<&[String]>,
    tail: &[OsString],
    resources: BuildResources,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    let command =
        QueryOptions::augment_args(clap::Command::new(name.to_owned()).bin_name(usage.to_owned()));
    let matches = match command.try_get_matches_from(
        std::iter::once(OsStr::new(name)).chain(tail.iter().map(OsString::as_os_str)),
    ) {
        Ok(matches) => matches,
        Err(error) if error.kind() == clap::error::ErrorKind::DisplayHelp => {
            return output.write_stdout(error.to_string());
        }
        Err(error) => return Err(error.into()),
    };
    let options = QueryOptions::from_arg_matches(&matches)?;
    super::read::check_output(&options.display, output)?;
    let scope = resources.input_scope();
    let result = tola_build::modules::query::run(
        config,
        module,
        query,
        name,
        resources,
        Vec::new(),
        &cancellation.token(),
    )?;
    complete(
        result,
        &options,
        projection.unwrap_or_default(),
        scope,
        output,
        cancellation,
    )
}

fn complete(
    mut result: QueryResult,
    options: &QueryOptions,
    projection: &[String],
    scope: tola_build::InputScope,
    output: &CommandOutput,
    cancellation: &Cancellation,
) -> Result<()> {
    if !result.success {
        if options.report {
            super::read::write_json(
                &serde_json::to_value(&result)?,
                &options.display,
                output,
                cancellation,
            )?;
        }
        return Err(DiagnosticError::new(
            format!("model query `{}` failed", result.query),
            result.diagnostics,
        )
        .into());
    }
    output.diagnostics(&result.diagnostics)?;
    let value = result
        .data
        .take()
        .expect("a successful materialized query has a value");
    let Some(mut selected) =
        super::read::select(&value, &result.query, &options.display, scope, cancellation)?
    else {
        return Ok(());
    };
    project(&mut selected, projection)?;
    let value = if options.report {
        result.result_digest = Some(format!(
            "blake3:{}",
            blake3::hash(&serde_json::to_vec(&selected)?).to_hex()
        ));
        result.data = Some(selected);
        serde_json::to_value(result)?
    } else {
        selected
    };
    super::read::write_json(&value, &options.display, output, cancellation)
}

fn project(value: &mut Value, fields: &[String]) -> Result<()> {
    if fields.is_empty() {
        return Ok(());
    }
    if let Value::Array(rows) = value {
        for row in rows {
            project(row, fields)?;
        }
        return Ok(());
    }
    let object = value
        .as_object_mut()
        .context("a command projection requires object rows")?;
    for field in fields {
        ensure!(
            object.contains_key(field),
            "query result has no field `{field}`"
        );
    }
    object.retain(|key, _| fields.contains(key));
    Ok(())
}
