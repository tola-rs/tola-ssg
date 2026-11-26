//! The fixed-point tap weights one axis of a resize uses, and the kernels they come from.

use std::f64::consts::PI;

use crate::RESIZE_TOO_LARGE;
use crate::cancellation::Cancellation;
use crate::recipe::ResizeFilter;
use crate::{ImageError, ImageResult};

/// Fractional bits of a quantized weight.
///
/// Fourteen bits hold a weight far below one sample step. The accumulator bound this precision has
/// to satisfy is stated and checked in `AxisWeights::new`.
const WEIGHT_PRECISION: u32 = 14;

/// The quantized representation of a weight sum of one.
const WEIGHT_ONE: i32 = 1 << WEIGHT_PRECISION;

const RESAMPLE_FAILURE: &str =
    "the image could not be resampled at this size; try another `width` or `height`";

/// Round a weighted tap sum back into the sample domain.
///
/// Every axis normalizes its weights to exactly [`WEIGHT_ONE`], so this is a shift rather than a
/// division by a value the loop would have to read.
pub(super) fn quantize(total: i32) -> i32 {
    (total + WEIGHT_ONE / 2) >> WEIGHT_PRECISION
}

pub(super) fn quantize_coverage(total: i64) -> i64 {
    (total + i64::from(WEIGHT_ONE / 2)) >> WEIGHT_PRECISION
}

pub(super) fn check_coverage_bounds(first: &AxisWeights, second: &AxisWeights) -> ImageResult<()> {
    let lane = quantize_coverage(65_025 * i64::from(first.absolute_sum));
    let sum = lane
        .checked_mul(i64::from(second.absolute_sum))
        .and_then(|sum| sum.checked_add(i64::from(WEIGHT_ONE / 2)))
        .ok_or_else(|| ImageError::unreadable(RESAMPLE_FAILURE))?;
    let output = sum >> WEIGHT_PRECISION;
    if lane > i64::from(i32::MAX)
        || output
            .checked_mul(255)
            .and_then(|value| value.checked_add(output / 2))
            .is_none()
    {
        return Err(ImageError::unreadable(RESAMPLE_FAILURE));
    }
    Ok(())
}

/// The magnitude a first-pass lane reaches at the axis's absolute weight sum.
///
/// A sample is one channel of one pixel, at most 255 either way, so `magnitude * u8::MAX` bounds the
/// fixed-point tap sum that [`quantize`] rounds into the lane domain.
fn first_pass_lane_bound(magnitude: i32) -> i64 {
    (i64::from(magnitude) * i64::from(u8::MAX) + i64::from(WEIGHT_ONE) / 2) >> WEIGHT_PRECISION
}

/// The taps and fixed-point weights one axis of a resize uses.
pub(super) struct AxisWeights {
    starts: Vec<u32>,
    offsets: Vec<usize>,
    weights: Vec<i16>,
    absolute_sum: i32,
    max_taps: u32,
}

