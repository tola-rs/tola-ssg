use nu_engine::CallExt;
use nu_protocol::{
    Category, OutDest, PipelineData, ShellError, Signature, SyntaxShape, Type, Value,
    engine::{Call, Command, EngineState, Redirection, Stack},
    ir,
    shell_error::generic::GenericError,
};

#[derive(Clone)]
pub(crate) struct Capture;

impl Command for Capture {
    fn name(&self) -> &str {
        "__tola_cli_capture"
    }

    fn description(&self) -> &str {
        "Capture finite Tola CLI output and its exit code in the current input scope."
    }

    fn extra_description(&self) -> &str {
        "Pass arguments as a string list, including --config where the target command accepts it. Nonzero exit codes are returned without terminating the script. Pipeline input follows Nu's external-command behavior; use native externals for interactive or continuous output."
    }

    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .required(
                "argv",
                SyntaxShape::List(Box::new(SyntaxShape::String)),
                "Tola command and arguments, preserving each argument boundary.",
            )
            .input_output_types(vec![(Type::Any, Type::record())])
            .category(Category::System)
    }

    fn run(
        &self,
        engine: &EngineState,
        stack: &mut Stack,
        call: &Call,
        input: PipelineData,
    ) -> Result<PipelineData, ShellError> {
        engine.signals().check(&call.head)?;
        let argv: Vec<String> = call.req(engine, stack, 0)?;
        let executable = stack
            .get_env_var(engine, "TOLA_EXECUTABLE")
            .ok_or_else(|| {
                ShellError::Generic(GenericError::new(
                    "Tola executable is unavailable",
                    "run the script with `tola nu`",
                    call.head,
                ))
            })?
            .as_str()?
            .to_owned();
        let scope = stack
            .get_env_var(engine, "TOLA_INPUT_SCOPE")
            .map(Value::as_str)
            .transpose()?;
        let scope_flag = match scope {
            None | Some("online") => None,
            Some("offline") => Some("--offline"),
            Some("pure") => Some("--pure"),
            Some(_) => {
                return Err(ShellError::Generic(GenericError::new(
                    "unknown Tola input scope",
                    "set `TOLA_INPUT_SCOPE` to `online`, `offline`, or `pure`",
                    call.head,
                )));
            }
        };
        let external_id = engine
            .find_decl(b"run-external", &[])
            .ok_or(ShellError::ExternalNotSupported { span: call.head })?;
        let mut external = ir::Call::build(external_id, call.head);
        external.add_positional(stack, call.head, Value::string(executable, call.head));
        if let Some(flag) = scope_flag {
            external.add_positional(stack, call.head, Value::string(flag, call.head));
        }
        for arg in argv {
            external.add_positional(stack, call.head, Value::string(arg, call.head));
        }
        let output = external.with(stack, |stack, external| {
            let mut stack = stack.push_redirection(
                Some(Redirection::Pipe(OutDest::PipeSeparate)),
                Some(Redirection::Pipe(OutDest::PipeSeparate)),
            );
            // Keep Empty and ByteStream intact: Nu owns stdin forwarding and child cancellation.
            nu_command::External.run(engine, &mut stack, external, input)
        })?;
        let captured = nu_command::Complete.run(engine, stack, &Call::new(call.head), output)?;
        engine.signals().check(&call.head)?;
        Ok(captured)
    }
}
