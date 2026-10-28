//! Complete image outputs derived from replayable native request observations.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, ensure};
use rayon::prelude::*;
use tola_typst::svg::{SvgCrop, SvgSource, looks_like_svg};
use tola_typst::{AccessedDeps, ContentDigest, ReadEvidence, TypstWorld};
use typst::comemo::Track;
use typst::foundations::Bytes;
use typst::loading::DataSource;
use typst::syntax::FileId;

use crate::cancellation::BuildCancellation;
use crate::compiler::BuildInputs;
use crate::diagnostic::{Diagnostic, Severity};
use crate::output::graph::OutputGraphBuilder;
use crate::output::semantics::{OutputDeclaration, ResponseMediaType};
use tola_address::OutputPath;
use tola_image::{
    DecodedSource, ImageFormat, ImageMetadata, ImageRecipe, PixelRecipe, ResampledPixels, inspect,
};
use tola_packages::ImageRequest;

use crate::image::{PixelCache, VariantCache};
use crate::resources::BuildResources;

/// Encoded variants retained only with the complete successful build that owns them.
#[derive(Default)]
pub(crate) struct ImageOutputs {
    variants: BTreeMap<OutputPath, EncodedVariant>,
}

const MAX_IMAGE_WORKERS: usize = 4;

/// One batch admits at most this sum of decoded RGBA bytes plus each source's largest output
/// RGBA plane. A larger source runs alone. Codec scratch, row buffers, encoded outputs and the
/// separate pixel-cache retention are not covered, so this is not a process-memory limit.
const SOURCE_BATCH_BYTES: u64 = 256 * 1024 * 1024;

fn image_workers(cancellation: &BuildCancellation) -> Result<rayon::ThreadPool> {
    cancellation.ensure_active()?;
    let workers = rayon::ThreadPoolBuilder::new()
        .num_threads(MAX_IMAGE_WORKERS.min(rayon::current_num_threads().max(1)))
        .thread_name(|index| format!("tola-image-{index}"))
        .build();
    cancellation.ensure_active()?;
    workers.map_err(|error| {
        let message = "Tola could not prepare image resizing";
        let diagnostic = Diagnostic::new(crate::codes::build::SITE, Severity::Error, message)
            .with_help("Close other programs and run the build again");
        crate::diagnostic::DiagnosticError::attach(
            anyhow::Error::new(error).context(message),
            vec![diagnostic],
        )
        .into()
    })
}

struct EncodedVariant {
    source_digest: ContentDigest,
    recipe: ImageRecipe,
    bytes: Arc<[u8]>,
}

struct SourceImage {
    bytes: Bytes,
    digest: ContentDigest,
    metadata: ImageMetadata,
    svg: Option<SvgSource>,
}

/// One output path this attempt publishes, with the frozen source bytes and recipe it claims.
///
/// A request that names an output another request already claimed must resolve to the same
/// variant, so every claim is recorded before any render starts.
struct VariantClaim {
    source_digest: ContentDigest,
    recipe: ImageRecipe,
}

/// One source's content, shared even when requests read it through different paths.
struct SourceGroup {
    source: usize,
    /// Geometries in the order their first claim appeared.
    geometries: Vec<RenderGeometry>,
}

/// The bytes one pending render produced, or why it failed.
struct RenderOutcome {
    index: usize,
    bytes: Result<Arc<[u8]>>,
}

/// One geometry of one source: the pixels every encoding of it shares.
struct RenderGeometry {
    pixels: PixelRecipe,
    /// Indices into the pending renders, ascending.
    renders: Vec<usize>,
}

/// A claimed variant whose bytes this attempt still has to render.
struct PendingRender {
    output: OutputPath,
    source: usize,
    source_id: FileId,
    /// The identity the stored bytes of this variant are kept under.
    key: ContentDigest,
}

impl SourceGroup {
    fn admission_bytes(&self, source: &SourceImage) -> u64 {
        let decoded = u64::from(source.metadata.width) * u64::from(source.metadata.height);
        let largest_geometry = self
            .geometries
            .iter()
            .map(|geometry| {
                u64::from(geometry.pixels.width()) * u64::from(geometry.pixels.height())
            })
            .max()
            .unwrap_or(0);
        decoded.saturating_add(largest_geometry).saturating_mul(4)
    }

