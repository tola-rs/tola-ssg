//! Image byte and file operations. Site publication belongs to the build engine.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use nu_engine::CallExt;
use nu_protocol::{
    ByteStream, Category, IntoPipelineData, PipelineData, PipelineMetadata, Record, ShellError,
    Signature, Span, SyntaxShape, Type, Value,
    engine::{Call, Command, EngineState, Stack},
    record,
    shell_error::generic::GenericError,
};
use tola_image::{
    Background, CropAnchor, DecodedSource, ImageFormat, ImageMetadata, ImageRecipe, OutputFormat,
    ResampledPixels, ResizeFilter, ResizeOperation, ResizeOptions,
};

pub(crate) fn declarations() -> Vec<Box<dyn Command>> {
    vec![
        Box::new(Metadata),
        Box::new(Resize),
        Box::new(MetadataFile),
        Box::new(ResizeFile),
    ]
}

#[derive(Clone)]
struct Metadata;

impl Command for Metadata {
    fn name(&self) -> &str {
        "__tola_image_metadata"
    }
    fn description(&self) -> &str {
        "Inspect encoded image bytes with Tola's shared image service."
    }
    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .input_output_types(vec![(Type::Binary, Type::record())])
            .category(Category::Formats)
    }
    fn run(
        &self,
        engine: &EngineState,
        _: &mut Stack,
        call: &Call,
        input: PipelineData,
    ) -> Result<PipelineData, ShellError> {
        engine.signals().check(&call.head)?;
        let bytes = binary_input(input, call.head)?;
        metadata_result(&bytes, engine, call.head)
    }
}

#[derive(Clone)]
struct Resize;

impl Command for Resize {
    fn name(&self) -> &str {
        "__tola_image_resize"
    }
    fn description(&self) -> &str {
        "Resize raster bytes with Tola's shared image recipe and renderer."
    }
    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .input_output_types(vec![(Type::Binary, Type::Binary)])
            .required(
                "options",
                SyntaxShape::Record(vec![].into()),
                "Image recipe options.",
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
        let options: Record = call.req(engine, stack, 0)?;
        let options = resize_options(&options)?;
        engine.signals().check(&call.head)?;
        let bytes = binary_input(input, call.head)?;
        let (pixels, recipe) = resize_pixels(bytes, options, engine, call.head)?;
        let cancelled = || engine.signals().interrupted();
        let mut output = Vec::new();
        let metadata = pixels
            .encode_to(&mut output, &recipe, &cancelled)
            .map_err(|error| render_error(error, call.head))?;
        drop(pixels);
        engine.signals().check(&call.head)?;
        Ok(PipelineData::ByteStream(
            ByteStream::read_binary(output, call.head, engine.signals().clone()),
            Some(PipelineMetadata {
                content_type: Some(metadata.format.mime().into()),
                custom: record! { "tola_image" => metadata_value(&metadata, call.head) },
                ..PipelineMetadata::default()
            }),
        ))
    }
}

#[derive(Clone)]
struct MetadataFile;

impl Command for MetadataFile {
    fn name(&self) -> &str {
        "__tola_image_metadata_file"
    }

    fn description(&self) -> &str {
        "Inspect an image file."
    }

    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .input_output_types(vec![(Type::Nothing, Type::record())])
            .required(
                "source",
                SyntaxShape::Filepath,
                "Expanded source image path.",
            )
            .category(Category::Formats)
    }

    fn run(
        &self,
        engine: &EngineState,
        stack: &mut Stack,
        call: &Call,
        _: PipelineData,
    ) -> Result<PipelineData, ShellError> {
        let source: String = call.req(engine, stack, 0)?;
        let bytes = source_bytes(&source, engine, call.head)?;
        metadata_result(&bytes, engine, call.head)
    }
}

#[derive(Clone)]
struct ResizeFile;

impl Command for ResizeFile {
    fn name(&self) -> &str {
        "__tola_image_resize_file"
    }

