//! Image byte and file operations. Site publication belongs to the build engine.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use nu_engine::CallExt;
use nu_protocol::{
    ByteStream, Category, IntoPipelineData, PipelineData, PipelineMetadata, Record, ShellError,
    Signature, Span, SyntaxShape, Type, Value,
    engine::{Call, Command, CommandType, EngineState, Stack},
    record,
    shell_error::generic::GenericError,
};
use tola_image::{
    Background, CropAnchor, DecodedSource, ImageMetadata, ImageRecipe, OutputFormat,
    ResampledPixels, ResizeFilter, ResizeOperation, ResizeOptions, SvgRender,
};
use tola_typst::svg::looks_like_svg;
use tola_typst::typst::foundations::Bytes;
use tola_typst::{BundleCancellation, FontStore, SvgSource};

pub(crate) fn declarations(host: Arc<OnceLock<crate::Host>>) -> Vec<Box<dyn Command>> {
    vec![
        Box::new(Metadata),
        Box::new(Resize {
            host,
            fonts: Arc::default(),
        }),
    ]
}

#[derive(Clone)]
struct Metadata;

impl Command for Metadata {
    fn command_type(&self) -> CommandType {
        CommandType::Custom
    }

    fn name(&self) -> &str {
        "inspect-image"
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
        let src = opt_str(call, engine, stack, 0)?;
        let bytes = image_input(src.as_deref(), input, engine, call.head)?;
        metadata_result(&bytes, engine, call.head)
    }
}

#[derive(Clone)]
struct Resize {
    host: Arc<OnceLock<crate::Host>>,
    fonts: Arc<FontStore>,
}

impl Resize {
    fn svg_fonts(&self, span: Span) -> Result<&Arc<FontStore>, ShellError> {
        Ok(self
            .host
            .get()
            .and_then(|host| host.svg_fonts.as_ref())
            .map(Result::as_ref)
            .transpose()
            .map_err(|err| image_error(err.as_str(), span))?
            .unwrap_or(&self.fonts))
    }

    fn svg_pixels(
        &self,
        bytes: Vec<u8>,
        req: ResizeRequest,
        engine: &EngineState,
        stack: &Stack,
        span: Span,
    ) -> Result<(ResampledPixels, ImageRecipe), ShellError> {
        let fonts = self.svg_fonts(span)?;
        let cancel = engine
            .signals()
            .interrupt_flag()
            .map_or_else(BundleCancellation::new, BundleCancellation::from_flag);
        let dir = engine.cwd(Some(stack))?;
        let svg =
            SvgSource::parse_with_fonts(Bytes::new(bytes), dir.as_ref(), fonts.clone(), &cancel);
        engine.signals().check(&span)?;
        let svg = svg.map_err(|err| image_error(err.to_string(), span))?;
        let (w, h) = svg.pixel_dimensions();
        let recipe = ImageRecipe::resolve_svg(
            (svg.width(), svg.height()),
            req.with_source_size(w, h),
            SvgRender::new(svg.hash()),
        )
        .map_err(|err| image_error(err.to_string(), span))?;
        let px = recipe.pixels();
        let crop = px
            .vector_geometry()
            .expect("an SVG recipe has a source rectangle")
            .crop();
        engine.signals().check(&span)?;
        let rgba = svg.render(px.width(), px.height(), Some(crop.into()));
        engine.signals().check(&span)?;
        let rgba = rgba.map_err(|err| image_error(err.to_string(), span))?;
        let cancelled = || engine.signals().interrupted();
        let rendered = ResampledPixels::from_rendered(px, rgba, &cancelled)
            .map_err(|err| render_error(err, span))?;
        Ok((rendered, recipe))
    }
}

impl Command for Resize {
    fn command_type(&self) -> CommandType {
        CommandType::Custom
    }

