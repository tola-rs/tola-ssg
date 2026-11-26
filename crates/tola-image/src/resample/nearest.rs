//! Point sampling: one source sample per output pixel, taken at the source area's center.

use crate::RESIZE_TOO_LARGE;
use crate::cancellation::Cancellation;
use crate::rgba_buffer;
use crate::rows::for_each_row;
use crate::{ImageError, ImageResult};

use super::plane::SourcePlane;

/// The source index whose sample one output index reads.
fn sampled_center(idx: u32, src: u32, dst: u32) -> u32 {
    let center = (2 * u128::from(idx) + 1) * u128::from(src) / (2 * u128::from(dst));
    center.min(u128::from(src) - 1) as u32
}

pub(super) fn nearest_pixels(
    plane: &SourcePlane<'_>,
    out_w: u32,
    out_h: u32,
    cancel: &dyn Cancellation,
) -> ImageResult<Vec<u8>> {
    let mut cols = Vec::new();
    cols.try_reserve_exact(out_w as usize)
        .map_err(|err| ImageError::codec(RESIZE_TOO_LARGE, err))?;
    for x in 0..out_w {
        cols.push(sampled_center(x, plane.width(), out_w) as usize);
    }
    let mut out = rgba_buffer(out_w, out_h)?;
    for_each_row(
        &mut out,
        out_w as usize * 4,
        || (),
        |(), y, row| {
            cancel.ensure_active()?;
            let src = plane.row(sampled_center(y as u32, plane.height(), out_h));
            for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let off = cols[x] * 4;
                px.copy_from_slice(&src[off..off + 4]);
            }
            Ok(())
        },
    )?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NO_CANCELLATION;
    use crate::resample::plane::AlphaRepresentation;
    use crate::resample::tests::test_support::plane;

    #[test]
    fn nearest_center_sample() {
        assert_eq!(sampled_center(0, 2, 4), 0);
        assert_eq!(sampled_center(1, 2, 4), 0);
        assert_eq!(sampled_center(2, 2, 4), 1);
        assert_eq!(sampled_center(3, 2, 4), 1);
        assert_eq!(sampled_center(0, 4, 2), 1);
        assert_eq!(sampled_center(1, 4, 2), 3);
        assert_eq!(sampled_center(0, 4, 4), 0);
        assert_eq!(sampled_center(3, 4, 4), 3);

        let src = [1u8, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255];
        let src = plane(&src, 4, 1, AlphaRepresentation::Opaque);
        let sampled = nearest_pixels(&src, 2, 1, &NO_CANCELLATION).unwrap();
        assert_eq!(sampled, [4, 5, 6, 255, 10, 11, 12, 255]);
    }
}