    fn description(&self) -> &str {
        "Resize an image into a new destination file."
    }

    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .input_output_types(vec![(Type::Nothing, Type::record())])
            .required(
                "source",
                SyntaxShape::Filepath,
                "Expanded source image path.",
            )
            .required(
                "destination",
                SyntaxShape::Filepath,
                "Expanded new destination path.",
            )
            .required(
                "options",
                SyntaxShape::Record(vec![].into()),
                "Image recipe options.",
            )
            .category(Category::Formats)
    }

    fn run(
        &self,
        engine: &EngineState,
        stack: &mut Stack,
        call: &Call,
        _: PipelineData,
    ) -> Result<PipelineData, ShellError> {
        let source: String = call.req(engine, stack, 0)?;
        let destination: String = call.req(engine, stack, 1)?;
        let options: Record = call.req(engine, stack, 2)?;
        let options = resize_options(&options)?;
        let bytes = source_bytes(&source, engine, call.head)?;
        let (pixels, recipe) = resize_pixels(bytes, options, engine, call.head)?;
        let parent = Path::new(&destination).parent().ok_or_else(|| {
            image_error(
                "choose a destination file inside an existing directory",
                call.head,
            )
        })?;
        let mut output = tempfile::NamedTempFile::new_in(parent).map_err(|_| {
            image_error(
                "the destination directory is missing or not writable; choose an existing writable directory",
                call.head,
            )
        })?;
        let cancelled = || engine.signals().interrupted();
        let metadata = {
            let mut writer = BufWriter::new(output.as_file_mut());
            let metadata = pixels
                .encode_to(&mut writer, &recipe, &cancelled)
                .map_err(|error| render_error(error, call.head))?;
            writer.flush().map_err(|_| {
                image_error(
                    "the resized image could not be written; check available disk space and destination permissions",
                    call.head,
                )
            })?;
            metadata
        };
        engine.signals().check(&call.head)?;
        output.persist_noclobber(&destination).map_err(|error| {
            let message = if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                "the destination already exists; choose a new destination file"
            } else {
                "the resized image could not be saved; check the destination directory permissions"
            };
            image_error(message, call.head)
        })?;
        let mut metadata = metadata_value(&metadata, call.head).into_record()?;
        metadata.push("source", Value::string(source, call.head));
        metadata.push("destination", Value::string(destination, call.head));
        Ok(Value::record(metadata, call.head).into_pipeline_data())
    }
}

fn source_bytes(source: &str, engine: &EngineState, span: Span) -> Result<Vec<u8>, ShellError> {
    engine.signals().check(&span)?;
    let bytes = fs::read(source);
    engine.signals().check(&span)?;
    bytes.map_err(|_| {
        image_error(
            "the source image could not be read; check its path and permissions",
            span,
        )
    })
}

fn resize_pixels(
    bytes: Vec<u8>,
    options: ResizeOptions,
    engine: &EngineState,
    span: Span,
) -> Result<(ResampledPixels, ImageRecipe), ShellError> {
    let source = tola_image::inspect(&bytes).map_err(|error| render_error(error, span))?;
    engine.signals().check(&span)?;
    if source.format == ImageFormat::Svg {
        return Err(image_error(
            "SVG resizing requires the build engine's font and resource context; image resize supports raster images only",
            span,
        ));
    }
    let recipe = ImageRecipe::resolve(&source, options)
        .map_err(|error| image_error(error.to_string(), span))?;
    let cancelled = || engine.signals().interrupted();
    let decoded =
        DecodedSource::open(&bytes, &cancelled).map_err(|error| render_error(error, span))?;
    drop(bytes);
    let pixels = decoded
        .pixels(recipe.pixels(), &cancelled)
        .map_err(|error| render_error(error, span))?;
    Ok((pixels, recipe))
}

fn render_error(error: tola_image::ImageError, span: Span) -> ShellError {
    match error {
        tola_image::ImageError::Cancelled(_) => ShellError::Interrupted { span },
        error => image_error(error.to_string(), span),
    }
}

fn binary_input(input: PipelineData, span: Span) -> Result<Vec<u8>, ShellError> {
    match input {
        PipelineData::ByteStream(stream, _) => stream.into_bytes(),
        other => other.into_value(span)?.into_binary(),
    }
}

