use std::cmp::Reverse;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use rayon::prelude::*;
use tola_diagnostic::{Diagnostic, DiagnosticError, Severity};
use tola_image::{DecodedSource, ImageMetadata, PixelRecipe, ResampledPixels};
use tola_typst::ContentDigest;
use tola_typst::svg::SvgSource;
use typst::foundations::Bytes;

use super::error::{ImageCause, ImageOperation, image_failure};
use super::{PendingRender, VariantClaim};
use crate::cancellation::BuildCancellation;
use crate::image::PixelCache;

pub(super) const MAX_SOURCE_WORKERS: usize = 4;

/// Batches sum each source's largest decode, resample, or encode estimate. A larger source runs
/// alone. Previously encoded variants, frozen inputs and pixel-cache retention are separate.
const SOURCE_BATCH_BYTES: u64 = 256 * 1024 * 1024;

pub(super) struct SourceImage {
    pub(super) bytes: Bytes,
    pub(super) digest: ContentDigest,
    pub(super) metadata: ImageMetadata,
    pub(super) svg: Option<SvgSource>,
}

struct SourceGroup {
    src: usize,
    geometries: Vec<RenderGeometry>,
}

struct RenderedImage {
    index: usize,
    bytes: Result<Arc<[u8]>, ImageCause>,
}

struct RenderGeometry {
    pixels: PixelRecipe,
    renders: Vec<usize>,
}

struct SourceBatch {
    count: usize,
    extra_workers: usize,
}

impl SourceBatch {
    fn encoding_workers(&self, idx: usize) -> usize {
        1 + self.extra_workers / self.count + usize::from(idx < self.extra_workers % self.count)
    }
}

fn image_workers(threads: usize, cancel: &BuildCancellation) -> Result<rayon::ThreadPool> {
    cancel.ensure_active()?;
    let workers = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|idx| format!("tola-image-{idx}"))
        .build();
    cancel.ensure_active()?;
    workers.map_err(|err| {
        let msg = "Tola could not prepare image resizing";
        let diag = Diagnostic::new(tola_diagnostic::codes::build::SITE, Severity::Error, msg)
            .with_help("Close other programs and run the build again");
        DiagnosticError::attach(anyhow::Error::new(err).context(msg), vec![diag]).into()
    })
}

impl RenderGeometry {
    fn encode_into(
        &self,
        pixels: &ResampledPixels,
        pending: &[PendingRender],
        claims: &[VariantClaim],
        cancel: &BuildCancellation,
        workers: usize,
        out: &mut Vec<RenderedImage>,
    ) {
        out.extend(self.renders.iter().map(|&idx| {
            RenderedImage {
                index: idx,
                bytes: pixels
                    .encode_with_workers(&claims[pending[idx].claim].recipe, cancel, workers)
                    .map_err(ImageCause::Image),
            }
        }));
    }
}

impl SourceGroup {
    fn admission_bytes(
        &self,
        src: &SourceImage,
        pending: &[PendingRender],
        claims: &[VariantClaim],
        workers: usize,
    ) -> u64 {
        let src_pixels = u64::from(src.metadata.width) * u64::from(src.metadata.height);
        let decoded = if src.svg.is_some() {
            0
        } else {
            src_pixels.saturating_mul(4)
        };
        let geometry_peak = self
            .geometries
            .iter()
            .map(|geo| {
                let out = u64::from(geo.pixels.width()) * u64::from(geo.pixels.height());
                let scratch = geo
                    .renders
                    .iter()
                    .map(|&idx| {
                        claims[pending[idx].claim]
                            .recipe
                            .estimated_encoding_scratch_bytes(workers, src.metadata.has_alpha)
                    })
                    .max()
                    .unwrap_or(0);
                let encoding = decoded
                    .saturating_add(out.saturating_mul(4))
                    .saturating_add(scratch);
                geo.pixels
                    .estimated_working_bytes(&src.metadata)
                    .max(encoding)
            })
            .max()
            .unwrap_or(0);
        // 16-bit RGBA decode uses 8 B/px before in-place normalization.
        let decoding = if src.svg.is_some() {
            0
        } else {
            src_pixels.saturating_mul(8)
        };
        decoding.max(geometry_peak)
    }

    fn render(
        &self,
        src: &SourceImage,
        pending: &[PendingRender],
        claims: &[VariantClaim],
        cache: &PixelCache,
        cancel: &BuildCancellation,
        workers: usize,
    ) -> Vec<RenderedImage> {
        let cap = self.geometries.iter().map(|g| g.renders.len()).sum();
        let mut out = Vec::with_capacity(cap);
        if let Some(svg) = &src.svg {
            self.render_svg(svg, pending, claims, cancel, workers, &mut out);
            return out;
        }
        self.render_raster(src, pending, claims, cache, cancel, workers, &mut out);
        out
    }

