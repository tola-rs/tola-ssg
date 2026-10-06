//! Row-parallel work: one output row per unit, split across threads only where rows are
//! independent.
//!
//! Every caller here derives a row of output from rows of input and never reads another output
//! row, so the split cannot change a byte of the result: the same code and the same values run,
//! on one thread or many. The threshold keeps small images on the calling thread, where a
//! hand-off costs more than the rows it divides.
use anyhow::{Context, Result};
use rayon::prelude::*;

/// Bytes of the split buffer a call must cover before its rows are worth handing to threads.
///
/// Measured with two pools in one process, alternating and taking the minimum of six rounds: jobs up
/// to sixteen megabytes are a wash on the pool, and a job of sixty-four megabytes runs 4.3 times
/// faster there, so a hand-off only pays once a job is far larger than one thread's working set.
const SPLIT_BYTES: usize = 1 << 24;

/// Tasks each thread takes from one split: enough to balance uneven rows, few enough that the
/// hand-off of a row stays amortized over the work it divides.
const TASKS_PER_THREAD: usize = 4;

/// Source bytes one chunk of [`rewrite_rows_in_place`] stages aside before its rows are rewritten.
///
/// The staging is the one buffer that rewrite adds, so the bound is stated in the bytes it holds:
/// the output a chunk covers is the bound scaled by the ratio of the two strides, and the bound is
/// the smaller of this constant and the plane's own output, so staging never costs more than the
/// plane copy it replaces.
///
/// Measured on a 4200×3200 RGB8 plane over the same pool, alternating and taking the minimum of
/// seven rounds: chunks covering between one and sixteen megabytes of output all land within noise
/// of each other, and only a chunk below a megabyte pays for its own hand-off, so the bound lives at
/// the cheap end of that band where the staging is a small fraction of the plane it replaces.
const STAGED_CHUNK_BYTES: usize = 1 << 21;

/// Output bytes a chunk must cover before its rows are worth handing to the pool.
///
/// The one caller reads at most two source bytes per output byte, so a chunk holding its whole
/// staging bound covers at least half of it in output: only a plane too small to fill one chunk,
/// and the chunk left over at the end, rewrite on the calling thread.
const SPLIT_STAGED_BYTES: usize = 1 << 18;

/// Whether a job covering `bytes` bytes, worth handing over only past `threshold`, goes to the pool.
fn splits_on_pool(bytes: usize, threshold: usize) -> bool {
    bytes >= threshold && rayon::current_num_threads() >= 2
}

/// Task size depends on row count; sample width only controls the hand-off threshold.
fn rows_per_pool_task<T>(rows: &[T], stride: usize) -> usize {
    rows.len()
        .div_ceil(stride)
        .div_ceil(rayon::current_num_threads() * TASKS_PER_THREAD)
        .max(1)
}

/// Run `row_work` for every `stride`-sized row of `rows`, with the row's index.
///
/// `initial` creates the scratch one thread keeps across its rows.
pub(crate) fn for_each_row<T, S, I, F>(
    rows: &mut [T],
    stride: usize,
    initial: I,
    row_work: F,
) -> Result<()>
where
    T: Send,
    S: Send,
    I: Fn() -> S + Send + Sync,
    F: Fn(&mut S, usize, &mut [T]) -> Result<()> + Send + Sync,
{
    if !splits_on_pool(size_of_val(rows), SPLIT_BYTES) {
        let mut state = initial();
        for (index, row) in rows.chunks_mut(stride).enumerate() {
            row_work(&mut state, index, row)?;
        }
        return Ok(());
    }
    let per_task = rows_per_pool_task(rows, stride);
    rows.par_chunks_mut(stride)
        .with_min_len(per_task)
        .enumerate()
        .try_for_each_init(initial, |state, (index, row)| row_work(state, index, row))
}

/// Run `row_work` for every output row together with the input row it reads.
///
/// The two slices must describe the same number of rows.
pub(crate) fn for_each_row_pair<T, U, S, I, F>(
    output: &mut [T],
    output_stride: usize,
    input: &[U],
    input_stride: usize,
    initial: I,
    row_work: F,
) -> Result<()>
where
    T: Send,
    U: Sync,
    S: Send,
    I: Fn() -> S + Send + Sync,
    F: Fn(&mut S, usize, &[U], &mut [T]) -> Result<()> + Send + Sync,
{
    if !splits_on_pool(size_of_val(output), SPLIT_BYTES) {
        let mut state = initial();
        for (index, (row, source)) in output
            .chunks_mut(output_stride)
            .zip(input.chunks(input_stride))
            .enumerate()
        {
            row_work(&mut state, index, source, row)?;
        }
        return Ok(());
    }
    let per_task = rows_per_pool_task(output, output_stride);
    output
        .par_chunks_mut(output_stride)
        .with_min_len(per_task)
        .zip(input.par_chunks(input_stride))
        .enumerate()
        .try_for_each_init(initial, |state, (index, (row, source))| {
            row_work(state, index, source, row)
        })
}

