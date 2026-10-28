//! Native image inspection and declarative derivative requests.
//!
//! Every function here loads through Typst's normal path loader: a source is a file path, never a
//! browser URL, and asset URL mappings never remap it. Their exported Func values are not wrapped
//! in Typst closures, so strings retain the native call-site span and explicit paths retain their
//! original site/package root.
//!
//! `image_metadata` only inspects. `resize_image` computes one derivative's dimensions and URL.
//! `image_srcset` does the same for several widths, ascending, or for the source's own width when
//! `widths` is `auto`. Either way the request registers as a tracked World read: a complete build
//! publishes every request its read evidence carries, while an editor source check evaluates the
//! same call without running an image producer.

// `#[func]` clears attributes on the function it rewrites, so `too_many_arguments` has to be
// allowed on the module.
#![allow(clippy::too_many_arguments)]

use std::num::NonZeroU32;

use tola_typst::ContentDigest;
use typst::World;
use typst::diag::{At, SourceResult, bail};
use typst::ecow::EcoString;
use typst::engine::Engine;
use typst::foundations::{
    Array, Content, Dict, IntoValue, NativeElement, PathOrStr, Str, Value, func,
};
use typst::loading::{DataSource, Load, LoadSource};
use typst::syntax::{FileId, RootedPath, Span, SpanKind, Spanned, VirtualPath, VirtualRoot};
use typst_html::{HtmlAttr, HtmlElem, attr, tag};

