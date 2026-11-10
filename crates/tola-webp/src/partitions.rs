//! VP8 mode and coefficient partitions, including entropy-table selection.

use crate::bool_writer::BoolWriter;
use crate::container::chunk_size;
use crate::encoder::{Error, MacroblockStrip, check_cancelled};
use crate::macroblock::{CoefficientSpool, Luma, MACROBLOCK_SIDE, Macroblock};
use crate::tables::DEFAULT_COEFF_PROBS;
use crate::token::{
    COEFFICIENT_PROBABILITIES, FittedPartition, ProbabilityFit, TokenCounts, TokenSink,
    TokenWriter, fit_probs,
};
use crate::vp8;

#[cfg(not(target_arch = "wasm32"))]
const PARALLEL_MIN_MACROBLOCKS: usize = 64;

/// Exact acceptance runs only while the estimated gain is small or the image is tiny.
const SMALL_FIT_SAVINGS: u64 = 8 * 256;
const SMALL_IMAGE_AREA: usize = 1 << 14;
/// A second partition write must promise at least eight bytes of savings (Q8 bit cost).
const MIN_SKIP_SAVINGS: u64 = 8 * 8 * 256;

pub(crate) fn encode_frame(
    width: usize,
    height: usize,
    quantizer: u8,
    filter_level: u8,
    strips: &[MacroblockStrip],
    cancelled: &(dyn Fn() -> bool + Sync),
    workers: usize,
) -> Result<vp8::Frame, Error> {
    let partitions = PartitionWriter {
        columns: width.div_ceil(MACROBLOCK_SIDE),
        rows: height.div_ceil(MACROBLOCK_SIDE),
        quantizer,
        filter_level,
        strips,
        cancelled,
        small_image: width * height < SMALL_IMAGE_AREA,
    };
    let mut tokens = TokenWriter::new(partitions.columns);
    let mut nonempty = TokenCounts::new();
    let mut empty = None;
    let mut empty_blocks = 0;
    partitions.for_each_macroblock(|x, macroblock, spool| {
        let counts = if macroblock.has_residual() {
            &mut nonempty
        } else {
            empty_blocks += 1;
            empty.get_or_insert_with(TokenCounts::new)
        };
        tokens.write_macroblock(counts, x, macroblock, spool);
    })?;

    let mut counts = nonempty.into_counts();
    let skip = empty.map(|empty| {
        let fit = fit_probs(&counts);
        let mut omitted_rate = 0;
        let ends = empty.into_counts();
        for ((counts, ends), probability) in
            counts.iter_mut().zip(ends.iter()).zip(DEFAULT_COEFF_PROBS)
        {
            // An empty block emits only the false branch of its end-of-block test.
            counts[0] += ends[0];
            omitted_rate +=
                u64::from(ends[0]) * u64::from(crate::cost::bool_cost(probability, false));
        }
        let coded_blocks = (partitions.macroblock_count() - empty_blocks) as u64;
        let empty_blocks = empty_blocks as u64;
        let total = coded_blocks + empty_blocks;
        let probability = ((coded_blocks * 256 + total / 2) / total).clamp(1, 255) as u8;
        let flag_rate = 8 * 256
            + coded_blocks * u64::from(crate::cost::bool_cost(probability, false))
            + empty_blocks * u64::from(crate::cost::bool_cost(probability, true));
        (probability, fit, omitted_rate, flag_rate)
    });

    let fitted = fit_probs(&counts);
    let mut selected = partitions.write_best_fit(None, fitted.as_ref(), workers)?;
    if let Some((probability, fit, omitted_rate, flag_rate)) = skip
        && omitted_rate + fit.as_ref().map_or(0, |fit| fit.savings)
            >= flag_rate + fitted.as_ref().map_or(0, |fit| fit.savings) + MIN_SKIP_SAVINGS
    {
        let skipped = partitions.write_best_fit(Some(probability), fit.as_ref(), workers)?;
        if skipped.fits() && (!selected.fits() || skipped.chunk_bytes() < selected.chunk_bytes()) {
            selected = skipped;
        }
    }
    vp8::Frame::new(
        width as u16,
        height as u16,
        selected.modes,
        selected.residual,
    )
    .ok_or(Error::HeaderTooLarge)
}

struct Partitions {
    modes: Vec<u8>,
    residual: Vec<u8>,
}

impl Partitions {
    fn fits(&self) -> bool {
        self.modes.len() < vp8::FIRST_PARTITION_LIMIT
    }

    fn chunk_bytes(&self) -> usize {
        chunk_size(vp8::Frame::HEADER_BYTES + self.modes.len() + self.residual.len())
    }
}

struct PartitionWriter<'a> {
    columns: usize,
    rows: usize,
    quantizer: u8,
    filter_level: u8,
    strips: &'a [MacroblockStrip],
    cancelled: &'a (dyn Fn() -> bool + Sync),
    small_image: bool,
}

