//! Pixel planes one process keeps between requests.
//!
//! The retention limit is bytes rather than entries: one decoded plane of a large source
//! outweighs hundreds of thumbnails, so a count would bound nothing. A plane larger than the
//! limit is not retained at all, and the least recently used plane leaves first.

use std::collections::VecDeque;
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
pub(crate) struct PixelCache {
    retained: Mutex<Retained>,
}

impl Default for PixelCache {
    fn default() -> Self {
        Self::with_budget(RETENTION_BYTES)
    }
}

impl PixelCache {
    /// A cache retaining at most `budget` bytes of planes.
    fn with_budget(budget: usize) -> Self {
        Self {
            retained: Mutex::new(Retained {
                budget,
                planes: VecDeque::new(),
                bytes: 0,
            }),
        }
    }
}

struct Retained {
    /// The bytes this cache may hold.
    budget: usize,
    /// Least recently used first.
    planes: VecDeque<RetainedPlane>,
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
        self.lock().reuse(|plane| match plane {
            RetainedPlane::Decoded { source: key, plane } if *key == source => {
                Some(Arc::clone(plane))
            }
            _ => None,
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
        self.lock().reuse(|plane| match plane {
            RetainedPlane::Resampled {
                source: key,
                pixels: held,
                plane,
            } if *key == source && *held == pixels => Some(Arc::clone(plane)),
            _ => None,
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
        let mut retained = self.lock();
        if bytes > LARGEST_RETAINED_BYTES.min(retained.budget) {
            return;
        }
        let key = plane.identity();
        if let Some(index) = retained
            .planes
            .iter()
            .rposition(|held| held.identity() == key)
        {
            let replaced = retained.planes.remove(index).unwrap();
            retained.bytes -= replaced.bytes();
        }
        while retained.bytes > retained.budget - bytes {
            let oldest = retained.planes.pop_front().unwrap();
            retained.bytes -= oldest.bytes();
        }
        retained.bytes += bytes;
        retained.planes.push_back(plane);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Retained> {
        self.retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Retained {
    fn reuse<T>(&mut self, extract: impl Fn(&RetainedPlane) -> Option<T>) -> Option<T> {
        let (index, value) = self
            .planes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, plane)| extract(plane).map(|value| (index, value)))?;
        let plane = self.planes.remove(index).unwrap();
        self.planes.push_back(plane);
        Some(value)
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

    fn bytes(&self) -> usize {
        match self {
            Self::Decoded { plane, .. } => plane.allocated_bytes(),
            Self::Resampled { plane, .. } => plane.allocated_bytes(),
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
                filter: ResizeFilter::Triangle,
                ..ResizeOptions::default()
            },
        )
        .unwrap()
        .pixels()
    }

    /// Bytes one decoded plane of `width` × `height` pixels costs the budget.
    const fn plane_bytes(width: u32, height: u32) -> usize {
        width as usize * height as usize * 4
    }

    /// Planes fill a small budget exactly; one more evicts the least recently used, and the plane
    /// used since then stays.
    #[test]
    fn eviction_drops_oldest_plane() {
        let plane = plane_bytes(8, 8);
        let cache = PixelCache::with_budget(plane * 3);
        for index in 0..3u8 {
            cache.retain_decoded(source(index), decoded(8, 8));
        }
        assert!(cache.decoded(source(0)).is_some(), "a hit keeps the plane");
        cache.retain_decoded(source(3), decoded(8, 8));
        assert!(cache.decoded(source(0)).is_some(), "the reused plane stays");
        assert!(
            cache.decoded(source(1)).is_none(),
            "the oldest plane leaves"
        );
    }

    /// A plane beyond the largest retained size is refused, so one huge source cannot empty
    /// the planes the author is working on.
    #[test]
    fn oversized_plane_is_not_retained() {
        let cache = PixelCache::default();
        cache.retain_decoded(source(1), decoded(4096, 4160));
        assert!(cache.decoded(source(1)).is_none());

        let cache = PixelCache::with_budget(plane_bytes(8, 8) - 1);
        cache.retain_decoded(source(1), decoded(8, 8));
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
