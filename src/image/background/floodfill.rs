use std::collections::VecDeque;

use image::RgbaImage;

use crate::image::background::mask::{BackgroundMask, CLASS_CORE, CLASS_EDGE, CLASS_NONE};

/// Apply edge-connected background mask using scanline flood fill.
///
/// Compared to pixel-by-pixel BFS, scanline fill reduces queue operations
/// for large contiguous regions and improves cache locality.
pub(super) fn apply_edge_connected_mask(output: &mut RgbaImage, mask: &BackgroundMask) {
    let width = mask.width;
    let height = mask.height;
    if width == 0 || height == 0 {
        return;
    }

    debug_assert_eq!(output.width(), width);
    debug_assert_eq!(output.height(), height);

    let len = width as usize * height as usize;
    let mut state = vec![0_u8; len]; // 0=unseen, 1=enqueued, 2=done
    let mut queue = VecDeque::with_capacity((width as usize + height as usize) * 2);

    enqueue_seed_runs_on_row(&mut queue, &mut state, mask, width, 0);
    if height > 1 {
        enqueue_seed_runs_on_row(&mut queue, &mut state, mask, width, height - 1);
    }
    for y in 1..height.saturating_sub(1) {
        enqueue_seed(&mut queue, &mut state, mask, width, 0, y);
        if width > 1 {
            enqueue_seed(&mut queue, &mut state, mask, width, width - 1, y);
        }
    }

    while let Some((sx, y)) = queue.pop_front() {
        let sidx = pixel_index(width, sx, y);
        if state[sidx] == 2 || mask.classes[sidx] == CLASS_NONE {
            continue;
        }

        let mut left = sx;
        while left > 0 {
            let nx = left - 1;
            let nidx = pixel_index(width, nx, y);
            if state[nidx] == 2 || mask.classes[nidx] == CLASS_NONE {
                break;
            }
            left = nx;
        }

        let mut right = sx;
        while right + 1 < width {
            let nx = right + 1;
            let nidx = pixel_index(width, nx, y);
            if state[nidx] == 2 || mask.classes[nidx] == CLASS_NONE {
                break;
            }
            right = nx;
        }

        let mut x = left;
        loop {
            let idx = pixel_index(width, x, y);
            apply_alpha(output, mask, idx, x, y);
            state[idx] = 2;

            if x == right {
                break;
            }
            x += 1;
        }

        if y > 0 {
            enqueue_neighbor_runs(&mut queue, &mut state, mask, width, left, right, y - 1);
        }
        if y + 1 < height {
            enqueue_neighbor_runs(&mut queue, &mut state, mask, width, left, right, y + 1);
        }
    }
}

#[inline]
fn enqueue_seed_runs_on_row(
    queue: &mut VecDeque<(u32, u32)>,
    state: &mut [u8],
    mask: &BackgroundMask,
    width: u32,
    y: u32,
) {
    let mut x = 0_u32;
    while x < width {
        let idx = pixel_index(width, x, y);
        if state[idx] == 0 && mask.classes[idx] != CLASS_NONE {
            queue.push_back((x, y));
            state[idx] = 1;

            x += 1;
            while x < width {
                let run_idx = pixel_index(width, x, y);
                if state[run_idx] != 0 || mask.classes[run_idx] == CLASS_NONE {
                    break;
                }
                state[run_idx] = 1;
                x += 1;
            }
        } else {
            x += 1;
        }
    }
}

#[inline]
fn enqueue_neighbor_runs(
    queue: &mut VecDeque<(u32, u32)>,
    state: &mut [u8],
    mask: &BackgroundMask,
    width: u32,
    left: u32,
    right: u32,
    y: u32,
) {
    let mut x = left;
    while x <= right {
        let idx = pixel_index(width, x, y);
        if state[idx] == 0 && mask.classes[idx] != CLASS_NONE {
            queue.push_back((x, y));
            state[idx] = 1;

            x += 1;
            while x <= right {
                let run_idx = pixel_index(width, x, y);
                if state[run_idx] != 0 || mask.classes[run_idx] == CLASS_NONE {
                    break;
                }
                state[run_idx] = 1;
                x += 1;
            }
        } else {
            x += 1;
        }
    }
}

#[inline]
fn enqueue_seed(
    queue: &mut VecDeque<(u32, u32)>,
    state: &mut [u8],
    mask: &BackgroundMask,
    width: u32,
    x: u32,
    y: u32,
) {
    let idx = pixel_index(width, x, y);
    if state[idx] == 0 && mask.classes[idx] != CLASS_NONE {
        state[idx] = 1;
        queue.push_back((x, y));
    }
}

