use image::RgbaImage;
use lab::Lab;

use crate::image::background::color::color_distance_sq;

const SAMPLE_WINDOW: u32 = 5;
const MIN_SAMPLE_ALPHA: u8 = 8;
const CLUSTER_THRESHOLD: f32 = 8.0;
const CLUSTER_THRESHOLD_SQ: f32 = CLUSTER_THRESHOLD * CLUSTER_THRESHOLD;

#[derive(Clone, Copy)]
struct LabCluster {
    sum_l: f32,
    sum_a: f32,
    sum_b: f32,
    weight: f32,
}

impl LabCluster {
    #[inline]
    fn from_lab(lab: Lab, weight: f32) -> Self {
        Self {
            sum_l: lab.l * weight,
            sum_a: lab.a * weight,
            sum_b: lab.b * weight,
            weight,
        }
    }

    #[inline]
    fn centroid(self) -> Lab {
        let n = self.weight.max(f32::EPSILON);
        Lab {
            l: self.sum_l / n,
            a: self.sum_a / n,
            b: self.sum_b / n,
        }
    }

    #[inline]
    fn add(&mut self, lab: Lab, weight: f32) {
        self.sum_l += lab.l * weight;
        self.sum_a += lab.a * weight;
        self.sum_b += lab.b * weight;
        self.weight += weight;
    }
}

/// Detect background color by sampling corner pixels and choosing the dominant LAB cluster.
pub(super) fn detect_background_color(img: &RgbaImage) -> Lab {
    let (width, height) = img.dimensions();
    if width == 0 || height == 0 {
        return white_lab();
    }

    let corners = [
        (0, 0),
        (width.saturating_sub(SAMPLE_WINDOW), 0),
        (0, height.saturating_sub(SAMPLE_WINDOW)),
        (
            width.saturating_sub(SAMPLE_WINDOW),
            height.saturating_sub(SAMPLE_WINDOW),
        ),
    ];

    let mut clusters: Vec<LabCluster> = Vec::with_capacity(8);
    for (cx, cy) in corners {
        for dy in 0..SAMPLE_WINDOW {
            for dx in 0..SAMPLE_WINDOW {
                let x = (cx + dx).min(width - 1);
                let y = (cy + dy).min(height - 1);
                let pixel = img.get_pixel(x, y);
                if pixel[3] < MIN_SAMPLE_ALPHA {
                    continue;
                }

                let lab = Lab::from_rgb(&[pixel[0], pixel[1], pixel[2]]);
                let weight = (pixel[3] as f32 / 255.0).max(0.1);
                add_to_cluster(&mut clusters, lab, weight);
            }
        }
    }

    clusters
        .into_iter()
        .max_by(|a, b| a.weight.total_cmp(&b.weight))
        .map(LabCluster::centroid)
        .unwrap_or_else(white_lab)
}

#[inline]
fn add_to_cluster(clusters: &mut Vec<LabCluster>, lab: Lab, weight: f32) {
    let mut best_idx: Option<usize> = None;
    let mut best_dist = f32::MAX;

    for (idx, cluster) in clusters.iter().enumerate() {
        let dist_sq = color_distance_sq(&cluster.centroid(), &lab);
        if dist_sq < best_dist {
            best_dist = dist_sq;
            best_idx = Some(idx);
        }
    }

    if let Some(idx) = best_idx
        && best_dist <= CLUSTER_THRESHOLD_SQ
    {
        clusters[idx].add(lab, weight);
    } else {
        clusters.push(LabCluster::from_lab(lab, weight));
    }
}

#[inline]
fn white_lab() -> Lab {
    Lab {
        l: 100.0,
        a: 0.0,
        b: 0.0,
    }
}