/// Rewrite every row of one buffer in place, every row's source bytes staged aside first.
///
/// A row's output may be wider or narrower than the samples it is rewritten from, so writing one
/// row can land on another row's samples. Staging a chunk before any row of it is rewritten keeps
/// each row's work on its own output, which is what lets a chunk's rows run in parallel. Chunks are
/// staged in the order the rewrite migrates: a row that grows writes past its own staged source into
/// the rows that follow it, so the last chunk is staged first; a row that shrinks writes back into
/// the rows that precede it, so the first chunk is staged first.
///
/// The caller states non-zero rows and strides, a buffer holding every destination byte the rows
/// produce (`rows * destination_stride` of them) and every source sample they read (the first
/// `rows * source_stride` bytes). A decode refuses a source without pixels before it gets here.
pub(crate) fn rewrite_rows_in_place<S, I, F>(
    buffer: &mut [u8],
    rows: usize,
    source_stride: usize,
    destination_stride: usize,
    initial: I,
    row_work: F,
) -> Result<()>
where
    S: Send,
    I: Fn() -> S + Send + Sync,
    F: Fn(&mut S, &[u8], &mut [u8]) -> Result<()> + Send + Sync,
{
    let staging_bound = STAGED_CHUNK_BYTES.min(rows.saturating_mul(destination_stride));
    let chunk_rows = (staging_bound / source_stride).max(1).min(rows.max(1));
    let chunks = rows.div_ceil(chunk_rows);
    let chunk_bytes = chunk_rows.saturating_mul(source_stride);
    let mut staging = Vec::new();
    staging
        .try_reserve_exact(chunk_bytes)
        .context("the image is too large to hold in memory; use a smaller image")?;
    staging.resize(chunk_bytes, 0);
    let growing = destination_stride >= source_stride;
    for step in 0..chunks {
        let chunk = if growing { chunks - 1 - step } else { step };
        let first = chunk * chunk_rows;
        let last = (first + chunk_rows).min(rows);
        let source = &buffer[first * source_stride..last * source_stride];
        let staged = &mut staging[..source.len()];
        staged.copy_from_slice(source);
        let destination = &mut buffer[first * destination_stride..last * destination_stride];
        if !splits_on_pool(destination.len(), SPLIT_STAGED_BYTES) {
            let mut state = initial();
            for (index, row) in destination.chunks_mut(destination_stride).enumerate() {
                row_work(
                    &mut state,
                    &staged[index * source_stride..][..source_stride],
                    row,
                )?;
            }
            continue;
        }
        let per_task = rows_per_pool_task(destination, destination_stride);
        destination
            .par_chunks_mut(destination_stride)
            .with_min_len(per_task)
            .enumerate()
            .try_for_each_init(&initial, |state, (index, row)| {
                row_work(
                    state,
                    &staged[index * source_stride..][..source_stride],
                    row,
                )
            })?;
    }
    Ok(())
}

