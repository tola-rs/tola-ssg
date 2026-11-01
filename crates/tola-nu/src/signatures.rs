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
    /// Read the signature of a script's `main` declaration without executing the script.
    pub fn command_signature(&self, path: &Path, prefix: &[String]) -> Result<Option<Signature>> {
        self.ensure_fresh()?;
        let (path, source) = crate::execution::source(&self.engine, path)?;
        let mut engine = self.engine.clone();
        engine.file = Some(path.clone());
        let path = path.to_str().ok_or(NuError::NotUtf8 { what: "script" })?;
        let mut working = StateWorkingSet::new(&engine);
        crate::execution::parse(&mut working, path, &source)?;
        let invocation = crate::execution::main_invocation(prefix);
        let block = nu_parser::parse_fresh(
            &mut working,
            Some("Tola command invocation"),
            invocation.as_bytes(),
            false,
        );
        if !working.compile_errors.is_empty()
            || working.parse_errors.iter().any(|error| {
                !matches!(
                    error,
                    ParseError::MissingPositional(..) | ParseError::MissingRequiredFlag(..)
                )
            })
        {
            return Ok(None);
        }
        let Some(Expr::Call(call)) = block
            .pipelines
            .first()
            .and_then(|pipeline| pipeline.elements.first())
            .map(|element| &element.expr.expr)
        else {
            return Ok(None);
        };
        let declaration = working.get_decl(call.decl_id);
        if declaration.name() != "main" && !declaration.name().starts_with("main ") {
            return Ok(None);
        }
        let Some(block_id) = declaration.block_id() else {
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