use crate::image_request::ImageRequest;
use tola_image::{
    Background, CropAnchor, ImageFormat, ImageMetadata, ImageRecipe, OutputFormat, ResizeFilter,
    ResizeOperation, ResizeOptions, SvgRender, inspect,
};
use tola_typst::svg::{SvgSource, looks_like_svg};
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
    source: Spanned<DataSource>,
) -> SourceResult<Dict> {
    let metadata = load_source_image(engine, source)?.metadata;
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

/// The crop anchor one `anchor` value names.
fn crop_anchor(anchor: &str) -> Option<CropAnchor> {
    match anchor {
        "center" => Some(CropAnchor::Center),
        "top" => Some(CropAnchor::Top),
        "bottom" => Some(CropAnchor::Bottom),
        "left" => Some(CropAnchor::Left),
        "right" => Some(CropAnchor::Right),
        "top-left" => Some(CropAnchor::TopLeft),
        "top-right" => Some(CropAnchor::TopRight),
        "bottom-left" => Some(CropAnchor::BottomLeft),
        "bottom-right" => Some(CropAnchor::BottomRight),
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

/// The output format one `format` value names, or the failure a site author reads.
fn output_format(format: &Str, span: Span) -> SourceResult<OutputFormat> {
    Ok(match format.as_str() {
        "auto" => OutputFormat::Auto,
        "jpg" | "jpeg" => OutputFormat::Jpeg,
        "png" => OutputFormat::Png,
        "webp" => OutputFormat::WebP,
        "avif" => bail!(
            span,
            "AVIF image output is not supported";
            hint: "use `format: \"webp\"`, `format: \"png\"`, `format: \"jpg\"`, or `format: \"auto\"` instead"
        ),
        _ => bail!(
            span,
            "`format` must be `auto`, `jpg` (or `jpeg`), `png`, or `webp`, not `{}`",
            format.as_str()
        ),
    })
}

/// The quality one `quality` value names, or the failure a site author reads.
fn checked_quality(quality: Option<i64>, span: Span) -> SourceResult<Option<u8>> {
    if let Some(invalid) = quality.filter(|quality| !(0..=100).contains(quality)) {
        bail!(span, "`quality` must be between 0 and 100, not `{invalid}`");
    }
    Ok(quality.map(|quality| quality as u8))
}

/// The opaque background one `background` value names, or the failure a site author reads.
fn checked_background(background: Option<Color>, span: Span) -> SourceResult<Option<Background>> {
    background
        .map(|color| opaque_srgb_background(color, span))
        .transpose()
}

/// One loaded source image: where it came from, the digest of its bytes, and what the container declares.
struct SourceImage {
    source: DataSource,
    /// The resolved file for a path source; the call site for a bytes source.
    origin: RootedPath,
    digest: ContentDigest,
    metadata: ImageMetadata,
    svg: Option<SvgSource>,
}

/// One derived image: the URL a document links, and the pixels that URL holds.
struct Derivative {
    url: String,
    width: u32,
    height: u32,
}

/// Load one source image and read what its container declares about itself.
fn load_source_image(
    engine: &mut Engine,
    source: Spanned<DataSource>,
) -> SourceResult<SourceImage> {
    let span = source.span;
    let written = source.v.clone();
    let loaded = source.load(engine.world)?;
    let (file, origin) = match loaded.source.v {
        LoadSource::Path(id) => {
            if let DataSource::Path(path) = &written {
                warn_shadowed_asset_url(engine, span, path, id)?;
            }
            (id, id.get().clone())
        }
        LoadSource::Bytes => {
            // A detached span has no call-site file; the synthetic path only anchors font lookup.
            let id = span.id().unwrap_or_else(|| {
                FileId::new(RootedPath::new(
                    VirtualRoot::Project,
                    VirtualPath::new("/_tola/bytes.typ").expect("synthetic bytes path is valid"),
                ))
            });
            (id, id.get().clone())
        }
    };
    let svg = looks_like_svg(&loaded.data)
        .then(|| SvgSource::parse(loaded.data.clone(), engine.world, file))
        .transpose()
        .map_err(|error| error.to_string())
        .at(span)?;
    let metadata = match &svg {
        Some(svg) => ImageMetadata {
            width: rounded_pixels(svg.width()),
            height: rounded_pixels(svg.height()),
            format: ImageFormat::Svg,
            has_alpha: true,
            is_lossy: false,
        },
        None => inspect(&loaded.data)
            .map_err(|error| error.to_string())
            .at(span)?,
    };
    Ok(SourceImage {
        source: written,
        origin,
        digest: ContentDigest::of(&loaded.data),
        metadata,
        svg,
    })
}

/// The integer pixel size an SVG reports at usvg's 96 DPI.
fn rounded_pixels(value: f64) -> u32 {
    value.round().clamp(1.0, f64::from(u32::MAX)) as u32
}

/// Request one derivative and return what a document needs to name it.
fn request_derivative(
    engine: &mut Engine,
    span: Span,
    source: &SourceImage,
    options: ResizeOptions,
) -> SourceResult<Derivative> {
    let recipe = match &source.svg {
        Some(svg) => ImageRecipe::resolve_svg(
            (svg.width(), svg.height()),
            options,
            SvgRender::new(svg.hash()),
        ),
        None => ImageRecipe::resolve(&source.metadata, options),
    }
    .map_err(|error| error.to_string())
    .at(span)?;
    let request = ImageRequest::new(
        source.source.clone(),
        source.origin.clone(),
        source.digest,
        recipe,
    );
    let url = super::library::browser_output_url(engine, span, &request.output_path())?;
    // The read is ordinary tracked World input, not an imperative output queue: an evaluated
    // call requests publication even when the returned dictionary is never used. Typst's
    // memoized native evaluation therefore replays both source and request reads, including
    // when the dictionary is retained in source metadata or used during a later contextual
    // evaluation.
    engine.world.file(request.observation_id()).at(span)?;
    Ok(Derivative {
        url,
        width: request.recipe().width(),
        height: request.recipe().height(),
    })
}

/// The widths one `widths` array names: positive pixel counts, ascending and distinct.
fn pixel_widths(widths: &Array, span: Span) -> SourceResult<Vec<NonZeroU32>> {
    if widths.is_empty() {
        bail!(span, "`widths` must name at least one width");
    }
    let mut ordered = Vec::with_capacity(widths.len());
    for width in widths {
        let Ok(width) = width.clone().cast::<NonZeroU32>() else {
            bail!(
                span,
                "`widths` must be positive pixel counts, not `{width:?}`"
            );
        };
        ordered.push(width);
    }
    ordered.sort_unstable();
    ordered.dedup();
    Ok(ordered)
}

/// The widths one `widths` argument names: `auto` for the source's own width, or the pixel counts
/// the array lists.
fn requested_widths(
    widths: &Value,
    metadata: &ImageMetadata,
    span: Span,
) -> SourceResult<Vec<NonZeroU32>> {
    match widths {
        Value::Auto => {
            let Some(width) = NonZeroU32::new(metadata.width) else {
                bail!(span, "the source image has no width to derive");
            };
            Ok(vec![width])
        }
        Value::Array(array) => pixel_widths(array, span),
        value => bail!(
            span,
            "`widths` must be `auto` or positive pixel counts, not `{value:?}`"
        ),
    }
}

/// The `srcset` attribute value: every derivative with its width descriptor, in ascending order.
fn srcset_attribute(derivatives: &[Derivative]) -> String {
    derivatives
        .iter()
        .map(|derivative| format!("{} {}w", derivative.url, derivative.width))
        .collect::<Vec<_>>()
        .join(", ")
}

#[func]
pub(super) fn resize_image(
    engine: &mut Engine,
    /// A path string relative to this call, or an already resolved path.
    source: Spanned<DataSource>,
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
    /// Which part of the source `fill` keeps: center, top, bottom, left, right, top-left,
    /// top-right, bottom-left, or bottom-right.
    #[named]
    #[default("center".into())]
    anchor: Str,
    /// One of auto, jpg (or jpeg), png, or webp.
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
    let span = source.span;
    let Some(operation) = resize_operation(op.as_str()) else {
        bail!(
            span,
            "`op` must be `scale`, `fit-width`, `fit-height`, `fit`, or `fill`, not `{}`",
            op.as_str()
        );
    };
    let Some(anchor) = crop_anchor(anchor.as_str()) else {
        bail!(
            span,
            "`anchor` must be `center`, `top`, `bottom`, `left`, `right`, `top-left`, `top-right`, `bottom-left`, or `bottom-right`, not `{}`",
            anchor.as_str()
        );
    };
    let format = output_format(&format, span)?;
    let Some(filter) = resize_filter(filter.as_str()) else {
        bail!(
            span,
            "`filter` must be `nearest`, `triangle`, `catmull-rom`, `gaussian`, or `lanczos3`, not `{}`",
            filter.as_str()
        );
    };
    let quality = checked_quality(quality, span)?;
    let background = checked_background(background, span)?;
    let options = ResizeOptions {
        width: width.map(NonZeroU32::get),
        height: height.map(NonZeroU32::get),
        operation,
        anchor,
        format,
        quality,
        filter,
        background,
    };
    let source = load_source_image(engine, source)?;
    let derivative = request_derivative(engine, span, &source, options)?;
    let values = [
        derivative.url.into_value(),
        derivative.width.into_value(),
        derivative.height.into_value(),
        source.metadata.width.into_value(),
        source.metadata.height.into_value(),
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

/// The fields `image-srcset` publishes, in the order it inserts them.
pub(crate) const IMAGE_SRCSET_FIELDS: [&str; 4] = ["url", "srcset", "width", "height"];

#[func]
pub(super) fn image_srcset(
    engine: &mut Engine,
    /// A path string relative to this call, or an already resolved path.
    source: Spanned<DataSource>,
    /// `auto` for the source's own width, or the positive pixel widths to derive.
    #[named]
    #[default(Value::Auto)]
    widths: Value,
    /// One of auto, jpg (or jpeg), png, or webp; `webp` serves the page.
    #[named]
    #[default("webp".into())]
    format: Str,
    /// JPEG/WebP quality 0–100; `none` selects lossless WebP. PNG ignores it.
    #[named]
    #[default(Some(75))]
    quality: Option<i64>,
    /// An opaque color to composite transparency onto before encoding.
    #[named]
    #[default]
    background: Option<Color>,
) -> SourceResult<Dict> {
    let span = source.span;
    let mut derivatives =
        request_derivatives(engine, source, &widths, &format, quality, background, span)?;
    let srcset = srcset_attribute(&derivatives);
    let widest = derivatives
        .pop()
        .expect("`requested_widths` never returns an empty list");
    let values = [
        widest.url.into_value(),
        srcset.into_value(),
        widest.width.into_value(),
        widest.height.into_value(),
    ];
    let mut result = Dict::new();
    for (name, value) in IMAGE_SRCSET_FIELDS.into_iter().zip(values) {
        result.insert(name.into(), value);
    }
    Ok(result)
}

/// The attributes `img` writes itself; `attrs` cannot name them.
const IMG_OWNED_ATTRIBUTES: [&str; 6] = ["src", "srcset", "sizes", "width", "height", "alt"];

/// Place one image at several widths as a responsive `<img>` element.
///
/// `path` behaves as in `resize-image`. The element carries the widest derivative's `src`, its
/// pixel `width` and `height`, and `alt`, written even when empty. `srcset` and `sizes` are
/// written only when more than one width was requested. `loading` and `decoding` default to
/// `lazy` and `async`, and `attrs` may override them.
///
/// Attributes are written raw rather than through the typed HTML image element: its `sizes` model
/// cannot express viewport units or the unconditional final entry a CSS source-size list needs.
///
/// The element is HTML: call it from an HTML document. A paged document uses `image` instead.
#[func]
pub(super) fn img(
    engine: &mut Engine,

    /// A path string relative to this call, or an already resolved path.
    source: Spanned<DataSource>,
    /// `auto` for the source's own width, or the positive pixel widths to derive.
    #[named]
    #[default(Value::Auto)]
    widths: Value,
    /// The alternative text; empty marks the image as decoration.
    #[named]
    #[default]
    alt: Str,
    /// How wide the element is displayed, as CSS.
    #[named]
    #[default("100vw".into())]
    sizes: Str,
    /// One of auto, jpg (or jpeg), png, or webp; `webp` serves the page.
    #[named]
    #[default("webp".into())]
    format: Str,
    /// JPEG/WebP quality 0–100; `none` selects lossless WebP. PNG ignores it.
    #[named]
    #[default(Some(75))]
    quality: Option<i64>,
    /// An opaque color to composite transparency onto before encoding.
    #[named]
    #[default]
    background: Option<Color>,
    /// The remaining HTML attributes, written unchanged.
    #[named]
    #[default]
    attrs: Dict,
) -> SourceResult<Content> {
    let span = source.span;
    let mut derivatives =
        request_derivatives(engine, source, &widths, &format, quality, background, span)?;
    let srcset = srcset_attribute(&derivatives);
    let widest = derivatives
        .pop()
        .expect("`requested_widths` never returns an empty list");
    let mut element = HtmlElem::new(tag::img)
        .with_attr(attr::src, widest.url.as_str())
        .with_attr(attr::width, widest.width.to_string())
        .with_attr(attr::height, widest.height.to_string())
        .with_attr(attr::alt, alt.as_str());
    if !derivatives.is_empty() {
        element = element
            .with_attr(attr::srcset, srcset)
            .with_attr(attr::sizes, sizes.as_str());
    }
    let mut extras = Vec::with_capacity(attrs.len());
    for (name, value) in attrs {
        let name = name.as_str();
        if let Some(owned) = IMG_OWNED_ATTRIBUTES
            .into_iter()
            .find(|owned| name.eq_ignore_ascii_case(owned))
        {
            bail!(span, "`img` writes `{owned}`; remove it from `attrs`");
        }
        let value = value.cast::<EcoString>().at(span)?;
        extras.push((name.to_owned(), value));
    }
    let mut loading = None;
    let mut decoding = None;
    for (name, value) in &extras {
        if name.eq_ignore_ascii_case("loading") {
            loading = Some(value.as_str());
        } else if name.eq_ignore_ascii_case("decoding") {
            decoding = Some(value.as_str());
        }
    }
    element = element
        .with_attr(attr::loading, loading.unwrap_or("lazy"))
        .with_attr(attr::decoding, decoding.unwrap_or("async"));
    for (name, value) in extras {
        if name.eq_ignore_ascii_case("loading") || name.eq_ignore_ascii_case("decoding") {
            continue;
        }
        element = element.with_attr(HtmlAttr::intern(&name).at(span)?, value);
    }
    Ok(element.pack().spanned(span))
}

/// The derivatives one `widths` request names, ascending.
fn request_derivatives(
    engine: &mut Engine,
    source: Spanned<DataSource>,
    widths: &Value,
    format: &Str,
    quality: Option<i64>,
    background: Option<Color>,
    span: Span,
) -> SourceResult<Vec<Derivative>> {
    let format = output_format(format, span)?;
    let quality = checked_quality(quality, span)?;
    let background = checked_background(background, span)?;
    // The source is loaded first: `widths: auto` names its own width.
    let source = load_source_image(engine, source)?;
    if source.svg.is_some() {
        bail!(
            span,
            "a responsive set cannot be derived from an SVG; use `resize-image` with an explicit raster format, or `image()` for vector display"
        );
    }
    let widths = requested_widths(widths, &source.metadata, span)?;
    // Every derivative keeps the source's aspect ratio, so only its width differs.
    let shared = ResizeOptions {
        operation: ResizeOperation::FitWidth,
        format,
        quality,
        background,
        ..ResizeOptions::default()
    };
    widths
        .iter()
        .map(|width| {
            let options = ResizeOptions {
                width: Some(width.get()),
                ..shared
            };
            request_derivative(engine, span, &source, options)
        })
        .collect()
}

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
    use typst::visualize::{LinearRgb, ProcessColor};

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
    fn translucent_background_is_rejected() {
        let color = Color::WHITE.with_alpha(0.999);
        assert!(opaque_srgb_background(color, Span::detached()).is_err());
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
