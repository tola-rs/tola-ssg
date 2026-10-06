//! Pixel planes one process keeps between requests.
//!
//! The retention limit is bytes rather than entries: one decoded plane of a large source
//! outweighs hundreds of thumbnails, so a count would bound nothing. A plane larger than the
//! limit is not retained at all, and the least recently used plane leaves first.

use std::sync::{Arc, Mutex};

use tola_image::{DecodedSource, PixelRecipe, ResampledPixels};
use tola_typst::ContentDigest;

/// Bytes every retained plane shares.
const RETENTION_BYTES: usize = 192 * 1024 * 1024;

/// The largest plane worth retaining: beyond this one hit would evict the whole rest.
const LARGEST_RETAINED_BYTES: usize = 64 * 1024 * 1024;

/// Resampled and decoded planes retained for the sources this process is working on.
///
/// Clones of one [`BuildResources`](crate::BuildResources) share the same retention, so an
/// author tuning one image reuses every plane the earlier requests already computed.
#[derive(Default)]
pub(crate) struct PixelCache {
    retained: Mutex<Retained>,
}

#[derive(Default)]
struct Retained {
    /// Least recently used first.
    planes: Vec<RetainedPlane>,
    bytes: usize,
}

enum RetainedPlane {
    Decoded {
        source: ContentDigest,
        plane: Arc<DecodedSource>,
    },
    Resampled {
        source: ContentDigest,
        pixels: PixelRecipe,
        plane: Arc<ResampledPixels>,
    },
}

impl PixelCache {
    /// The retained decode of one source, when this process already decoded those bytes.
    pub(crate) fn decoded(&self, source: ContentDigest) -> Option<Arc<DecodedSource>> {
        let mut retained = self.lock();
        let index = retained.planes.iter().position(
            |plane| matches!(plane, RetainedPlane::Decoded { source: key, .. } if *key == source),
        )?;
        retained.reuse(index, |plane| match plane {
            RetainedPlane::Decoded { plane, .. } => Some(Arc::clone(plane)),
            RetainedPlane::Resampled { .. } => None,
        })
    }

    /// Retain one source's decoded plane for the requests that follow.
    pub(crate) fn retain_decoded(&self, source: ContentDigest, plane: Arc<DecodedSource>) {
        self.retain(RetainedPlane::Decoded { source, plane });
    }

    /// The retained pixels of one geometry, when this process already resampled them.
    pub(crate) fn resampled(
        &self,
        source: ContentDigest,
        pixels: PixelRecipe,
    ) -> Option<Arc<ResampledPixels>> {
        let mut retained = self.lock();
        let index = retained.planes.iter().position(|plane| {
            matches!(plane, RetainedPlane::Resampled { source: key, pixels: held, .. }
                if *key == source && *held == pixels)
        })?;
        retained.reuse(index, |plane| match plane {
            RetainedPlane::Resampled { plane, .. } => Some(Arc::clone(plane)),
            RetainedPlane::Decoded { .. } => None,
        })
    }

    /// Retain one geometry's pixels, which every encoding of that geometry shares.
    pub(crate) fn retain_resampled(
        &self,
        source: ContentDigest,
        pixels: PixelRecipe,
        plane: Arc<ResampledPixels>,
    ) {
        self.retain(RetainedPlane::Resampled {
            source,
            pixels,
            plane,
        });
    }

