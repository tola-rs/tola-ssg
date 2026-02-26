use image::RgbaImage;
use lab::Lab;
use rayon::prelude::*;

use crate::image::background::color::color_distance_sq;

pub(super) const CLASS_NONE: u8 = 0;
pub(super) const CLASS_EDGE: u8 = 1;
pub(super) const CLASS_CORE: u8 = 2;

pub(super) struct BackgroundMask {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) classes: Vec<u8>,
    pub(super) edge_alpha: Vec<u8>,
}

const PARALLEL_PIXEL_THRESHOLD: usize = 32 * 1024;

struct PixelClassifier<'a> {
    bg_lab: &'a Lab,
    core_threshold: f32,
    core_threshold_sq: f32,
    edge_threshold_sq: f32,
    min_opaque_alpha: u8,
    threshold_span: f32,
}

impl PixelClassifier<'_> {
    #[inline]
    fn classify(&self, class_out: &mut u8, edge_alpha_out: &mut u8, pixel: &[u8], lab: &Lab) {
        let alpha = pixel[3];
        if alpha < self.min_opaque_alpha {
            return;
        }

        let distance_sq = color_distance_sq(lab, self.bg_lab);
        if distance_sq <= self.core_threshold_sq {
            *class_out = CLASS_CORE;
        } else if distance_sq <= self.edge_threshold_sq {
            *class_out = CLASS_EDGE;
            let distance = distance_sq.sqrt();
            let alpha_ratio =
                ((distance - self.core_threshold) / self.threshold_span).clamp(0.0, 1.0);
            *edge_alpha_out = (alpha as f32 * alpha_ratio).round() as u8;
        }
    }
}

/// Build a compact per-pixel mask for background flood fill.
///
/// `classes` marks whether a pixel is non-background / edge / core background.
/// `edge_alpha` stores the final alpha for edge pixels (others are zero).
pub(super) fn build_background_mask(
    img: &RgbaImage,
    labs: &[Lab],
    bg_lab: &Lab,
    core_threshold: f32,
    edge_threshold: f32,
    min_opaque_alpha: u8,
) -> BackgroundMask {
    let (width, height) = img.dimensions();
    let len = width as usize * height as usize;
    assert_eq!(
        labs.len(),
        len,
        "LAB buffer length mismatch: labs={} pixels={}",
        labs.len(),
        len
    );

    let mut classes = vec![CLASS_NONE; len];
    let mut edge_alpha = vec![0_u8; len];

    let core_threshold_sq = core_threshold * core_threshold;
    let edge_threshold_sq = edge_threshold * edge_threshold;
    let threshold_span = (edge_threshold - core_threshold).max(f32::EPSILON);
    let classifier = PixelClassifier {
        bg_lab,
        core_threshold,
        core_threshold_sq,
        edge_threshold_sq,
        min_opaque_alpha,
        threshold_span,
    };
    let raw = img.as_raw();

    if len >= PARALLEL_PIXEL_THRESHOLD {
        classes
            .par_iter_mut()
            .zip(edge_alpha.par_iter_mut())
            .zip(labs.par_iter())
            .zip(raw.par_chunks_exact(4))
            .for_each(|(((class, edge_alpha_out), lab), pixel)| {
                classifier.classify(class, edge_alpha_out, pixel, lab);
            });
    } else {
        for (((class, edge_alpha_out), lab), pixel) in classes
            .iter_mut()
            .zip(edge_alpha.iter_mut())
            .zip(labs.iter())
            .zip(raw.chunks_exact(4))
        {
            classifier.classify(class, edge_alpha_out, pixel, lab);
        }
    }

    BackgroundMask {
        width,
        height,
        classes,
        edge_alpha,
    }
}
