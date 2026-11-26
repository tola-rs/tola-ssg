//! Complete image outputs derived from replayable native request observations.

mod error;
mod render;
#[cfg(test)]
mod tests;

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, ensure};
use rayon::prelude::*;
use tola_address::OutputPath;
use tola_image::{ImageFormat, ImageMetadata, ImageRecipe, inspect};
use tola_input::layout;
use tola_packages::ImageRequest;
use tola_typst::svg::{SvgSource, looks_like_svg};
use tola_typst::{AccessedDeps, ContentDigest, ReadRecord, TypstWorld};
use typst::comemo::Track;
use typst::loading::DataSource;
use typst::syntax::FileId;

use self::error::{ImageCause, ImageOperation, image_failure};
use self::render::{MAX_SOURCE_WORKERS, SourceImage, render_pending_variants};
use crate::cancellation::{BuildCancellation, BuildCancelled};
use crate::compiler::BuildInputs;
use crate::image::VariantCache;
use crate::output::graph::{OutputFile, OutputGraphBuilder};
use crate::output::owner::OutputOwner;
use crate::output::semantics::{OutputDeclaration, ResponseMediaType};
use crate::resources::BuildResources;

/// Encoded variants retained only with the complete successful build that owns them.
#[derive(Default)]
pub(crate) struct ImageOutputs {
    variants: BTreeMap<OutputPath, EncodedVariant>,
}

struct EncodedVariant {
    source_digest: ContentDigest,
    recipe: ImageRecipe,
    bytes: Arc<[u8]>,
    digest: ContentDigest,
}

pub(super) struct VariantClaim {
    pub(super) source_digest: ContentDigest,
    pub(super) recipe: ImageRecipe,
}

impl VariantClaim {
    fn encoded(&self, bytes: Arc<[u8]>, digest: ContentDigest) -> EncodedVariant {
        EncodedVariant {
            source_digest: self.source_digest,
            recipe: self.recipe.clone(),
            bytes,
            digest,
        }
    }
}

pub(super) struct PendingRender {
    pub(super) output: OutputPath,
    pub(super) claim: usize,
    pub(super) source: usize,
    pub(super) file: String,
    pub(super) key: ContentDigest,
}

#[derive(Default)]
struct ImageBatch {
    sources: Vec<SourceImage>,
    claims: Vec<VariantClaim>,
    claimed: BTreeMap<OutputPath, usize>,
    pending: Vec<PendingRender>,
    variants: BTreeMap<OutputPath, EncodedVariant>,
}

impl ImageBatch {
    fn claim(
        &mut self,
        output: OutputPath,
        src: usize,
        file: &str,
        recipe: ImageRecipe,
        previous: Option<&ImageOutputs>,
    ) -> Result<()> {
        let source_digest = self.sources[src].digest;
        if let Some(&idx) = self.claimed.get(&output) {
            let existing = &self.claims[idx];
            ensure!(
                existing.source_digest == source_digest && existing.recipe == recipe,
                "two image variants publish the same output `{output}`; change one recipe or output",
            );
            return Ok(());
        }
        let claim = VariantClaim {
            source_digest,
            recipe,
        };
        let retained = previous
            .and_then(|prev| prev.variants.get(&output))
            .filter(|cached| {
                cached.source_digest == source_digest && cached.recipe == claim.recipe
            });
        if let Some(cached) = retained {
            self.variants.insert(
                output.clone(),
                claim.encoded(Arc::clone(&cached.bytes), cached.digest),
            );
        } else {
            self.pending.push(PendingRender {
                output: output.clone(),
                claim: self.claims.len(),
                source: src,
                file: file.to_owned(),
                key: ImageRequest::variant_key(source_digest, &claim.recipe),
            });
        }
        self.claimed.insert(output, self.claims.len());
        self.claims.push(claim);
        Ok(())
    }

