//! Native image inspection and declarative derivative requests.
//!
//! Both functions load through Typst's normal path loader: a source is a file path, never a
//! browser URL, and asset URL mappings never remap it. Their exported Func values are not
//! wrapped in Typst closures, so strings retain the native call-site span and explicit paths
//! retain their original site/package root.
//!
//! `image_metadata` only inspects. `resize_image` computes the derivative's dimensions and
//! URL and registers the request as a tracked World read; a complete build publishes every
//! request its read evidence carries, while an editor source check evaluates the same call
//! without running an image producer.

// `#[func]` clears attributes on the function it rewrites, so the allowance for a native's
// parameter-per-option signature has to sit on the module.
#![allow(clippy::too_many_arguments)]

use std::num::NonZeroU32;

use tola_typst::ContentDigest;
use typst::World;
use typst::diag::{At, SourceResult, bail};
use typst::engine::Engine;
use typst::foundations::{Bytes, Dict, IntoValue, PathOrStr, Str, Value, func};
use typst::loading::{DataSource, Load, LoadSource};
use typst::syntax::{FileId, Span, SpanKind, Spanned, VirtualRoot};

use crate::image_request::ImageRequest;
use tola_image::{
    Background, ImageRecipe, OutputFormat, ResizeFilter, ResizeOperation, ResizeOptions, inspect,
};
use typst::visualize::Color;

const ASSET_URL_SHADOWED_ORIGIN: &str = "AssetUrlShadowed";

/// A physical image path that also names an asset published from another source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetUrlShadowed<'a> {
    /// The path spelling supplied to the native image function.
    pub written: &'a str,
    /// The site-root file that spelling reads.
    pub resolved: &'a str,
    /// The site-root file the asset declaration publishes.
    pub published_from: &'a str,
}

impl<'a> AssetUrlShadowed<'a> {
    /// Interpret only the image producer's named evidence, never diagnostic prose.
    pub fn from_origin(origin: &'a tola_typst::DiagnosticOrigin) -> Option<Self> {
        let tola_typst::DiagnosticOrigin::Producer(origin) = origin else {
            return None;
        };
        if origin.name() != ASSET_URL_SHADOWED_ORIGIN || origin.fields().len() != 3 {
            return None;
        }
        let field = |name: &str| {
            origin
                .fields()
                .iter()
                .find_map(|(key, value)| (key == name).then_some(value.as_str()))
        };
        Some(Self {
            written: field("written")?,
            resolved: field("resolved")?,
            published_from: field("published_from")?,
        })
    }
}

/// The fields `image-metadata` publishes, in the order it inserts them.
pub(crate) const IMAGE_METADATA_FIELDS: [&str; 6] =
    ["width", "height", "format", "mime", "has-alpha", "is-lossy"];

#[func]
pub(super) fn image_metadata(
    engine: &mut Engine,
    /// A path string relative to this call, or an already resolved path.
    path: Spanned<PathOrStr>,
) -> SourceResult<Dict> {
    let span = path.span;
    let (_, bytes) = load_source(engine, path)?;
    let metadata = inspect(&bytes)
        .map_err(|error| error.to_string())
        .at(span)?;
    let values = [
        metadata.width.into_value(),
        metadata.height.into_value(),
        metadata.format.extension().into_value(),
        metadata.format.mime().into_value(),
        metadata.has_alpha.into_value(),
        metadata.is_lossy.into_value(),
    ];
    let mut result = Dict::new();
    for (name, value) in IMAGE_METADATA_FIELDS.into_iter().zip(values) {
        result.insert(name.into(), value);
    }
    Ok(result)
}

/// The resize operation one `op` value names.
fn resize_operation(op: &str) -> Option<ResizeOperation> {
    match op {
        "scale" => Some(ResizeOperation::Scale),
        "fit-width" => Some(ResizeOperation::FitWidth),
        "fit-height" => Some(ResizeOperation::FitHeight),
        "fit" => Some(ResizeOperation::Fit),
        "fill" => Some(ResizeOperation::Fill),
        _ => None,
    }
}

/// The resampling kernel one `filter` value names.
fn resize_filter(filter: &str) -> Option<ResizeFilter> {
    match filter {
        "nearest" => Some(ResizeFilter::Nearest),
        "triangle" => Some(ResizeFilter::Triangle),
        "catmull-rom" => Some(ResizeFilter::CatmullRom),
        "gaussian" => Some(ResizeFilter::Gaussian),
        "lanczos3" => Some(ResizeFilter::Lanczos3),
        _ => None,
    }
}

