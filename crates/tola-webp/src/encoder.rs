//! WebP frame encoding and macroblock traversal.

use std::ops::Range;

use crate::EncodeOptions;
use crate::color::{RgbaFrame, YUV_BYTES_PER_MACROBLOCK, YuvRow, transparent};
use crate::container;
use crate::macroblock::{CHROMA_SIDE, CoefficientSpool, Luma, MACROBLOCK_SIDE, Macroblock};
use crate::partitions::encode_frame;
use crate::quantize;
#[cfg(not(target_arch = "wasm32"))]
use crate::reconstruction::Plane;
use crate::reconstruction::Reconstruction;
#[cfg(not(target_arch = "wasm32"))]
use crate::token::Edges;
use crate::token::TokenContexts;
use crate::trellis;
use crate::vp8;
use fearless_simd::{Level, Simd, dispatch};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{self, Receiver, SyncSender};

mod search;

use self::search::{Quantizer, prefers_intra4, search_macroblock};

const CANCEL_MACROBLOCKS: usize = 8;

#[cfg(not(target_arch = "wasm32"))]
const PARALLEL_MIN_PIXELS: usize = 16_384;
#[cfg(not(target_arch = "wasm32"))]
const STRIP_MIN_PIXELS: usize = 256 * 256;
#[cfg(not(target_arch = "wasm32"))]
const STRIP_MIN_COLUMNS: usize = 8;
#[cfg(not(target_arch = "wasm32"))]
const FOUR_STRIP_MIN_PIXELS: usize = 128 * 1024;
#[cfg(not(target_arch = "wasm32"))]
const FOUR_STRIP_MIN_COLUMNS: usize = 16;

pub(crate) fn estimated_scratch_bytes(
    w: u32,
    h: u32,
    workers: usize,
    has_transparency: bool,
) -> u64 {
    let px = u64::from(w) * u64::from(h);
    let cols = u64::from(w.div_ceil(MACROBLOCK_SIDE as u32));
    let mbs = cols * u64::from(h.div_ceil(MACROBLOCK_SIDE as u32));
    #[cfg(not(target_arch = "wasm32"))]
    let alpha_overlaps = workers > 1 && px >= PARALLEL_MIN_PIXELS as u64;
    #[cfg(target_arch = "wasm32")]
    let alpha_overlaps = false;
    let strips = strip_count(w as usize, h as usize, workers.max(1)) as u64;
    let modes = mbs * size_of::<Macroblock>() as u64;
    let coeffs = CoefficientSpool::estimated_bytes(mbs, strips);
    let yuv = cols * YUV_BYTES_PER_MACROBLOCK as u64;
    let prev_edge = u64::from(h > MACROBLOCK_SIDE as u32);
    let y_w = cols * MACROBLOCK_SIDE as u64 + strips.saturating_sub(1) * 5 + 4;
    let uv_w = cols * CHROMA_SIDE as u64 + strips.saturating_sub(1);
    let row_rings =
        y_w * (MACROBLOCK_SIDE as u64 + prev_edge) + 2 * uv_w * (CHROMA_SIDE as u64 + prev_edge);
    let filled = if has_transparency {
        transparent::scratch_bytes(px)
    } else {
        0
    };
    let analysis = yuv + row_rings + trellis::restricted_scratch_bytes(strips) + modes + filled;
    let part_bufs = px.saturating_mul(8);
    let parts = modes.saturating_add(part_bufs);
    let frame = coeffs.saturating_add(analysis.max(parts));
    if !has_transparency {
        return frame;
    }
    let alpha = px.saturating_mul(9).saturating_add(128 * 1024);
    if alpha_overlaps {
        frame.saturating_add(alpha)
    } else {
        frame.max(part_bufs.saturating_add(alpha))
    }
}

/// Failure reason for WebP frame encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Image dimension is zero or exceeds 16,383 pixels.
    #[error("the image is empty or larger than WebP's 16,383-pixel limit")]
    Dimensions,
    /// Pixel buffer length does not match `width * height * 4`.
    #[error("the pixel buffer does not match width × height × 4")]
    BufferLength,
    /// Frame header and mode partition exceed VP8's 19-bit length limit.
    #[error("the image's prediction modes exceed WebP's frame-header limit")]
    HeaderTooLarge,
    /// Encoding was cancelled by the caller.
    #[error("WebP encoding was cancelled")]
    Cancelled,
}

