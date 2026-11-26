use std::path::Path;

use nu_protocol::{
    ParseError, Signature,
    ast::{Argument, Call, Expr},
    engine::{EngineState, StateWorkingSet},
};

use crate::{Runtime, error::Result};

impl Runtime {
    fn prepare_script(&self, path: &Path) -> Result<(EngineState, String, Vec<u8>)> {
        self.ensure_fresh()?;
        let (path, src) = crate::execution::source(&self.engine, path)?;
        let mut engine = self.engine.clone();
        engine.file = Some(path.clone());
        let name = crate::path_str(&path, "script")?.to_owned();
        Ok((engine, name, src))
    }

    /// Read the signatures of the commands a script defines, without evaluating it.
    pub fn script_signatures(&self, path: &Path) -> Result<Vec<Signature>> {
        let (engine, name, src) = self.prepare_script(path)?;
        let mut ws = StateWorkingSet::new(&engine);
        let block = crate::execution::parse(&mut ws, &name, &src)?;
        let mut sigs = crate::execution::main_declarations(&ws, &block)?
            .into_iter()
            .filter_map(|(name, id)| {
                let decl = ws.get_decl(id);
                let block = ws.get_block(decl.block_id()?);
                let mut sig = block.signature.as_ref().clone();
                sig.name = String::from_utf8_lossy(&name).into_owned();
                Some(sig)
            })
            .collect::<Vec<_>>();
        sigs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(sigs)
    }

    /// Read the signature of a script's `main` declaration without executing the script.
    pub fn command_signature(&self, path: &Path, prefix: &[String]) -> Result<Option<Signature>> {
        let (engine, name, src) = self.prepare_script(path)?;
        let mut ws = StateWorkingSet::new(&engine);
        crate::execution::parse(&mut ws, &name, &src)?;
        let block = crate::execution::parse_main(&mut ws, prefix);
        let Some(Expr::Call(call)) = block
            .pipelines
            .first()
            .and_then(|pipeline| pipeline.elements.first())
            .map(|el| &el.expr.expr)
        else {
            return Ok(None);
        };
        let block_id = {
            let decl = ws.get_decl(call.decl_id);
            if decl.name() != "main" && !decl.name().starts_with("main ") {
                return Ok(None);
            }
            decl.block_id()
        };
        ws.parse_errors.retain(|err| {
            !matches!(
                err,
                ParseError::MissingPositional(..) | ParseError::MissingRequiredFlag(..)
            )
        });
        crate::execution::check_errors(&ws)?;
        let Some(block_id) = block_id else {
            return Ok(None);
        };
        // The parser writes the authoritative signature onto the block; `BlockCommand`
        // keeps a copy that missed later prefix consumption.
        let mut sig = ws.get_block(block_id).signature.as_ref().clone();
        consume_prefix_args(&mut sig, call);
        Ok(Some(sig))
    }
}

fn consume_prefix_args(sig: &mut Signature, call: &Call) {
    let mut consumed = call
        .arguments
        .iter()
        .filter(
            |arg| matches!(arg, Argument::Positional(expr) if !matches!(&expr.expr, Expr::Garbage)),
        )
        .count();
    let req = consumed.min(sig.required_positional.len());
    sig.required_positional.drain(..req);
    consumed -= req;
    let opt = consumed.min(sig.optional_positional.len());
    sig.optional_positional.drain(..opt);
    for arg in &call.arguments {
        let Argument::Named((long, short, _)) = arg else {
            continue;
        };
        for flag in &mut sig.named {
            if (!long.item.is_empty() && flag.long == long.item)
                || short
                    .as_ref()
                    .is_some_and(|s| flag.short.is_some_and(|ch| s.item == ch.to_string()))
            {
                flag.required = false;
            }
        }
    }
}