/// Inspect encoded image bytes and answer the record both metadata commands publish.
fn metadata_result(
    bytes: &[u8],
    engine: &EngineState,
    span: Span,
) -> Result<PipelineData, ShellError> {
    let metadata = tola_image::inspect(bytes).map_err(|error| render_error(error, span))?;
    engine.signals().check(&span)?;
    Ok(metadata_value(&metadata, span).into_pipeline_data())
}

fn metadata_value(metadata: &ImageMetadata, span: Span) -> Value {
    Value::record(
        record! {
            "width" => Value::int(i64::from(metadata.width), span),
            "height" => Value::int(i64::from(metadata.height), span),
            "format" => Value::string(metadata.format.extension(), span),
            "mime" => Value::string(metadata.format.mime(), span),
            "has-alpha" => Value::bool(metadata.has_alpha, span),
            "is-lossy" => Value::bool(metadata.is_lossy, span),
        },
        span,
    )
}

fn image_error(message: impl Into<String>, span: Span) -> ShellError {
    GenericError::new("Image operation failed", message.into(), span)
        .with_code("tola::image")
        .into()
}

fn field<'a>(options: &'a Record, name: &str) -> Option<&'a Value> {
    options
        .get(name)
        .filter(|value| !matches!(value, Value::Nothing { .. }))
}

fn positive_dimension(options: &Record, name: &str) -> Result<Option<u32>, ShellError> {
    field(options, name)
        .map(|value| {
            let number = value.as_int()?;
            u32::try_from(number)
                .ok()
                .filter(|number| *number > 0)
                .ok_or_else(|| {
                    image_error(
                        format!(
                            "--{name} must be a positive integer no larger than {}",
                            u32::MAX
                        ),
                        value.span(),
                    )
                })
        })
        .transpose()
}