    fn retain(&self, plane: RetainedPlane) {
        let bytes = plane.bytes();
        if bytes > LARGEST_RETAINED_BYTES {
            return;
        }
        let mut retained = self.lock();
        let key = plane.identity();
        if let Some(index) = retained
            .planes
            .iter()
            .position(|held| held.identity() == key)
        {
            let replaced = retained.planes.remove(index);
            retained.bytes -= replaced.bytes();
        }
        retained.bytes += bytes;
        retained.planes.push(plane);
        while retained.bytes > RETENTION_BYTES && retained.planes.len() > 1 {
            let oldest = retained.planes.remove(0);
            retained.bytes -= oldest.bytes();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Retained> {
        self.retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Retained {
    /// Move the plane at `index` to the most recently used end and extract from it.
    fn reuse<T>(&mut self, index: usize, extract: impl Fn(&RetainedPlane) -> T) -> T {
        let plane = self.planes.remove(index);
        let extracted = extract(&plane);
        self.planes.push(plane);
        extracted
    }
}

/// What tells two retained planes apart: the source, and the geometry for resampled pixels.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PlaneIdentity {
    Decoded(ContentDigest),
    Resampled(ContentDigest, PixelRecipe),
}

impl RetainedPlane {
    fn identity(&self) -> PlaneIdentity {
        match self {
            Self::Decoded { source, .. } => PlaneIdentity::Decoded(*source),
            Self::Resampled { source, pixels, .. } => PlaneIdentity::Resampled(*source, *pixels),
        }
    }

    /// What one plane costs the budget: the bytes it hands out, never the capacity its allocation
    /// holds, so a plane with spare capacity is retained on the strength of what it publishes.
    fn bytes(&self) -> usize {
        match self {
            Self::Decoded { plane, .. } => {
                let metadata = plane.metadata();
                metadata.width as usize * metadata.height as usize * 4
            }
            Self::Resampled { plane, .. } => plane.rgba().len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    use tola_image::{
        ImageFormat, ImageMetadata, ImageRecipe, NO_CANCELLATION, OutputFormat, ResizeFilter,
        ResizeOperation, ResizeOptions,
    };

    fn source(index: u8) -> ContentDigest {
        ContentDigest::of(&[index])
    }

    fn decoded(width: u32, height: u32) -> Arc<DecodedSource> {
        let pixels = ::image::RgbaImage::from_pixel(width, height, ::image::Rgba([0, 0, 0, 255]));
        let mut encoded = Cursor::new(Vec::new());
        ::image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut encoded, ::image::ImageFormat::Png)
            .unwrap();
        Arc::new(DecodedSource::open(encoded.get_ref(), &NO_CANCELLATION).unwrap())
    }

    fn geometry(width: u32, height: u32) -> PixelRecipe {
        ImageRecipe::resolve(
            &ImageMetadata {
                width,
                height,
                format: ImageFormat::Png,
                has_alpha: false,
                is_lossy: false,
            },
            ResizeOptions {
                width: Some(width),
                height: Some(height),
                operation: ResizeOperation::Scale,
                format: OutputFormat::Png,
                quality: None,
                filter: ResizeFilter::Triangle,
                background: None,
            },
        )
        .unwrap()
        .pixels()
    }

    /// Twenty-four 8 MiB planes fill the limit exactly; one more evicts the plane that was not
    /// used since, and the plane used most recently stays.
    #[test]
    fn budget_evicts_the_least_recently_used() {
        let cache = PixelCache::default();
        for index in 0..24u8 {
            cache.retain_decoded(source(index), decoded(2048, 1024));
        }
        assert!(cache.decoded(source(0)).is_some());
        cache.retain_decoded(source(24), decoded(2048, 1024));
        assert!(cache.decoded(source(0)).is_some());
        assert!(cache.decoded(source(1)).is_none());
    }

    /// A plane beyond the largest retained size is refused, so one huge source cannot empty
    /// the planes the author is working on.
    #[test]
    fn oversized_plane_is_not_retained() {
        let cache = PixelCache::default();
        cache.retain_decoded(source(1), decoded(4096, 4160));
        assert!(cache.decoded(source(1)).is_none());
    }

    #[test]
    fn resampled_planes_are_kept_per_geometry() {
        let cache = PixelCache::default();
        let (source, pixels) = (source(2), geometry(4, 4));
        let ones = Arc::new(decoded(4, 4).pixels(pixels, &NO_CANCELLATION).unwrap());
        cache.retain_resampled(source, pixels, Arc::clone(&ones));
        assert!(cache.resampled(source, pixels).is_some());
        assert!(cache.resampled(source, geometry(8, 8)).is_none());
    }
}
