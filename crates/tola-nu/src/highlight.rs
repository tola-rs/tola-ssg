//! The `__tola_highlight` command: ANSI source text for terminal previews.

use nu_engine::CallExt;
use nu_protocol::{
    Category, IntoPipelineData, PipelineData, ShellError, Signature, SyntaxShape, Type, Value,
    engine::{Call, Command, EngineState, Stack},
};
use tola_highlight::{ThemeChoice, render};

pub(crate) fn declarations() -> Vec<Box<dyn Command>> {
    vec![Box::new(Highlight)]
}

#[derive(Clone)]
struct Highlight;

impl Command for Highlight {
    fn name(&self) -> &str {
        "__tola_highlight"
    }

    fn description(&self) -> &str {
        "Highlight source text for a terminal preview."
    }

    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .input_output_types(vec![
                (Type::String, Type::String),
                (Type::Binary, Type::String),
            ])
            .required(
                "language",
                SyntaxShape::String,
                "Language token or file extension, for example rust, toml, or typ.",
            )
            .named(
                "theme",
                SyntaxShape::String,
                "dark, light, or plain. Defaults to dark.",
                None,
            )
            .category(Category::Formats)
    }

    fn run(
        &self,
        engine: &EngineState,
        stack: &mut Stack,
        call: &Call,
        input: PipelineData,
    ) -> Result<PipelineData, ShellError> {
        engine.signals().check(&call.head)?;
        let language: String = call.req(engine, stack, 0)?;
        let theme = call
            .get_flag::<String>(engine, stack, "theme")?
            .as_deref()
            .and_then(ThemeChoice::parse)
            .unwrap_or(ThemeChoice::Dark);
        let text = match input {
            PipelineData::ByteStream(stream, _) => {
                String::from_utf8_lossy(&stream.into_bytes()?).into_owned()
            }
            PipelineData::Value(Value::Binary { val, .. }, _) => {
                String::from_utf8_lossy(&val).into_owned()
            }
            other => other.into_value(call.head)?.coerce_string()?,
        };
        engine.signals().check(&call.head)?;
        Ok(Value::string(render(&text, &language, theme), call.head).into_pipeline_data())
    }
}
