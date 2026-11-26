//! Execute named memory sources with Nu's parser, evaluator, and command-line handling.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nu_protocol::{
    DeclId, PipelineData, ShellError, Span, Spanned, Value,
    ast::Block,
    debugger::WithoutDebug,
    engine::{EngineState, Stack, StateWorkingSet},
    parser_path::ParserPath,
    report_parse_warning, report_shell_error,
};

use crate::error::{NuError, Result};

pub(super) fn source(engine: &EngineState, path: &Path) -> Result<(PathBuf, Vec<u8>)> {
    let name = crate::path_str(path, "script")?;
    let ws = StateWorkingSet::new(engine);
    if let Some(vpath) = ws.find_virtual_path(name) {
        let buf = ParserPath::from_virtual_path(&ws, name, vpath)
            .read(&ws)
            .ok_or(NuError::ModuleSourceUnloaded)?;
        return Ok((path.to_path_buf(), buf));
    }
    let path = PathBuf::from(engine.cwd_as_string(None)?).join(path);
    let buf = std::fs::read(&path).map_err(|source| NuError::Input {
        action: "read Nu script",
        path: path.clone(),
        source,
    })?;
    Ok((path, buf))
}

pub(super) fn parse(ws: &mut StateWorkingSet, name: &str, src: &[u8]) -> Result<Arc<Block>> {
    let block = nu_parser::parse_fresh(ws, Some(name), src, false);
    check_errors(ws)?;
    Ok(block)
}

/// Report the first parse or compile failure the working set recorded.
pub(super) fn check_errors(ws: &StateWorkingSet) -> Result<()> {
    if let Some(err) = ws.parse_errors.first() {
        return Err(NuError::ModuleParse {
            report: nu_protocol::format_cli_error(None, ws, err, Some("nu::parser::error")),
        });
    }
    if let Some(err) = ws.compile_errors.first() {
        return Err(NuError::ModuleParse {
            report: nu_protocol::format_cli_error(None, ws, err, Some("nu::compile::error")),
        });
    }
    Ok(())
}

/// Find the `main` commands defined in this script, excluding imported declarations.
pub(super) fn main_declarations(
    ws: &StateWorkingSet,
    script: &Block,
) -> Result<Vec<(Vec<u8>, DeclId)>> {
    let script_span = script.span.ok_or(NuError::ModuleSourceUnloaded)?;
    Ok(ws
        .last_overlay()
        .decls
        .iter()
        .filter(|(name, _)| name.as_slice() == b"main" || name.starts_with(b"main "))
        .filter(|(_, id)| {
            ws.get_decl(**id).block_id().is_some_and(|block_id| {
                ws.get_block(block_id)
                    .span
                    .is_some_and(|span| script_span.contains_span(span))
            })
        })
        .map(|(name, id)| (name.clone(), *id))
        .collect())
}

/// Parse one command with argv boundaries fixed before the Nu parser sees it.
pub(super) fn parse_main(ws: &mut StateWorkingSet, args: &[String]) -> Block {
    use nu_parser::{Token, TokenContents};

    let mut spelling = String::from("main");
    let mut ranges = Vec::with_capacity(args.len() + 1);
    ranges.push(0..spelling.len());
    for arg in args {
        spelling.push(' ');
        let start = spelling.len();
        spelling.push_str(&crate::escape_argument(arg));
        ranges.push(start..spelling.len());
    }
    // Fixed TokenContents::Item boundaries keep argument bytes from injecting pipelines or redirections.
    let file = ws.add_file("<commandline>", spelling.as_bytes());
    let span = ws.get_span_for_file(file);
    let tokens = ranges
        .into_iter()
        .map(|range| {
            Token::new(
                TokenContents::Item,
                Span::new(span.start + range.start, span.start + range.end),
            )
        })
        .collect::<Vec<_>>();
    nu_parser::parse_block(ws, &tokens, span, false, false, None)
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
        Err(NuError::Shell(err)) => {
            report_shell_error(Some(stack), engine, &err);
            err.exit_code().unwrap_or(1)
        }
        Err(err) => {
            eprintln!("{err}");
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
    let (file, src) = source(engine, path)?;
    let name = crate::path_str(&file, "script")?.to_owned();
    engine.file = Some(file.clone());
    for (var, val) in [
        ("FILE_PWD", file.parent().unwrap().to_string_lossy()),
        ("CURRENT_FILE", file.to_string_lossy()),
        ("PROCESS_PATH", path.to_string_lossy()),
    ] {
        stack.add_env_var(var.into(), Value::string(val, Span::unknown()));
    }
    let mut ws = StateWorkingSet::new(engine);
    let block = parse(&mut ws, &name, &src)?;
    if let Some(warning) = ws.parse_warnings.first() {
        report_parse_warning(Some(stack), &ws, warning);
    }
    let script_name = file.file_name().unwrap().to_string_lossy();
    let decls = main_declarations(&ws, &block)?;
    let has_main = !decls.is_empty();
    let mut aliases = Vec::new();
    for (name, id) in decls {
        let mut alias = script_name.as_bytes().to_vec();
        alias.extend_from_slice(&name[4..]);
        if let Some(block_id) = ws.get_decl(id).block_id()
            && let Some(block) = block_id
                .get()
                .checked_sub(engine.num_blocks())
                .and_then(|idx| ws.delta.blocks.get_mut(idx))
        {
            Arc::make_mut(block).signature.name = String::from_utf8_lossy(&alias).into_owned();
        }
        aliases.push((alias, id));
    }
    if !aliases.is_empty() {
        ws.use_decls(aliases);
    }
    engine.merge_delta(ws.render())?;
    if !has_main {
        return eval_and_print(engine, stack, &block, input);
    }
    let mut ws = StateWorkingSet::new(engine);
    let mut inv = parse_main(&mut ws, args);
    check_errors(&ws)?;
    match nu_engine::compile(&ws, &inv) {
        Ok(ir) => inv.ir_block = Some(ir),
        Err(err) => ws.compile_errors.push(err),
    }
    check_errors(&ws)?;
    engine.merge_delta(ws.render())?;
    let init = nu_engine::eval_block::<WithoutDebug>(engine, stack, &block, PipelineData::empty())?;
    if init.early_return {
        return Ok(0);
    }
    print_output(engine, stack, init.body, true)?;
    eval_and_print(engine, stack, &inv, input)
}

fn eval_and_print(
    engine: &mut EngineState,
    stack: &mut Stack,
    block: &Block,
    input: PipelineData,
) -> Result<i32> {
    let out = nu_engine::eval_block_with_early_return::<WithoutDebug>(engine, stack, block, input)?;
    let no_newline = matches!(&out.body, PipelineData::ByteStream(..));
    print_output(engine, stack, out.body, no_newline)?;
    check_pipeline(out.exit)?;
    Ok(0)
}

fn check_pipeline(exit: Vec<Option<nu_protocol::process::ExitStatusGuard>>) -> Result<()> {
    if nu_experimental::PIPE_FAIL.get() {
        nu_protocol::process::check_exit_status_future(exit)?;
    }
    Ok(())
}

/// Print via CLI display hooks; an empty block forwards the pipeline unchanged.
fn print_output(
    engine: &mut EngineState,
    stack: &mut Stack,
    out: PipelineData,
    no_newline: bool,
) -> Result<()> {
    nu_cli::evaluate_commands(
        &Spanned {
            item: String::new(),
            span: Span::unknown(),
        },
        engine,
        stack,
        out,
        Vec::new(),
        nu_cli::EvaluateCommandsOpts {
            no_newline,
            ..Default::default()
        },
    )?;
    Ok(())
}
