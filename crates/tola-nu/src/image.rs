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
    vec![Box::new(Metadata), Box::new(Resize)]
}

#[derive(Clone)]
struct Metadata;

impl Command for Metadata {
    fn name(&self) -> &str {
        "__tola_image_metadata"
    }
    fn description(&self) -> &str {
        "Inspect image metadata from a path or pipeline bytes."
    }
    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .input_output_types(vec![
                (Type::Nothing, Type::record()),
                (Type::Binary, Type::record()),
            ])
            .optional(
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
        input: PipelineData,
    ) -> Result<PipelineData, ShellError> {
        let source = call.opt::<Option<String>>(engine, stack, 0)?.flatten();
        let bytes = image_input(source.as_deref(), input, engine, call.head)?;
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
        "Resize a raster image from a path or pipeline bytes."
    }
    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .input_output_types(vec![(Type::Nothing, Type::Any), (Type::Binary, Type::Any)])
            .required(
                "options",
                SyntaxShape::Record(vec![].into()),
                "Image recipe options.",
            )
            .optional(
                "source",
                SyntaxShape::Filepath,
                "Expanded source image path.",
            )
            .optional(
                "output",
                SyntaxShape::Filepath,
                "Expanded new destination path.",
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
        let request = resize_request(&options)?;
        let source = call.opt::<Option<String>>(engine, stack, 1)?.flatten();
        let destination = call.opt::<Option<String>>(engine, stack, 2)?.flatten();
        let bytes = image_input(source.as_deref(), input, engine, call.head)?;
        let output = destination
            .map(|path| Ok::<_, ShellError>((temporary_output(&path, call.head)?, path)))
            .transpose()?;
        let (pixels, recipe) = resize_pixels(bytes, request, engine, call.head)?;
        if let Some((file, path)) = output {
            return write_file(file, path, source, &pixels, &recipe, engine, call.head);
        }
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

fn image_input(
    source: Option<&str>,
    input: PipelineData,
    engine: &EngineState,
    span: Span,
) -> Result<Vec<u8>, ShellError> {
    engine.signals().check(&span)?;
    match (source, input.is_nothing()) {
        (Some(source), true) => source_bytes(source, engine, span),
        (None, false) => binary_input(input, span),
        _ => Err(image_error(
            "provide a source path or pipe image bytes, but not both",
            span,
        )),
    }
}

fn temporary_output(destination: &str, span: Span) -> Result<tempfile::NamedTempFile, ShellError> {
    let parent = Path::new(destination).parent().ok_or_else(|| {
        image_error(
            "choose a destination file inside an existing directory",
            span,
        )
    })?;
    tempfile::NamedTempFile::new_in(parent).map_err(|_| {
        image_error(
            "the destination directory is missing or not writable; choose an existing writable directory",
            span,
        )
    })
}

fn write_file(
    mut output: tempfile::NamedTempFile,
    destination: String,
    source: Option<String>,
    pixels: &ResampledPixels,
    recipe: &ImageRecipe,
    engine: &EngineState,
    span: Span,
) -> Result<PipelineData, ShellError> {
    let cancelled = || engine.signals().interrupted();
    let metadata = {
        let mut writer = BufWriter::new(output.as_file_mut());
        let metadata = pixels
            .encode_to(&mut writer, recipe, &cancelled)
            .map_err(|error| render_error(error, span))?;
        writer.flush().map_err(|_| {
            image_error(
                "the resized image could not be written; check available disk space and destination permissions",
                span,
            )
        })?;
        metadata
    };
    engine.signals().check(&span)?;
    output.persist_noclobber(&destination).map_err(|error| {
        let message = if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            "the destination already exists; choose a new destination file"
        } else {
            "the resized image could not be saved; check the destination directory permissions"
        };
        image_error(message, span)
    })?;
    let mut metadata = metadata_value(&metadata, span).into_record()?;
    metadata.push(
        "source",
        source.map_or_else(|| Value::nothing(span), |path| Value::string(path, span)),
    );
    metadata.push("destination", Value::string(destination, span));
    Ok(Value::record(metadata, span).into_pipeline_data())
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

struct ResizeRequest {
    options: ResizeOptions,
    keep_size: bool,
}

fn resize_pixels(
    bytes: Vec<u8>,
    request: ResizeRequest,
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
    let mut options = request.options;
    if request.keep_size {
        options.width = Some(source.width);
        options.height = Some(source.height);
    }
    let recipe = ImageRecipe::resolve(&source, options)
        .map_err(|error| image_error(error.to_string(), span))?;
    let cancelled = || engine.signals().interrupted();
    let decoded =
        DecodedSource::open(&bytes, &cancelled).map_err(|error| render_error(error, span))?;
    drop(bytes);
    let pixels = decoded
        .into_pixels(recipe.pixels(), &cancelled)
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

/// Inspect encoded image bytes and return their metadata record.
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

fn resize_request(options: &Record) -> Result<ResizeRequest, ShellError> {
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
    let width = positive_dimension(options, "width")?;
    let height = positive_dimension(options, "height")?;
    let keep_size = width.is_none() && height.is_none() && field(options, "op").is_none();
    let default_operation = match (width, height) {
        (None, None) => ResizeOperation::Scale,
        (Some(_), None) => ResizeOperation::FitWidth,
        (None, Some(_)) => ResizeOperation::FitHeight,
        (Some(_), Some(_)) => ResizeOperation::Fill,
    };
    let options = ResizeOptions {
        width,
        height,
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
            default_operation,
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
    Ok(ResizeRequest { options, keep_size })
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
        let mut stack = Stack::new();
        stack.add_env_var(
            "PWD".into(),
            Value::string(env!("CARGO_MANIFEST_DIR"), Span::test_data()),
        );
        nu_engine::eval_subexpression::<WithoutDebug>(
            &engine,
            &mut stack,
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
            &format!("{} | image resize --width 4 --format png", png_input()),
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
    fn format_only_keeps_source_dimensions() {
        let bytes = value(&format!("{} | image resize --format webp", png_input()))
            .into_binary()
            .unwrap();

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.png");
        let destination = directory.path().join("converted.webp");
        fs::write(&source, PNG).unwrap();
        let source_path = source.to_str().unwrap();
        let info = value(&format!("image metadata '{source_path}'"));
        assert_eq!(
            info.as_record()
                .unwrap()
                .get("width")
                .unwrap()
                .as_int()
                .unwrap(),
            2
        );
        let from_path = value(&format!("image resize '{source_path}' --format webp"))
            .into_binary()
            .unwrap();
        assert_eq!(from_path, bytes);

        let piped = directory.path().join("piped.webp");
        let piped_path = piped.to_str().unwrap();
        let piped_resize =
            format!("open --raw '{source_path}' | image resize --format webp -o '{piped_path}'");
        let info = value(&piped_resize);
        let info = info.as_record().unwrap();
        assert!(info.get("source").unwrap().is_nothing());
        assert_eq!(
            info.get("destination").unwrap().as_str().unwrap(),
            piped_path
        );
        assert_eq!(fs::read(&piped).unwrap(), bytes);
        assert!(eval(&piped_resize, Signals::empty()).is_err());
        assert_eq!(fs::read(piped).unwrap(), bytes);

        let runtime = crate::Runtime::new(Vec::new(), Vec::new())
            .unwrap()
            .with_cancellation(Arc::new(AtomicBool::new(false)));
        let entry = runtime.resolve_locator("@tola-scripts/image").unwrap();
        let args = [
            "resize".into(),
            source.to_str().unwrap().into(),
            "--output".into(),
            destination.to_str().unwrap().into(),
            "--format".into(),
            "webp".into(),
        ];
        assert_eq!(runtime.run_module(&entry, &args, false, None), 0);

        for bytes in [bytes, fs::read(destination).unwrap()] {
            let image = tola_image::inspect(&bytes).unwrap();
            assert_eq!((image.width, image.height), (2, 1));
            assert_eq!(image.format, ImageFormat::WebP);
        }
    }

    #[test]
    fn module_rejects_empty_damaged_and_nonsensical_requests() {
        for command in ["image metadata", "image resize"] {
            assert!(eval(command, Signals::empty()).is_err());
            let conflict = format!("{} | {command} 'unused.png'", png_input());
            let error = eval(&conflict, Signals::empty()).unwrap_err();
            assert!(format!("{error:?}").contains("pipe image bytes, but not both"));
        }
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
        for name in [
            "--output",
            "--width",
            "--height",
            "--op",
            "--quality",
            "--background",
        ] {
            assert!(help.contains(name), "missing {name}: {help}");
        }
        let (engine, _) = parsed("null", Signals::empty());
        for (name, expected_output) in [
            ("image metadata", Type::record()),
            ("image resize", Type::Any),
        ] {
            let declaration = engine
                .find_decl(name.as_bytes(), &[])
                .expect("the module exports this command");
            let signature = engine.get_decl(declaration).signature();
            assert_eq!(
                signature.input_output_types,
                vec![
                    (Type::Nothing, expected_output.clone()),
                    (Type::Binary, expected_output)
                ],
                "{name}"
            );
        }
        let resize = engine
            .get_decl(engine.find_decl(b"image resize", &[]).unwrap())
            .signature();
        for (name, shape) in [
            ("output", SyntaxShape::Filepath),
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
        let mut names: Vec<_> = public
            .as_list()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["image metadata", "image resize"]);
        assert!(
            !value(
                "scope commands | any {|command| $command.name | str starts-with '__tola_image_' }"
            )
            .as_bool()
            .unwrap()
        );
    }
}
