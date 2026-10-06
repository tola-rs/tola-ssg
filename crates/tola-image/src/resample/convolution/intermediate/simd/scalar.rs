//! Other targets: no vector kernel, so every tap falls to the shared scalar remainder.

use fearless_simd::Level;

use super::super::ACCUMULATE_PIXELS;

/// No vector kernel covers any tap on this target.
pub(in crate::resample::convolution::intermediate) fn accumulate_pairs(
    _window: &[u8],
    _weights: &[i16],
    _totals: &mut [i32; 4],
    _level: Level,
) -> usize {
    0
}

/// No vector kernel covers any tap on this target.
pub(in crate::resample::convolution::intermediate) fn accumulate_pairs_four(
    _windows: [&[u8]; 4],
    _weights: &[i16],
    _totals: &mut [[i32; 4]; 4],
    _level: Level,
) -> usize {
    0
}

/// No vector kernel accumulates a tile on this target.
pub(in crate::resample::convolution::intermediate) fn accumulate_tile(
    _tile: &mut [[i32; 4]; ACCUMULATE_PIXELS],
    _window: &[u8],
    _weight: i16,
    _level: Level,
) -> bool {
    false
}
