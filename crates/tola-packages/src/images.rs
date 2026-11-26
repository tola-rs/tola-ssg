//! Native image inspection and declarative derivative requests.
//!
//! Sources load via Typst's path loader (never remapped by asset URLs), and exported `Func` values
//! are kept unwrapped so strings retain call-site spans and paths retain their site/package root.

#![expect(
    clippy::too_many_arguments,
    reason = "`#[func]` clears attributes on the function it rewrites"
)]

use std::num::NonZeroU32;

use tola_image::{
    Background, CropAnchor, ImageFormat, ImageMetadata, ImageRecipe, OutputFormat, ResizeFilter,
    ResizeOperation, ResizeOptions, SvgRender, inspect,
};
use tola_typst::ContentDigest;
use tola_typst::svg::{SvgSource, looks_like_svg};
use typst::World;
use typst::diag::{At, SourceResult, bail};
use typst::ecow::EcoString;
use typst::engine::Engine;
use typst::foundations::{
    Array, Content, Dict, IntoValue, NativeElement, PathOrStr, Repr, Str, Value, func,
};
use typst::loading::{DataSource, Load, LoadSource};
use typst::syntax::{FileId, RootedPath, Span, SpanKind, Spanned, VirtualPath, VirtualRoot};
use typst::visualize::Color;
use typst_html::{HtmlAttr, HtmlElem, attr, tag};

use crate::image_request::ImageRequest;

const ASSET_URL_SHADOWED_ORIGIN: &str = "AssetUrlShadowed";

/// Physical image path that also names an asset published from a different source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetUrlShadowed<'a> {
    pub written: &'a str,
    pub resolved: &'a str,
    pub published_from: &'a str,
}

impl<'a> AssetUrlShadowed<'a> {
    /// Decodes only the producer's structured origin fields.
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
                .find_map(|(k, v)| (k == name).then_some(v.as_str()))
        };
        Some(Self {
            written: field("written")?,
            resolved: field("resolved")?,
            published_from: field("published_from")?,
        })
    }
}

pub(crate) const IMAGE_METADATA_FIELDS: [&str; 6] =
    ["width", "height", "format", "mime", "has-alpha", "is-lossy"];

#[func]
pub(super) fn image_metadata(
    engine: &mut Engine,
    /// A path string relative to this call, or an already resolved path.
    source: Spanned<DataSource>,
) -> SourceResult<Dict> {
    let meta = load_source_image(engine, source)?.meta;
    Ok(crate::dict(IMAGE_METADATA_FIELDS.into_iter().zip([
        meta.width.into_value(),
        meta.height.into_value(),
        meta.format.extension().into_value(),
        meta.format.mime().into_value(),
        meta.has_alpha.into_value(),
        meta.is_lossy.into_value(),
    ])))
}

fn parse_op(op: &str, span: Span) -> SourceResult<ResizeOperation> {
    Ok(match op {
        "scale" => ResizeOperation::Scale,
        "fit-width" => ResizeOperation::FitWidth,
        "fit-height" => ResizeOperation::FitHeight,
        "fit" => ResizeOperation::Fit,
        "fill" => ResizeOperation::Fill,
        _ => bail!(
            span,
            "`op` must be `scale`, `fit-width`, `fit-height`, `fit`, or `fill`, not `{op}`"
        ),
    })
}

fn parse_anchor(anchor: &str, span: Span) -> SourceResult<CropAnchor> {
    Ok(match anchor {
        "center" => CropAnchor::Center,
        "top" => CropAnchor::Top,
        "bottom" => CropAnchor::Bottom,
        "left" => CropAnchor::Left,
        "right" => CropAnchor::Right,
        "top-left" => CropAnchor::TopLeft,
        "top-right" => CropAnchor::TopRight,
        "bottom-left" => CropAnchor::BottomLeft,
        "bottom-right" => CropAnchor::BottomRight,
        _ => bail!(
            span,
            "`anchor` must be `center`, `top`, `bottom`, `left`, `right`, `top-left`, `top-right`, `bottom-left`, or `bottom-right`, not `{anchor}`"
        ),
    })
}

fn parse_filter(filter: &str, span: Span) -> SourceResult<ResizeFilter> {
    Ok(match filter {
        "nearest" => ResizeFilter::Nearest,
        "triangle" => ResizeFilter::Triangle,
        "catmull-rom" => ResizeFilter::CatmullRom,
        "gaussian" => ResizeFilter::Gaussian,
        "lanczos3" => ResizeFilter::Lanczos3,
        _ => bail!(
            span,
            "`filter` must be `nearest`, `triangle`, `catmull-rom`, `gaussian`, or `lanczos3`, not `{filter}`"
        ),
    })
}