    fn name(&self) -> &str {
        "resize-image"
    }
    fn description(&self) -> &str {
        "Resize an image from a path or pipeline bytes."
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
        let span = call.head;
        let opts: Record = call.req(engine, stack, 0)?;
        let req = resize_request(&opts)?;
        let src = opt_str(call, engine, stack, 1)?;
        let dst = opt_str(call, engine, stack, 2)?;
        let bytes = image_input(src.as_deref(), input, engine, span)?;
        let out_file = dst
            .map(|path| Ok::<_, ShellError>((temporary_output(&path, span)?, path)))
            .transpose()?;
        let (px, recipe) = if looks_like_svg(&bytes) {
            self.svg_pixels(bytes, req, engine, stack, span)?
        } else {
            resize_pixels(bytes, req, engine, span)?
        };
        if let Some((file, path)) = out_file {
            return write_file(file, path, src, &px, &recipe, engine, span);
        }
        let cancelled = || engine.signals().interrupted();
        let mut out = Vec::new();
        let meta = px
            .encode_to(&mut out, &recipe, &cancelled)
            .map_err(|err| render_error(err, span))?;
        drop(px);
        engine.signals().check(&span)?;
        Ok(PipelineData::ByteStream(
            ByteStream::read_binary(out, span, engine.signals().clone()),
            Some(PipelineMetadata {
                content_type: Some(meta.format.mime().into()),
                custom: record! { "tola_image" => metadata_value(&meta, span) },
                ..PipelineMetadata::default()
            }),
        ))
    }
}

fn opt_str(
    call: &Call,
    engine: &EngineState,
    stack: &mut Stack,
    idx: usize,
) -> Result<Option<String>, ShellError> {
    Ok(call.opt::<Option<String>>(engine, stack, idx)?.flatten())
}

fn image_input(
    src: Option<&str>,
    input: PipelineData,
    engine: &EngineState,
    span: Span,
) -> Result<Vec<u8>, ShellError> {
    engine.signals().check(&span)?;
    match (src, input.is_nothing()) {
        (Some(src), true) => source_bytes(src, engine, span),
        (None, false) => binary_input(input, span),
        _ => Err(image_error(
            "provide a source path or pipe image bytes, but not both",
            span,
        )),
    }
}