    fn render_svg(
        &self,
        svg: &SvgSource,
        pending: &[PendingRender],
        claims: &[VariantClaim],
        cancel: &BuildCancellation,
        workers: usize,
        out: &mut Vec<RenderedImage>,
    ) {
        for geo in &self.geometries {
            let pixels = geo.pixels;
            let rendered = (|| -> Result<ResampledPixels, ImageCause> {
                let vector = pixels.vector_geometry().ok_or(ImageCause::RasterGeometry)?;
                let rgba = svg
                    .render(pixels.width(), pixels.height(), Some(vector.crop().into()))
                    .map_err(ImageCause::Svg)?;
                ResampledPixels::from_rendered(pixels, rgba, cancel).map_err(ImageCause::Image)
            })();
            match rendered {
                Ok(px) => geo.encode_into(&px, pending, claims, cancel, workers, out),
                Err(err) => out.push(RenderedImage {
                    index: geo.renders[0],
                    bytes: Err(err),
                }),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_raster(
        &self,
        src: &SourceImage,
        pending: &[PendingRender],
        claims: &[VariantClaim],
        cache: &PixelCache,
        cancel: &BuildCancellation,
        workers: usize,
        out: &mut Vec<RenderedImage>,
    ) {
        // Consume retained geometries before decode or new resamples can evict them.
        let mut missing = Vec::new();
        for geo in &self.geometries {
            match cache.resampled(src.digest, geo.pixels) {
                Some(px) => geo.encode_into(&px, pending, claims, cancel, workers, out),
                None => missing.push(geo),
            }
        }
        let Some(first_missing) = missing.first() else {
            return;
        };
        let decoded = match cache.decoded(src.digest) {
            Some(retained) => retained,
            None => match DecodedSource::open(src.bytes.as_slice(), cancel) {
                Ok(decoded) => {
                    let decoded = Arc::new(decoded);
                    cache.retain_decoded(src.digest, Arc::clone(&decoded));
                    decoded
                }
                Err(err) => {
                    out.push(RenderedImage {
                        index: first_missing.renders[0],
                        bytes: Err(ImageCause::Image(err)),
                    });
                    return;
                }
            },
        };
        for geo in missing {
            match decoded.pixels(geo.pixels, cancel) {
                Ok(px) => {
                    let px = Arc::new(px);
                    cache.retain_resampled(src.digest, geo.pixels, Arc::clone(&px));
                    geo.encode_into(&px, pending, claims, cancel, workers, out);
                }
                Err(err) => out.push(RenderedImage {
                    index: geo.renders[0],
                    bytes: Err(ImageCause::Image(err)),
                }),
            }
        }
    }
}

fn source_batch(
    groups: &[SourceGroup],
    sources: &[SourceImage],
    pending: &[PendingRender],
    claims: &[VariantClaim],
    src_workers: usize,
    cpu_threads: usize,
) -> SourceBatch {
    let admission = |group: &SourceGroup, workers| {
        group.admission_bytes(&sources[group.src], pending, claims, workers)
    };
    let mut bytes = 0u64;
    let mut count = 0;
    for group in groups.iter().take(src_workers) {
        let admitted = bytes.saturating_add(admission(group, 1));
        if count > 0 && admitted > SOURCE_BATCH_BYTES {
            break;
        }
        bytes = admitted;
        count += 1;
    }
    if count == 1 {
        return SourceBatch {
            count,
            extra_workers: cpu_threads - 1,
        };
    }

    // Other sources may still use the Rayon pool for rows; share only slots outside it.
    let mut batch = SourceBatch {
        count,
        extra_workers: cpu_threads - src_workers,
    };
    if batch.extra_workers == 0 {
        return batch;
    }
    let total = groups[..count]
        .iter()
        .enumerate()
        .fold(0u64, |acc, (idx, group)| {
            acc.saturating_add(admission(group, batch.encoding_workers(idx)))
        });
    if total > SOURCE_BATCH_BYTES {
        batch.extra_workers = 0;
    }
    batch
}

fn group_pending(
    pending: &[PendingRender],
    claims: &[VariantClaim],
    src_count: usize,
) -> Vec<SourceGroup> {
    let mut geo_idx = HashMap::<(usize, PixelRecipe), (usize, usize)>::new();
    let mut groups = Vec::<SourceGroup>::new();
    let mut src_groups = vec![None; src_count];
    for (idx, job) in pending.iter().enumerate() {
        let pixels = claims[job.claim].recipe.pixels();
        if let Some(&(g, geo)) = geo_idx.get(&(job.source, pixels)) {
            groups[g].geometries[geo].renders.push(idx);
            continue;
        }
        let g = match src_groups[job.source] {
            Some(g) => g,
            None => {
                let g = groups.len();
                groups.push(SourceGroup {
                    src: job.source,
                    geometries: Vec::new(),
                });
                src_groups[job.source] = Some(g);
                g
            }
        };
        let geo = groups[g].geometries.len();
        groups[g].geometries.push(RenderGeometry {
            pixels,
            renders: vec![idx],
        });
        geo_idx.insert((job.source, pixels), (g, geo));
    }
    groups
}

pub(super) fn render_pending_variants(
    pending: &[PendingRender],
    claims: &[VariantClaim],
    sources: &[SourceImage],
    pixel_cache: &PixelCache,
    jobs: usize,
    cancel: &BuildCancellation,
) -> Result<Vec<Arc<[u8]>>> {
    let mut groups = group_pending(pending, claims, sources.len());
    let mut rendered = vec![None; pending.len()];
    let mut failure = None::<(usize, ImageCause)>;

    if !groups.is_empty() {
        let cpu_threads = std::thread::available_parallelism()
            .map_or(1, |t| t.get())
            .min(rayon::current_num_threads());
        let src_workers = cpu_threads.min(jobs);
        let pool = image_workers(src_workers, cancel)?;
        pool.install(|| -> Result<()> {
            let mut sizes = groups
                .iter()
                .map(|g| g.admission_bytes(&sources[g.src], pending, claims, 1))
                .collect::<Vec<_>>();
            sizes.sort_unstable_by(|a, b| b.cmp(a));
            let lanes = src_workers > 1
                && cpu_threads == src_workers
                && sizes
                    .iter()
                    .take(src_workers)
                    .copied()
                    .fold(0u64, u64::saturating_add)
                    <= SOURCE_BATCH_BYTES;

            let mut record = |out: RenderedImage| match out.bytes {
                Ok(bytes) => rendered[out.index] = Some(bytes),
                Err(err) => {
                    if failure.as_ref().is_none_or(|(idx, _)| out.index < *idx) {
                        failure = Some((out.index, err));
                    }
                }
            };

            if lanes {
                // Start larger sources first so trailing variants overlap other work.
                groups.sort_by_cached_key(|g| {
                    let src = &sources[g.src];
                    let decoded = if src.svg.is_some() {
                        0
                    } else {
                        u64::from(src.metadata.width) * u64::from(src.metadata.height)
                    };
                    let work = g.geometries.iter().fold(decoded, |acc, geo| {
                        let px = u64::from(geo.pixels.width()) * u64::from(geo.pixels.height());
                        acc.saturating_add(px.saturating_mul(geo.renders.len() as u64))
                    });
                    Reverse(work)
                });
                let next = AtomicUsize::new(0);
                let outcomes = pool.broadcast(|_| {
                    let mut out = Vec::new();
                    while !cancel.is_cancelled() {
                        let idx = next.fetch_add(1, Ordering::Relaxed);
                        let Some(g) = groups.get(idx) else {
                            break;
                        };
                        out.extend(g.render(
                            &sources[g.src],
                            pending,
                            claims,
                            pixel_cache,
                            cancel,
                            1,
                        ));
                    }
                    out
                });
                outcomes.into_iter().flatten().for_each(record);
                return Ok(());
            }

            let mut rem = groups.as_slice();
            while !rem.is_empty() {
                cancel.ensure_active()?;
                let batch = source_batch(rem, sources, pending, claims, src_workers, cpu_threads);
                let (active, next) = rem.split_at(batch.count);
                let outcomes = active
                    .par_iter()
                    .enumerate()
                    .map(|(idx, g)| {
                        g.render(
                            &sources[g.src],
                            pending,
                            claims,
                            pixel_cache,
                            cancel,
                            batch.encoding_workers(idx),
                        )
                    })
                    .collect::<Vec<_>>();
                outcomes.into_iter().flatten().for_each(&mut record);
                rem = next;
            }
            Ok(())
        })?;
    }

    cancel.ensure_active()?;
    if let Some((idx, err)) = failure {
        let job = &pending[idx];
        let op = if sources[job.source].svg.is_some() {
            ImageOperation::Rasterize
        } else {
            ImageOperation::Resize
        };
        return Err(image_failure(&job.file, op, err));
    }
    Ok(rendered
        .into_iter()
        .map(|bytes| bytes.expect("every claimed render has its encoded bytes"))
        .collect())
}
