use std::path::Path;

use nu_protocol::{
    ParseError, Signature,
    ast::{Argument, Expr},
    engine::StateWorkingSet,
};

use crate::{
    Runtime,
    error::{NuError, Result},
};

impl Runtime {
    /// Read the signatures of the commands a script defines, without evaluating it.
    pub fn script_signatures(&self, path: &Path) -> Result<Vec<Signature>> {
        self.ensure_fresh()?;
        let (path, source) = crate::execution::source(&self.engine, path)?;
        let mut engine = self.engine.clone();
        engine.file = Some(path.clone());
        let name = path.to_str().ok_or(NuError::NotUtf8 { what: "script" })?;
        let mut working = StateWorkingSet::new(&engine);
        let block = crate::execution::parse(&mut working, name, &source)?;
        let mut signatures = crate::execution::main_declarations(&working, &block)?
            .into_iter()
            .filter_map(|(name, id)| {
                let command = working.get_decl(id);
                let block = working.get_block(command.block_id()?);
                let mut signature = block.signature.as_ref().clone();
                signature.name = String::from_utf8_lossy(&name).into_owned();
                Some(signature)
            })
            .collect::<Vec<_>>();
        signatures.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(signatures)
    }

    /// Read the signature of a script's `main` declaration without executing the script.
    pub fn command_signature(&self, path: &Path, prefix: &[String]) -> Result<Option<Signature>> {
        self.ensure_fresh()?;
        let (path, source) = crate::execution::source(&self.engine, path)?;
        let mut engine = self.engine.clone();
        engine.file = Some(path.clone());
        let path = path.to_str().ok_or(NuError::NotUtf8 { what: "script" })?;
        let mut working = StateWorkingSet::new(&engine);
        crate::execution::parse(&mut working, path, &source)?;
        let block = crate::execution::parse_main(&mut working, prefix);
        let Some(Expr::Call(call)) = block
            .pipelines
            .first()
            .and_then(|pipeline| pipeline.elements.first())
            .map(|element| &element.expr.expr)
        else {
            return Ok(None);
        };
        let block_id = {
            let declaration = working.get_decl(call.decl_id);
            if declaration.name() != "main" && !declaration.name().starts_with("main ") {
                return Ok(None);
            }
            declaration.block_id()
        };
        working.parse_errors.retain(|error| {
            !matches!(
                error,
                ParseError::MissingPositional(..) | ParseError::MissingRequiredFlag(..)
            )
        });
        crate::execution::check_errors(&working)?;
        let Some(block_id) = block_id else {
            return Ok(None);
        };
        // The parser writes the authoritative signature onto the block; the `BlockCommand`
        // declaration keeps a copy that missed later prefix consumption.
        let mut signature = working.get_block(block_id).signature.as_ref().clone();
        let mut consumed = call.arguments.iter().filter(|argument| matches!(argument, Argument::Positional(expression) if !matches!(&expression.expr, Expr::Garbage))).count();
        let required = consumed.min(signature.required_positional.len());
        signature.required_positional.drain(..required);
        consumed -= required;
        let optional = consumed.min(signature.optional_positional.len());
        signature.optional_positional.drain(..optional);
        for argument in &call.arguments {
            if let Argument::Named((long, short, _)) = argument {
                for flag in &mut signature.named {
                    if (!long.item.is_empty() && flag.long == long.item)
                        || short.as_ref().is_some_and(|name| {
                            flag.short
                                .is_some_and(|short| name.item == short.to_string())
                        })
                    {
                        flag.required = false;
                    }
                }
            }
        }
        Ok(Some(signature))
    }
}