fn choice<T: Copy>(
    options: &Record,
    name: &str,
    choices: &[(&str, T)],
    default: T,
) -> Result<T, ShellError> {
    let Some(value) = field(options, name) else {
        return Ok(default);
    };
    let selected = value.as_str()?;
    choices
        .iter()
        .find(|(name, _)| *name == selected)
        .map(|(_, value)| *value)
        .ok_or_else(|| {
            image_error(
                format!(
                    "--{name} must be one of: {}",
                    choices
                        .iter()
                        .map(|(name, _)| *name)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                value.span(),
            )
        })
}

fn resize_options(options: &Record) -> Result<ResizeOptions, ShellError> {
    let quality = field(options, "quality")
        .map(|value| {
            let quality = value.as_int()?;
            u8::try_from(quality)
                .ok()
                .filter(|quality| *quality <= 100)
                .ok_or_else(|| {
                    image_error(
                        "--quality must be between 0 and 100 (JPEG requires at least 1)",
                        value.span(),
                    )
                })
        })
        .transpose()?;
    let background = field(options, "background")
        .map(|value| {
            let text = value.as_str()?;
            let rgb = text
                .strip_prefix('#')
                .filter(|rgb| rgb.len() == 6 && rgb.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .and_then(|rgb| u32::from_str_radix(rgb, 16).ok())
                .ok_or_else(|| {
                    image_error(
                        "--background must be an opaque sRGB color in #RRGGBB form",
                        value.span(),
                    )
                })?;
            Ok::<_, ShellError>(Background {
                red: (rgb >> 16) as u8,
                green: (rgb >> 8) as u8,
                blue: rgb as u8,
            })
        })
        .transpose()?;
    let options = ResizeOptions {
        width: positive_dimension(options, "width")?,
        height: positive_dimension(options, "height")?,
        operation: choice(
            options,
            "op",
            &[
                ("scale", ResizeOperation::Scale),
                ("fit-width", ResizeOperation::FitWidth),
                ("fit-height", ResizeOperation::FitHeight),
                ("fit", ResizeOperation::Fit),
                ("fill", ResizeOperation::Fill),
            ],
            ResizeOperation::Fill,
        )?,
        anchor: choice(
            options,
            "anchor",
            &[
                ("center", CropAnchor::Center),
                ("top", CropAnchor::Top),
                ("bottom", CropAnchor::Bottom),
                ("left", CropAnchor::Left),
                ("right", CropAnchor::Right),
                ("top-left", CropAnchor::TopLeft),
                ("top-right", CropAnchor::TopRight),
                ("bottom-left", CropAnchor::BottomLeft),
                ("bottom-right", CropAnchor::BottomRight),
            ],
            CropAnchor::Center,
        )?,
        format: choice(
            options,
            "format",
            &[
                ("auto", OutputFormat::Auto),
                ("jpeg", OutputFormat::Jpeg),
                ("jpg", OutputFormat::Jpeg),
                ("png", OutputFormat::Png),
                ("webp", OutputFormat::WebP),
            ],
            OutputFormat::Auto,
        )?,
        filter: choice(
            options,
            "filter",
            &[
                ("nearest", ResizeFilter::Nearest),
                ("triangle", ResizeFilter::Triangle),
                ("catmull-rom", ResizeFilter::CatmullRom),
                ("gaussian", ResizeFilter::Gaussian),
                ("lanczos3", ResizeFilter::Lanczos3),
            ],
            ResizeFilter::Lanczos3,
        )?,
        quality,
        background,
    };
    // The shared resolver validates each operation's required dimensions and codec quality.
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nu_protocol::{Signals, debugger::WithoutDebug, engine::StateWorkingSet};
    use std::sync::{Arc, atomic::AtomicBool};

    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 244, 34, 127, 138, 0, 0, 0, 15, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240,
        31, 8, 27, 0, 16, 121, 3, 126, 125, 99, 206, 215, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
        130,
    ];

    fn parsed(command: &str, signals: Signals) -> (EngineState, Arc<nu_protocol::ast::Block>) {
        let engine = nu_cmd_lang::add_default_context(EngineState::new());
        let mut engine = nu_command::add_shell_command_context(engine);
        engine.set_signals(signals);
        let mut working = StateWorkingSet::new(&engine);
        working.enter_scope();
        for command in declarations() {
            working.add_decl(command);
        }
        let core = format!("use '{}/nu/image_core.nu'", env!("CARGO_MANIFEST_DIR"));
        nu_parser::parse(
            &mut working,
            Some("image-core-test.nu"),
            core.as_bytes(),
            false,
        );
        assert!(
            working.parse_errors.is_empty(),
            "{:?}",
            working.parse_errors
        );
        working.exit_scope();
        let source = format!(
            "use '{}/nu/image.nu'\n{}",
            env!("CARGO_MANIFEST_DIR"),
            command
        );
        let block = nu_parser::parse(
            &mut working,
            Some("image-test.nu"),
            source.as_bytes(),
            false,
        );
        assert!(
            working.parse_errors.is_empty(),
            "{:?}",
            working.parse_errors
        );
        engine.merge_delta(working.render()).unwrap();
        (engine, block)
    }

    fn eval(command: &str, signals: Signals) -> Result<PipelineData, ShellError> {
        let (engine, block) = parsed(command, signals);
        nu_engine::eval_subexpression::<WithoutDebug>(
            &engine,
            &mut Stack::new(),
            &block,
            PipelineData::empty(),
        )
    }

    fn png_input() -> String {
        format!(
            "0x[{}]",
            PNG.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ")
        )
    }

    fn value(command: &str) -> Value {
        eval(command, Signals::empty())
            .unwrap()
            .into_value(Span::test_data())
            .unwrap()
    }

    #[test]
    fn module_metadata_and_resize_use_encoded_pixels() {
        let info = value(&format!("{} | image metadata", png_input()));
        let info = info.as_record().unwrap();
        assert_eq!(info.get("width").unwrap().as_int().unwrap(), 2);
        assert_eq!(info.get("height").unwrap().as_int().unwrap(), 1);
        assert_eq!(info.get("mime").unwrap().as_str().unwrap(), "image/png");
        assert!(info.get("has-alpha").unwrap().as_bool().unwrap());
        let output = eval(
            &format!(
                "{} | image resize --width 4 --op fit-width --format png",
                png_input()
            ),
            Signals::empty(),
        )
        .unwrap();
        assert!(matches!(&output, PipelineData::ByteStream(..)));
        assert_eq!(
            output.metadata_ref().unwrap().content_type.as_deref(),
            Some("image/png")
        );
        let bytes = output
            .into_value(Span::test_data())
            .unwrap()
            .into_binary()
            .unwrap();
        let resized = tola_image::inspect(&bytes).unwrap();
        assert_eq!((resized.width, resized.height), (4, 2));
        assert_eq!(resized.format, ImageFormat::Png);
    }

    #[test]
    fn module_rejects_empty_damaged_and_nonsensical_requests() {
        for input in ["0x[]", "0x[00 01 02]"] {
            assert!(eval(&format!("{input} | image metadata"), Signals::empty()).is_err());
        }
        for arguments in [
            "--width 0 --op fit-width",
            "--height -1 --op fit-height",
            "--width 4294967296 --op fit-width",
            "--width 2 --op unknown",
            "--width 2 --op fit-width --filter unknown",
            "--width 2 --op fit-width --anchor unknown",
            "--width 2 --op fit-width --format gif",
            "--width 2 --op fit-width --quality 101",
            "--width 2 --op fit-width --format jpeg --quality 0",
            "--width 2 --op fit-width --background '#FFFFFF00'",
            "--op fill",
        ] {
            let result = eval(
                &format!("{} | image resize {arguments}", png_input()),
                Signals::empty(),
            );
            assert!(result.is_err(), "request was accepted: {arguments}");
        }
    }

    #[test]
    fn svg_metadata_but_resize_needs_context() {
        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"4\"/>";
        let literal = format!(
            "0x[{}]",
            svg.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let info = value(&format!("{literal} | image metadata"));
        assert_eq!(
            info.as_record()
                .unwrap()
                .get("format")
                .unwrap()
                .as_str()
                .unwrap(),
            "svg"
        );
        let error = eval(
            &format!("{literal} | image resize --width 4 --op fit-width"),
            Signals::empty(),
        )
        .unwrap_err();
        assert!(format!("{error:?}").contains("font and resource context"));
    }

    #[test]
    fn interrupted_resize_is_a_nushell_interrupt() {
        let signals = Signals::new(Arc::new(AtomicBool::new(true)));
        let error = eval(
            &format!("{} | image resize --width 4 --op fit-width", png_input()),
            signals,
        )
        .unwrap_err();
        assert!(matches!(error, ShellError::Interrupted { .. }));
    }

    #[test]
    fn module_help_and_signature_are_public_nushell_commands() {
        let help = value("help image resize");
        let help = help.as_str().unwrap();
        for name in ["--width", "--height", "--op", "--quality", "--background"] {
            assert!(help.contains(name), "missing {name}: {help}");
        }
        let (engine, _) = parsed("null", Signals::empty());
        for (name, expected_input, expected_output) in [
            ("image metadata", Type::Binary, Type::record()),
            ("image resize", Type::Binary, Type::Binary),
            ("image metadata-file", Type::Nothing, Type::record()),
            ("image resize-file", Type::Nothing, Type::record()),
        ] {
            let declaration = engine
                .find_decl(name.as_bytes(), &[])
                .expect("the module exports this command");
            let signature = engine.get_decl(declaration).signature();
            assert_eq!(
                signature.input_output_types,
                vec![(expected_input, expected_output)],
                "{name}"
            );
        }
        let resize = engine
            .get_decl(engine.find_decl(b"image resize", &[]).unwrap())
            .signature();
        for (name, shape) in [
            ("width", SyntaxShape::Int),
            ("height", SyntaxShape::Int),
            ("quality", SyntaxShape::Int),
            ("background", SyntaxShape::String),
        ] {
            assert_eq!(
                resize
                    .named
                    .iter()
                    .find(|flag| flag.long == name)
                    .unwrap()
                    .arg
                    .as_ref(),
                Some(&shape),
                "--{name}"
            );
        }
        let public = value("scope commands | where name =~ '^image ' | get name");
        let names: Vec<_> = public
            .as_list()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        assert!(names.contains(&"image metadata"));
        assert!(names.contains(&"image resize"));
        assert!(!names.contains(&"image main resize"));
        assert!(
            !value(
                "scope commands | any {|command| $command.name | str starts-with '__tola_image_' }"
            )
            .as_bool()
            .unwrap()
        );
    }
}