#[func]
pub(super) fn resize_image(
    engine: &mut Engine,
    /// A path string relative to this call, or an already resolved path.
    path: Spanned<PathOrStr>,
    /// Positive output width in pixels, as required by the operation.
    #[named]
    #[default]
    width: Option<NonZeroU32>,
    /// Positive output height in pixels, as required by the operation.
    #[named]
    #[default]
    height: Option<NonZeroU32>,
    /// One of scale, fit-width, fit-height, fit, or fill.
    #[named]
    #[default("fill".into())]
    op: Str,
    /// One of auto, jpg, png, or webp.
    #[named]
    #[default("auto".into())]
    format: Str,
    /// JPEG quality is 1–100 (default 75). WebP is lossless unless a
    /// quality from 0–100 is supplied. PNG is always lossless.
    #[named]
    #[default]
    quality: Option<i64>,
    /// One of nearest, triangle, catmull-rom, gaussian, or lanczos3.
    #[named]
    #[default("lanczos3".into())]
    filter: Str,
    /// An opaque color to composite transparency onto before encoding.
    #[named]
    #[default]
    background: Option<Color>,
) -> SourceResult<Dict> {
    let span = path.span;
    let Some(operation) = resize_operation(op.as_str()) else {
        bail!(
            span,
            "`op` must be `scale`, `fit-width`, `fit-height`, `fit`, or `fill`, not `{}`",
            op.as_str()
        );
    };
    let format = match format.as_str() {
        "auto" => OutputFormat::Auto,
        "jpg" => OutputFormat::Jpeg,
        "png" => OutputFormat::Png,
        "webp" => OutputFormat::WebP,
        "avif" => bail!(
            span,
            "AVIF image output is not supported yet";
            hint: "use `format: \"webp\"`, `format: \"png\"`, `format: \"jpg\"`, or `format: \"auto\"` instead"
        ),
        _ => bail!(
            span,
            "`format` must be `auto`, `jpg`, `png`, or `webp`, not `{}`",
            format.as_str()
        ),
    };
    let Some(filter) = resize_filter(filter.as_str()) else {
        bail!(
            span,
            "`filter` must be `nearest`, `triangle`, `catmull-rom`, `gaussian`, or `lanczos3`, not `{}`",
            filter.as_str()
        );
    };
    if let Some(invalid) = quality.filter(|quality| !(0..=100).contains(quality)) {
        bail!(span, "`quality` must be between 0 and 100, not `{invalid}`");
    }
    let background = background
        .map(|color| opaque_srgb_background(color, span))
        .transpose()?;
    let options = ResizeOptions {
        width: width.map(NonZeroU32::get),
        height: height.map(NonZeroU32::get),
        operation,
        format,
        quality: quality.map(|quality| quality as u8),
        filter,
        background,
    };
    let (source_id, bytes) = load_source(engine, path)?;
    let metadata = inspect(&bytes)
        .map_err(|error| error.to_string())
        .at(span)?;
    let recipe = ImageRecipe::resolve(&metadata, options)
        .map_err(|error| error.to_string())
        .at(span)?;
    let request = ImageRequest::new(source_id.get().clone(), ContentDigest::of(&bytes), recipe);
    let url = super::library::browser_output_url(engine, span, &request.output_path())?;
    // The read is ordinary tracked World input, not an imperative output queue: an
    // evaluated call requests publication even when the returned dictionary is never
    // used. Typst's memoized native evaluation therefore replays both source and request
    // reads, including when the dictionary is retained in source metadata or used during
    // a later contextual evaluation.
    engine.world.file(request.observation_id()).at(span)?;

    let values = [
        url.into_value(),
        request.recipe().width().into_value(),
        request.recipe().height().into_value(),
        metadata.width.into_value(),
        metadata.height.into_value(),
    ];
    let mut result = Dict::new();
    for (name, value) in RESIZE_IMAGE_FIELDS.into_iter().zip(values) {
        result.insert(name.into(), value);
    }
    Ok(result)
}

/// The fields `resize-image` publishes, in the order it inserts them.
pub(crate) const RESIZE_IMAGE_FIELDS: [&str; 5] = [
    "url",
    "width",
    "height",
    "original-width",
    "original-height",
];

fn opaque_srgb_background(color: Color, span: Span) -> SourceResult<Background> {
    if color.alpha().is_some_and(|alpha| alpha != 1.0) {
        bail!(
            span,
            "`background` must be opaque; use a color without transparency"
        );
    }
    let (red, green, blue, _) = color.to_rgb().into_format::<u8, u8>().into_components();
    Ok(Background { red, green, blue })
}