    fn render(
        mut self,
        site_root: &Path,
        resources: &BuildResources,
        jobs: usize,
        cancel: &BuildCancellation,
        started: std::time::Instant,
    ) -> Result<ImageOutputs> {
        let reused = self.variants.len();
        let mut restored = 0usize;
        let mut unwritable = 0usize;
        let cache = VariantCache::new(layout::image_cache(site_root));

        // Keep cache misses in claim order for deterministic render diagnostics.
        let cached = self
            .pending
            .par_iter()
            .map(|job| {
                cancel.ensure_active()?;
                Ok(cache.read(job.key))
            })
            .collect::<Result<Vec<_>, BuildCancelled>>()?;
        let pending: Vec<_> = self
            .pending
            .into_iter()
            .zip(cached)
            .filter_map(|(job, hit)| match hit {
                Some((bytes, digest)) => {
                    restored += 1;
                    self.variants
                        .insert(job.output, self.claims[job.claim].encoded(bytes, digest));
                    None
                }
                None => Some(job),
            })
            .collect();

        let encoded = render_pending_variants(
            &pending,
            &self.claims,
            &self.sources,
            &resources.pixel_cache(),
            jobs,
            cancel,
        )?;
        for (job, bytes) in pending.iter().zip(encoded) {
            cancel.ensure_active()?;
            let claim = &self.claims[job.claim];
            let digest = ContentDigest::of(&bytes);
            if let Err(err) = cache.store(job.key, &bytes, digest) {
                unwritable += 1;
                tracing::debug!(target: "tola::compile", %err, "image variant not stored");
            }
            self.variants
                .insert(job.output.clone(), claim.encoded(bytes, digest));
        }
        cancel.ensure_active()?;
        tracing::debug!(
            target: "tola::compile",
            images_ms = started.elapsed().as_secs_f64() * 1000.0,
            sources = self.sources.len(),
            variants = self.variants.len(),
            rendered = pending.len(),
            reused,
            restored,
            unwritable,
            "prepared image outputs"
        );
        Ok(ImageOutputs {
            variants: self.variants,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn load_source_image(
    world: &TypstWorld,
    request: &ImageRequest,
    src_id: FileId,
    file: &str,
    batch: &mut ImageBatch,
    content_indexes: &mut HashMap<ContentDigest, usize>,
    inputs: &mut BuildInputs,
    accessed: &mut AccessedDeps,
    cancel: &BuildCancellation,
) -> Result<usize> {
    let bytes = match request.source() {
        DataSource::Path(_) => {
            let (bytes, observed) = world.read_file_with_evidence(src_id);
            inputs.record_accessed(&observed);
            accessed.reads.extend(observed.reads);
            accessed.disk_reads.extend(observed.disk_reads);
            accessed.package_checks.extend(observed.package_checks);
            cancel.ensure_active()?;
            bytes.map_err(|err| image_failure(file, ImageOperation::Read, ImageCause::File(err)))?
        }
        DataSource::Bytes(bytes) => bytes.clone(),
    };
    let digest = ContentDigest::of(bytes.as_slice());
    match content_indexes.entry(digest) {
        Entry::Occupied(entry) => Ok(*entry.get()),
        Entry::Vacant(entry) => {
            let svg = if looks_like_svg(bytes.as_slice()) {
                Some(
                    SvgSource::parse(bytes.clone(), (world as &dyn typst::World).track(), src_id)
                        .map_err(|err| {
                        image_failure(file, ImageOperation::RasterizeSvg, ImageCause::Svg(err))
                    })?,
                )
            } else {
                None
            };
            let metadata = match &svg {
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
                None => inspect(bytes.as_slice()).map_err(|err| {
                    image_failure(file, ImageOperation::Read, ImageCause::Image(err))
                })?,
            };
            let idx = batch.sources.len();
            batch.sources.push(SourceImage {
                bytes,
                digest,
                metadata,
                svg,
            });
            Ok(*entry.insert(idx))
        }
    }
}

impl ImageOutputs {
    pub(crate) fn prepare<'a>(
        world: &TypstWorld,
        evidence: impl IntoIterator<Item = &'a ReadRecord>,
        previous: Option<&Self>,
        site_root: &Path,
        resources: &BuildResources,
        inputs: &mut BuildInputs,
        cancel: &BuildCancellation,
    ) -> Result<(Arc<Self>, AccessedDeps)> {
        cancel.ensure_active()?;
        let started = std::time::Instant::now();
        let mut batch = ImageBatch::default();
        let mut src_indexes = HashMap::<FileId, usize>::new();
        let mut content_indexes = HashMap::<ContentDigest, usize>::new();
        let mut accessed = AccessedDeps::default();

        // Content aliases share pixels, but each requested path retains its own read evidence.
        for read in evidence {
            cancel.ensure_active()?;
            let Some(request) = ImageRequest::from_read(read.locator())? else {
                continue;
            };
            let src_id = FileId::new(request.origin().clone());
            let file = src_id.vpath().get_without_slash();
            let src_idx = match src_indexes.get(&src_id).copied() {
                Some(idx) => idx,
                None => {
                    let idx = load_source_image(
                        world,
                        &request,
                        src_id,
                        file,
                        &mut batch,
                        &mut content_indexes,
                        inputs,
                        &mut accessed,
                        cancel,
                    )?;
                    src_indexes.insert(src_id, idx);
                    idx
                }
            };
            let src = &batch.sources[src_idx];
            ensure!(
                src.digest == request.source_digest(),
                "the image `{}` changed since Tola prepared its variants; run the build again",
                src_id.vpath().get_with_slash(),
            );
            match &src.svg {
                Some(svg) => {
                    request
                        .recipe()
                        .validate_svg(svg.width(), svg.height(), svg.hash())?
                }
                None => request.recipe().validate(&src.metadata)?,
            }
            batch.claim(
                request.output_path(),
                src_idx,
                file,
                request.recipe().clone(),
                previous,
            )?;
        }

        let images = batch.render(site_root, resources, MAX_SOURCE_WORKERS, cancel, started)?;
        Ok((Arc::new(images), accessed))
    }

    pub(crate) fn insert_into(
        &self,
        outputs: &mut OutputGraphBuilder,
        cancel: &BuildCancellation,
    ) -> Result<()> {
        let owner = OutputOwner::system("image");
        for (path, variant) in &self.variants {
            cancel.ensure_active()?;
            let media = response_media_type(variant.recipe.format());
            outputs.insert(OutputFile::from_byte_owner_with_digest(
                path.clone(),
                OutputDeclaration::image(media),
                owner.clone(),
                Arc::clone(&variant.bytes),
                variant.digest,
            ))?;
        }
        Ok(())
    }
}

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