impl AxisWeights {
    pub(super) fn new(
        src: u32,
        dst: u32,
        filter: ResizeFilter,
        cancel: &dyn Cancellation,
    ) -> ImageResult<Self> {
        let scale = f64::from(src) / f64::from(dst);
        let filter_scale = scale.max(1.0);
        let support = filter.support() * filter_scale;
        let taps = filter.tap_count_bound(src, dst) as usize;
        let mut axis = Self {
            starts: Vec::new(),
            offsets: Vec::new(),
            weights: Vec::new(),
            absolute_sum: 0,
            max_taps: 0,
        };
        axis.starts
            .try_reserve_exact(dst as usize)
            .map_err(|err| ImageError::codec(RESIZE_TOO_LARGE, err))?;
        axis.offsets
            .try_reserve_exact(
                (dst as usize)
                    .checked_add(1)
                    .ok_or_else(|| ImageError::unreadable(RESIZE_TOO_LARGE))?,
            )
            .map_err(|err| ImageError::codec(RESIZE_TOO_LARGE, err))?;
        axis.weights
            .try_reserve_exact(
                (dst as usize)
                    .checked_mul(taps)
                    .ok_or_else(|| ImageError::unreadable(RESIZE_TOO_LARGE))?,
            )
            .map_err(|err| ImageError::codec(RESIZE_TOO_LARGE, err))?;
        let mut raw = Vec::new();
        raw.try_reserve_exact(taps)
            .map_err(|err| ImageError::codec(RESIZE_TOO_LARGE, err))?;
        for idx in 0..dst {
            cancel.ensure_active()?;
            let center = (f64::from(idx) + 0.5) * scale;
            let start = ((center - support + 0.5).floor().max(0.0) as u64).min(u64::from(src) - 1);
            let end = ((center + support + 0.5).floor() as u64).clamp(start + 1, u64::from(src));
            axis.max_taps = axis.max_taps.max((end - start) as u32);
            raw.clear();
            let mut total = 0.0f64;
            for s in start..end {
                let w = filter.kernel((s as f64 + 0.5 - center) / filter_scale);
                raw.push(w);
                total += w;
            }
            if total.is_nan() || total <= 0.0 {
                return Err(ImageError::unreadable(RESAMPLE_FAILURE));
            }
            axis.starts.push(start as u32);
            let first_w = axis.weights.len();
            axis.offsets.push(first_w);
            let mut mag = 0i32;
            let mut sum = 0i32;
            let mut largest = (0usize, 0.0f64);
            for (off, w) in raw.iter().enumerate() {
                let scaled = (w / total * f64::from(WEIGHT_ONE)).round();
                if scaled.is_nan() || scaled.abs() >= f64::from(i16::MAX) {
                    return Err(ImageError::unreadable(RESAMPLE_FAILURE));
                }
                let wi = scaled as i16;
                axis.weights.push(wi);
                sum += i32::from(wi);
                mag = mag.saturating_add(i32::from(wi).abs());
                if w.abs() > largest.1 {
                    largest = (off, w.abs());
                }
            }
            // Absorb rounding residual into the strongest tap so every axis sums to WEIGHT_ONE.
            let strongest = &mut axis.weights[first_w + largest.0];
            let old_w = i32::from(*strongest);
            let adj = old_w + (WEIGHT_ONE - sum);
            if !(-i32::from(i16::MAX)..=i32::from(i16::MAX)).contains(&adj) {
                return Err(ImageError::unreadable(RESAMPLE_FAILURE));
            }
            mag += adj.abs() - old_w.abs();
            *strongest = adj as i16;
            // Accumulator bound uses |weight| sum (negative lobes can cancel in signed sum).
            let lane_bound = first_pass_lane_bound(mag).max(i64::from(u8::MAX));
            if lane_bound > i64::from(i16::MAX)
                || i64::from(mag) * lane_bound + i64::from(WEIGHT_ONE) / 2 > i64::from(i32::MAX)
            {
                return Err(ImageError::unreadable(RESAMPLE_FAILURE));
            }
            axis.absolute_sum = axis.absolute_sum.max(mag);
        }
        axis.offsets.push(axis.weights.len());
        Ok(axis)
    }

    pub(super) const fn max_taps(&self) -> u32 {
        self.max_taps
    }

    pub(super) fn at(&self, idx: u32) -> (u32, &[i16]) {
        let idx = idx as usize;
        (
            self.starts[idx],
            &self.weights[self.offsets[idx]..self.offsets[idx + 1]],
        )
    }
}

impl ResizeFilter {
    /// Upper bound on taps for one output sample.
    pub(super) fn tap_count_bound(self, src: u32, dst: u32) -> u32 {
        let scale = (f64::from(src) / f64::from(dst)).max(1.0);
        let taps = (2.0 * (self.support() * scale).ceil() + 1.0) as u32;
        taps.min(src)
    }

    /// The kernel's support in source samples at unit scale: a tap beyond it has no weight.
    fn support(self) -> f64 {
        match self {
            Self::Nearest => 0.5,
            Self::Triangle => 1.0,
            Self::CatmullRom => 2.0,
            Self::Gaussian => 2.0,
            Self::Lanczos3 => 3.0,
        }
    }

    /// The kernel's weight at a distance in source samples from an output sample's center.
    fn kernel(self, dist: f64) -> f64 {
        let d = dist.abs();
        match self {
            Self::Nearest => f64::from(d <= 0.5),
            Self::Triangle => (1.0 - d).max(0.0),
            Self::CatmullRom => {
                const A: f64 = -0.5;
                if d < 1.0 {
                    ((A + 2.0) * d - (A + 3.0)) * d * d + 1.0
                } else if d < 2.0 {
                    (((d - 5.0) * d + 8.0) * d - 4.0) * A
                } else {
                    0.0
                }
            }
            Self::Gaussian => exp_neg(0.5 * d * d),
            Self::Lanczos3 => {
                if d < 3.0 {
                    sinc(d) * sinc(d / 3.0)
                } else {
                    0.0
                }
            }
        }
    }
}

