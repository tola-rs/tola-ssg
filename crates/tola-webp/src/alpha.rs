//! Lossless alpha payloads for extended WebP images.

use crate::Error;
use crate::encoder::check_cancelled;

mod groups;
mod palette;
mod repeated;
mod rle;
mod spatial;

const RAW_ALPHA: u8 = 0;
const VP8L_ALPHA: u8 = 1;
const PIXELS_PER_CHECK: usize = 16 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Predictor {
    None = 0,
    Horizontal = 1,
    Vertical = 2,
    Gradient = 3,
    Half,
}

pub(crate) fn encode(
    rgba: &[u8],
    width: usize,
    height: usize,
    has_transparency: Option<bool>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, Error> {
    check_cancelled(cancelled)?;
    if has_transparency == Some(false) {
        return Ok(None);
    }

    let raw_len = width * height + 1;
    let first = rgba[3];
    if rgba.last() == Some(&first)
        && (first != u8::MAX || has_transparency.is_none())
        && uniform_alpha(rgba, first, cancelled)?
    {
        if first == u8::MAX {
            return Ok(None);
        }
        // The 1×1 frame has one Huffman symbol per channel and no per-pixel bits.
        // ALPH supplies the actual dimensions and decodes that symbol for every pixel.
        let alpha = rle::encode(&[first], Predictor::None.stream(), raw_len, cancelled)?
            .unwrap_or_else(|| {
                let mut raw = vec![first; raw_len];
                raw[0] = RAW_ALPHA;
                raw
            });
        check_cancelled(cancelled)?;
        return Ok(Some(alpha));
    }

    let pixels = rgba.as_chunks::<4>().0;
    let mut alpha = vec![0; raw_len];
    let ([alternative, repeated], try_half) = alternative_predictors(pixels, width, height);
    // Small marks often repeat the same few alpha values without a predictor.
    let unfiltered = (pixels.len() <= 512 * 1024 && alternative != Some(Predictor::None))
        .then_some(Predictor::None);
    let mut best = encode_predictors(
        pixels,
        width,
        &mut alpha[1..],
        raw_len,
        [Some(Predictor::Vertical), unfiltered, alternative, repeated]
            .into_iter()
            .flatten(),
        cancelled,
    )?;
    // Compare complete payloads. A flat half-gradient stream can still beat
    // the previous winner after its own tile codes are applied.
    if try_half
        && let Some(half) = encode_predictors(
            pixels,
            width,
            &mut alpha[1..],
            raw_len,
            [Predictor::Half],
            cancelled,
        )?
        && best
            .as_ref()
            .is_none_or(|legacy| half.len().next_multiple_of(2) < legacy.len().next_multiple_of(2))
    {
        best = Some(half);
    }
    if let Some(bytes) = best.as_ref()
        && bytes.len() >= 4096
        && bytes.len() * 12 >= pixels.len()
        && (64 * 1024..=1024 * 1024).contains(&pixels.len())
        && let Some(predictor) = spatial::palette_predictor(pixels, width, height, bytes[0])
    {
        filter(predictor, pixels, width, &mut alpha[1..], cancelled)?;
        let method = VP8L_ALPHA | (predictor as u8) << 2;
        if let Some(bytes) = palette::encode(
            &mut alpha[1..],
            width,
            height,
            method,
            bytes.len(),
            true,
            cancelled,
        )? {
            best = Some(bytes);
        }
    }
    if (64 * 1024..=512 * 1024).contains(&pixels.len())
        && width >= 16
        && height >= 16
        && best.as_ref().map_or(raw_len, Vec::len) * 3 >= pixels.len()
    {
        filter(Predictor::None, pixels, width, &mut alpha[1..], cancelled)?;
        let limit = best.as_ref().map_or(raw_len, Vec::len);
        if let Some(bytes) = spatial::average(&mut alpha[1..], width, limit, cancelled)? {
            best = Some(bytes);
        }
    }
    let limit = best.as_ref().map_or(raw_len, Vec::len);
    if (64 * 1024..=1024 * 1024).contains(&pixels.len())
        && width >= 16
        && height >= 16
        && limit * 6 >= pixels.len()
        && let Some(predictor) = spatial::outer(pixels, width, height)
    {
        filter(predictor, pixels, width, &mut alpha[1..], cancelled)?;
        let method = VP8L_ALPHA | (predictor as u8) << 2;
        if let Some(bytes) = spatial::encode(&mut alpha[1..], width, method, limit, cancelled)? {
            best = Some(bytes);
        }
    }
    let limit = best.as_ref().map_or(raw_len, Vec::len);
    if width >= 16
        && height >= 16
        && (64 * 1024..=1024 * 1024).contains(&pixels.len())
        && let Some((predictor, side)) = spatial::detailed_predictor(pixels, width, height, limit)
    {
        filter(predictor, pixels, width, &mut alpha[1..], cancelled)?;
        if let Some(candidate) = spatial::encode_tiles(
            &mut alpha[1..],
            width,
            VP8L_ALPHA | (predictor as u8) << 2,
            limit,
            side,
            spatial::TileSearch::Detailed,
            cancelled,
        )? {
            best = Some(candidate);
        }
    }
    if width >= 16 && height >= 16 && (64 * 1024..=1024 * 1024).contains(&pixels.len()) {
        let mut below = 0;
        let mut partial = 0;
        let mut count = 0;
        for rgba in pixels.iter().step_by((pixels.len() / 2048).max(1)) {
            below += usize::from(rgba[3] < 128);
            partial += usize::from(rgba[3] > 0 && rgba[3] < 255);
            count += 1;
        }
        let limit = best.as_ref().map_or(raw_len, Vec::len);
        let cfg = if below == 0 && partial * 2 >= count && limit * 4 >= pixels.len() {
            Some((64, spatial::TileSearch::HighOpacity))
        } else if partial * 32 >= count && partial * 12 <= count && limit * 40 >= pixels.len() {
            Some((32, spatial::TileSearch::SparseEdges))
        } else {
            None
        };
        if let Some((side, kind)) = cfg {
            filter(Predictor::None, pixels, width, &mut alpha[1..], cancelled)?;
            if let Some(new) = spatial::encode_tiles(
                &mut alpha[1..],
                width,
                VP8L_ALPHA,
                limit,
                side,
                kind,
                cancelled,
            )? {
                best = Some(new);
            }
        }
    }
    if best.is_some() {
        return Ok(best);
    }

    for (raw, pixels) in alpha[1..]
        .chunks_mut(PIXELS_PER_CHECK)
        .zip(pixels.chunks(PIXELS_PER_CHECK))
    {
        check_cancelled(cancelled)?;
        for (value, pixel) in raw.iter_mut().zip(pixels) {
            *value = pixel[3];
        }
    }
    Ok(Some(alpha))
}

fn encode_predictors(
    pixels: &[[u8; 4]],
    width: usize,
    residuals: &mut [u8],
    limit: usize,
    predictors: impl IntoIterator<Item = Predictor>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<u8>>, Error> {
    let mut best = None::<(Predictor, repeated::Payload)>;
    let mut last = Predictor::Vertical;
    for predictor in predictors {
        last = predictor;
        filter(predictor, pixels, width, residuals, cancelled)?;
        let stream = predictor.stream();
        let cap = best.as_ref().map_or(limit, |(_, payload)| payload.len());
        if let Some(compressed) = repeated::encode(residuals, width, stream, cap, cancelled)? {
            best = Some((predictor, compressed));
        }
        let cap = best.as_ref().map_or(limit, |(_, payload)| payload.len());
        if let Some(compressed) = rle::encode(residuals, stream, cap, cancelled)? {
            best = Some((predictor, repeated::Payload::Bytes(compressed)));
        }
    }
    if let Some((predictor, best)) = best {
        let try_palette = predictor != Predictor::Half
            && best.len() >= 4096
            && (64 * 1024..=1024 * 1024).contains(&residuals.len());
        if predictor != last && (try_palette || matches!(best, repeated::Payload::Tiles(_))) {
            filter(predictor, pixels, width, residuals, cancelled)?;
        }
        let best = best.finish(residuals, width, cancelled)?;
        if try_palette
            && let Some(candidate) = palette::encode(
                residuals,
                width,
                residuals.len() / width,
                best[0],
                best.len(),
                false,
                cancelled,
            )?
        {
            return Ok(Some(candidate));
        }
        return Ok(Some(best));
    }
    Ok(None)
}

/// Check opaque inputs before reserving a worker for alpha encoding.
pub(crate) fn has_transparency(
    rgba: &[u8],
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<bool, Error> {
    Ok(!uniform_alpha(rgba, u8::MAX, cancelled)?)
}

fn alternative_predictors(
    pixels: &[[u8; 4]],
    width: usize,
    height: usize,
) -> ([Option<Predictor>; 2], bool) {
    if width < 2 || height < 2 {
        return ([None; 2], false);
    }
    // Sample off-grid to avoid aliasing with periodic masks.
    const SAMPLES: u32 = 2048;
    let try_half = (65_536..=512 * 1024).contains(&pixels.len());
    let mut histograms = [[0u32; 256]; 4];
    let mut half_counts = [0u32; 256];
    let mut repeated = [0u32; 4];
    let mut seed = 0x71a5_b963u32;
    for _ in 0..SAMPLES {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let x = 1 + (seed >> 12) as usize % (width - 1);
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let y = 1 + (seed >> 12) as usize % (height - 1);
        let at = y * width + x;
        let [value, left, top, corner] =
            [at, at - 1, at - width, at - width - 1].map(|index| pixels[index][3]);
        for (histogram, prediction) in
            histograms
                .iter_mut()
                .zip([left, top, gradient(left, top, corner), 0])
        {
            histogram[usize::from(value.wrapping_sub(prediction))] += 1;
        }
        if try_half {
            half_counts[usize::from(value.wrapping_sub(half(left, top, corner)))] += 1;
        }
        if y > 1 {
            let above = at - width;
            let [above_left, previous, previous_left] =
                [above - 1, above - width, above - width - 1].map(|index| pixels[index][3]);
            let current = [left, top, gradient(left, top, corner), 0];
            let previous = [
                above_left,
                previous,
                gradient(above_left, previous, previous_left),
                0,
            ];
            for (index, count) in repeated.iter_mut().enumerate() {
                *count += u32::from(
                    value.wrapping_sub(current[index]) == top.wrapping_sub(previous[index]),
                );
            }
        }
    }
    // The collision count measures concentration without a floating-point entropy calculation.
    let scores =
        histograms.map(|histogram| histogram.iter().map(|count| count * count).sum::<u32>());
    let alternative = [Predictor::Horizontal, Predictor::Gradient, Predictor::None]
        .into_iter()
        .filter(|&predictor| scores[predictor.score_index()] > scores[1])
        .max_by_key(|&predictor| scores[predictor.score_index()]);
    let try_half = if try_half {
        let [horizontal, vertical, gradient, unfiltered] = histograms.each_ref().map(entropy_score);
        let half = entropy_score(&half_counts);
        // Prefer the half gradient only if it beats the linear filters and
        // stays close to the full gradient within the sampling error.
        let best_linear = horizontal.max(vertical).max(unfiltered);
        half > best_linear + SAMPLES / 2 && half + SAMPLES >= gradient
    } else {
        false
    };
    // A concentrated histogram need not form long runs. Try another direction
    // when it repeats substantially more often across rows than the chosen filters.
    let included = alternative.map_or(repeated[1], |p| repeated[1].max(repeated[p.score_index()]));
    let row_repeats = [Predictor::Horizontal, Predictor::Gradient]
        .into_iter()
        .filter(|&p| repeated[p.score_index()] > included + SAMPLES / 12)
        .max_by_key(|&p| repeated[p.score_index()]);
    ([alternative, row_repeats], try_half)
}

// Approximate sum(n × log2(n)) in units of 1/16 bit. Sample counts are fixed.
fn entropy_score(counts: &[u32; 256]) -> u32 {
    counts.iter().copied().map(entropy_cost).sum()
}

const fn entropy_cost(n: u32) -> u32 {
    const LOG_FRACTION: [u32; 16] = [0, 1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 15];
    if n < 2 {
        return 0;
    }
    let log = n.ilog2();
    let normalized = (n << (31 - log)) >> 27;
    n * (16 * log + LOG_FRACTION[(normalized & 15) as usize])
}

#[derive(Clone, Copy)]
enum Stream {
    Filtered(u8),
    HalfGradient,
}

impl Predictor {
    fn stream(self) -> Stream {
        match self {
            Self::Half => Stream::HalfGradient,
            predictor => Stream::Filtered(VP8L_ALPHA | (predictor as u8) << 2),
        }
    }

    fn score_index(self) -> usize {
        match self {
            Self::Horizontal => 0,
            Self::Vertical => 1,
            Self::Gradient => 2,
            Self::None => 3,
            Self::Half => 4,
        }
    }
}

fn filter(
    predictor: Predictor,
    pixels: &[[u8; 4]],
    width: usize,
    destination: &mut [u8],
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), Error> {
    let mut above: &[[u8; 4]] = &[];
    for (y, (row, output)) in pixels
        .chunks_exact(width)
        .zip(destination.chunks_exact_mut(width))
        .enumerate()
    {
        check_cancelled(cancelled)?;
        match (predictor, y == 0) {
            (Predictor::None, _) => {
                for (value, pixel) in output.iter_mut().zip(row) {
                    *value = pixel[3];
                }
            }
            (Predictor::Vertical, false) => {
                for ((value, pixel), top) in output.iter_mut().zip(row).zip(above) {
                    *value = pixel[3].wrapping_sub(top[3]);
                }
            }
            (Predictor::Gradient | Predictor::Half, false) => {
                output[0] = row[0][3].wrapping_sub(above[0][3]);
                for ((value, pair), top) in output[1..]
                    .iter_mut()
                    .zip(row.windows(2))
                    .zip(above.windows(2))
                {
                    let estimate = if predictor == Predictor::Half {
                        half(pair[0][3], top[1][3], top[0][3])
                    } else {
                        gradient(pair[0][3], top[1][3], top[0][3])
                    };
                    *value = pair[1][3].wrapping_sub(estimate);
                }
            }
            _ => {
                output[0] = row[0][3].wrapping_sub(above.first().map_or(0, |pixel| pixel[3]));
                for (value, pair) in output[1..].iter_mut().zip(row.windows(2)) {
                    *value = pair[1][3].wrapping_sub(pair[0][3]);
                }
            }
        }
        above = row;
    }
    Ok(())
}

fn gradient(left: u8, top: u8, corner: u8) -> u8 {
    (i16::from(left) + i16::from(top) - i16::from(corner)).clamp(0, 255) as u8
}

fn half(left: u8, top: u8, corner: u8) -> u8 {
    let average = (u16::from(left) + u16::from(top)) as i16 / 2;
    (average + (average - i16::from(corner)) / 2).clamp(0, 255) as u8
}

fn uniform_alpha(
    rgba: &[u8],
    value: u8,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<bool, Error> {
    const MASK: u64 = u64::from_ne_bytes([0, 0, 0, 255, 0, 0, 0, 255]);
    let alpha = u64::from_ne_bytes([0, 0, 0, value, 0, 0, 0, value]);
    for pixels in rgba.chunks(4 * PIXELS_PER_CHECK) {
        check_cancelled(cancelled)?;
        let (pairs, tail) = pixels.as_chunks::<8>();
        if pairs
            .iter()
            .any(|pair| u64::from_ne_bytes(*pair) & MASK != alpha)
            || tail.get(3).is_some_and(|&last| last != value)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{VP8L_ALPHA, rle};

    #[test]
    fn rle_uses_actual_stream_size() {
        let mut samples: Vec<_> = (0..=u8::MAX).cycle().take(256 * 16).collect();
        assert!(
            rle::encode(
                &samples,
                super::Predictor::None.stream(),
                samples.len() + 1,
                &|| false
            )
            .unwrap()
            .is_none()
        );

        let middle = (samples.len() - 256) / 2;
        samples[..middle].fill(0);
        samples[middle + 256..].fill(0);
        assert!(
            rle::encode(
                &samples,
                super::Predictor::None.stream(),
                samples.len() + 1,
                &|| false
            )
            .unwrap()
            .is_some()
        );
    }

    #[test]
    fn alpha_round_trips() {
        for (width, height) in [(1, 1), (1, 7), (8, 3), (17, 19), (32, 33)] {
            for pattern in 0..4 {
                let rgba: Vec<u8> = (0..width * height)
                    .flat_map(|index| {
                        let alpha = match pattern {
                            0 => 0,
                            1 => 170,
                            2 => (index % 17) as u8,
                            _ => (index as u8).wrapping_mul(37),
                        };
                        [37, 123, 211, alpha]
                    })
                    .collect();
                let webp = crate::encode_rgba(&rgba, width, height, 75).unwrap();
                let decoded = image::load_from_memory(&webp).unwrap().into_rgba8();
                assert_eq!(decoded.dimensions(), (width, height));
                for (actual, expected) in decoded.pixels().zip(rgba.as_chunks::<4>().0) {
                    assert_eq!(
                        actual[3], expected[3],
                        "{width}x{height}, pattern {pattern}"
                    );
                }
            }
        }
    }

    #[test]
    fn repeated_alpha_rows_round_trip() {
        let (width, height) = (129, 97);
        let rgba: Vec<_> = (0..height)
            .flat_map(|y| (0..width).flat_map(move |x| [37, 123, 211, ((x + y) / 4) as u8]))
            .collect();
        let alpha = super::encode(&rgba, width, height, Some(true), &|| false)
            .unwrap()
            .unwrap();
        assert!(alpha.len() < width * height / 20);
        let webp = crate::encode_rgba(&rgba, width as u32, height as u32, 80).unwrap();
        let decoded = image::load_from_memory(&webp).unwrap().into_rgba8();
        assert!(
            decoded
                .as_raw()
                .iter()
                .skip(3)
                .step_by(4)
                .eq(rgba.iter().skip(3).step_by(4))
        );
    }

    #[test]
    fn half_gradient_decodes_first_row_and_column() {
        let (width, height) = (35, 37);
        let pixels: Vec<_> = (0..height)
            .flat_map(|y| (0..width).map(move |x| [12, 93, 160, (x * 11 + y * 7) as u8]))
            .collect();
        let mut residuals = vec![0; pixels.len()];
        let predictor = super::Predictor::Half;
        super::filter(predictor, &pixels, width, &mut residuals, &|| false).unwrap();
        let alpha = rle::encode(&residuals, predictor.stream(), residuals.len(), &|| false)
            .unwrap()
            .unwrap();
        let mut options = crate::EncodeOptions::with_workers(80, 1);
        options.has_transparency = Some(false);
        let mut frame = crate::encoder::encode(
            pixels.as_flattened(),
            width as u32,
            height as u32,
            options,
            &|| false,
        )
        .unwrap();
        frame.alpha = Some(alpha);
        let decoded = image::load_from_memory(&frame.into_bytes())
            .unwrap()
            .into_rgba8();
        assert!(
            decoded
                .pixels()
                .map(|pixel| pixel[3])
                .eq(pixels.iter().map(|pixel| pixel[3]))
        );
    }

    #[test]
    fn indexed_alpha_rows_round_trip() {
        let (width, height) = (513usize, 257usize);
        let rgba: Vec<_> = (0..height)
            .flat_map(|y| {
                (0..width).flat_map(move |x| {
                    let band = ((x + 3 * y) / 17) % 8;
                    [12, 93, 160, (band * 31) as u8]
                })
            })
            .collect();
        let mut samples: Vec<_> = rgba.iter().copied().skip(3).step_by(4).collect();
        let alpha = super::palette::encode(
            &mut samples,
            width,
            height,
            VP8L_ALPHA,
            width * height + 1,
            false,
            &|| false,
        )
        .unwrap()
        .unwrap();
        let mut options = crate::EncodeOptions::with_workers(80, 1);
        options.has_transparency = Some(false);
        let mut image =
            crate::encoder::encode(&rgba, width as u32, height as u32, options, &|| false).unwrap();
        assert!(alpha.len() < width * height / 8);
        image.alpha = Some(alpha);
        let webp = image.into_bytes();
        let decoded = image::load_from_memory(&webp).unwrap().into_rgba8();
        assert!(
            decoded
                .as_raw()
                .iter()
                .skip(3)
                .step_by(4)
                .eq(rgba.iter().skip(3).step_by(4))
        );
    }
}
