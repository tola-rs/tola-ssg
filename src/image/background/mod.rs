//! Remove background from images.
//!
//! Uses LAB color space for perceptually accurate color matching,
//! with flood fill from edges to remove connected background regions.

use std::collections::VecDeque;
use std::fs;
use std::path::Path;

use anyhow::Result;
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba, RgbaImage};
use lab::{Lab, rgb_bytes_to_labs};

/// Default threshold for color distance in LAB space (ΔE)
/// Values < 10 are generally considered "same color" to human eyes
const DEFAULT_THRESHOLD: f32 = 10.0;

/// Extended threshold for anti-aliased edge pixels
/// Pixels within this range get gradual transparency
const EDGE_THRESHOLD: f32 = 25.0;

/// Remove background from an image file
///
/// Reads the image, auto-detects background color from edges,
/// removes it using flood fill, and writes the result as PNG
pub fn remove_background(input: &Path, output: &Path) -> Result<()> {
    let img = image::open(input)?;
    let processed = process_image(img);

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    processed.save_with_format(output, ImageFormat::Png)?;
    Ok(())
}

/// Process image to remove background
fn process_image(img: DynamicImage) -> DynamicImage {
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();

    // Detect background color from edges
    let bg_lab = detect_background_color(&rgba);

    // Pre-convert entire image to LAB (SIMD accelerated)
    let labs = preconvert_to_lab(&rgba);

    // Create output buffer
    let mut output: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(width, height);

    // Copy original pixels
    for (x, y, pixel) in rgba.enumerate_pixels() {
        output.put_pixel(x, y, *pixel);
    }

    // Track visited pixels
    let mut visited = vec![false; (width * height) as usize];

    // Flood fill from all edges
    let mut queue = VecDeque::new();

    // Add edge pixels to queue
    for x in 0..width {
        queue.push_back((x, 0));
        queue.push_back((x, height - 1));
    }
    for y in 1..height - 1 {
        queue.push_back((0, y));
        queue.push_back((width - 1, y));
    }

    // BFS flood fill
    while let Some((x, y)) = queue.pop_front() {
        let idx = (y * width + x) as usize;

        if visited[idx] {
            continue;
        }
        visited[idx] = true;

        let pixel = rgba.get_pixel(x, y);

        // Skip already transparent pixels
        if pixel[3] < 128 {
            continue;
        }

        // Check color distance to background
        let distance = color_distance(&labs[idx], &bg_lab);

        if distance < DEFAULT_THRESHOLD {
            // Core background: fully transparent
            output.put_pixel(x, y, Rgba([pixel[0], pixel[1], pixel[2], 0]));

            // Add neighbors to queue
            if x > 0 {
                queue.push_back((x - 1, y));
            }
            if x < width - 1 {
                queue.push_back((x + 1, y));
            }
            if y > 0 {
                queue.push_back((x, y - 1));
            }
            if y < height - 1 {
                queue.push_back((x, y + 1));
            }
        } else if distance < EDGE_THRESHOLD {
            // Edge pixel: gradual transparency based on distance
            let alpha_ratio = (distance - DEFAULT_THRESHOLD) / (EDGE_THRESHOLD - DEFAULT_THRESHOLD);
            let new_alpha = (pixel[3] as f32 * alpha_ratio) as u8;
            output.put_pixel(x, y, Rgba([pixel[0], pixel[1], pixel[2], new_alpha]));

            // Still propagate to neighbors for edge detection
            if x > 0 {
                queue.push_back((x - 1, y));
            }
            if x < width - 1 {
                queue.push_back((x + 1, y));
            }
            if y > 0 {
                queue.push_back((x, y - 1));
            }
            if y < height - 1 {
                queue.push_back((x, y + 1));
            }
        }
    }

    DynamicImage::ImageRgba8(output)
}

/// Detect background color by sampling edge pixels
///
/// Samples pixels from the four corners and finds the most common color
fn detect_background_color(img: &RgbaImage) -> Lab {
    let (width, height) = img.dimensions();
    let mut samples: Vec<[u8; 3]> = Vec::with_capacity(100);

    // Sample from corners (5x5 area each)
    let corners = [
        (0, 0),                                              // top-left
        (width.saturating_sub(5), 0),                        // top-right
        (0, height.saturating_sub(5)),                       // bottom-left
        (width.saturating_sub(5), height.saturating_sub(5)), // bottom-right
    ];

    for (cx, cy) in corners {
        for dy in 0..5 {
            for dx in 0..5 {
                let x = (cx + dx).min(width - 1);
                let y = (cy + dy).min(height - 1);
                let pixel = img.get_pixel(x, y);
                // Skip transparent pixels
                if pixel[3] >= 128 {
                    samples.push([pixel[0], pixel[1], pixel[2]]);
                }
            }
        }
    }

    // If no valid samples, default to white
    if samples.is_empty() {
        return Lab {
            l: 100.0,
            a: 0.0,
            b: 0.0,
        };
    }

    // Find most common color (simple: use average)
    let sum: (u32, u32, u32) = samples.iter().fold((0, 0, 0), |acc, rgb| {
        (
            acc.0 + rgb[0] as u32,
            acc.1 + rgb[1] as u32,
            acc.2 + rgb[2] as u32,
        )
    });
    let n = samples.len() as u32;
    let avg_rgb = [(sum.0 / n) as u8, (sum.1 / n) as u8, (sum.2 / n) as u8];

    Lab::from_rgb(&avg_rgb)
}

/// Pre-convert entire image to LAB color space
///
/// Uses SIMD-accelerated batch conversion for performance
fn preconvert_to_lab(img: &RgbaImage) -> Vec<Lab> {
    let rgb_bytes: Vec<u8> = img.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();

    rgb_bytes_to_labs(&rgb_bytes)
}

/// Calculate color distance in LAB space (ΔE)
#[inline]
fn color_distance(c1: &Lab, c2: &Lab) -> f32 {
    let dl = c1.l - c2.l;
    let da = c1.a - c2.a;
    let db = c1.b - c2.b;
    (dl * dl + da * da + db * db).sqrt()
}