fn parse_format(format: &str, span: Span) -> SourceResult<OutputFormat> {
    Ok(match format {
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
            "`format` must be `auto`, `jpg` (or `jpeg`), `png`, or `webp`, not `{format}`"
        ),
    })
}

fn checked_quality(quality: Option<i64>, span: Span) -> SourceResult<Option<u8>> {
    if let Some(invalid) = quality.filter(|q| !(0..=100).contains(q)) {
        bail!(span, "`quality` must be between 0 and 100, not `{invalid}`");
    }
    Ok(quality.map(|q| q as u8))
}

fn checked_background(background: Option<Color>, span: Span) -> SourceResult<Option<Background>> {
    background
        .map(|c| opaque_srgb_background(c, span))
        .transpose()
}

struct SourceImage {
    src: DataSource,
    origin: RootedPath,
    digest: ContentDigest,
    meta: ImageMetadata,
    svg: Option<SvgSource>,
}

struct Derivative {
    url: String,
    width: u32,
    height: u32,
}

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
        .map_err(|e| e.to_string())
        .at(span)?;
    let meta = match &svg {
        Some(svg) => {
            let (width, height) = svg.pixel_dimensions();
            ImageMetadata {
                width,
                height,
                format: ImageFormat::Svg,
                has_alpha: true,
                is_lossy: false,
            }
        }
        None => inspect(&loaded.data).map_err(|e| e.to_string()).at(span)?,
    };
    Ok(SourceImage {
        src: written,
        origin,
        digest: ContentDigest::of(&loaded.data),
        meta,
        svg,
    })
}

fn request_derivative(
    engine: &mut Engine,
    span: Span,
    img: &SourceImage,
    opts: ResizeOptions,
) -> SourceResult<Derivative> {
    let recipe = match &img.svg {
        Some(svg) => ImageRecipe::resolve_svg(
            (svg.width(), svg.height()),
            opts,
            SvgRender::new(svg.hash()),
        ),
        None => ImageRecipe::resolve(&img.meta, opts),
    }
    .map_err(|e| e.to_string())
    .at(span)?;
    let req = ImageRequest::new(img.src.clone(), img.origin.clone(), img.digest, recipe);
    let url = super::library::browser_output_url(engine, span, &req.output_path())?;
    // Tracked read requests publication even when the caller discards the returned dictionary.
    engine.world.file(req.observation_id()).at(span)?;
    Ok(Derivative {
        url,
        width: req.recipe().width(),
        height: req.recipe().height(),
    })
}

