//! The next step one image failure asks of its author.

use std::error::Error;

use crate::recipe::OutputFormat;
use crate::{ImageCancelled, ImageFormat};

/// The result of one image operation.
pub type ImageResult<T> = std::result::Result<T, ImageError>;

/// Why one image operation could not produce its result.
///
/// A variant exists only where the author's next step differs from every other failure's. Every
/// other failure keeps the decoder's or codec's own sentence in [`ImageError::Decode`].
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    /// The caller cancelled the work before the image was produced.
    #[error("{0}")]
    Cancelled(#[from] ImageCancelled),
    /// The image uses a feature or colour model Tola cannot read; re-exporting it is the fix.
    #[error("{}", reexport_sentence(format, problem))]
    Reexport {
        /// The container the refused image is in.
        format: ImageFormat,
        /// What the container declares that Tola cannot read.
        problem: ReexportProblem,
    },
    /// The image, or one plane it needs, exceeds a size this operation admits.
    #[error("{}", too_large_sentence(limit, allowed))]
    TooLarge {
        /// The operation that refused the size.
        limit: SizeLimit,
        /// The largest size the operation admits, in the unit the limit names.
        allowed: u64,
        /// The size the image or plane actually needs, in the same unit.
        actual: u64,
    },
    /// The operation works on raster images only.
    #[error("{}", needs_raster_sentence())]
    NeedsRaster,
    /// This build does not encode the requested format.
    #[error("{}", encoding_unavailable_sentence(format))]
    EncodingUnavailable {
        /// The requested output format.
        format: OutputFormat,
    },
    /// Anything else: bytes Tola cannot read, a damaged container, a codec's own refusal.
    #[error("{reason}")]
    Decode {
        /// The container, when the failure names one.
        format: Option<ImageFormat>,
        /// The failing decoder's or codec's own sentence.
        #[source]
        reason: Box<dyn Error + Send + Sync>,
    },
}

/// What a container declares that Tola cannot read.
#[derive(Clone, Copy, Debug)]
pub enum ReexportProblem {
    /// A chunk or feature of the container that Tola does not implement.
    UnsupportedFeature,
    /// A colour model (CMYK or YCCK) Tola does not read.
    ColorModel,
}

/// The operation whose size limit one image exceeded.
#[derive(Clone, Copy, Debug)]
pub enum SizeLimit {
    /// Decoding the source into its own plane.
    Decode,
    /// Holding a decoded or working plane in memory.
    HoldInMemory,
    /// One axis of a JPEG frame.
    FrameAxis,
}

impl ImageError {
    /// The consequence the site author reads, without the next step.
    pub fn note(&self) -> String {
        match self {
            Self::Reexport { format, problem } => reexport_note(format, problem),
            Self::TooLarge { limit, allowed, .. } => too_large_note(limit, allowed),
            Self::NeedsRaster => needs_raster_note().to_owned(),
            Self::EncodingUnavailable { format } => encoding_unavailable_note(format),
            Self::Decode { reason, .. } => reason.to_string(),
            Self::Cancelled(cancelled) => cancelled.to_string(),
        }
    }

    /// The imperative next step this class of failure has, when it has one.
    pub fn help(&self) -> Option<String> {
        match self {
            Self::Reexport { problem, .. } => Some(reexport_help(problem)),
            Self::TooLarge { limit, allowed, .. } => Some(too_large_help(limit, allowed)),
            Self::NeedsRaster => Some(needs_raster_help().to_owned()),
            Self::EncodingUnavailable { .. } => Some(encoding_unavailable_help().to_owned()),
            Self::Decode { .. } | Self::Cancelled(_) => None,
        }
    }

    /// Wrap one failure: a classified failure keeps its variant, anything else is a decode reason.
    pub(crate) fn wrapping(error: anyhow::Error, format: Option<ImageFormat>) -> Self {
        if error.chain().any(|cause| cause.is::<ImageCancelled>()) {
            return Self::Cancelled(ImageCancelled);
        }
        match error.downcast::<Self>() {
            Ok(classified) => classified,
            Err(error) => Self::Decode {
                format,
                reason: Box::from(error),
            },
        }
    }
}

/// The imperative a re-export failure shares, after the semicolon of its sentence.
fn reexport_sentence(format: &ImageFormat, problem: &ReexportProblem) -> String {
    format!(
        "{}; {}",
        reexport_note(format, problem),
        reexport_help(problem)
    )
}

fn reexport_note(format: &ImageFormat, problem: &ReexportProblem) -> String {
    match problem {
        ReexportProblem::UnsupportedFeature => {
            format!(
                "the {} image uses a feature Tola cannot read",
                source_name(*format)
            )
        }
        ReexportProblem::ColorModel => "CMYK and YCCK JPEG images are not supported".to_owned(),
    }
}

fn reexport_help(problem: &ReexportProblem) -> String {
    match problem {
        ReexportProblem::UnsupportedFeature => "re-export it".to_owned(),
        ReexportProblem::ColorModel => "re-export the image as RGB".to_owned(),
    }
}

/// The largest-image sentence one size limit renders.
fn too_large_sentence(limit: &SizeLimit, allowed: &u64) -> String {
    let note = too_large_note(limit, allowed);
    match limit {
        // The frame's own wording already states its limit; nothing follows it.
        SizeLimit::FrameAxis => note,
        // The author read this sentence before help existed; keep it byte for byte.
        SizeLimit::Decode | SizeLimit::HoldInMemory => format!("{note}; {SMALLER_IMAGE}"),
    }
}

fn too_large_note(limit: &SizeLimit, allowed: &u64) -> String {
    match limit {
        SizeLimit::Decode => "the image is too large to decode".to_owned(),
        SizeLimit::HoldInMemory => "the image is too large to hold in memory".to_owned(),
        SizeLimit::FrameAxis => {
            format!("a JPEG frame holds at most {allowed} pixels per axis")
        }
    }
}

fn too_large_help(limit: &SizeLimit, allowed: &u64) -> String {
    match limit {
        SizeLimit::Decode => format!("use an image no larger than {}", size(*allowed)),
        SizeLimit::HoldInMemory => SMALLER_IMAGE.to_owned(),
        SizeLimit::FrameAxis => {
            format!("use an image no larger than {allowed} pixels per axis")
        }
    }
}

/// What the author read before the next step moved out of the note.
const SMALLER_IMAGE: &str = "use a smaller image";

fn needs_raster_sentence() -> String {
    format!("{}; {}", needs_raster_note(), needs_raster_help())
}

fn needs_raster_note() -> &'static str {
    "`resize-image` cannot resize an SVG or XML file"
}

fn needs_raster_help() -> &'static str {
    "use a PNG, JPEG, WebP, GIF, or BMP image"
}

fn encoding_unavailable_sentence(format: &OutputFormat) -> String {
    encoding_unavailable_note(format)
}

fn encoding_unavailable_note(format: &OutputFormat) -> String {
    format!(
        "{} encoding is not enabled in this build",
        output_name(*format)
    )
}

fn encoding_unavailable_help() -> &'static str {
    "use a build with that encoding enabled, or choose another `format`"
}

/// The short name one source container has in a sentence.
fn source_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Png => "PNG",
        ImageFormat::WebP => "WebP",
        ImageFormat::Gif => "GIF",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Svg => "SVG",
    }
}

/// The short name one requested output format has in a sentence.
fn output_name(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Auto => "automatic",
        OutputFormat::Jpeg => "JPEG",
        OutputFormat::Png => "PNG",
        OutputFormat::WebP => "WebP",
    }
}

/// One byte budget as an author reads it.
fn size(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else {
        format!("{bytes} bytes")
    }
}
