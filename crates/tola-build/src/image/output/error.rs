use tola_diagnostic::{Diagnostic, DiagnosticError, Severity, collapse_whitespace};
use tola_image::ImageError;
use tola_typst::svg::SvgError;

fn file_failure_reason(err: &typst::diag::FileError) -> &'static str {
    use typst::diag::FileError;
    match err {
        FileError::NotFound(_) => "it does not exist",
        FileError::AccessDenied => "Tola is not allowed to read it",
        FileError::IsDirectory => "it is a directory, not a file",
        FileError::NotSource => "it is not a supported file",
        FileError::InvalidUtf8 => "its contents are not valid UTF-8",
        FileError::Realize(_) => "its path is not valid on this platform",
        FileError::Package(_) => "the package could not be loaded",
        FileError::Other(_) => "the file could not be read",
    }
}

pub(super) enum ImageOperation {
    Read,
    Resize,
    Rasterize,
    RasterizeSvg,
}

impl ImageOperation {
    fn message(self, file: &str) -> String {
        match self {
            Self::Read => format!("Tola could not read the image `{file}`"),
            Self::Resize => format!("Tola could not resize the image `{file}`"),
            Self::Rasterize => format!("Tola could not rasterize the image `{file}`"),
            Self::RasterizeSvg => format!("Tola could not rasterize the SVG `{file}`"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(super) enum ImageCause {
    #[error("{0}")]
    File(#[source] typst::diag::FileError),
    #[error("{0}")]
    Image(#[source] ImageError),
    #[error("{0}")]
    Svg(#[source] SvgError),
    #[error("the raster geometry is not renderable")]
    RasterGeometry,
}

impl ImageCause {
    fn note(&self) -> String {
        match self {
            Self::File(err) => file_failure_reason(err).to_owned(),
            Self::Image(err) => err.note(),
            Self::Svg(err) => collapse_whitespace(&err.to_string()),
            Self::RasterGeometry => self.to_string(),
        }
    }

    fn help(&self) -> Option<String> {
        match self {
            Self::Image(err) => err.help(),
            Self::File(_) | Self::Svg(_) | Self::RasterGeometry => None,
        }
    }
}

pub(super) fn image_failure(file: &str, op: ImageOperation, cause: ImageCause) -> anyhow::Error {
    let msg = op.message(file);
    let mut diag = Diagnostic::new(
        tola_diagnostic::codes::build::SITE,
        Severity::Error,
        msg.clone(),
    )
    .with_note(cause.note());
    if let Some(help) = cause.help() {
        diag = diag.with_help(help);
    }
    let err = match cause {
        ImageCause::File(err) => anyhow::Error::new(err),
        ImageCause::Image(err) => anyhow::Error::new(err),
        ImageCause::Svg(err) => anyhow::Error::new(err),
        ImageCause::RasterGeometry => anyhow::Error::new(cause),
    };
    DiagnosticError::attach(err.context(msg), vec![diag]).into()
}
