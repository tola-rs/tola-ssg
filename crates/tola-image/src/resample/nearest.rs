//! Point sampling: one source sample per output pixel, taken at the source area's center.

use anyhow::{Context, Result};

use crate::cancellation::Cancellation;
use crate::rgba_buffer;
use crate::rows::for_each_row;

use super::plane::SourcePlane;

/// The source index whose sample one output index reads.
fn sampled_center(index: u32, input: u32, output: u32) -> u32 {
    let center = (2 * u128::from(index) + 1) * u128::from(input) / (2 * u128::from(output));
    center.min(u128::from(input) - 1) as u32
}

pub(super) fn nearest_pixels(
    plane: &SourcePlane<'_>,
    output_width: u32,
    output_height: u32,
    cancellation: &dyn Cancellation,
) -> Result<Vec<u8>> {
    let mut columns = Vec::new();
    columns
        .try_reserve_exact(output_width as usize)
        .context("the image is too large to resize; use a smaller image")?;
    for x in 0..output_width {
        columns.push(sampled_center(x, plane.width(), output_width) as usize);
    }
    let mut output = rgba_buffer(output_width, output_height)?;
    for_each_row(
        &mut output,
        output_width as usize * 4,
        || (),
        |(), y, row| {
            cancellation.ensure_active()?;
            let source = plane.row(sampled_center(y as u32, plane.height(), output_height));
            for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let offset = columns[x] * 4;
                pixel.copy_from_slice(&source[offset..offset + 4]);
            }
            Ok(())
        },
    )?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resample::plane::AlphaRepresentation;
    use crate::resample::test_support::{active, plane};

    #[test]
    fn nearest_reads_the_nearest_sample() {
        assert_eq!(sampled_center(0, 2, 4), 0);
        assert_eq!(sampled_center(1, 2, 4), 0);
        assert_eq!(sampled_center(2, 2, 4), 1);
        assert_eq!(sampled_center(3, 2, 4), 1);
        assert_eq!(sampled_center(0, 4, 2), 1);
        assert_eq!(sampled_center(1, 4, 2), 3);
        assert_eq!(sampled_center(0, 4, 4), 0);
        assert_eq!(sampled_center(3, 4, 4), 3);

        let source = [1u8, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255];
        let source = plane(&source, 4, 1, AlphaRepresentation::Opaque);
        let sampled = nearest_pixels(&source, 2, 1, &active).unwrap();
        assert_eq!(sampled, [4, 5, 6, 255, 10, 11, 12, 255]);
    }
}
