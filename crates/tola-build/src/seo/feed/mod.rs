//! RSS, Atom, and JSON Feed exports of explicit Bundle declarations.

mod atom;
mod content;
mod declaration;
mod document;
mod html;
mod json;
mod rss;
pub(super) use content::FeedContentViolation;

use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::config::ResolvedSiteConfig;
use crate::seo::RenderError;
use crate::seo::declaration::SeoDeclaration;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FeedFormat {
    #[default]
    Rss,
    Atom,
    Json,
}

impl FeedFormat {
    const ALL: [Self; 3] = [Self::Rss, Self::Atom, Self::Json];

    pub(crate) fn from_authored_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|format| format.as_str() == name)
    }

    pub(crate) const fn mime_type(self) -> &'static str {
        match self {
            Self::Rss => "application/rss+xml",
            Self::Atom => "application/atom+xml",
            Self::Json => "application/feed+json",
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rss => "rss",
            Self::Atom => "atom",
            Self::Json => "json",
        }
    }

    /// Everything rendering one feed of this format needs beyond its declaration.
    const fn rendering(self) -> FeedRendering {
        match self {
            Self::Rss => FeedRendering {
                serialize: rss::render,
                declaration: crate::output::semantics::OutputDeclaration::rss_feed(),
                xml: true,
            },
            Self::Atom => FeedRendering {
                serialize: atom::render,
                declaration: crate::output::semantics::OutputDeclaration::atom_feed(),
                xml: true,
            },
            Self::Json => FeedRendering {
                serialize: json::render,
                declaration: crate::output::semantics::OutputDeclaration::json_feed(),
                xml: false,
            },
        }
    }
}

/// One feed format's serializer, published output semantics, and XML representation.
struct FeedRendering {
    serialize: fn(&declaration::Feed, &BuildCancellation) -> anyhow::Result<String>,
    declaration: crate::output::semantics::OutputDeclaration,
    xml: bool,
}

/// Classify one feed serializer failure as cancellation or an invalid `/output` declaration.
fn serialize_error(declaration: &SeoDeclaration, error: anyhow::Error) -> RenderError {
    match error.downcast::<BuildCancelled>() {
        Ok(cancelled) => RenderError::Cancelled(cancelled),
        Err(_) => declaration
            .invalid("", "the feed could not be serialized")
            .into(),
    }
}

pub(super) fn render_outputs(
    config: &ResolvedSiteConfig,
    compilation: &tola_typst::BundleCompilation,
    cancellation: &BuildCancellation,
) -> Result<super::RenderedOutputs, RenderError> {
    cancellation.ensure_active()?;
    let mut document_contents = document::DocumentContents::new(config, compilation);
    let outputs = compilation
        .metadata_declarations("tola-feed")
        .into_iter()
        .map(|raw| {
            cancellation.ensure_active()?;
            let declaration = SeoDeclaration::parse("tola-feed", raw)?;
            let feed = declaration::Feed::parse(
                &declaration,
                config,
                compilation,
                cancellation,
                &mut document_contents,
            )?;
            let rendering = feed.format.rendering();
            let content = (rendering.serialize)(&feed, cancellation)
                .map_err(|error| serialize_error(&declaration, error))?;
            cancellation.ensure_active()?;
            if rendering.xml {
                roxmltree::Document::parse(&content).map_err(|_| {
                    declaration.invalid(
                        "",
                        "the generated feed is not well-formed XML; check the feed's entries for text that cannot be serialized",
                    )
                })?;
            }
            cancellation.ensure_active()?;
            let url = tola_address::asset_url_from_output(&feed.output);
            Ok(super::RenderedOutput {
                producer: "feed",
                url,
                declaration: rendering.declaration,
                bytes: content.into_bytes().into(),
            })
        })
        .collect::<Result<Vec<_>, RenderError>>()?;
    Ok(super::RenderedOutputs {
        outputs,
        warnings: document_contents.into_warnings(),
    })
}

// Bound each feed-buffer copy between cancellation checks.
const FEED_WRITE_CHUNK_BYTES: usize = 64 * 1024;

/// Buffer used by the feed serializers so their own write loops remain cancellable.
struct FeedBuffer<'a> {
    bytes: Vec<u8>,
    cancellation: &'a BuildCancellation,
}

impl<'a> FeedBuffer<'a> {
    fn new(cancellation: &'a BuildCancellation) -> Self {
        Self {
            bytes: Vec::new(),
            cancellation,
        }
    }

    fn into_text(self) -> anyhow::Result<String> {
        self.cancellation.ensure_active()?;
        String::from_utf8(self.bytes)
            .map_err(|_| anyhow::anyhow!("the generated feed is not UTF-8 text"))
    }
}

/// Serialize one feed document into a buffer bounded by the same cancellation contract.
fn write_serialized(
    cancellation: &BuildCancellation,
    serialize: impl FnOnce(&mut FeedBuffer<'_>) -> anyhow::Result<()>,
) -> anyhow::Result<String> {
    cancellation.ensure_active()?;
    let mut output = FeedBuffer::new(cancellation);
    let serialized = serialize(&mut output);
    cancellation.ensure_active()?;
    serialized?;
    output.into_text()
}

/// Map the authors of one feed or entry, observing cancellation per author.
fn map_authors<'a, T>(
    authors: &'a [declaration::Author],
    cancellation: &BuildCancellation,
    convert: impl Fn(&'a declaration::Author) -> T,
) -> Result<Vec<T>, BuildCancelled> {
    authors
        .iter()
        .map(|author| {
            cancellation.ensure_active()?;
            Ok(convert(author))
        })
        .collect()
}

impl std::io::Write for FeedBuffer<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        for chunk in bytes.chunks(FEED_WRITE_CHUNK_BYTES) {
            self.cancellation
                .ensure_active()
                .map_err(std::io::Error::other)?;
            self.bytes.extend_from_slice(chunk);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.cancellation
            .ensure_active()
            .map_err(std::io::Error::other)
    }
}