/// The normalized cardinal sine.
fn sinc(x: f64) -> f64 {
    if x == 0.0 { 1.0 } else { sin_pi(x) / (PI * x) }
}

/// `sin(π · x)` for `x` in `[0, 4]`, evaluated with arithmetic only.
///
/// Terms through `z^17/17!` on a quarter period stay within `1e-13`, far below one weight step.
fn sin_pi(x: f64) -> f64 {
    const TERMS: [f64; 8] = [
        -1.0 / 6.0,
        1.0 / 120.0,
        -1.0 / 5040.0,
        1.0 / 362_880.0,
        -1.0 / 39_916_800.0,
        1.0 / 6_227_020_800.0,
        -1.0 / 1_307_674_368_000.0,
        1.0 / 355_687_428_096_000.0,
    ];
    debug_assert!((0.0..=4.0).contains(&x));
    let whole = x.floor();
    let frac = x - whole;
    let frac = if frac > 0.5 { 1.0 - frac } else { frac };
    let z = PI * frac;
    let z2 = z * z;
    let mut poly = 0.0f64;
    for term in TERMS.iter().rev() {
        poly = term + z2 * poly;
    }
    let series = z * (1.0 + z2 * poly);
    if (whole as u64).is_multiple_of(2) {
        series
    } else {
        -series
    }
}

/// `exp(−x)` for `x` in `[0, 2]`, evaluated with arithmetic only.
fn exp_neg(x: f64) -> f64 {
    let mut term = 1.0f64;
    let mut total = 1.0f64;
    for i in 1..=20 {
        term *= -x / f64::from(i);
        total += term;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NO_CANCELLATION;

    #[test]
    fn kernel_series_match_ref() {
        let mut x = 0.0f64;
        while x <= 4.0 {
            assert!((sin_pi(x) - (PI * x).sin()).abs() < 1e-12, "sin_pi({x})");
            x += 0.000_976_562_5;
        }
        let mut x = 0.0f64;
        while x <= 2.0 {
            assert!((exp_neg(x) - (-x).exp()).abs() < 1e-13, "exp_neg({x})");
            x += 0.001_953_125;
        }
    }

    #[test]
    fn quantize_rounds_halves_up() {
        for v in [-2i32, -1, 0, 1, 2, 3, 1000] {
            assert_eq!(quantize(v * WEIGHT_ONE), v);
            assert_eq!(quantize(v * WEIGHT_ONE + WEIGHT_ONE / 2), v + 1);
        }
    }

    #[test]
    fn wider_kernels_more_taps() {
        let taps = |filter| {
            let weights = AxisWeights::new(64, 16, filter, &NO_CANCELLATION).unwrap();
            let (start, taps) = weights.at(8);
            assert!(start + taps.len() as u32 <= 64);
            let sum: i32 = taps.iter().map(|w| i32::from(*w)).sum();
            assert_eq!(sum, WEIGHT_ONE, "{filter:?} tap sum drifted");
            taps.len()
        };
        assert!(taps(ResizeFilter::Triangle) > 0);
        assert!(taps(ResizeFilter::CatmullRom) > taps(ResizeFilter::Triangle));
        assert!(taps(ResizeFilter::Gaussian) >= taps(ResizeFilter::CatmullRom));
        assert!(taps(ResizeFilter::Lanczos3) > taps(ResizeFilter::Gaussian));
    }

    #[test]
    fn kernels_decay_from_center() {
        for filter in [
            ResizeFilter::Triangle,
            ResizeFilter::CatmullRom,
            ResizeFilter::Gaussian,
            ResizeFilter::Lanczos3,
        ] {
            let support = filter.support();
            let center = filter.kernel(0.0);
            assert!(center > 0.0);
            assert!(filter.kernel(support * 0.5).abs() < center);
            assert!(filter.kernel(support * 1.5) >= 0.0);
        }
    }
}
