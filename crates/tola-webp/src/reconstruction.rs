//! Reconstructed samples retained for the current and previous macroblock rows.

pub(crate) struct Reconstruction {
    pub(crate) y: Plane,
    pub(crate) u: Plane,
    pub(crate) v: Plane,
}

impl Reconstruction {
    pub(crate) fn new(y_stride: usize, chroma_stride: usize, macroblock_rows: usize) -> Self {
        Self {
            y: Plane::luma(y_stride, macroblock_rows),
            u: Plane::chroma(chroma_stride, macroblock_rows),
            v: Plane::chroma(chroma_stride, macroblock_rows),
        }
    }

    /// A vertical strip's rings. Luma keeps one column on the left for the neighbouring
    /// strip's edge and four on the right for the above-right samples; chroma needs only
    /// the left column. Coordinates stay global, so the strip only changes storage.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn strip(
        y_stride: usize,
        chroma_stride: usize,
        macroblock_rows: usize,
        x_start: usize,
        x_end: usize,
    ) -> Self {
        Self {
            y: Plane::luma_strip(y_stride, macroblock_rows, x_start, x_end),
            u: Plane::chroma_strip(chroma_stride, macroblock_rows, x_start, x_end),
            v: Plane::chroma_strip(chroma_stride, macroblock_rows, x_start, x_end),
        }
    }
}

/// Global rows remain readable until the following macroblock row finishes.
pub(crate) struct Plane {
    samples: Vec<u8>,
    /// Local row width, including the strip halos.
    stride: usize,
    /// Full frame row width; only right-edge padding samples are clamped to it.
    frame_stride: usize,
    /// Global column represented by local column `left_halo`.
    x_base: usize,
    left_halo: usize,
    row_mask: usize,
}

impl Plane {
    pub(crate) fn luma(stride: usize, macroblock_rows: usize) -> Self {
        Self::new(stride, 16 * macroblock_rows.min(2), stride, 0, 0)
    }

    pub(crate) fn chroma(stride: usize, macroblock_rows: usize) -> Self {
        Self::new(stride, 8 * macroblock_rows.min(2), stride, 0, 0)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn luma_strip(
        frame_stride: usize,
        macroblock_rows: usize,
        x_start: usize,
        x_end: usize,
    ) -> Self {
        let left_halo = usize::from(x_start > 0);
        Self::new(
            (x_end - x_start) * 16 + left_halo + 4,
            16 * macroblock_rows.min(2),
            frame_stride,
            x_start * 16,
            left_halo,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn chroma_strip(
        frame_stride: usize,
        macroblock_rows: usize,
        x_start: usize,
        x_end: usize,
    ) -> Self {
        let left_halo = usize::from(x_start > 0);
        Self::new(
            (x_end - x_start) * 8 + left_halo,
            8 * macroblock_rows.min(2),
            frame_stride,
            x_start * 8,
            left_halo,
        )
    }

    fn new(
        stride: usize,
        rows: usize,
        frame_stride: usize,
        x_base: usize,
        left_halo: usize,
    ) -> Self {
        // Masking replaces a modulo; the retained row counts are 8, 16, or 32.
        debug_assert!(rows.is_power_of_two());
        debug_assert!(left_halo <= 1);
        Self {
            samples: vec![128; stride * rows],
            stride,
            frame_stride,
            x_base,
            left_halo,
            row_mask: rows - 1,
        }
    }

    fn local(&self, x: usize) -> usize {
        let local = x + self.left_halo - self.x_base;
        debug_assert!(local < self.stride);
        local
    }

    #[cfg(test)]
    pub(crate) fn stride(&self) -> usize {
        self.stride
    }

    pub(crate) fn frame_stride(&self) -> usize {
        self.frame_stride
    }

    pub(crate) fn row(&self, y: usize) -> &[u8] {
        let start = (y & self.row_mask) * self.stride;
        &self.samples[start..start + self.stride]
    }

    pub(crate) fn row_mut(&mut self, y: usize) -> &mut [u8] {
        let start = (y & self.row_mask) * self.stride;
        &mut self.samples[start..start + self.stride]
    }

    pub(crate) fn slice(&self, x: usize, y: usize, length: usize) -> &[u8] {
        let local = self.local(x);
        &self.row(y)[local..local + length]
    }

    pub(crate) fn sample(&self, x: usize, y: usize) -> u8 {
        self.row(y)[self.local(x)]
    }

    pub(crate) fn write(&mut self, y: usize, x: usize, values: &[u8]) {
        let local = self.local(x);
        self.row_mut(y)[local..local + values.len()].copy_from_slice(values);
    }
}