fn pixel_widths(widths: &Array, span: Span) -> SourceResult<Vec<NonZeroU32>> {
    if widths.is_empty() {
        bail!(span, "`widths` must name at least one width");
    }
    let mut out = Vec::with_capacity(widths.len());
    for w in widths {
        let Ok(w) = w.clone().cast::<NonZeroU32>() else {
            bail!(
                span,
                "`widths` must hold positive pixel counts, not `{}`",
                w.repr()
            );
        };
        out.push(w);
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

fn requested_widths(
    widths: &Value,
    meta: &ImageMetadata,
    span: Span,
) -> SourceResult<Vec<NonZeroU32>> {
    match widths {
        Value::Auto => {
            let Some(w) = NonZeroU32::new(meta.width) else {
                bail!(span, "the source image has no width to derive");
            };
            Ok(vec![w])
        }
        Value::Array(arr) => pixel_widths(arr, span),
        _ => bail!(
            span,
            "`widths` must be `auto` or a list of positive pixel counts"
        ),
    }
}

fn srcset_attr(derivatives: &[Derivative]) -> String {
    derivatives
        .iter()
        .map(|d| format!("{} {}w", d.url, d.width))
        .collect::<Vec<_>>()
        .join(", ")
}

#[func]
pub(super) fn resize_image(
    engine: &mut Engine,
    /// A path string relative to this call, or an already resolved path.
    source: Spanned<DataSource>,
    /// Positive target width in pixels. Required for `fit-width`, `scale`, `fit`, and `fill`.
    /// `fit-height` calculates it from the aspect ratio.
    #[named]
    #[default]
    width: Option<NonZeroU32>,
    /// Positive target height in pixels. Required for `fit-height`, `scale`, `fit`, and `fill`.
    /// `fit-width` calculates it from the aspect ratio.
    #[named]
    #[default]
    height: Option<NonZeroU32>,
    /// Resize operation. If omitted, `fill` crops to the requested width and height.
    /// Use `fit-width` or `fit-height` to set one dimension and keep the aspect ratio.
    #[named]
    #[default("fill".into())]
    op: Str,
    /// Which part `fill` keeps when cropping. If omitted, crop around the center.
    /// With `fill` you can also use `top`, `bottom`, `left`, `right`, `top-left`,
    /// `top-right`, `bottom-left`, or `bottom-right`. Other operations accept only `center`.
    #[named]
    #[default("center".into())]
    anchor: Str,
    /// Use `jpg` (or `jpeg`), `png`, or `webp`. If omitted or `auto`, use JPEG for a lossy
    /// source without alpha, and PNG otherwise. For SVG, specify a raster format.
    #[named]
    #[default("auto".into())]
    format: Str,
    /// JPEG accepts 1–100. If omitted or set to `none`, use 75.
    /// WebP uses lossy compression with 0–100. Omit this option or pass `none` for lossless WebP.
    /// PNG ignores this option.
    #[named]
    #[default]
    quality: Option<i64>,
    /// Resampling filter. If omitted, use `lanczos3`. Other choices are `nearest`,
    /// `triangle`, `catmull-rom`, and `gaussian`. SVG ignores this option.
    #[named]
    #[default("lanczos3".into())]
    filter: Str,
    /// An opaque background color. Required when converting an image with alpha to JPEG.
    #[named]
    #[default]
    background: Option<Color>,
) -> SourceResult<Dict> {
    let span = source.span;
    let opts = ResizeOptions {
        width: width.map(NonZeroU32::get),
        height: height.map(NonZeroU32::get),
        operation: parse_op(op.as_str(), span)?,
        anchor: parse_anchor(anchor.as_str(), span)?,
        format: parse_format(format.as_str(), span)?,
        quality: checked_quality(quality, span)?,
        filter: parse_filter(filter.as_str(), span)?,
        background: checked_background(background, span)?,
    };
    let img = load_source_image(engine, source)?;
    let deriv = request_derivative(engine, span, &img, opts)?;
    Ok(crate::dict(RESIZE_IMAGE_FIELDS.into_iter().zip([
        deriv.url.into_value(),
        deriv.width.into_value(),
        deriv.height.into_value(),
        img.meta.width.into_value(),
        img.meta.height.into_value(),
    ])))
}

pub(crate) const RESIZE_IMAGE_FIELDS: [&str; 5] = [
    "url",
    "width",
    "height",
    "original-width",
    "original-height",
];

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
    /// JPEG accepts 1–100, WebP accepts 0–100, and both use 75 if omitted.
    /// Pass `none` for lossless WebP. PNG ignores this option.
    #[named]
    #[default(Some(75))]
    quality: Option<i64>,
    /// An opaque background color. Required when converting an image with alpha to JPEG.
    #[named]
    #[default]
    background: Option<Color>,
) -> SourceResult<Dict> {
    let span = source.span;
    let mut derivs =
        request_derivatives(engine, source, &widths, &format, quality, background, span)?;
    let srcset = srcset_attr(&derivs);
    let widest = derivs.pop().expect("requested_widths is nonempty");
    Ok(crate::dict(IMAGE_SRCSET_FIELDS.into_iter().zip([
        widest.url.into_value(),
        srcset.into_value(),
        widest.width.into_value(),
        widest.height.into_value(),
    ])))
}

const IMG_OWNED_ATTRS: [&str; 6] = ["src", "srcset", "sizes", "width", "height", "alt"];

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
    /// Tell the browser how wide the image will appear. If omitted, use `100vw` (full viewport).
    /// The attribute is written when at least two distinct widths are generated.
    #[named]
    #[default("100vw".into())]
    sizes: Str,
    /// One of auto, jpg (or jpeg), png, or webp; `webp` serves the page.
    #[named]
    #[default("webp".into())]
    format: Str,
    /// JPEG accepts 1–100, WebP accepts 0–100, and both use 75 if omitted.
    /// Pass `none` for lossless WebP. PNG ignores this option.
    #[named]
    #[default(Some(75))]
    quality: Option<i64>,
    /// An opaque background color. Required when converting an image with alpha to JPEG.
    #[named]
    #[default]
    background: Option<Color>,
    /// The remaining HTML attributes, written unchanged.
    #[named]
    #[default]
    attrs: Dict,
) -> SourceResult<Content> {
    let span = source.span;
    let mut derivs =
        request_derivatives(engine, source, &widths, &format, quality, background, span)?;
    let srcset = srcset_attr(&derivs);
    let widest = derivs.pop().expect("requested_widths is nonempty");
    let mut elem = HtmlElem::new(tag::img)
        .with_attr(attr::src, widest.url.as_str())
        .with_attr(attr::width, widest.width.to_string())
        .with_attr(attr::height, widest.height.to_string())
        .with_attr(attr::alt, alt.as_str());
    if !derivs.is_empty() {
        elem = elem
            .with_attr(attr::srcset, srcset)
            .with_attr(attr::sizes, sizes.as_str());
    }
    let img_attrs = img_extras(attrs, span)?;
    elem = elem
        .with_attr(
            attr::loading,
            img_attrs.loading.unwrap_or_else(|| "lazy".into()),
        )
        .with_attr(
            attr::decoding,
            img_attrs.decoding.unwrap_or_else(|| "async".into()),
        );
    for (name, val) in img_attrs.extras {
        elem = elem.with_attr(HtmlAttr::intern(&name).at(span)?, val);
    }
    Ok(elem.pack().spanned(span))
}