#[inline]
fn apply_alpha(output: &mut RgbaImage, mask: &BackgroundMask, idx: usize, x: u32, y: u32) {
    match mask.classes[idx] {
        CLASS_CORE => output.get_pixel_mut(x, y)[3] = 0,
        CLASS_EDGE => output.get_pixel_mut(x, y)[3] = mask.edge_alpha[idx],
        _ => {}
    }
}

#[inline]
fn pixel_index(width: u32, x: u32, y: u32) -> usize {
    y as usize * width as usize + x as usize
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use image::{Rgba, RgbaImage};

    use super::apply_edge_connected_mask;
    use crate::image::background::mask::{BackgroundMask, CLASS_CORE, CLASS_EDGE, CLASS_NONE};

    #[test]
    fn matches_reference_bfs_on_random_masks() {
        for seed in 0_u64..48 {
            let width = 31_u32;
            let height = 19_u32;
            let len = width as usize * height as usize;
            let mut rng = Lcg::new(seed.wrapping_mul(1_048_583).wrapping_add(97));

            let mut classes = vec![CLASS_NONE; len];
            let mut edge_alpha = vec![0_u8; len];

            for i in 0..len {
                let v = (rng.next_u32() % 100) as u8;
                classes[i] = if v < 40 {
                    CLASS_NONE
                } else if v < 75 {
                    CLASS_EDGE
                } else {
                    CLASS_CORE
                };
                edge_alpha[i] = (rng.next_u32() & 0xFF) as u8;
            }

            let mask = BackgroundMask {
                width,
                height,
                classes,
                edge_alpha,
            };

            let mut output_scanline = make_random_image(width, height, &mut rng);
            let mut output_bfs = output_scanline.clone();

            apply_edge_connected_mask(&mut output_scanline, &mask);
            apply_reference_bfs(&mut output_bfs, &mask);

            assert_eq!(output_scanline, output_bfs, "seed={seed}");
        }
    }

    fn apply_reference_bfs(output: &mut RgbaImage, mask: &BackgroundMask) {
        let width = mask.width;
        let height = mask.height;
        let len = width as usize * height as usize;
        let mut visited = vec![false; len];
        let mut q = VecDeque::new();

        for x in 0..width {
            enqueue(&mut q, &mut visited, mask, width, x, 0);
            if height > 1 {
                enqueue(&mut q, &mut visited, mask, width, x, height - 1);
            }
        }
        for y in 1..height.saturating_sub(1) {
            enqueue(&mut q, &mut visited, mask, width, 0, y);
            if width > 1 {
                enqueue(&mut q, &mut visited, mask, width, width - 1, y);
            }
        }

        while let Some((x, y)) = q.pop_front() {
            let idx = idx(width, x, y);

            match mask.classes[idx] {
                CLASS_CORE => output.get_pixel_mut(x, y)[3] = 0,
                CLASS_EDGE => output.get_pixel_mut(x, y)[3] = mask.edge_alpha[idx],
                _ => continue,
            }

            if x > 0 {
                enqueue(&mut q, &mut visited, mask, width, x - 1, y);
            }
            if x + 1 < width {
                enqueue(&mut q, &mut visited, mask, width, x + 1, y);
            }
            if y > 0 {
                enqueue(&mut q, &mut visited, mask, width, x, y - 1);
            }
            if y + 1 < height {
                enqueue(&mut q, &mut visited, mask, width, x, y + 1);
            }
        }
    }

    fn enqueue(
        q: &mut VecDeque<(u32, u32)>,
        visited: &mut [bool],
        mask: &BackgroundMask,
        width: u32,
        x: u32,
        y: u32,
    ) {
        let i = idx(width, x, y);
        if !visited[i] && mask.classes[i] != CLASS_NONE {
            visited[i] = true;
            q.push_back((x, y));
        }
    }

    #[inline]
    fn idx(width: u32, x: u32, y: u32) -> usize {
        y as usize * width as usize + x as usize
    }

    fn make_random_image(width: u32, height: u32, rng: &mut Lcg) -> RgbaImage {
        let mut image = RgbaImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                image.put_pixel(
                    x,
                    y,
                    Rgba([
                        (rng.next_u32() & 0xFF) as u8,
                        (rng.next_u32() & 0xFF) as u8,
                        (rng.next_u32() & 0xFF) as u8,
                        (rng.next_u32() & 0xFF) as u8,
                    ]),
                );
            }
        }
        image
    }

    struct Lcg {
        state: u64,
    }

    impl Lcg {
        fn new(seed: u64) -> Self {
            Self { state: seed }
        }

        fn next_u32(&mut self) -> u32 {
            self.state = self
                .state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            (self.state >> 32) as u32
        }
    }
}