/// Whether any row holds a value the predicate accepts, over rows the caller may split.
///
/// The answer does not depend on the split: every row is tested with the same predicate, and the
/// first acceptance ends the walk whether it runs here or on another thread.
pub(crate) fn any_row<T: Sync>(
    rows: &[T],
    stride: usize,
    holds: impl Fn(&T) -> bool + Send + Sync,
) -> bool {
    if !splits_on_pool(size_of_val(rows), SPLIT_BYTES) {
        return rows.chunks(stride).any(|row| row.iter().any(&holds));
    }
    rows.par_chunks(stride).any(|row| row.iter().any(&holds))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row wide enough that a whole buffer's worth of them is worth the hand-off the constant
    /// describes, and few enough rows that the pool's tasks spread across threads.
    const ROW: usize = 1 << 17;

    /// A job exactly at the threshold: one thread takes the serial path and two or more take the
    /// split one, which is what lets the runs below compare the two paths instead of one of them.
    /// A smaller job would leave both runs on the serial path and prove nothing.
    const JOB: usize = SPLIT_BYTES;

    /// One output byte: the sample beneath it and its own row's index, never zero so a byte no row
    /// wrote stays visible.
    fn expected(index: usize, offset: usize, sample: u8) -> u8 {
        ((usize::from(sample) + index + offset) % 251 + 1) as u8
    }

    fn pool(threads: usize) -> rayon::ThreadPool {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
    }

    /// The bytes a one-thread and a four-thread run of `job` must land: every byte the one its own
    /// row wrote, and the two runs identical.
    fn assert_runs_agree(job: impl Fn(usize) -> Vec<u8>, expected: impl Fn(usize) -> u8) {
        let serial = job(1);
        let parallel = job(4);
        let foreign = serial
            .iter()
            .enumerate()
            .find(|(offset, byte)| **byte != expected(*offset))
            .map(|(offset, _)| offset);
        assert!(foreign.is_none(), "byte {foreign:?} is not its own row's");
        assert_eq!(serial, parallel);
    }

    /// Every byte is written from its own row's index, and four threads sharing the rows land the
    /// same bytes as one thread taking them all.
    #[test]
    fn splitting_rows_does_not_change_the_bytes() {
        let run = |threads| {
            let mut buffer = vec![0u8; JOB];
            pool(threads).install(|| {
                for_each_row(
                    &mut buffer,
                    ROW,
                    || (),
                    |(), index, row| {
                        for (offset, byte) in row.iter_mut().enumerate() {
                            *byte = expected(index, offset, 0);
                        }
                        Ok(())
                    },
                )
                .unwrap()
            });
            buffer
        };
        assert_runs_agree(run, |offset| expected(offset / ROW, offset % ROW, 0));
    }

    #[test]
    fn task_division_counts_rows() {
        pool(4).install(|| {
            let bytes = [0u8; 128 * 4];
            let samples = [[0i32; 4]; 128 * 4];
            assert_eq!(rows_per_pool_task(&bytes, 4), 8);
            assert_eq!(rows_per_pool_task(&samples, 4), 8);
        });
    }

    #[test]
    fn typed_rows_preserve_samples() {
        let stride = ROW / size_of::<[i32; 4]>();
        let run = |threads| {
            let mut samples = vec![[0i32; 4]; JOB / size_of::<[i32; 4]>()];
            pool(threads).install(|| {
                for_each_row(
                    &mut samples,
                    stride,
                    || (),
                    |(), y, row| {
                        for (x, sample) in row.iter_mut().enumerate() {
                            *sample = [y as i32, x as i32, (y + x) as i32, 255];
                        }
                        Ok(())
                    },
                )
                .unwrap();
            });
            samples
        };
        let serial = run(1);
        assert_eq!(serial, run(4));
        for (offset, sample) in serial.iter().enumerate() {
            let (y, x) = (offset / stride, offset % stride);
            assert_eq!(*sample, [y as i32, x as i32, (y + x) as i32, 255]);
        }
    }

    /// Every output row is written from the input row at its own index, and four threads sharing the
    /// rows land the same bytes as one thread taking them all.
    #[test]
    fn parallel_row_pairs_land_the_same_bytes() {
        let input: Vec<u8> = (0..SPLIT_BYTES)
            .map(|offset| (offset % 251) as u8)
            .collect();
        let run = |threads| {
            let mut buffer = vec![0u8; JOB];
            pool(threads).install(|| {
                for_each_row_pair(
                    &mut buffer,
                    ROW,
                    &input,
                    ROW,
                    || (),
                    |(), index, source, row| {
                        for (offset, (byte, sample)) in row.iter_mut().zip(source).enumerate() {
                            *byte = expected(index, offset, *sample);
                        }
                        Ok(())
                    },
                )
                .unwrap()
            });
            buffer
        };
        assert_runs_agree(run, |offset| {
            expected(offset / ROW, offset % ROW, input[offset])
        });
    }

    /// One rewritten row: its own source bytes, offset by the byte's position in the row, so a
    /// byte written from another row's samples is visible.
    fn rewritten_row(source: &[u8], row: &mut [u8]) {
        for (offset, byte) in row.iter_mut().enumerate() {
            *byte = source[offset % source.len()].wrapping_add(offset as u8);
        }
    }

    /// A rewrite that grows or shrinks its rows lands the bytes a rewrite into separate buffers
    /// lands, run on one thread or four, and across more chunks than one staging holds.
    #[test]
    fn staged_rewrite_matches_separate_buffers() {
        for (source_stride, destination_stride) in [(3072, 4096), (8192, 4096)] {
            let rows = if source_stride < destination_stride {
                3000
            } else {
                1500
            };
            let source: Vec<u8> = (0..rows * source_stride)
                .map(|offset| (offset % 251) as u8)
                .collect();
            let mut expected = vec![0u8; rows * destination_stride];
            for (row, output) in expected.chunks_mut(destination_stride).enumerate() {
                let input = &source[row * source_stride..][..source_stride];
                rewritten_row(input, output);
            }
            let run = |threads| {
                let mut buffer = source.clone();
                if destination_stride > source_stride {
                    buffer.resize(rows * destination_stride, 0);
                }
                pool(threads).install(|| {
                    rewrite_rows_in_place(
                        &mut buffer,
                        rows,
                        source_stride,
                        destination_stride,
                        || (),
                        |(), input, output| {
                            rewritten_row(input, output);
                            Ok(())
                        },
                    )
                    .unwrap()
                });
                buffer.truncate(rows * destination_stride);
                buffer
            };
            assert_runs_agree(run, |offset| expected[offset]);
        }
    }
}