pub(crate) fn check_cancelled(cancel: &(dyn Fn() -> bool + Sync)) -> Result<(), Error> {
    if cancel() {
        return Err(Error::Cancelled);
    }
    Ok(())
}

pub(crate) fn encode(
    rgba: &[u8],
    w: u32,
    h: u32,
    opts: EncodeOptions,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<container::WebP, Error> {
    check_cancelled(cancel)?;
    const MAX_DIM: u32 = 16_383;
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
        return Err(Error::Dimensions);
    }
    if rgba.len() != w as usize * h as usize * 4 {
        return Err(Error::BufferLength);
    }
    dispatch!(Level::new(), simd => encode_with_simd(simd, rgba, w, h, opts, cancel))
}

fn encode_with_simd<S: Simd>(
    simd: S,
    rgba: &[u8],
    w: u32,
    h: u32,
    opts: EncodeOptions,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<container::WebP, Error> {
    let (pw, ph) = (w as usize, h as usize);
    let check_opacity = opts.quality == 84;
    #[cfg(not(target_arch = "wasm32"))]
    let check_opacity = check_opacity || (opts.workers > 1 && pw * ph >= PARALLEL_MIN_PIXELS);
    let opts = if opts.has_transparency.is_none() && check_opacity {
        EncodeOptions {
            has_transparency: Some(crate::alpha::has_transparency(rgba, cancel)?),
            ..opts
        }
    } else {
        opts
    };
    let alpha = || crate::alpha::encode(rgba, pw, ph, opts.has_transparency, cancel);
    let frame = |workers| {
        encode_frame_from_rgba(
            simd,
            rgba,
            pw,
            ph,
            EncodeOptions { workers, ..opts },
            cancel,
        )
    };
    let workers = opts.workers.max(1);
    #[cfg(not(target_arch = "wasm32"))]
    if workers > 1 && opts.has_transparency != Some(false) && pw * ph >= PARALLEL_MIN_PIXELS {
        return std::thread::scope(|scope| {
            let alpha = scope.spawn(alpha);
            let frame = frame(workers - 1)?;
            let alpha = alpha.join().expect("alpha encoding panicked")?;
            Ok(container::WebP {
                width: w,
                height: h,
                frame,
                alpha,
            })
        });
    }
    let frame = frame(workers)?;
    Ok(container::WebP {
        width: w,
        height: h,
        frame,
        alpha: alpha()?,
    })
}

fn strip_count(w: usize, h: usize, workers: usize) -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (w, h, workers);
        1
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let cols = w.div_ceil(MACROBLOCK_SIDE);
        let area = w * h;
        if area < STRIP_MIN_PIXELS || cols < STRIP_MIN_COLUMNS || h <= MACROBLOCK_SIDE {
            return 1;
        }
        let wanted = if area >= FOUR_STRIP_MIN_PIXELS && cols >= FOUR_STRIP_MIN_COLUMNS {
            4
        } else {
            2
        };
        wanted.min(workers)
    }
}

