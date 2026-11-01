//! Execute named memory sources with Nu's parser, evaluator, and command-line handling.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nu_protocol::{
    PipelineData, ShellError, Span, Spanned, Value,
    ast::Block,
    debugger::WithoutDebug,
    engine::{EngineState, Stack, StateWorkingSet},
    parser_path::ParserPath,
    report_parse_warning, report_shell_error,
};

use crate::error::{NuError, Result};

pub(super) fn source(engine: &EngineState, path: &Path) -> Result<(PathBuf, Vec<u8>)> {
    let name = path.to_str().ok_or(NuError::NotUtf8 { what: "script" })?;
    let working = StateWorkingSet::new(engine);
    if let Some(virtual_path) = working.find_virtual_path(name) {
        let bytes = ParserPath::from_virtual_path(&working, name, virtual_path)
            .read(&working)
            .ok_or(NuError::ModuleSourceUnloaded)?;
        return Ok((path.to_path_buf(), bytes));
    }
    let path = PathBuf::from(engine.cwd_as_string(None)?).join(path);
    let bytes = std::fs::read(&path).map_err(|source| NuError::Input {
        action: "read Nu script",
        path: path.clone(),
        source,
    })?;
    Ok((path, bytes))
}

pub(super) fn parse(
    working: &mut StateWorkingSet,
    name: &str,
    source: &[u8],
) -> Result<Arc<Block>> {
    let block = nu_parser::parse_fresh(working, Some(name), source, false);
    check_errors(working)?;
    Ok(block)
}

/// Report the first parse or compile failure the working set recorded.
pub(super) fn check_errors(working: &StateWorkingSet) -> Result<()> {
    if let Some(error) = working.parse_errors.first() {
        return Err(NuError::ModuleParse {
            report: nu_protocol::format_cli_error(None, working, error, Some("nu::parser::error")),
        });
    }
    if let Some(error) = working.compile_errors.first() {
        return Err(NuError::ModuleParse {
            report: nu_protocol::format_cli_error(None, working, error, Some("nu::compile::error")),
        });
    }
    Ok(())
}

/// The Nu command line that calls a script's `main` with `args`.
pub(super) fn main_invocation(args: &[String]) -> String {
    let args = args
        .iter()
        .map(|arg| crate::escape_argument(arg))
        .collect::<Vec<_>>()
        .join(" ");
    format!("main {args}")
}

pub(super) fn evaluate(
    path: &Path,
    args: &[String],
    engine: &mut EngineState,
    stack: &mut Stack,
    input: PipelineData,
) -> i32 {
    match evaluate_source(path, args, engine, stack, input) {
        Ok(code) => code,
        Err(NuError::Shell(ShellError::Exit { code, .. })) => code,
        Err(NuError::Shell(error)) => {
            report_shell_error(Some(stack), engine, &error);
            error.exit_code().unwrap_or(1)
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn evaluate_source(
    path: &Path,
    args: &[String],
    engine: &mut EngineState,
    stack: &mut Stack,
    input: PipelineData,
) -> Result<i32> {
    let (filename, bytes) = source(engine, path)?;
    let name = filename
        .to_str()
        .ok_or(NuError::NotUtf8 { what: "script" })?;
    engine.file = Some(filename.clone());
    for (variable, value) in [
        ("FILE_PWD", filename.parent().unwrap().to_string_lossy()),
        ("CURRENT_FILE", filename.to_string_lossy()),
        ("PROCESS_PATH", path.to_string_lossy()),
    ] {
        stack.add_env_var(variable.into(), Value::string(value, Span::unknown()));
    }
    let mut working = StateWorkingSet::new(engine);
    let block = parse(&mut working, name, &bytes)?;
    if let Some(warning) = working.parse_warnings.first() {
        report_parse_warning(Some(stack), &working, warning);
    }
    let script_name = filename.file_name().unwrap().to_string_lossy();
    let script_span = working
        .get_span_for_filename(name)
        .ok_or(NuError::ModuleSourceUnloaded)?;
    // Only declarations whose block belongs to this script are its entry points. A `use` of a
    // module that exports main must not make this file look like a script with its own main.
    let declarations: Vec<_> = working
        .last_overlay()
        .decls
        .iter()
        .filter(|(name, _)| name.as_slice() == b"main" || name.starts_with(b"main "))
        .filter(|(_, id)| {
            working.get_decl(**id).block_id().is_some_and(|block_id| {
                working
                    .get_block(block_id)
                    .span
                    .is_some_and(|span| script_span.contains_span(span))
            })
        })
        .map(|(name, id)| (name.clone(), *id))
        .collect();
    let has_main = !declarations.is_empty();
    let mut aliases = Vec::new();
    for (name, id) in declarations {
        let mut alias = script_name.as_bytes().to_vec();
        alias.extend_from_slice(&name[4..]);
        if let Some(block_id) = working.get_decl(id).block_id()
            && let Some(block) = block_id
                .get()
                .checked_sub(engine.num_blocks())
                .and_then(|index| working.delta.blocks.get_mut(index))
        {
            Arc::make_mut(block).signature.name = String::from_utf8_lossy(&alias).into_owned();
        }
        aliases.push((alias, id));
    }
    if !aliases.is_empty() {
        working.use_decls(aliases);
    }
    engine.merge_delta(working.render())?;
    if !has_main {
        let evaluated =
            nu_engine::eval_block_with_early_return::<WithoutDebug>(engine, stack, &block, input)?;
        let no_newline = matches!(&evaluated.body, PipelineData::ByteStream(..));
        print_output(engine, stack, evaluated.body, no_newline)?;
        check_pipeline(evaluated.exit)?;
        return Ok(0);
    }
    let initialized =
        nu_engine::eval_block::<WithoutDebug>(engine, stack, &block, PipelineData::empty())?;
    if initialized.early_return {
        return Ok(0);
    }
    print_output(engine, stack, initialized.body, true)?;
    let command = main_invocation(args);
    let mut working = StateWorkingSet::new(engine);
    let block = parse(&mut working, "<commandline>", command.as_bytes())?;
    engine.merge_delta(working.render())?;
    let invoked =
        nu_engine::eval_block_with_early_return::<WithoutDebug>(engine, stack, &block, input)?;
    let no_newline = matches!(&invoked.body, PipelineData::ByteStream(..));
    print_output(engine, stack, invoked.body, no_newline)?;
    check_pipeline(invoked.exit)?;
    Ok(0)
}

/// Apply Nushell's pipefail setting to a finished pipeline, matching the CLI's exit contract.
fn check_pipeline(exit: Vec<Option<nu_protocol::process::ExitStatusGuard>>) -> Result<()> {
    if nu_experimental::PIPE_FAIL.get() {
        nu_protocol::process::check_exit_status_future(exit)?;
    }
    Ok(())
}

/// Print one pipeline through the CLI's configured display hooks. An empty Nu block forwards
/// the pipeline unchanged, so this is the public way to reuse the CLI's printing convention.
fn print_output(
    engine: &mut EngineState,
    stack: &mut Stack,
    output: PipelineData,
    no_newline: bool,
) -> Result<()> {
    nu_cli::evaluate_commands(
        &Spanned {
            item: String::new(),
            span: Span::unknown(),
        },
        engine,
        stack,
        output,
        Vec::new(),
        nu_cli::EvaluateCommandsOpts {
            no_newline,
            ..Default::default()
        },
    )?;
    Ok(())
}