struct ImgAttrs {
    loading: Option<EcoString>,
    decoding: Option<EcoString>,
    extras: Vec<(String, EcoString)>,
}

fn img_extras(attrs: Dict, span: Span) -> SourceResult<ImgAttrs> {
    let mut loading = None;
    let mut decoding = None;
    let mut extras = Vec::with_capacity(attrs.len());
    for (name, val) in attrs {
        let name = name.as_str();
        if let Some(owned) = IMG_OWNED_ATTRS
            .into_iter()
            .find(|o| name.eq_ignore_ascii_case(o))
        {
            bail!(span, "`img` writes `{owned}`; remove it from `attrs`");
        }
        let val = val.cast::<EcoString>().at(span)?;
        if name.eq_ignore_ascii_case("loading") {
            loading = Some(val);
        } else if name.eq_ignore_ascii_case("decoding") {
            decoding = Some(val);
        } else {
            extras.push((name.to_owned(), val));
        }
    }
    Ok(ImgAttrs {
        loading,
        decoding,
        extras,
    })
}

fn request_derivatives(
    engine: &mut Engine,
    source: Spanned<DataSource>,
    widths: &Value,
    format: &Str,
    quality: Option<i64>,
    background: Option<Color>,
    span: Span,
) -> SourceResult<Vec<Derivative>> {
    let format = parse_format(format.as_str(), span)?;
    let quality = checked_quality(quality, span)?;
    let background = checked_background(background, span)?;
    let img = load_source_image(engine, source)?;
    if img.svg.is_some() {
        bail!(
            span,
            "a responsive set cannot be derived from an SVG; use `resize-image` with an explicit raster format, or `image()` for vector display"
        );
    }
    let widths = requested_widths(widths, &img.meta, span)?;
    let shared = ResizeOptions {
        operation: ResizeOperation::FitWidth,
        format,
        quality,
        background,
        ..ResizeOptions::default()
    };
    widths
        .iter()
        .map(|w| {
            request_derivative(
                engine,
                span,
                &img,
                ResizeOptions {
                    width: Some(w.get()),
                    ..shared
                },
            )
        })
        .collect()
}

fn opaque_srgb_background(color: Color, span: Span) -> SourceResult<Background> {
    if color.alpha().is_some_and(|a| a != 1.0) {
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
        PathOrStr::Str(s) => s.as_str(),
        PathOrStr::Path(p) => p.vpath().get_without_slash(),
    };
    let named = written.split('?').next().unwrap_or_default();
    let Some(origins) = super::library::asset_origins(engine) else {
        return Ok(());
    };
    let Some(published_from) = crate::dict_str(origins, named.trim_start_matches('/')) else {
        return Ok(());
    };
    if id.root() != &VirtualRoot::Project {
        return Ok(());
    }
    let resolved = id.vpath().get_without_slash();
    if resolved == published_from {
        return Ok(());
    }
    // Keep numbered call-site source tracked for native evaluation.
    if let SpanKind::Number { id, num } = span.get() {
        let src = engine.world.source(id).at(span)?;
        src.range(num, None)
            .expect("native argument span belongs to its source");
    }
    let warning = tola_typst::diagnostic::producer_warning(
        span,
        ASSET_URL_SHADOWED_ORIGIN,
        &[
            ("written", named),
            ("resolved", resolved),
            ("published_from", published_from),
        ],
        format!(
            "`{named}` is also a published asset URL; this reads `{resolved}`, while the site \
             publishes that URL from `{published_from}`"
        ),
    )
    .with_hint("Write the file the declaration publishes from, or use the URL where a URL is expected (`asset-url(\"…\")`)");
    engine.sink.warn(warning);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::visualize::{LinearRgb, ProcessColor};

    #[test]
    fn background_linear_srgb() {
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
    fn background_rejects_alpha() {
        let color = Color::WHITE.with_alpha(0.999);
        assert!(opaque_srgb_background(color, Span::detached()).is_err());
    }

    #[test]
    fn asset_url_shadowed_reordered_fields() {
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
        let bytes = serde_json::to_vec(native.origin()).unwrap();
        let origin: tola_typst::DiagnosticOrigin = serde_json::from_slice(&bytes).unwrap();
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
