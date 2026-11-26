//! Row-parallel work: one output row per unit, split across threads only where rows are
//! independent.
//!
//! Every caller here derives a row of output from rows of input and never reads another output
//! row, so the split cannot change a byte of the result: the same code and the same values run,
//! on one thread or many. The threshold keeps small images on the calling thread, where a
//! hand-off costs more than the rows it divides.
use rayon::prelude::*;

use crate::HOLD_IN_MEMORY;
use crate::{ImageError, ImageResult};

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
///
/// wasm has no thread pool to hand work to, so every job runs on the calling thread there.
#[cfg(target_arch = "wasm32")]
fn splits_on_pool(_bytes: usize, _threshold: usize) -> bool {
    false
}

/// Whether a job covering `bytes` bytes, worth handing over only past `threshold`, goes to the pool.
#[cfg(not(target_arch = "wasm32"))]
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
    init: I,
    work: F,
) -> ImageResult<()>
where
    T: Send,
    S: Send,
    I: Fn() -> S + Send + Sync,
    F: Fn(&mut S, usize, &mut [T]) -> ImageResult<()> + Send + Sync,
{
    let work_bytes = size_of_val(rows);
    for_each_row_with_work(rows, stride, work_bytes, init, work)
}

/// Use the first pass's work to decide whether to split when it exceeds the output size.
pub(crate) fn for_each_row_with_work<T, S, I, F>(
    rows: &mut [T],
    stride: usize,
    work_bytes: usize,
    init: I,
    work: F,
) -> ImageResult<()>
where
    T: Send,
    S: Send,
    I: Fn() -> S + Send + Sync,
    F: Fn(&mut S, usize, &mut [T]) -> ImageResult<()> + Send + Sync,
{
    if row_workers(work_bytes.max(size_of_val(rows))) == 1 {
        let mut state = init();
        for (idx, row) in rows.chunks_mut(stride).enumerate() {
            work(&mut state, idx, row)?;
        }
        return Ok(());
    }
    let per_task = rows_per_pool_task(rows, stride);
    rows.par_chunks_mut(stride)
        .with_min_len(per_task)
        .enumerate()
        .try_for_each_init(init, |state, (idx, row)| work(state, idx, row))
}

pub(crate) fn row_workers(work_bytes: usize) -> usize {
    if splits_on_pool(work_bytes, SPLIT_BYTES) {
        rayon::current_num_threads()
    } else {
        1
    }
}

/// Rewrite every row of one buffer in place, staging each chunk's source bytes aside first.
///
/// Chunks iterate backward when `dst_stride >= src_stride` (growing rows overwrite following
/// rows) and forward otherwise (shrinking rows overwrite preceding rows).
pub(crate) fn rewrite_rows_in_place<S, I, F>(
    buf: &mut [u8],
    rows: usize,
    src_stride: usize,
    dst_stride: usize,
    init: I,
    work: F,
) -> ImageResult<()>
where
    S: Send,
    I: Fn() -> S + Send + Sync,
    F: Fn(&mut S, &[u8], &mut [u8]) -> ImageResult<()> + Send + Sync,
{
    let bound = STAGED_CHUNK_BYTES.min(rows.saturating_mul(dst_stride));
    let chunk_rows = (bound / src_stride).max(1).min(rows.max(1));
    let chunks = rows.div_ceil(chunk_rows);
    let chunk_bytes = chunk_rows.saturating_mul(src_stride);
    let mut staging = Vec::new();
    staging
        .try_reserve_exact(chunk_bytes)
        .map_err(|err| ImageError::codec(HOLD_IN_MEMORY, err))?;
    staging.resize(chunk_bytes, 0);
    let growing = dst_stride >= src_stride;
    for step in 0..chunks {
        let chunk = if growing { chunks - 1 - step } else { step };
        let first = chunk * chunk_rows;
        let last = (first + chunk_rows).min(rows);
        let src = &buf[first * src_stride..last * src_stride];
        let staged = &mut staging[..src.len()];
        staged.copy_from_slice(src);
        let dst = &mut buf[first * dst_stride..last * dst_stride];
        if !splits_on_pool(dst.len(), SPLIT_STAGED_BYTES) {
            let mut state = init();
            for (idx, row) in dst.chunks_mut(dst_stride).enumerate() {
                work(&mut state, &staged[idx * src_stride..][..src_stride], row)?;
            }
            continue;
        }
        let per_task = rows_per_pool_task(dst, dst_stride);
        dst.par_chunks_mut(dst_stride)
            .with_min_len(per_task)
            .enumerate()
            .try_for_each_init(&init, |state, (idx, row)| {
                work(state, &staged[idx * src_stride..][..src_stride], row)
            })?;
    }
    Ok(())
}