    fn render(
        &self,
        source: &SourceImage,
        pending: &[PendingRender],
        claims: &BTreeMap<OutputPath, VariantClaim>,
        cache: &PixelCache,
        cancellation: &BuildCancellation,
    ) -> Vec<RenderOutcome> {
        let mut outcomes = Vec::with_capacity(
            self.geometries
                .iter()
                .map(|geometry| geometry.renders.len())
                .sum(),
        );
        if let Some(svg) = &source.svg {
            for geometry in &self.geometries {
                let pixels = geometry.pixels;
                let rendered = (|| -> Result<ResampledPixels> {
                    let vector = pixels
                        .vector_geometry()
                        .ok_or_else(|| anyhow::anyhow!("the raster geometry is not renderable"))?;
                    let (x, y, width, height) = vector.crop();
                    let rgba = svg.render(
                        pixels.width(),
                        pixels.height(),
                        Some(SvgCrop {
                            x,
                            y,
                            width,
                            height,
                        }),
                    )?;
                    ResampledPixels::from_rendered(pixels, rgba, cancellation)
                })();
                match rendered {
                    Ok(rendered) => {
                        for index in &geometry.renders {
                            outcomes.push(RenderOutcome {
                                index: *index,
                                bytes: rendered
                                    .encode(&claims[&pending[*index].output].recipe, cancellation),
                            });
                        }
                    }
                    Err(error) => outcomes.push(RenderOutcome {
                        index: geometry.renders[0],
                        bytes: Err(error),
                    }),
                }
            }
            return outcomes;
        }
        let mut missing_geometries = Vec::new();
        // Use this source's retained geometries before its decode or new resamples can evict them.
        // Release each hit after encoding rather than pinning every cached plane for the source.
        for geometry in &self.geometries {
            match cache.resampled(source.digest, geometry.pixels) {
                Some(pixels) => {
                    geometry.encode_into(&pixels, pending, claims, cancellation, &mut outcomes);
                }
                None => missing_geometries.push(geometry),
            }
        }
        let Some(first_missing) = missing_geometries.first() else {
            return outcomes;
        };
        let decoded = match cache.decoded(source.digest) {
            Some(retained) => retained,
            None => match DecodedSource::open(source.bytes.as_slice(), cancellation) {
                Ok(decoded) => {
                    let decoded = Arc::new(decoded);
                    cache.retain_decoded(source.digest, Arc::clone(&decoded));
                    decoded
                }
                Err(error) => {
                    outcomes.push(RenderOutcome {
                        index: first_missing.renders[0],
                        bytes: Err(error),
                    });
                    return outcomes;
                }
            },
        };
        for geometry in missing_geometries {
            match decoded.pixels(geometry.pixels, cancellation) {
                Ok(pixels) => {
                    let pixels = Arc::new(pixels);
                    cache.retain_resampled(source.digest, geometry.pixels, Arc::clone(&pixels));
                    geometry.encode_into(&pixels, pending, claims, cancellation, &mut outcomes);
                }
                Err(error) => outcomes.push(RenderOutcome {
                    index: geometry.renders[0],
                    bytes: Err(error),
                }),
            }
        }
        outcomes
    }
}

/// The integer pixel size an SVG reports at usvg's 96 DPI.
fn svg_pixels(value: f64) -> u32 {
    value.round().clamp(1.0, f64::from(u32::MAX)) as u32
}

impl RenderGeometry {
    fn encode_into(
        &self,
        pixels: &ResampledPixels,
        pending: &[PendingRender],
        claims: &BTreeMap<OutputPath, VariantClaim>,
        cancellation: &BuildCancellation,
        outcomes: &mut Vec<RenderOutcome>,
    ) {
        outcomes.extend(self.renders.iter().map(|index| RenderOutcome {
            index: *index,
            bytes: pixels.encode(&claims[&pending[*index].output].recipe, cancellation),
        }));
    }
}

fn source_batch_len(groups: &[SourceGroup], sources: &[SourceImage], workers: usize) -> usize {
    let mut bytes = 0u64;
    let mut count = 0;
    for group in groups.iter().take(workers) {
        let admitted = bytes.saturating_add(group.admission_bytes(&sources[group.source]));
        if count > 0 && admitted > SOURCE_BATCH_BYTES {
            break;
        }
        bytes = admitted;
        count += 1;
    }
    count
}