impl PartitionWriter<'_> {
    fn macroblock_count(&self) -> usize {
        self.columns * self.rows
    }

    fn for_each_macroblock(
        &self,
        mut write: impl FnMut(usize, &Macroblock, &CoefficientSpool),
    ) -> Result<(), Error> {
        for y in 0..self.rows {
            check_cancelled(self.cancelled)?;
            let mut x = 0;
            for strip in self.strips {
                let row = y * strip.columns;
                for macroblock in &strip.macroblocks[row..row + strip.columns] {
                    write(x, macroblock, &strip.spool);
                    x += 1;
                }
            }
        }
        Ok(())
    }

    fn write_best_fit(
        &self,
        skip: Option<u8>,
        fit: Option<&ProbabilityFit>,
        workers: usize,
    ) -> Result<Partitions, Error> {
        let Some(fit) = fit else {
            return self.write_partitions(skip, &DEFAULT_COEFF_PROBS, workers);
        };
        if !self.small_image && fit.savings > SMALL_FIT_SAVINGS {
            let candidate = self.write_partitions(skip, &fit.probs, workers)?;
            if candidate.fits() {
                return Ok(candidate);
            }
            return self.write_partitions(skip, &DEFAULT_COEFF_PROBS, workers);
        }

        // Measure the default residual length alongside the fitted bytes in one token traversal.
        let mut partition = FittedPartition::new(&fit.probs);
        self.write_residual(&mut partition, &fit.probs, skip.is_some())?;
        let (fitted_bytes, default_len) = partition.finish();
        if fitted_bytes.len() >= default_len {
            return self.write_partitions(skip, &DEFAULT_COEFF_PROBS, workers);
        }

        let mut default = self.mode_header(&DEFAULT_COEFF_PROBS, skip);
        let mut candidate = self.mode_header(&fit.probs, skip);
        self.write_modes(
            &mut ModeTee {
                default: &mut default,
                candidate: &mut candidate,
            },
            skip,
        )?;
        let default = default.finish();
        let candidate = Partitions {
            modes: candidate.finish(),
            residual: fitted_bytes,
        };
        if candidate.fits()
            && (default.len() >= vp8::FIRST_PARTITION_LIMIT
                || candidate.modes.len() + candidate.residual.len() < default.len() + default_len)
        {
            return Ok(candidate);
        }
        Ok(Partitions {
            modes: default,
            residual: self.residual_partition(&DEFAULT_COEFF_PROBS, skip)?,
        })
    }

    fn write_partitions(
        &self,
        skip: Option<u8>,
        probs: &[u8; COEFFICIENT_PROBABILITIES],
        workers: usize,
    ) -> Result<Partitions, Error> {
        #[cfg(not(target_arch = "wasm32"))]
        if workers > 1 && self.macroblock_count() >= PARALLEL_MIN_MACROBLOCKS {
            return std::thread::scope(|scope| {
                let modes = scope.spawn(|| self.mode_partition(probs, skip));
                let residual = self.residual_partition(probs, skip);
                Ok(Partitions {
                    modes: modes.join().expect("mode partition panicked")?,
                    residual: residual?,
                })
            });
        }
        #[cfg(target_arch = "wasm32")]
        let _ = workers;

        Ok(Partitions {
            modes: self.mode_partition(probs, skip)?,
            residual: self.residual_partition(probs, skip)?,
        })
    }

    fn mode_header(&self, probs: &[u8; COEFFICIENT_PROBABILITIES], skip: Option<u8>) -> BoolWriter {
        let mut writer = BoolWriter::new();
        vp8::write_frame_header(&mut writer, self.quantizer, self.filter_level, probs, skip);
        writer
    }

    fn mode_partition(
        &self,
        probs: &[u8; COEFFICIENT_PROBABILITIES],
        skip: Option<u8>,
    ) -> Result<Vec<u8>, Error> {
        let mut writer = self.mode_header(probs, skip);
        self.write_modes(&mut writer, skip)?;
        Ok(writer.finish())
    }

    fn write_modes(&self, writer: &mut impl TokenSink, skip: Option<u8>) -> Result<(), Error> {
        let mut modes = vp8::BmodeContexts::new(self.columns);
        self.for_each_macroblock(|x, macroblock, _| {
            if let Some(probability) = skip {
                writer.write_bool(probability, !macroblock.has_residual());
            }
            match &macroblock.luma {
                Luma::Intra16 { mode, .. } => {
                    modes.write_intra(writer, x, *mode, macroblock.chroma_mode);
                }
                Luma::Intra4 { modes: subblocks } => {
                    modes.write_b_pred(writer, x, subblocks, macroblock.chroma_mode);
                }
            }
        })
    }

    fn residual_partition(
        &self,
        probs: &[u8; COEFFICIENT_PROBABILITIES],
        skip: Option<u8>,
    ) -> Result<Vec<u8>, Error> {
        let mut writer = BoolWriter::new();
        self.write_residual(&mut writer, probs, skip.is_some())?;
        Ok(writer.finish())
    }

    fn write_residual(
        &self,
        writer: &mut impl TokenSink,
        probs: &[u8; COEFFICIENT_PROBABILITIES],
        skip: bool,
    ) -> Result<(), Error> {
        let mut tokens = TokenWriter::with_probs(self.columns, *probs);
        self.for_each_macroblock(|x, macroblock, spool| {
            if skip && !macroblock.has_residual() {
                tokens.skip_macroblock(x, &macroblock.luma);
            } else {
                tokens.write_macroblock(writer, x, macroblock, spool);
            }
        })
    }
}

/// Feed one mode sequence to two first partitions whose coefficient headers differ.
struct ModeTee<'a> {
    default: &'a mut BoolWriter,
    candidate: &'a mut BoolWriter,
}

impl TokenSink for ModeTee<'_> {
    fn write_bool(&mut self, probability: u8, value: bool) {
        self.default.write_bool(probability, value);
        self.candidate.write_bool(probability, value);
    }
}
