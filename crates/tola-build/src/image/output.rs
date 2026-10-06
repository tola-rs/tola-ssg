//! Complete image outputs derived from replayable native request observations.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, ensure};
use rayon::prelude::*;
use tola_typst::{AccessedDeps, ContentDigest, ReadEvidence, TypstWorld};
use typst::foundations::Bytes;
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
            let source_id = FileId::new(request.source().clone());
            let source_index = match source_indexes.get(&source_id).copied() {
                Some(index) => index,
                None => {
                    let (bytes, observed) = world.read_file_with_evidence(source_id);
                    // Preserve missing-source recovery even when the producer stops here.
                    inputs.record_accessed(&observed);
                    accessed.reads.extend(observed.reads);
                    accessed.disk_reads.extend(observed.disk_reads);
                    accessed.package_checks.extend(observed.package_checks);
                    cancellation.ensure_active()?;
                    let bytes = bytes.map_err(|error| {
                        let reason = file_failure_reason(&error);
                        image_failure(
                            anyhow::Error::new(error),
                            format!(
                                "Tola could not read the image `{}`",
                                source_id.vpath().get_without_slash()
                            ),
                            reason,
                        )
                    })?;
                    let digest = ContentDigest::of(bytes.as_slice());
                    let index = match content_indexes.entry(digest) {
                        std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            let metadata = inspect(bytes.as_slice()).map_err(|error| {
                                let reason = error.to_string();
                                image_failure(
                                    error,
                                    format!(
                                        "Tola could not read the image `{}`",
                                        source_id.vpath().get_without_slash()
                                    ),
                                    reason,
                                )
                            })?;
                            let index = sources.len();
                            sources.push(SourceImage {
                                bytes,
                                digest,
                                metadata,
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
            request.recipe().validate(&source.metadata)?;
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
            return Err(image_failure(
                error,
                format!(
                    "Tola could not resize the image `{}`",
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
    use std::collections::BTreeSet;
    use std::fs;
    use std::io::Cursor;
    use tola_image::{OutputFormat, ResizeFilter, ResizeOperation, ResizeOptions};
    use typst::syntax::{RootedPath, VirtualPath, VirtualRoot};

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

    fn recipe(
        metadata: &ImageMetadata,
        width: u32,
        height: u32,
        format: OutputFormat,
    ) -> ImageRecipe {
        ImageRecipe::resolve(
            metadata,
            ResizeOptions {
                width: Some(width),
                height: Some(height),
                operation: ResizeOperation::Scale,
                format,
                quality: None,
                filter: ResizeFilter::Triangle,
                background: None,
            },
        )
        .unwrap()
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

    fn install(
        session: &mut BuildSession,
        build: crate::build::SiteBuild,
    ) -> crate::site::SiteRevision {
        let checked = match build
            .into_unchecked_revision(None)
            .check(&BuildCancellation::new())
            .unwrap()
        {
            RevisionCheck::Fresh(checked) => checked,
            RevisionCheck::Stale(_) => panic!("test candidate unexpectedly became stale"),
        };
        session.install_revision(checked, Ok).unwrap()
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
        let published = built
            .graph()
            .outputs()
            .iter()
            .find(|output| output.path().as_str().starts_with("_tola/images/"))
            .expect("the build published the resized image")
            .bytes()
            .to_vec();
        let stored = stored_variants(config.get_root());
        assert_eq!(stored.len(), 1, "the build stored the variant it published");
        assert_eq!(fs::read(&stored[0].1).unwrap(), published);
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
        let error = match crate::build::build_site(&site_config.config, BuildMode::Production) {
            Err(error) => error,
            Ok(_) => panic!("an image without pixel data must fail the build"),
        };
        let diagnostics = crate::build::error_diagnostics(&error, site_config.config.get_root());
        let [diagnostic] = diagnostics.as_slice() else {
            panic!("expected one diagnostic: {diagnostics:#?}")
        };
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
        assert!(
            !built
                .graph()
                .outputs()
                .iter()
                .any(|output| output.path().as_str().starts_with("_tola/images/"))
        );
    }

    /// One output path is one variant whatever source or spelling claimed it, and a
    /// retained or contextual build publishes what a cold build publishes.
    #[test]
    fn requests_match_retained_cold_builds() {
        let cover = config_with_program(
            r#"#import "@tola/source:0.0.0": all-sources
#let cover = all-sources().first().meta.cover
#for output in ("index.html", "second.html") {
  document(output)[#html.img(src: cover.url, width: cover.width, height: cover.height, alt: "Photo")]
}"#,
        );
        let root = cover.config.get_root();
        let source = cover.config.build.content_dir.join("post.typ");
        let photo = cover.config.build.content_dir.join("photo.png");
        png(&photo, [20, 100, 200]);
        fs::write(
            &source,
            r#"#import "@tola/image:0.0.0": resize-image
#import "@tola/source:0.0.0": tola-meta
#tola-meta((cover: resize-image("photo.png", width: 4, op: "fit-width", format: "png"),))"#,
        )
        .unwrap();
        let config = Arc::new(cover.config.clone());
        let mut session = BuildSession::new();
        let first = session
            .prepare(
                Arc::clone(&config),
                BuildRequest::new(BuildMode::Development),
            )
            .run()
            .unwrap();
        let images = first
            .graph()
            .outputs()
            .iter()
            .filter(|output| output.path().as_str().starts_with("_tola/images/"))
            .collect::<Vec<_>>();
        assert_eq!(images.len(), 1);
        let decoded = ::image::load_from_memory(images[0].bytes()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (4, 2));
        let original_image = images[0].path().clone();
        let href = format!("/docs/{}", original_image.as_str());
        for name in ["index.html", "second.html"] {
            let page = first
                .graph()
                .outputs()
                .iter()
                .find(|output| output.path().as_str() == name)
                .unwrap();
            assert!(std::str::from_utf8(page.bytes()).unwrap().contains(&href));
        }
        let initial_hashes = hashes(&first);
        let _first_revision = install(&mut session, first);
        let retained = session
            .prepare(Arc::clone(&config), request(Vec::new()))
            .run()
            .unwrap();
        assert_eq!(hashes(&retained), initial_hashes);
        let _retained_revision = install(&mut session, retained);

        let modified = fs::metadata(&photo).unwrap().modified().unwrap();
        png(&photo, [200, 50, 20]);
        fs::File::options()
            .write(true)
            .open(&photo)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let changed = session
            .prepare(Arc::clone(&config), request(vec![photo.clone()]))
            .run()
            .unwrap();
        assert!(
            !changed
                .graph()
                .outputs()
                .iter()
                .any(|output| output.path() == &original_image)
        );
        let authoritative = cold_build(config);
        assert_eq!(hashes(&changed), hashes(&authoritative));
        assert!(!root.join("public").exists());

        let contextual = config_with_program(
            r#"#import "@tola/image:0.0.0": resize-image
#document("index.html")[
  #context {
    let width = if query(heading).len() == 0 { 2 } else { 4 }
    let resized = resize-image("photo.png", width: width, op: "fit-width", format: "png")
    html.img(src: resized.url, width: resized.width, height: resized.height, alt: "Context")
  }
  = Heading
]"#,
        );
        png(
            &contextual.config.get_root().join("photo.png"),
            [40, 90, 180],
        );
        let config = Arc::new(contextual.config.clone());
        let mut session = BuildSession::new();
        let first = session
            .prepare(
                Arc::clone(&config),
                BuildRequest::new(BuildMode::Development),
            )
            .run()
            .unwrap();
        let expected = hashes(&first);
        let _current = install(&mut session, first);
        let reused = session
            .prepare(Arc::clone(&config), request(Vec::new()))
            .run()
            .unwrap();
        assert_eq!(hashes(&reused), expected);
        let cold = cold_build(config);
        assert_eq!(hashes(&cold), expected);

        let equivalent = config_with_program(
            r#"#import "@tola/image:0.0.0": resize-image
#import "templates/process.typ": process
#let first = resize-image("one.png", width: 4, op: "fit-width", format: "png", quality: 10)
#let second = process(path("two.png"))
#assert.eq(first.url, second.url)
#document("index.html")[#html.img(src: first.url, alt: "One") #html.img(src: second.url, alt: "Two")]"#,
        );
        fs::create_dir_all(equivalent.config.get_root().join("templates")).unwrap();
        fs::write(equivalent.config.get_root().join("templates/process.typ"), r#"#import "@tola/image:0.0.0": resize-image
#let process(source) = resize-image(source, width: 4, height: 2, op: "scale", format: "png", quality: 90)"#).unwrap();
        png(&equivalent.config.get_root().join("one.png"), [10, 40, 90]);
        fs::copy(
            equivalent.config.get_root().join("one.png"),
            equivalent.config.get_root().join("two.png"),
        )
        .unwrap();
        let built = crate::build::build_site(&equivalent.config, BuildMode::Production).unwrap();
        assert_eq!(
            built
                .graph()
                .outputs()
                .iter()
                .filter(|output| output.path().as_str().starts_with("_tola/images/"))
                .count(),
            1
        );

        let variants = config_with_program(
            r#"#import "@tola/image:0.0.0": resize-image
#let variants = ("one.png", "two.png", "three.png").map(source => (
  resize-image(source, width: 4, op: "fit-width", format: "png"),
  resize-image(source, width: 2, op: "fit-width", format: "png"),
))
#document("index.html")[
  #for pair in variants {
    for variant in pair { html.img(src: variant.url, alt: "Variant") }
  }
]"#,
        );
        for (name, color) in [
            ("one.png", [200, 20, 20]),
            ("two.png", [20, 200, 20]),
            ("three.png", [20, 20, 200]),
        ] {
            png(&variants.config.get_root().join(name), color);
        }
        let built = crate::build::build_site(&variants.config, BuildMode::Production).unwrap();
        let rendered = built
            .graph()
            .outputs()
            .iter()
            .filter(|output| output.path().as_str().starts_with("_tola/images/"))
            .map(|output| {
                let decoded = ::image::load_from_memory(output.bytes())
                    .unwrap()
                    .to_rgba8();
                (decoded.width(), decoded.get_pixel(0, 0).0)
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            rendered,
            BTreeSet::from([
                (4, [200, 20, 20, 255]),
                (2, [200, 20, 20, 255]),
                (4, [20, 200, 20, 255]),
                (2, [20, 200, 20, 255]),
                (4, [20, 20, 200, 255]),
                (2, [20, 20, 200, 255]),
            ])
        );
    }

    #[test]
    fn geometry_hits_skip_decode() {
        let bytes = png_bytes([30, 60, 90]);
        let source = SourceImage {
            digest: ContentDigest::of(&bytes),
            metadata: inspect(&bytes).unwrap(),
            bytes: Bytes::new(bytes),
        };
        let recipes = [
            recipe(&source.metadata, 4, 2, OutputFormat::Png),
            recipe(&source.metadata, 4, 2, OutputFormat::WebP),
        ];
        let geometry = RenderGeometry {
            pixels: recipes[0].pixels(),
            renders: vec![0, 1],
        };
        let cache = PixelCache::default();
        let cancellation = BuildCancellation::new();
        let decoded = DecodedSource::open(source.bytes.as_slice(), &cancellation).unwrap();
        cache.retain_resampled(
            source.digest,
            geometry.pixels,
            Arc::new(decoded.pixels(geometry.pixels, &cancellation).unwrap()),
        );
        drop(decoded);

        let mut pending = Vec::new();
        let mut claims = BTreeMap::new();
        for recipe in recipes {
            let request = ImageRequest::new(
                RootedPath::new(
                    VirtualRoot::Project,
                    VirtualPath::new("/photo.png").unwrap(),
                ),
                source.digest,
                recipe,
            );
            pending.push(PendingRender {
                output: request.output_path(),
                source: 0,
                source_id: FileId::new(request.source().clone()),
                key: request.key(),
            });
            claims.insert(
                request.output_path(),
                VariantClaim {
                    source_digest: source.digest,
                    recipe: request.recipe().clone(),
                },
            );
        }
        let group = SourceGroup {
            source: 0,
            geometries: vec![geometry],
        };
        let outcomes = group
            .render(&source, &pending, &claims, &cache, &cancellation)
            .into_iter()
            .map(|outcome| (outcome.index, outcome.bytes.unwrap()))
            .collect::<BTreeMap<_, _>>();
        for index in 0..pending.len() {
            let output = ::image::load_from_memory(&outcomes[&index])
                .unwrap()
                .to_rgba8();
            assert_eq!(output.dimensions(), (4, 2));
            assert!(output.pixels().all(|pixel| pixel.0 == [30, 60, 90, 255]));
        }
        assert!(cache.decoded(source.digest).is_none());
    }

    #[test]
    fn source_batches_respect_limits() {
        let sources = [
            (8, 4),
            (6000, 4000),
            (6000, 4000),
            (10000, 10000),
            (8, 4),
            (8, 4),
            (8, 4),
            (8, 4),
            (8, 4),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (width, height))| SourceImage {
            bytes: Bytes::new(Vec::new()),
            digest: ContentDigest::of(&[index as u8]),
            metadata: ImageMetadata {
                width,
                height,
                format: ImageFormat::Png,
                has_alpha: false,
                is_lossy: false,
            },
        })
        .collect::<Vec<_>>();
        let groups = sources
            .iter()
            .enumerate()
            .map(|(index, source)| SourceGroup {
                source: index,
                geometries: vec![RenderGeometry {
                    pixels: recipe(
                        &source.metadata,
                        source.metadata.width,
                        source.metadata.height,
                        OutputFormat::Png,
                    )
                    .pixels(),
                    renders: vec![index],
                }],
            })
            .collect::<Vec<_>>();
        for workers in [1, 4] {
            let mut remaining = groups.as_slice();
            while !remaining.is_empty() {
                let admitted = source_batch_len(remaining, &sources, workers);
                assert!((1..=workers).contains(&admitted));
                let (batch, next) = remaining.split_at(admitted);
                let bytes = batch
                    .iter()
                    .map(|group| group.admission_bytes(&sources[group.source]))
                    .sum::<u64>();
                assert!(
                    bytes <= SOURCE_BATCH_BYTES || admitted == 1,
                    "{workers} {bytes}"
                );
                remaining = next;
            }
        }
    }

    #[test]
    fn content_alias_edits_change_outputs() {
        let site_config = config_with_program(
            r#"#import "@tola/image:0.0.0": resize-image
#let wide = resize-image("one.png", width: 4, op: "fit-width", format: "png")
#let narrow = resize-image("two.png", width: 2, op: "fit-width", format: "png")
#document("index.html")[
  #html.img(src: wide.url, alt: "Wide")
  #html.img(src: narrow.url, alt: "Narrow")
]"#,
        );
        let root = site_config.config.get_root();
        png(&root.join("one.png"), [200, 20, 20]);
        fs::copy(root.join("one.png"), root.join("two.png")).unwrap();
        let config = Arc::new(site_config.config.clone());
        let mut session = BuildSession::new();
        let original = session
            .prepare(
                Arc::clone(&config),
                BuildRequest::new(BuildMode::Development),
            )
            .run()
            .unwrap();
        let _revision = install(&mut session, original);

        png(&root.join("two.png"), [20, 20, 200]);
        let changed = session
            .prepare(config, request(vec![root.join("two.png")]))
            .run()
            .unwrap();
        let pixels = changed
            .graph()
            .outputs()
            .iter()
            .filter(|output| output.path().as_str().starts_with("_tola/images/"))
            .map(|output| {
                let image = ::image::load_from_memory(output.bytes())
                    .unwrap()
                    .to_rgba8();
                (image.width(), image.get_pixel(0, 0).0)
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            pixels,
            BTreeSet::from([(4, [200, 20, 20, 255]), (2, [20, 20, 200, 255])])
        );
    }
}