fn file_failure_reason(error: &typst::diag::FileError) -> &'static str {
    use typst::diag::FileError;
    match error {
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

/// One image failure: what Tola could not do, with the decoder's own reason.
fn image_failure(
    cause: anyhow::Error,
    message: String,
    reason: impl Into<String>,
) -> anyhow::Error {
    let diagnostic = Diagnostic::new(crate::codes::build::SITE, Severity::Error, message.clone())
        .with_note(one_line(&reason.into()));
    crate::diagnostic::DiagnosticError::attach(cause.context(message), vec![diagnostic]).into()
}

/// A decoder reason may span several lines; the rendered note stays one line.
fn one_line(reason: &str) -> String {
    reason.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl ImageOutputs {
    pub(crate) fn prepare<'a>(
        world: &TypstWorld,
        evidence: impl IntoIterator<Item = &'a ReadEvidence>,
        previous: Option<&Self>,
        site_root: &Path,
        resources: &BuildResources,
        inputs: &mut BuildInputs,
        cancellation: &BuildCancellation,
    ) -> Result<(Arc<Self>, AccessedDeps)> {
        cancellation.ensure_active()?;
        let started = std::time::Instant::now();
        let mut variants = BTreeMap::<OutputPath, EncodedVariant>::new();
        let mut claims = BTreeMap::<OutputPath, VariantClaim>::new();
        let mut pending = Vec::<PendingRender>::new();
        let mut sources = Vec::<SourceImage>::new();
        let mut source_indexes = HashMap::<FileId, usize>::new();
        let mut content_indexes = HashMap::<ContentDigest, usize>::new();
        let mut accessed = AccessedDeps::default();
        let mut reused = 0usize;
        let mut restored = 0usize;
        let mut unwritable = 0usize;
        let variant_cache = VariantCache::new(BuildResources::image_cache_directory(site_root));
        let pixel_cache = resources.pixel_cache();

        // Content aliases share pixels, but every requested path must retain its own read evidence.
        for read in evidence {
            cancellation.ensure_active()?;
            let Some(request) = ImageRequest::from_read(read.locator())? else {
                continue;
            };
            let source_id = FileId::new(request.origin().clone());
            let source_index = match source_indexes.get(&source_id).copied() {
                Some(index) => index,
                None => {
                    let bytes = match request.source() {
                        DataSource::Path(_) => {
                            let (bytes, observed) = world.read_file_with_evidence(source_id);
                            // Preserve missing-source recovery even when the producer stops here.
                            inputs.record_accessed(&observed);
                            accessed.reads.extend(observed.reads);
                            accessed.disk_reads.extend(observed.disk_reads);
                            accessed.package_checks.extend(observed.package_checks);
                            cancellation.ensure_active()?;
                            bytes.map_err(|error| {
                                let reason = file_failure_reason(&error);
                                image_failure(
                                    anyhow::Error::new(error),
                                    format!(
                                        "Tola could not read the image `{}`",
                                        source_id.vpath().get_without_slash()
                                    ),
                                    reason,
                                )
                            })?
                        }
                        DataSource::Bytes(bytes) => bytes.clone(),
                    };
                    let digest = ContentDigest::of(bytes.as_slice());
                    let index = match content_indexes.entry(digest) {
                        std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            let svg = if looks_like_svg(bytes.as_slice()) {
                                Some(
                                    SvgSource::parse(
                                        bytes.clone(),
                                        (world as &dyn typst::World).track(),
                                        source_id,
                                    )
                                    .map_err(|error| {
                                        let reason = error.to_string();
                                        image_failure(
                                            anyhow::Error::new(error),
                                            format!(
                                                "Tola could not rasterize the SVG `{}`",
                                                source_id.vpath().get_without_slash()
                                            ),
                                            reason,
                                        )
                                    })?,
                                )
                            } else {
                                None
                            };
                            let metadata = match &svg {
                                Some(svg) => ImageMetadata {
                                    width: svg_pixels(svg.width()),
                                    height: svg_pixels(svg.height()),
                                    format: ImageFormat::Svg,
                                    has_alpha: true,
                                    is_lossy: false,
                                },
                                None => inspect(bytes.as_slice()).map_err(|error| {
                                    let reason = error.to_string();
                                    image_failure(
                                        error,
                                        format!(
                                            "Tola could not read the image `{}`",
                                            source_id.vpath().get_without_slash()
                                        ),
                                        reason,
                                    )
                                })?,
                            };
                            let index = sources.len();
                            sources.push(SourceImage {
                                bytes,
                                digest,
                                metadata,
                                svg,
                            });
                            *entry.insert(index)
                        }
                    };
                    source_indexes.insert(source_id, index);
                    index
                }
            };
            let source = &sources[source_index];
            ensure!(
                source.digest == request.source_digest(),
                "the image `{}` changed since Tola prepared its variants; run the build again",
                source_id.vpath().get_with_slash(),
            );
            match &source.svg {
                Some(svg) => {
                    request
                        .recipe()
                        .validate_svg(svg.width(), svg.height(), svg.hash())?
                }
                None => request.recipe().validate(&source.metadata)?,
            }
            let output = request.output_path();
            if let Some(existing) = claims.get(&output) {
                ensure!(
                    existing.source_digest == source.digest && existing.recipe == *request.recipe(),
                    "two image variants publish the same output `{output}`; change one recipe or output",
                );
                continue;
            }
            match previous.and_then(|previous| previous.variants.get(&output)) {
                Some(cached)
                    if cached.source_digest == source.digest
                        && cached.recipe == *request.recipe() =>
                {
                    reused += 1;
                    variants.insert(
                        output.clone(),
                        EncodedVariant {
                            source_digest: source.digest,
                            recipe: request.recipe().clone(),
                            bytes: Arc::clone(&cached.bytes),
                        },
                    );
                }
                _ => match variant_cache.read(request.key()) {
                    Some(bytes) => {
                        restored += 1;
                        variants.insert(
                            output.clone(),
                            EncodedVariant {
                                source_digest: source.digest,
                                recipe: request.recipe().clone(),
                                bytes,
                            },
                        );
                    }
                    None => pending.push(PendingRender {
                        output: output.clone(),
                        source: source_index,
                        source_id,
                        key: request.key(),
                    }),
                },
            }
            claims.insert(
                output,
                VariantClaim {
                    source_digest: source.digest,
                    recipe: request.recipe().clone(),
                },
            );
        }

        // Claim indices survive regrouping so a later encoding cannot hide an earlier failure
        // from another geometry or source.
        let mut group_indexes = HashMap::<(usize, PixelRecipe), (usize, usize)>::new();
        let mut groups = Vec::<SourceGroup>::new();
        let mut source_groups = vec![None; sources.len()];
        for (index, job) in pending.iter().enumerate() {
            let pixels = claims[&job.output].recipe.pixels();
            match group_indexes.get(&(job.source, pixels)).copied() {
                Some((group, geometry)) => {
                    groups[group].geometries[geometry].renders.push(index);
                }
                None => {
                    let group = match source_groups[job.source] {
                        Some(group) => group,
                        None => {
                            let group = groups.len();
                            groups.push(SourceGroup {
                                source: job.source,
                                geometries: Vec::new(),
                            });
                            source_groups[job.source] = Some(group);
                            group
                        }
                    };
                    let geometry_index = groups[group].geometries.len();
                    groups[group].geometries.push(RenderGeometry {
                        pixels,
                        renders: vec![index],
                    });
                    group_indexes.insert((job.source, pixels), (group, geometry_index));
                }
            }
        }
        let mut rendered_bytes = vec![None; pending.len()];
        let mut failure = None::<(usize, anyhow::Error)>;
        if !groups.is_empty() {
            let workers = image_workers(cancellation)?;
            workers.install(|| -> Result<()> {
                let mut remaining = groups.as_slice();
                while !remaining.is_empty() {
                    cancellation.ensure_active()?;
                    let count =
                        source_batch_len(remaining, &sources, workers.current_num_threads());
                    let (batch, next) = remaining.split_at(count);
                    // Only this batch enters Rayon. A suspended row task cannot start another
                    // batch, and each admitted source owns at most one active geometry/encoding.
                    let outcomes = batch
                        .par_iter()
                        .map(|group| {
                            group.render(
                                &sources[group.source],
                                &pending,
                                &claims,
                                &pixel_cache,
                                cancellation,
                            )
                        })
                        .collect::<Vec<_>>();
                    for outcome in outcomes.into_iter().flatten() {
                        match outcome.bytes {
                            Ok(bytes) => rendered_bytes[outcome.index] = Some(bytes),
                            Err(error) => {
                                if failure
                                    .as_ref()
                                    .is_none_or(|(index, _)| outcome.index < *index)
                                {
                                    failure = Some((outcome.index, error));
                                }
                            }
                        }
                    }
                    remaining = next;
                }
                Ok(())
            })?;
        }
        cancellation.ensure_active()?;
        if let Some((index, error)) = failure {
            let job = &pending[index];
            let reason = error.to_string();
            let action = if sources[job.source].svg.is_some() {
                "rasterize"
            } else {
                "resize"
            };
            return Err(image_failure(
                error,
                format!(
                    "Tola could not {action} the image `{}`",
                    job.source_id.vpath().get_without_slash()
                ),
                reason,
            ));
        }
        for (job, bytes) in pending.iter().zip(rendered_bytes) {
            let bytes = bytes.expect("every claimed render has its encoded bytes");
            let claim = &claims[&job.output];
            if let Err(error) = variant_cache.store(job.key, &bytes) {
                unwritable += 1;
                tracing::debug!(target: "tola::compile", %error, "image variant not stored");
            }
            variants.insert(
                job.output.clone(),
                EncodedVariant {
                    source_digest: claim.source_digest,
                    recipe: claim.recipe.clone(),
                    bytes,
                },
            );
        }
        let rendered = pending.len();
        cancellation.ensure_active()?;
        tracing::debug!(target: "tola::compile",
            images_ms = started.elapsed().as_secs_f64() * 1000.0,
            sources = source_indexes.len(), variants = variants.len(), rendered, reused, restored, unwritable,
            "prepared image outputs");
        Ok((Arc::new(Self { variants }), accessed))
    }

    pub(crate) fn insert_into(
        &self,
        outputs: &mut OutputGraphBuilder,
        cancellation: &BuildCancellation,
    ) -> Result<()> {
        for (path, variant) in &self.variants {
            cancellation.ensure_active()?;
            let media = response_media_type(variant.recipe.format());
            outputs.insert_system(
                "image",
                path.as_str(),
                OutputDeclaration::image(media),
                Arc::clone(&variant.bytes),
            )?;
        }
        Ok(())
    }
}