fn temporary_output(dst: &str, span: Span) -> Result<tempfile::NamedTempFile, ShellError> {
    let parent = Path::new(dst).parent().ok_or_else(|| {
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
    mut out: tempfile::NamedTempFile,
    dst: String,
    src: Option<String>,
    px: &ResampledPixels,
    recipe: &ImageRecipe,
    engine: &EngineState,
    span: Span,
) -> Result<PipelineData, ShellError> {
    let cancelled = || engine.signals().interrupted();
    let meta = {
        let mut writer = BufWriter::new(out.as_file_mut());
        let meta = px
            .encode_to(&mut writer, recipe, &cancelled)
            .map_err(|err| render_error(err, span))?;
        writer.flush().map_err(|_| {
            image_error(
                "the resized image could not be written; check available disk space and destination permissions",
                span,
            )
        })?;
        meta
    };
    engine.signals().check(&span)?;
    out.persist_noclobber(&dst).map_err(|err| {
        let msg = if err.error.kind() == std::io::ErrorKind::AlreadyExists {
            "the destination already exists; choose a new destination file"
        } else {
            "the resized image could not be saved; check the destination directory permissions"
        };
        image_error(msg, span)
    })?;
    let mut meta = metadata_value(&meta, span).into_record()?;
    meta.push(
        "source",
        src.map_or_else(|| Value::nothing(span), |path| Value::string(path, span)),
    );
    meta.push("destination", Value::string(dst, span));
    Ok(Value::record(meta, span).into_pipeline_data())
}

fn source_bytes(src: &str, engine: &EngineState, span: Span) -> Result<Vec<u8>, ShellError> {
    engine.signals().check(&span)?;
    let bytes = fs::read(src);
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

impl ResizeRequest {
    fn with_source_size(self, w: u32, h: u32) -> ResizeOptions {
        let mut opts = self.options;
        if self.keep_size {
            opts.width = Some(w);
            opts.height = Some(h);
        }
        opts
    }
}

fn resize_pixels(
    bytes: Vec<u8>,
    req: ResizeRequest,
    engine: &EngineState,
    span: Span,
) -> Result<(ResampledPixels, ImageRecipe), ShellError> {
    let src = tola_image::inspect(&bytes).map_err(|err| render_error(err, span))?;
    engine.signals().check(&span)?;
    let opts = req.with_source_size(src.width, src.height);
    let recipe =
        ImageRecipe::resolve(&src, opts).map_err(|err| image_error(err.to_string(), span))?;
    let cancelled = || engine.signals().interrupted();
    let decoded = DecodedSource::open(&bytes, &cancelled).map_err(|err| render_error(err, span))?;
    drop(bytes);
    let px = decoded
        .into_pixels(recipe.pixels(), &cancelled)
        .map_err(|err| render_error(err, span))?;
    Ok((px, recipe))
}

fn render_error(err: tola_image::ImageError, span: Span) -> ShellError {
    match err {
        tola_image::ImageError::Cancelled(_) => ShellError::Interrupted { span },
        err => image_error(err.to_string(), span),
    }
}

fn binary_input(input: PipelineData, span: Span) -> Result<Vec<u8>, ShellError> {
    match input {
        PipelineData::ByteStream(stream, _) => stream.into_bytes(),
        other => other.into_value(span)?.into_binary(),
    }
}

fn metadata_result(
    bytes: &[u8],
    engine: &EngineState,
    span: Span,
) -> Result<PipelineData, ShellError> {
    let meta = tola_image::inspect(bytes).map_err(|err| render_error(err, span))?;
    engine.signals().check(&span)?;
    Ok(metadata_value(&meta, span).into_pipeline_data())
}

fn metadata_value(meta: &ImageMetadata, span: Span) -> Value {
    Value::record(
        record! {
            "width" => Value::int(i64::from(meta.width), span),
            "height" => Value::int(i64::from(meta.height), span),
            "format" => Value::string(meta.format.extension(), span),
            "mime" => Value::string(meta.format.mime(), span),
            "has-alpha" => Value::bool(meta.has_alpha, span),
            "is-lossy" => Value::bool(meta.is_lossy, span),
        },
        span,
    )
}

fn image_error(msg: impl Into<String>, span: Span) -> ShellError {
    GenericError::new("Image operation failed", msg.into(), span)
        .with_code("tola::image")
        .into()
}

fn field<'a>(opts: &'a Record, name: &str) -> Option<&'a Value> {
    opts.get(name)
        .filter(|val| !matches!(val, Value::Nothing { .. }))
}

fn int_in_range<T: TryFrom<i64>>(
    opts: &Record,
    name: &str,
    valid: impl FnOnce(&T) -> bool,
    err_msg: impl FnOnce() -> String,
) -> Result<Option<T>, ShellError> {
    field(opts, name)
        .map(|val| {
            let n = val.as_int()?;
            T::try_from(n)
                .ok()
                .filter(valid)
                .ok_or_else(|| image_error(err_msg(), val.span()))
        })
        .transpose()
}

fn choice<T: Copy>(
    opts: &Record,
    name: &str,
    choices: &[(&str, T)],
    default: T,
) -> Result<T, ShellError> {
    let Some(val) = field(opts, name) else {
        return Ok(default);
    };
    let selected = val.as_str()?;
    choices
        .iter()
        .find(|(k, _)| *k == selected)
        .map(|(_, v)| *v)
        .ok_or_else(|| {
            image_error(
                format!(
                    "--{name} must be one of: {}",
                    choices
                        .iter()
                        .map(|(k, _)| *k)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                val.span(),
            )
        })
}

const OPS: &[(&str, ResizeOperation)] = &[
    ("scale", ResizeOperation::Scale),
    ("fit-width", ResizeOperation::FitWidth),
    ("fit-height", ResizeOperation::FitHeight),
    ("fit", ResizeOperation::Fit),
    ("fill", ResizeOperation::Fill),
];

const ANCHORS: &[(&str, CropAnchor)] = &[
    ("center", CropAnchor::Center),
    ("top", CropAnchor::Top),
    ("bottom", CropAnchor::Bottom),
    ("left", CropAnchor::Left),
    ("right", CropAnchor::Right),
    ("top-left", CropAnchor::TopLeft),
    ("top-right", CropAnchor::TopRight),
    ("bottom-left", CropAnchor::BottomLeft),
    ("bottom-right", CropAnchor::BottomRight),
];

const FORMATS: &[(&str, OutputFormat)] = &[
    ("auto", OutputFormat::Auto),
    ("jpeg", OutputFormat::Jpeg),
    ("jpg", OutputFormat::Jpeg),
    ("png", OutputFormat::Png),
    ("webp", OutputFormat::WebP),
];

const FILTERS: &[(&str, ResizeFilter)] = &[
    ("nearest", ResizeFilter::Nearest),
    ("triangle", ResizeFilter::Triangle),
    ("catmull-rom", ResizeFilter::CatmullRom),
    ("gaussian", ResizeFilter::Gaussian),
    ("lanczos3", ResizeFilter::Lanczos3),
];

fn parse_background(opts: &Record) -> Result<Option<Background>, ShellError> {
    field(opts, "background")
        .map(|val| {
            let text = val.as_str()?;
            let rgb = text
                .strip_prefix('#')
                .filter(|hex| hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .ok_or_else(|| {
                    image_error(
                        "--background must be an opaque sRGB color in #RRGGBB form",
                        val.span(),
                    )
                })?;
            Ok(Background {
                red: (rgb >> 16) as u8,
                green: (rgb >> 8) as u8,
                blue: rgb as u8,
            })
        })
        .transpose()
}

fn resize_request(opts: &Record) -> Result<ResizeRequest, ShellError> {
    let quality = int_in_range::<u8>(
        opts,
        "quality",
        |&q| q <= 100,
        || "--quality must be between 0 and 100 (JPEG requires at least 1)".into(),
    )?;
    let background = parse_background(opts)?;
    fn dim_err(name: &'static str) -> impl FnOnce() -> String {
        move || {
            format!(
                "--{name} must be a positive integer no larger than {}",
                u32::MAX
            )
        }
    }
    let w = int_in_range::<u32>(opts, "width", |&v| v > 0, dim_err("width"))?;
    let h = int_in_range::<u32>(opts, "height", |&v| v > 0, dim_err("height"))?;
    let keep_size = w.is_none() && h.is_none() && field(opts, "op").is_none();
    let default_op = match (w, h) {
        (None, None) => ResizeOperation::Scale,
        (Some(_), None) => ResizeOperation::FitWidth,
        (None, Some(_)) => ResizeOperation::FitHeight,
        (Some(_), Some(_)) => ResizeOperation::Fill,
    };
    let options = ResizeOptions {
        width: w,
        height: h,
        operation: choice(opts, "op", OPS, default_op)?,
        anchor: choice(opts, "anchor", ANCHORS, CropAnchor::Center)?,
        format: choice(opts, "format", FORMATS, OutputFormat::Auto)?,
        filter: choice(opts, "filter", FILTERS, ResizeFilter::Lanczos3)?,
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
    use std::sync::atomic::AtomicBool;
    use tola_image::ImageFormat;

    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 244, 34, 127, 138, 0, 0, 0, 15, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240,
        31, 8, 27, 0, 16, 121, 3, 126, 125, 99, 206, 215, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
        130,
    ];

    fn parsed(cmd: &str, signals: Signals) -> (EngineState, Arc<nu_protocol::ast::Block>) {
        let mut engine = crate::Runtime::new(Vec::new(), Vec::new()).unwrap().engine;
        engine.set_signals(signals);
        let mut ws = StateWorkingSet::new(&engine);
        let src = format!("use tola image\n{cmd}");
        let block = crate::execution::parse(&mut ws, "image-test.nu", src.as_bytes()).unwrap();
        engine.merge_delta(ws.render()).unwrap();
        (engine, block)
    }

    fn eval(cmd: &str, signals: Signals) -> Result<PipelineData, ShellError> {
        let (engine, block) = parsed(cmd, signals);
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

    fn hex_literal(bytes: &[u8]) -> String {
        format!(
            "0x[{}]",
            bytes
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" ")
        )
    }

    fn val(cmd: &str) -> Value {
        eval(cmd, Signals::empty())
            .unwrap()
            .into_value(Span::test_data())
            .unwrap()
    }

    #[test]
    fn metadata_and_resize_bytes() {
        let png = hex_literal(PNG);
        let info = val(&format!("{png} | image metadata"));
        let info = info.as_record().unwrap();
        assert_eq!(info.get("width").unwrap().as_int().unwrap(), 2);
        assert_eq!(info.get("height").unwrap().as_int().unwrap(), 1);
        assert_eq!(info.get("mime").unwrap().as_str().unwrap(), "image/png");
        assert!(info.get("has-alpha").unwrap().as_bool().unwrap());
        let out = eval(
            &format!("{png} | image resize --width 4 --format png"),
            Signals::empty(),
        )
        .unwrap();
        assert!(matches!(&out, PipelineData::ByteStream(..)));
        assert_eq!(
            out.metadata_ref().unwrap().content_type.as_deref(),
            Some("image/png")
        );
        let bytes = out
            .into_value(Span::test_data())
            .unwrap()
            .into_binary()
            .unwrap();
        let resized = tola_image::inspect(&bytes).unwrap();
        assert_eq!((resized.width, resized.height), (4, 2));
        assert_eq!(resized.format, ImageFormat::Png);
    }

    #[test]
    fn format_only_resize() {
        let bytes = val(&format!(
            "{} | image resize --format webp",
            hex_literal(PNG)
        ))
        .into_binary()
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("source.png");
        let dst = dir.path().join("converted.webp");
        fs::write(&src, PNG).unwrap();
        let src_path = src.to_str().unwrap();
        let info = val(&format!("image metadata '{src_path}'"));
        assert_eq!(
            info.as_record()
                .unwrap()
                .get("width")
                .unwrap()
                .as_int()
                .unwrap(),
            2
        );
        let from_path = val(&format!("image resize '{src_path}' --format webp"))
            .into_binary()
            .unwrap();
        assert_eq!(from_path, bytes);

        let piped = dir.path().join("piped.webp");
        let piped_path = piped.to_str().unwrap();
        let piped_resize =
            format!("open --raw '{src_path}' | image resize --format webp -o '{piped_path}'");
        let info = val(&piped_resize);
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
            src.to_str().unwrap().into(),
            "--output".into(),
            dst.to_str().unwrap().into(),
            "--format".into(),
            "webp".into(),
        ];
        assert_eq!(runtime.run_module(&entry, &args, false, None), 0);

        for bytes in [bytes, fs::read(dst).unwrap()] {
            let img = tola_image::inspect(&bytes).unwrap();
            assert_eq!((img.width, img.height), (2, 1));
            assert_eq!(img.format, ImageFormat::WebP);
        }
    }

    #[test]
    fn reject_invalid_image_args() {
        let png = hex_literal(PNG);
        for cmd in ["image metadata", "image resize"] {
            assert!(eval(cmd, Signals::empty()).is_err());
            let conflict = format!("{png} | {cmd} 'unused.png'");
            let err = eval(&conflict, Signals::empty()).unwrap_err();
            assert!(format!("{err:?}").contains("pipe image bytes, but not both"));
        }
        for input in ["0x[]", "0x[00 01 02]"] {
            assert!(eval(&format!("{input} | image metadata"), Signals::empty()).is_err());
        }
        for args in [
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
            let res = eval(&format!("{png} | image resize {args}"), Signals::empty());
            assert!(res.is_err(), "request was accepted: {args}");
        }
    }

    #[test]
    fn svg_resize_raster_format() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 4"><path fill="red" d="M0 0H8V4H0Z"/></svg>"#;
        let literal = hex_literal(svg);
        let info = val(&format!("{literal} | image metadata"));
        assert_eq!(
            info.as_record()
                .unwrap()
                .get("format")
                .unwrap()
                .as_str()
                .unwrap(),
            "svg"
        );
        assert!(
            eval(
                &format!("{literal} | image resize --width 4 --op fit-width"),
                Signals::empty(),
            )
            .is_err()
        );
        let png = val(&format!("{literal} | image resize --width 4 --format png"))
            .into_binary()
            .unwrap();
        let cancelled = || false;
        let src = DecodedSource::open(&png, &cancelled).unwrap();
        let info = src.metadata();
        assert_eq!(
            (info.format, info.width, info.height),
            (ImageFormat::Png, 4, 2)
        );
        let recipe = ImageRecipe::resolve(
            info,
            ResizeOptions {
                width: Some(4),
                height: Some(2),
                ..ResizeOptions::default()
            },
        )
        .unwrap();
        let px = src.into_pixels(recipe.pixels(), &cancelled).unwrap();
        assert_eq!(px.rgba(), [255u8, 0, 0, 255].repeat(8));
    }

    #[test]
    fn interrupted_resize_error() {
        let signals = Signals::new(Arc::new(AtomicBool::new(true)));
        let err = eval(
            &format!(
                "{} | image resize --width 4 --op fit-width",
                hex_literal(PNG)
            ),
            signals,
        )
        .unwrap_err();
        assert!(matches!(err, ShellError::Interrupted { .. }));
    }

    #[test]
    fn image_command_signatures() {
        let help = val("help image resize");
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
        for (name, expected_out) in [
            ("image metadata", Type::record()),
            ("image resize", Type::Any),
        ] {
            let decl = engine
                .find_decl(name.as_bytes(), &[])
                .expect("the module exports this command");
            let sig = engine.get_decl(decl).signature();
            assert_eq!(
                sig.input_output_types,
                vec![
                    (Type::Nothing, expected_out.clone()),
                    (Type::Binary, expected_out)
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
        let public = val("scope commands | where name =~ '^image ' | get name");
        let mut names: Vec<_> = public
            .as_list()
            .unwrap()
            .iter()
            .map(|val| val.as_str().unwrap())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["image metadata", "image resize"]);
    }
}