fn load_source(engine: &mut Engine, source: Spanned<PathOrStr>) -> SourceResult<(FileId, Bytes)> {
    let span = source.span;
    let written = source.v.clone();
    let loaded = source.map(DataSource::Path).load(engine.world)?;
    let LoadSource::Path(id) = loaded.source.v else {
        bail!(loaded.source.span, "an image source must be a file path");
    };
    warn_shadowed_asset_url(engine, span, &written, id)?;
    Ok((id, loaded.data))
}

fn warn_shadowed_asset_url(
    engine: &mut Engine,
    span: Span,
    written: &PathOrStr,
    id: FileId,
) -> SourceResult<()> {
    let written = match written {
        PathOrStr::Str(string) => string.as_str(),
        PathOrStr::Path(path) => path.vpath().get_without_slash(),
    };
    let named = written.split('?').next().unwrap_or_default();
    let Some(origins) = super::library::asset_origins(engine) else {
        return Ok(());
    };
    let Ok(Value::Str(published_from)) = origins.get(named.trim_start_matches('/')) else {
        return Ok(());
    };
    if id.root() != &VirtualRoot::Project {
        return Ok(());
    }
    let resolved = id.vpath().get_without_slash();
    if resolved == published_from.as_str() {
        return Ok(());
    }
    // A numbered call-site source remains a tracked input of the native evaluation.
    if let SpanKind::Number { id, num } = span.get() {
        let source = engine.world.source(id).at(span)?;
        source
            .range(num, None)
            .expect("a native argument span belongs to its source");
    }
    let warning = tola_typst::diagnostic::producer_warning(
        span,
        ASSET_URL_SHADOWED_ORIGIN,
        &[("written", named), ("resolved", resolved), ("published_from", published_from.as_str())],
        format!(
            "`{named}` is also a published asset URL; this reads `{resolved}`, while the site \
             publishes that URL from `{published_from}`"
        ),
    ).with_hint("Write the file the declaration publishes from, or use the URL where a URL is expected (`asset-url(\"…\")`)");
    engine.sink.warn(warning);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::visualize::{Cmyk, LinearRgb, ProcessColor};

    #[test]
    fn linear_background_uses_srgb() {
        let color = Color::Process(ProcessColor::LinearRgb(LinearRgb::new(
            0.5, 0.25, 0.75, 1.0,
        )));
        assert_eq!(
            opaque_srgb_background(color, Span::detached()).unwrap(),
            Background {
                red: 188,
                green: 137,
                blue: 225,
            }
        );
    }

    #[test]
    fn black_ink_darkens_background() {
        let background = |[c, m, y, k]: [f32; 4]| {
            opaque_srgb_background(
                Color::Process(ProcessColor::Cmyk(Cmyk { c, m, y, k })),
                Span::detached(),
            )
            .unwrap()
        };
        let cyan = background([1.0, 0.0, 0.0, 0.0]);
        let ink = background([1.0, 0.0, 0.0, 1.0]);
        assert!(ink.red <= cyan.red);
        assert!(ink.green < cyan.green);
        assert!(ink.blue < cyan.blue);
    }

    #[test]
    fn translucent_background_is_rejected() {
        let color = Color::WHITE.with_alpha(0.999);
        assert!(opaque_srgb_background(color, Span::detached()).is_err());
    }

    #[test]
    fn resize_operation_rejects_underscores() {
        assert_eq!(
            resize_operation("fit-width"),
            Some(ResizeOperation::FitWidth)
        );
        assert_eq!(
            resize_operation("fit-height"),
            Some(ResizeOperation::FitHeight)
        );
        assert_eq!(resize_operation("fit_width"), None);
        assert_eq!(resize_operation("fit_height"), None);
    }

    #[test]
    fn resize_filter_rejects_underscores() {
        assert_eq!(resize_filter("catmull-rom"), Some(ResizeFilter::CatmullRom));
        assert_eq!(resize_filter("catmull_rom"), None);
    }

    #[test]
    fn reordered_evidence_keeps_asset_identity() {
        let warning = tola_typst::diagnostic::producer_warning(
            Span::detached(),
            ASSET_URL_SHADOWED_ORIGIN,
            &[
                ("published_from", "assets/logo.svg"),
                ("resolved", "assets/brand.svg"),
                ("written", "/assets/brand.svg"),
            ],
            "independent warning wording",
        );
        let native = tola_typst::NativeDiagnostic::from(warning);
        let serialized = serde_json::to_vec(native.origin()).unwrap();
        let origin: tola_typst::DiagnosticOrigin = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(
            AssetUrlShadowed::from_origin(&origin),
            Some(AssetUrlShadowed {
                written: "/assets/brand.svg",
                resolved: "assets/brand.svg",
                published_from: "assets/logo.svg",
            })
        );
    }
}