/// The response media type the codec's encoded output is published with.
fn response_media_type(format: ImageFormat) -> ResponseMediaType {
    match format {
        ImageFormat::Jpeg => ResponseMediaType::JPEG,
        ImageFormat::Png => ResponseMediaType::PNG,
        ImageFormat::WebP => ResponseMediaType::WEBP,
        ImageFormat::Gif => ResponseMediaType::GIF,
        ImageFormat::Bmp => ResponseMediaType::BMP,
        ImageFormat::Svg => ResponseMediaType::SVG,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;

    use super::*;
    use crate::build::{
        BuildMode, BuildRequest, BuildReuse, BuildSession, BuildTrigger, RevisionCheck,
    };
    use crate::config::ResolvedSiteConfig;
    use crate::config::tests::OwnedSiteConfig;

    fn config_with_program(program: &str) -> OwnedSiteConfig {
        let site_config = OwnedSiteConfig::new("[site]\nbase-path = '/docs/'\n");
        fs::create_dir_all(&site_config.config.build.content_dir).unwrap();
        fs::write(&site_config.config.build.entry, program).unwrap();
        site_config
    }

    fn png_bytes(color: [u8; 3]) -> Vec<u8> {
        let pixels = ::image::RgbImage::from_pixel(8, 4, ::image::Rgb(color));
        let mut encoded = Cursor::new(Vec::new());
        ::image::DynamicImage::ImageRgb8(pixels)
            .write_to(&mut encoded, ::image::ImageFormat::Png)
            .unwrap();
        encoded.into_inner()
    }

    fn png(path: &std::path::Path, color: [u8; 3]) {
        fs::write(path, png_bytes(color)).unwrap();
    }

    /// Write an SVG source with an explicit pixel size and the given body markup.
    fn svg(path: &std::path::Path, width: u32, height: u32, body: &str) {
        fs::write(
            path,
            format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}">{body}</svg>"##
            ),
        )
        .unwrap();
    }

    /// Keep an encoded PNG's structure while removing its pixel data, so reading the image
    /// succeeds where decoding it cannot.
    fn strip_pixel_data(path: &std::path::Path) {
        let bytes = fs::read(path).unwrap();
        let chunk = bytes
            .windows(4)
            .position(|window| window == b"IDAT")
            .expect("an encoded PNG has pixel data");
        let length = u32::from_be_bytes(bytes[chunk - 4..chunk].try_into().unwrap()) as usize;
        let mut stripped = Vec::new();
        stripped.extend_from_slice(&bytes[..chunk - 4]);
        stripped.extend_from_slice(&0u32.to_be_bytes());
        stripped.extend_from_slice(b"IDAT");
        stripped.extend_from_slice(&bytes[chunk + 4 + length..]);
        fs::write(path, stripped).unwrap();
    }

    fn request(paths: Vec<std::path::PathBuf>) -> BuildRequest {
        BuildRequest {
            mode: BuildMode::Development,
            trigger: BuildTrigger::Paths(paths.into()),
            reuse: BuildReuse {
                content_inventory: true,
                typst_compilation: true,
                configured_assets: true,
            },
            ..BuildRequest::new(BuildMode::Development)
        }
    }

    fn hashes(build: &crate::build::SiteBuild) -> BTreeMap<OutputPath, ContentDigest> {
        build
            .graph()
            .outputs()
            .iter()
            .map(|output| (output.path().clone(), ContentDigest::of(output.bytes())))
            .collect()
    }

    fn install(session: &mut BuildSession, build: crate::build::SiteBuild) {
        let checked = match build
            .into_unchecked_revision(None)
            .check(&BuildCancellation::new())
            .unwrap()
        {
            RevisionCheck::Fresh(checked) => checked,
            RevisionCheck::Stale(_) => panic!("test candidate unexpectedly became stale"),
        };
        session.install_revision(checked, Ok).unwrap();
    }

    fn cold_build(config: Arc<ResolvedSiteConfig>) -> crate::build::SiteBuild {
        BuildSession::new()
            .prepare(config, BuildRequest::new(BuildMode::Development))
            .run()
            .unwrap()
    }

    /// Damage the bytes every stored variant points at, leaving the pointers that name them.
    fn damage_stored_variants(root: &std::path::Path) {
        for (_pointer, stored) in stored_variants(root) {
            fs::write(stored, b"damaged").unwrap();
        }
    }

    /// Every stored variant as its pointer file and the file holding its bytes.
    fn stored_variants(root: &std::path::Path) -> Vec<(std::path::PathBuf, std::path::PathBuf)> {
        let directory = root.join(".tola/cache/images");
        let Ok(entries) = fs::read_dir(&directory) else {
            return Vec::new();
        };
        entries
            .map(|entry| entry.unwrap().path())
            .filter_map(|pointer| {
                let name = fs::read_to_string(&pointer).ok()?;
                let stored = directory.join(name.trim());
                stored.is_file().then_some((pointer, stored))
            })
            .collect()
    }

    fn one_image_config() -> OwnedSiteConfig {
        let site_config = config_with_program(
            r#"#import "@tola/image:0.0.0": resize-image
#let resized = resize-image("photo.png", width: 4, op: "fit-width", format: "png")
#document("index.html")[#html.img(src: resized.url, alt: "Photo")]"#,
        );
        png(
            &site_config.config.get_root().join("photo.png"),
            [30, 60, 90],
        );
        site_config
    }

    /// A build retains the bytes it published, so the next build can reuse them.
    #[test]
    fn build_stores_its_variants() {
        let site_config = one_image_config();
        let config = Arc::new(site_config.config.clone());
        let built = cold_build(Arc::clone(&config));
        let published = derivative_outputs(&built)
            .first()
            .expect("the build published the resized image")
            .bytes()
            .to_vec();
        let stored = stored_variants(config.get_root());
        assert_eq!(stored.len(), 1, "the build stored the variant it published");
        assert_eq!(fs::read(&stored[0].1).unwrap(), published);
    }

    /// Every derivative output in one build, in output order.
    fn derivative_outputs(
        build: &crate::build::SiteBuild,
    ) -> Vec<&crate::output::graph::OutputFile> {
        build
            .graph()
            .outputs()
            .iter()
            .filter(|output| output.path().as_str().starts_with("_tola/images/"))
            .collect()
    }

    /// Every derivative output path in one build, in output order.
    fn derivative_paths(build: &crate::build::SiteBuild) -> Vec<String> {
        derivative_outputs(build)
            .iter()
            .map(|output| output.path().as_str().to_owned())
            .collect()
    }

    /// The sole diagnostic a production build fails with.
    fn only_diagnostic(site_config: &OwnedSiteConfig) -> crate::diagnostic::Diagnostic {
        let error = match crate::build::build_site(&site_config.config, BuildMode::Production) {
            Err(error) => error,
            Ok(_) => panic!("the build must fail"),
        };
        let diagnostics = crate::build::error_diagnostics(&error, site_config.config.get_root());
        let [diagnostic] = diagnostics.as_slice() else {
            panic!("expected one diagnostic: {diagnostics:#?}")
        };
        diagnostic.clone()
    }

    /// One published page as text.
    fn page_text<'a>(build: &'a crate::build::SiteBuild, name: &str) -> &'a str {
        let page = build
            .graph()
            .outputs()
            .iter()
            .find(|output| output.path().as_str() == name)
            .expect("the build published the page");
        std::str::from_utf8(page.bytes()).unwrap()
    }

    /// `image-srcset` publishes one derivative per distinct width and names each in its `srcset`;
    /// a width that cannot be a pixel count fails the build.
    #[test]
    fn srcset_publishes_each_width_once() {
        let site_config = config_with_program(
            r#"#import "@tola/image:0.0.0": image-srcset
#let photo = image-srcset("photo.png", widths: (4, 2, 4))
#document("index.html")[
  #html.elem("img", attrs: (
    src: photo.url, srcset: photo.srcset, width: str(photo.width), height: str(photo.height), alt: "Photo",
  ))
]"#,
        );
        png(
            &site_config.config.get_root().join("photo.png"),
            [30, 60, 90],
        );
        let built = cold_build(Arc::new(site_config.config.clone()));
        let derivatives = derivative_paths(&built);
        assert_eq!(
            derivatives.len(),
            2,
            "one derivative per distinct width: {derivatives:#?}"
        );
        let page = page_text(&built, "index.html");
        assert_eq!(page.matches(" 2w").count(), 1, "{page}");
        assert_eq!(page.matches(" 4w").count(), 1, "{page}");

        let impossible = config_with_program(
            r#"#import "@tola/image:0.0.0": image-srcset
#let photo = image-srcset("photo.png", widths: (4, 0))
#document("index.html")[#html.img(src: photo.url, alt: "Photo")]"#,
        );
        png(
            &impossible.config.get_root().join("photo.png"),
            [30, 60, 90],
        );
        let diagnostic = only_diagnostic(&impossible);
        assert!(
            diagnostic.message.contains("widths"),
            "the failure names the option: {diagnostic:#?}"
        );
    }

    /// The renderer writes the attributes an `<img>` needs, and the page carries its own values.
    #[test]
    fn img_writes_element_attributes() {
        let site_config = config_with_program(
            r#"#import "@tola/image:0.0.0": img
#document("index.html")[
  #img("photo.png", widths: (2, 4), alt: "Photo", sizes: "(max-width: 600px) 480px, 100vw")
]"#,
        );
        png(
            &site_config.config.get_root().join("photo.png"),
            [30, 60, 90],
        );
        let built = cold_build(Arc::new(site_config.config.clone()));
        let page = page_text(&built, "index.html");
        assert!(page.contains("src=\"/docs/_tola/images/"), "{page}");
        assert!(page.contains(".webp\""), "{page}");
        assert!(page.contains("srcset=\""), "{page}");
        assert!(page.contains(" 2w") && page.contains(" 4w"), "{page}");
        assert!(
            page.contains("sizes=\"(max-width: 600px) 480px, 100vw\""),
            "{page}"
        );
        assert!(page.contains("width=\"4\""), "{page}");
        assert!(page.contains("height=\"2\""), "{page}");
        assert!(page.contains("alt=\"Photo\""), "{page}");
        assert!(page.contains("loading=\"lazy\""), "{page}");
        assert!(page.contains("decoding=\"async\""), "{page}");
    }

    /// A call without widths derives the source's own width and writes no candidate list.
    #[test]
    fn img_omits_the_candidate_list_for_the_source_width() {
        let site_config = config_with_program(
            r#"#import "@tola/image:0.0.0": img
#document("index.html")[#img("photo.png", alt: "Photo")]"#,
        );
        png(
            &site_config.config.get_root().join("photo.png"),
            [30, 60, 90],
        );
        let built = cold_build(Arc::new(site_config.config.clone()));
        assert_eq!(derivative_paths(&built).len(), 1);
        let page = page_text(&built, "index.html");
        assert!(page.contains("width=\"8\""), "{page}");
        assert!(page.contains("height=\"4\""), "{page}");
        assert!(!page.contains("srcset="), "{page}");
        assert!(!page.contains("sizes="), "{page}");
    }

    /// A request the image package refuses fails the build, and the failure names what to change.
    #[test]
    fn refused_image_requests_name_the_reason() {
        for (program, expected) in [
            (
                r#"#import "@tola/image:0.0.0": img
#document("index.html")[#img("photo.png", attrs: (src: "photo.png"))]"#,
                "src",
            ),
            (
                r#"#import "@tola/image:0.0.0": resize-image
#let resized = resize-image("logo.svg", width: 4)
#document("index.html")[#html.img(src: resized.url, alt: "Logo")]"#,
                "cannot choose a format for an SVG",
            ),
            (
                r#"#import "@tola/image:0.0.0": resize-image
#let resized = resize-image("linked.svg", width: 4, op: "fit-width", format: "png")
#document("index.html")[#html.img(src: resized.url, alt: "Linked")]"#,
                "inline the image as a data URI",
            ),
            (
                r#"#import "@tola/image:0.0.0": image-srcset
#let photo = image-srcset("logo.svg")
#document("index.html")[#html.img(src: photo.url, alt: "Logo")]"#,
                "responsive set cannot be derived from an SVG",
            ),
        ] {
            let site_config = config_with_program(program);
            let root = site_config.config.get_root().to_path_buf();
            png(&root.join("photo.png"), [30, 60, 90]);
            svg(
                &root.join("logo.svg"),
                8,
                4,
                r##"<rect width="8" height="4" fill="#ff0000"/>"##,
            );
            svg(
                &root.join("linked.svg"),
                8,
                4,
                r##"<image href="photo.png"/>"##,
            );
            let diagnostic = only_diagnostic(&site_config);
            assert!(
                diagnostic.message.contains(expected),
                "{expected:?} missing from {diagnostic:#?}"
            );
        }
    }

    /// Damaged stored bytes are a miss rather than a different image.
    #[test]
    fn damaged_store_keeps_output() {
        let site_config = one_image_config();
        let config = Arc::new(site_config.config.clone());
        let storing = cold_build(Arc::clone(&config));
        let expected = hashes(&storing);
        damage_stored_variants(config.get_root());

        let rebuilt = cold_build(config);
        assert_eq!(hashes(&rebuilt), expected);
    }

    #[test]
    fn failed_render_reports_the_decoder_reason() {
        let site_config = config_with_program(
            r#"#import "@tola/image:0.0.0": resize-image
#let broken = resize-image("broken.png", width: 2, op: "fit-width", format: "png")
#document("index.html")[#html.img(src: broken.url, alt: "Broken")]"#,
        );
        let broken = site_config.config.get_root().join("broken.png");
        png(&broken, [30, 60, 90]);
        strip_pixel_data(&broken);
        let diagnostic = only_diagnostic(&site_config);
        assert!(diagnostic.message.contains("broken.png"), "{diagnostic:#?}");
        assert!(
            diagnostic.notes.iter().any(|note| note.contains("damaged")),
            "{diagnostic:#?}"
        );
    }

    /// Editor inspection renders the compilation without publishing an image.
    #[test]
    fn checks_never_publish_image_files() {
        let site_config = config_with_program(
            r#"#import "@tola/image:0.0.0": image-metadata, resize-image
#let original = image-metadata("photo.png")
#assert.eq((original.width, original.height, original.format, original.mime), (8, 4, "png", "image/png"))
#let resized = resize-image("photo.png", width: 4, op: "fit-width", format: "webp")
#document("index.html")[#html.img(src: resized.url, alt: "Photo")]"#,
        );
        png(
            &site_config.config.get_root().join("photo.png"),
            [50, 100, 150],
        );
        let config = Arc::new(site_config.config.clone());
        let mut editor = crate::check::SourceDiagnosticSession::new(Arc::clone(&config));
        let checked = editor
            .inspect(Vec::new(), &BuildCancellation::new())
            .unwrap();
        assert!(
            checked
                .checked()
                .is_some_and(|checked| checked.bundle().is_some()),
            "{:?}",
            checked.diagnostics()
        );
        assert!(!config.build.publish_dir.exists());
        fs::write(
            &config.build.entry,
            r#"#import "@tola/image:0.0.0": image-metadata
#let original = image-metadata("photo.png")
#document("index.html")[#original.width x #original.height]"#,
        )
        .unwrap();
        let built = crate::build::build_site(&config, BuildMode::Production).unwrap();
        assert!(derivative_outputs(&built).is_empty());
    }

    /// One image program, the source it reads, and the edit that replaces it.
    struct RebuildCase {
        program: &'static str,
        sources: &'static [(&'static str, [u8; 3])],
        edit: (&'static str, [u8; 3]),
        /// The derivatives a cold build publishes; `None` where a `context` query decides it.
        derivatives: Option<usize>,
    }

    /// A rebuild of an edited source publishes the same outputs a cold build does.
    #[test]
    fn retained_build_matches_cold() {
        let cases = [
            RebuildCase {
                program: r#"#import "@tola/image:0.0.0": resize-image
#document("index.html")[
  #context {
    let width = if query(heading).len() == 0 { 2 } else { 4 }
    let resized = resize-image("photo.png", width: width, op: "fit-width", format: "png")
    html.img(src: resized.url, width: resized.width, height: resized.height, alt: "Context")
  }
  = Heading
]"#,
                sources: &[("photo.png", [40, 90, 180])],
                edit: ("photo.png", [180, 90, 40]),
                derivatives: None,
            },
            RebuildCase {
                program: r#"#import "@tola/image:0.0.0": resize-image
#let variants = ("one.png", "two.png").map(source => (
  resize-image(source, width: 4, op: "fit-width", format: "png"),
  resize-image(source, width: 2, op: "fit-width", format: "png"),
))
#document("index.html")[
  #for pair in variants {
    for variant in pair { html.img(src: variant.url, alt: "Variant") }
  }
]"#,
                sources: &[("one.png", [200, 20, 20]), ("two.png", [20, 200, 20])],
                edit: ("two.png", [20, 20, 200]),
                derivatives: Some(4),
            },
            RebuildCase {
                program: r#"#import "@tola/image:0.0.0": resize-image
#let wide = resize-image("one.png", width: 4, op: "fit-width", format: "png")
#let narrow = resize-image("two.png", width: 2, op: "fit-width", format: "png")
#document("index.html")[
  #html.img(src: wide.url, alt: "Wide")
  #html.img(src: narrow.url, alt: "Narrow")
]"#,
                sources: &[("one.png", [200, 20, 20]), ("two.png", [200, 20, 20])],
                edit: ("two.png", [20, 20, 200]),
                derivatives: Some(2),
            },
        ];
        for case in cases {
            let site_config = config_with_program(case.program);
            let root = site_config.config.get_root().to_path_buf();
            for (name, color) in case.sources {
                png(&root.join(name), *color);
            }
            let config = Arc::new(site_config.config.clone());
            let mut session = BuildSession::new();
            let first = session
                .prepare(
                    Arc::clone(&config),
                    BuildRequest::new(BuildMode::Development),
                )
                .run()
                .unwrap();
            if let Some(derivatives) = case.derivatives {
                assert_eq!(
                    derivative_outputs(&first).len(),
                    derivatives,
                    "{}",
                    case.program
                );
            }
            let initial = hashes(&first);
            install(&mut session, first);

            let retained = session
                .prepare(Arc::clone(&config), request(Vec::new()))
                .run()
                .unwrap();
            assert_eq!(hashes(&retained), initial, "{}", case.program);
            install(&mut session, retained);

            let (edited, color) = case.edit;
            png(&root.join(edited), color);
            let changed = session
                .prepare(Arc::clone(&config), request(vec![root.join(edited)]))
                .run()
                .unwrap();
            assert_eq!(
                hashes(&changed),
                hashes(&cold_build(config)),
                "{}",
                case.program
            );
            assert!(!root.join("public").exists());
        }
    }
}