/// Whether any row holds a value the predicate accepts, over rows the caller may split.
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

    const ROW: usize = 1 << 17;
    const JOB: usize = SPLIT_BYTES;

    fn expected(idx: usize, offset: usize, sample: u8) -> u8 {
        ((usize::from(sample) + idx + offset) % 251 + 1) as u8
    }

    fn pool(threads: usize) -> rayon::ThreadPool {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
    }

    fn assert_runs_agree(job: impl Fn(usize) -> Vec<u8>, expected: impl Fn(usize) -> u8) {
        let serial = job(1);
        let parallel = job(4);
        let foreign = serial
            .iter()
            .enumerate()
            .find(|(offset, b)| **b != expected(*offset))
            .map(|(offset, _)| offset);
        assert!(foreign.is_none(), "byte {foreign:?} is not its own row's");
        assert_eq!(serial, parallel);
    }

    #[test]
    fn parallel_row_split() {
        let run = |threads| {
            let mut buf = vec![0u8; JOB];
            pool(threads).install(|| {
                for_each_row(
                    &mut buf,
                    ROW,
                    || (),
                    |(), idx, row| {
                        for (offset, b) in row.iter_mut().enumerate() {
                            *b = expected(idx, offset, 0);
                        }
                        Ok(())
                    },
                )
                .unwrap()
            });
            buf
        };
        assert_runs_agree(run, |offset| expected(offset / ROW, offset % ROW, 0));
    }

    #[test]
    fn task_row_count() {
        pool(4).install(|| {
            let bytes = [0u8; 128 * 4];
            let samples = [[0i32; 4]; 128 * 4];
            assert_eq!(rows_per_pool_task(&bytes, 4), 8);
            assert_eq!(rows_per_pool_task(&samples, 4), 8);
        });
    }

    #[test]
    fn typed_row_split() {
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

    fn rewritten_row(src: &[u8], row: &mut [u8]) {
        for (offset, b) in row.iter_mut().enumerate() {
            *b = src[offset % src.len()].wrapping_add(offset as u8);
        }
    }

    #[test]
    fn staged_rewrite_matches_ref() {
        for (src_stride, dst_stride) in [(3072, 4096), (8192, 4096)] {
            let rows = if src_stride < dst_stride { 3000 } else { 1500 };
            let src: Vec<u8> = (0..rows * src_stride).map(|i| (i % 251) as u8).collect();
            let mut expected = vec![0u8; rows * dst_stride];
            for (row, out) in expected.chunks_mut(dst_stride).enumerate() {
                let input = &src[row * src_stride..][..src_stride];
                rewritten_row(input, out);
            }
            let run = |threads| {
                let mut buf = src.clone();
                if dst_stride > src_stride {
                    buf.resize(rows * dst_stride, 0);
                }
                pool(threads).install(|| {
                    rewrite_rows_in_place(
                        &mut buf,
                        rows,
                        src_stride,
                        dst_stride,
                        || (),
                        |(), input, out| {
                            rewritten_row(input, out);
                            Ok(())
                        },
                    )
                    .unwrap()
                });
                buf.truncate(rows * dst_stride);
                buf
            };
            assert_runs_agree(run, |offset| expected[offset]);
        }
    }
}