fn encode_frame_from_rgba<S: Simd>(
    simd: S,
    rgba: &[u8],
    w: usize,
    h: usize,
    opts: EncodeOptions,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<vp8::Frame, Error> {
    let EncodeOptions {
        quality,
        trellis,
        workers,
        ..
    } = opts;
    let (q, mut lambda) = quantize::frame_quantizer(quality);
    if quality == 84 && opts.has_transparency == Some(false) {
        lambda = lambda * 112 / 100;
    }
    let lf = (q / 2).min(63);
    let filled = if opts.has_transparency == Some(false) {
        None
    } else {
        transparent::extend(rgba, w, h, cancel)?
    };
    let weighted =
        opts.has_transparency != Some(false) && crate::color::prefers_weighted_chroma(rgba, w, h);
    let src =
        RgbaFrame::new(filled.as_deref().unwrap_or(rgba), w, h).with_weighted_chroma(weighted);
    let search = Quantizer::new(simd, quantize::factors(q), lambda, trellis);
    let intra4_first = prefers_intra4(simd, src);
    #[cfg(not(target_arch = "wasm32"))]
    let strips = strip_count(w, h, workers);
    #[cfg(not(target_arch = "wasm32"))]
    if strips > 1 {
        let strips = analyze_strips(src, search, strips, intra4_first, cancel)?;
        drop(filled);
        return encode_frame(w, h, q, lf, &strips, cancel, workers);
    }
    let strip = analyze_rows(src, search, intra4_first, cancel)?;
    drop(filled);
    encode_frame(w, h, q, lf, &[strip], cancel, workers)
}

pub(crate) struct MacroblockStrip {
    pub(crate) columns: usize,
    pub(crate) macroblocks: Vec<Macroblock>,
    pub(crate) spool: CoefficientSpool,
}

fn analyze_rows<S: Simd>(
    src: RgbaFrame<'_>,
    search: Quantizer<S>,
    intra4_first: bool,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<MacroblockStrip, Error> {
    let cols = src.columns();
    let rows = src.rows();
    let mut raster = MacroblockRaster::full_frame(src, search, intra4_first);
    let mut mbs = Vec::with_capacity(cols * rows);
    for y in 0..rows {
        check_cancelled(cancel)?;
        raster.load_row(y);
        for x in 0..cols {
            if x != 0 && x % CANCEL_MACROBLOCKS == 0 {
                check_cancelled(cancel)?;
            }
            mbs.push(raster.encode(x, y));
        }
    }
    Ok(MacroblockStrip {
        columns: cols,
        macroblocks: mbs,
        spool: raster.spool,
    })
}

struct MacroblockRaster<'a, S> {
    src: RgbaFrame<'a>,
    row: YuvRow,
    recon: Reconstruction,
    q: Quantizer<S>,
    tokens: TokenContexts,
    modes: vp8::BmodeContexts,
    spool: CoefficientSpool,
    intra4_first: bool,
}

impl<'a, S: Simd> MacroblockRaster<'a, S> {
    fn new(
        src: RgbaFrame<'a>,
        recon: Reconstruction,
        q: Quantizer<S>,
        cols: Range<usize>,
        intra4_first: bool,
    ) -> Self {
        Self {
            src,
            row: YuvRow::new(cols),
            recon,
            q,
            tokens: TokenContexts::new(src.columns()),
            modes: vp8::BmodeContexts::new(src.columns()),
            spool: CoefficientSpool::new(),
            intra4_first,
        }
    }

    fn full_frame(src: RgbaFrame<'a>, q: Quantizer<S>, intra4_first: bool) -> Self {
        let cols = src.columns();
        let recon = Reconstruction::new(cols * MACROBLOCK_SIDE, cols * CHROMA_SIDE, src.rows());
        Self::new(src, recon, q, 0..cols, intra4_first)
    }

    fn load_row(&mut self, y: usize) {
        self.row.load(self.q.simd, self.src, y);
    }

    fn encode(&mut self, x: usize, y: usize) -> Macroblock {
        let dense = search_macroblock(
            &self.row,
            &mut self.recon,
            &self.q,
            self.tokens.contexts(x),
            &self.modes,
            (x, y),
            self.intra4_first,
        );
        match &dense.luma {
            Luma::Intra16 { mode, .. } => self.modes.advance_intra(x, *mode),
            Luma::Intra4 { modes } => self.modes.advance_b_pred(x, modes),
        }
        let mb = Macroblock::pack(dense, &mut self.spool);
        self.tokens.advance(x, &mb);
        mb
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct StripBoundary {
    y: [u8; MACROBLOCK_SIDE],
    uv: [[u8; CHROMA_SIDE]; 2],
    tokens: Edges,
    modes: [u8; 4],
}

#[cfg(not(target_arch = "wasm32"))]
impl StripBoundary {
    fn read<S>(raster: &MacroblockRaster<'_, S>, x: usize, y: usize) -> Self {
        fn col<const SIDE: usize>(plane: &Plane, x: usize, y: usize) -> [u8; SIDE] {
            std::array::from_fn(|row| plane.sample(x * SIDE - 1, y * SIDE + row))
        }
        Self {
            y: col(&raster.recon.y, x, y),
            uv: [&raster.recon.u, &raster.recon.v].map(|plane| col(plane, x, y)),
            tokens: raster.tokens.left(),
            modes: raster.modes.left_modes(),
        }
    }

    fn write<S>(self, raster: &mut MacroblockRaster<'_, S>, x: usize, y: usize) {
        fn col<const SIDE: usize>(plane: &mut Plane, x: usize, y: usize, px: [u8; SIDE]) {
            for (row, val) in px.into_iter().enumerate() {
                plane.write(y * SIDE + row, x * SIDE - 1, &[val]);
            }
        }
        col(&mut raster.recon.y, x, y, self.y);
        let [u, v] = self.uv;
        col(&mut raster.recon.u, x, y, u);
        col(&mut raster.recon.v, x, y, v);
        raster.tokens.set_left(self.tokens);
        raster.modes.set_left_modes(self.modes);
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct StripChannels {
    from_left: Option<Receiver<StripBoundary>>,
    to_right: Option<SyncSender<StripBoundary>>,
    from_above_right: Option<Receiver<[u8; 4]>>,
    to_below_left: Option<SyncSender<[u8; 4]>>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
struct StripContext<'a> {
    src: RgbaFrame<'a>,
    intra4_first: bool,
    cols: usize,
    rows: usize,
    cancel: &'a (dyn Fn() -> bool + Sync),
}

#[cfg(not(target_arch = "wasm32"))]
fn analyze_strips<S: Simd>(
    src: RgbaFrame<'_>,
    q: Quantizer<S>,
    strips: usize,
    intra4_first: bool,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<MacroblockStrip>, Error> {
    let cols = src.columns();
    let rows = src.rows();
    let ctx = StripContext {
        src,
        intra4_first,
        cols,
        rows,
        cancel,
    };

    let mut chans: Vec<_> = (0..strips).map(|_| StripChannels::default()).collect();
    for right in 1..strips {
        let left = right - 1;
        let (to_right, from_left) = mpsc::sync_channel(1);
        let (to_below_left, from_above_right) = mpsc::sync_channel(1);
        chans[left].to_right = Some(to_right);
        chans[left].from_above_right = Some(from_above_right);
        chans[right].from_left = Some(from_left);
        chans[right].to_below_left = Some(to_below_left);
    }

    std::thread::scope(|scope| {
        let run = |i, q, chans| {
            let start = i * cols / strips;
            let end = (i + 1) * cols / strips;
            analyze_strip(ctx, q, start, end, chans)
        };
        let last = chans.pop().expect("there are at least two strips");
        let handles: Vec<_> = chans
            .into_iter()
            .enumerate()
            .map(|(i, chans)| {
                let q = q.fork();
                scope.spawn(move || run(i, q, chans))
            })
            .collect();
        let last = run(strips - 1, q, last);

        handles
            .into_iter()
            .map(|h| h.join().expect("an analysis strip panicked"))
            .chain(std::iter::once(last))
            .collect()
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn analyze_strip<S: Simd>(
    ctx: StripContext<'_>,
    q: Quantizer<S>,
    x0: usize,
    x1: usize,
    chans: StripChannels,
) -> Result<MacroblockStrip, Error> {
    let StripContext {
        src,
        intra4_first,
        cols,
        rows,
        cancel,
    } = ctx;
    let recon = Reconstruction::strip(cols * MACROBLOCK_SIDE, cols * CHROMA_SIDE, rows, x0, x1);
    let mut raster = MacroblockRaster::new(src, recon, q, x0..x1, intra4_first);
    let mut mbs = Vec::with_capacity((x1 - x0) * rows);

    for y in 0..rows {
        check_cancelled(cancel)?;
        raster.load_row(y);
        if let Some(rx) = &chans.from_left {
            rx.recv()
                .map_err(|_| Error::Cancelled)?
                .write(&mut raster, x0, y);
        }
        for x in x0..x1 {
            if x != x0 && (x - x0).is_multiple_of(CANCEL_MACROBLOCKS) {
                check_cancelled(cancel)?;
            }
            // Only the row's final macroblock reads the right strip's above-right edge.
            if x + 1 == x1
                && y > 0
                && let Some(rx) = &chans.from_above_right
            {
                let px = rx.recv().map_err(|_| Error::Cancelled)?;
                raster
                    .recon
                    .y
                    .write(y * MACROBLOCK_SIDE - 1, x1 * MACROBLOCK_SIDE, &px);
            }
            let mb = raster.encode(x, y);
            if x == x0
                && y + 1 < rows
                && let Some(tx) = &chans.to_below_left
            {
                let px =
                    raster
                        .recon
                        .y
                        .slice(x0 * MACROBLOCK_SIDE, (y + 1) * MACROBLOCK_SIDE - 1, 4);
                tx.send(px.try_into().unwrap())
                    .map_err(|_| Error::Cancelled)?;
            }
            mbs.push(mb);
        }

        if let Some(tx) = &chans.to_right {
            tx.send(StripBoundary::read(&raster, x1, y))
                .map_err(|_| Error::Cancelled)?;
        }
    }

    Ok(MacroblockStrip {
        columns: x1 - x0,
        macroblocks: mbs,
        spool: raster.spool,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrellisMode;
    use crate::encode_rgba;

    fn gradient(w: usize, h: usize) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[
                    (x * 255 / w.max(1)) as u8,
                    (y * 255 / h.max(1)) as u8,
                    96,
                    255,
                ]);
            }
        }
        rgba
    }

    #[test]
    fn gradient_encode_decode() {
        let rgba = gradient(32, 32);
        let webp = encode_rgba(&rgba, 32, 32, 80).unwrap();
        let decoded = image::load_from_memory_with_format(&webp, image::ImageFormat::WebP)
            .unwrap()
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (32, 32));
        let mut total = 0u64;
        for (src, dst) in rgba.as_chunks::<4>().0.iter().zip(decoded.pixels()) {
            for (&a, &b) in src.iter().take(3).zip(dst.0.iter().take(3)) {
                total += u64::from(a.abs_diff(b));
            }
        }
        let mean = total / (32 * 32 * 3) as u64;
        assert!(mean < 32, "mean absolute error {mean} is too high");
    }

    #[test]
    fn reject_invalid_inputs() {
        assert_eq!(encode_rgba(&[], 0, 1, 80), Err(Error::Dimensions));
        assert_eq!(encode_rgba(&[], 1, 0, 80), Err(Error::Dimensions));
        assert_eq!(encode_rgba(&[], 16_384, 1, 80), Err(Error::Dimensions));
        assert_eq!(encode_rgba(&[], 1, 16_384, 80), Err(Error::Dimensions));
        assert_eq!(
            encode_rgba(&[], u32::MAX, u32::MAX, 80),
            Err(Error::Dimensions)
        );
        assert_eq!(encode_rgba(&[0; 4], 2, 2, 80), Err(Error::BufferLength));
    }

    #[test]
    fn recon_matches_decoder() {
        for (w, h) in [(1, 33), (17, 1), (7, 7), (15, 15), (31, 17), (65, 33)] {
            let mut rng = 1u32;
            let rgba: Vec<u8> = (0..w * h)
                .flat_map(|_| {
                    let mut px = [255; 4];
                    for ch in &mut px[..3] {
                        rng ^= rng << 13;
                        rng ^= rng >> 17;
                        rng ^= rng << 5;
                        *ch = (rng >> 24) as u8;
                    }
                    px
                })
                .collect();
            let simd = fearless_simd::Fallback::new();
            let src = RgbaFrame::new(&rgba, w, h);
            let cols = src.columns();
            let rows = src.rows();
            let intra4_first = prefers_intra4(simd, src);
            for quality in [0, 25, 75, 100] {
                let (q, lambda) = quantize::frame_quantizer(quality);
                let factors = quantize::factors(q);
                let search = Quantizer::new(simd, factors, lambda, TrellisMode::Exact);
                let mut raster = MacroblockRaster::full_frame(src, search, intra4_first);
                let mut mbs = Vec::with_capacity(cols * rows);
                let mut expected = [MACROBLOCK_SIDE, CHROMA_SIDE, CHROMA_SIDE]
                    .map(|side| vec![0; cols * rows * side * side]);
                for y in 0..rows {
                    raster.load_row(y);
                    for x in 0..cols {
                        mbs.push(raster.encode(x, y));
                        for ((plane, side), buf) in [
                            (&raster.recon.y, MACROBLOCK_SIDE),
                            (&raster.recon.u, CHROMA_SIDE),
                            (&raster.recon.v, CHROMA_SIDE),
                        ]
                        .into_iter()
                        .zip(&mut expected)
                        {
                            for row in 0..side {
                                let start = (y * side + row) * plane.frame_stride() + x * side;
                                buf[start..start + side].copy_from_slice(plane.slice(
                                    x * side,
                                    y * side + row,
                                    side,
                                ));
                            }
                        }
                    }
                }
                let strip = MacroblockStrip {
                    columns: cols,
                    macroblocks: mbs,
                    spool: raster.spool,
                };
                // Zero loop filter isolates predictor and coefficient reconstruction.
                let frame = encode_frame(w, h, q, 0, &[strip], &|| false, 1).unwrap();
                let decoded = image_webp::vp8::Vp8Decoder::decode_frame(std::io::Cursor::new(
                    frame.chunks().concat(),
                ))
                .unwrap();
                for ((name, exp), act) in ["Y", "U", "V"].into_iter().zip(&expected).zip([
                    &decoded.ybuf,
                    &decoded.ubuf,
                    &decoded.vbuf,
                ]) {
                    assert_eq!(exp, act, "{w}x{h} q{quality} {name}");
                }
            }
        }
    }
}
