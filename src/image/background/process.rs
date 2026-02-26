use std::fs;
use std::path::Path;

use anyhow::Result;
use image::{DynamicImage, ImageFormat};

use crate::image::background::color::preconvert_to_lab;
use crate::image::background::detect::detect_background_color;
use crate::image::background::floodfill::apply_edge_connected_mask;
use crate::image::background::mask::build_background_mask;

/// Default threshold for color distance in LAB space (ΔE).
const DEFAULT_THRESHOLD: f32 = 10.0;
/// Extended threshold for anti-aliased edge pixels.
const EDGE_THRESHOLD: f32 = 25.0;
/// Pixels with alpha below this value are treated as transparent in mask classification.
///
/// Use 1 so semi-transparent background can still be removed if it is edge-connected.
const MIN_PROCESS_ALPHA: u8 = 1;

/// Remove background from an image file and write PNG output.
pub fn remove_background(input: &Path, output: &Path) -> Result<()> {
    let img = image::open(input)?;
    let processed = process_image(img);

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    processed.save_with_format(output, ImageFormat::Png)?;
    Ok(())
}

/// Process image to remove edge-connected background.
fn process_image(img: DynamicImage) -> DynamicImage {
    let mut output = img.to_rgba8();
    let (width, height) = output.dimensions();
    if width == 0 || height == 0 {
        return DynamicImage::ImageRgba8(output);
    }

    let bg_lab = detect_background_color(&output);
    let labs = preconvert_to_lab(&output);
    let mask = build_background_mask(
        &output,
        &labs,
        &bg_lab,
        DEFAULT_THRESHOLD,
        EDGE_THRESHOLD,
        MIN_PROCESS_ALPHA,
    );
    apply_edge_connected_mask(&mut output, &mask);

    DynamicImage::ImageRgba8(output)
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::process_image;

    #[test]
    fn removes_single_pixel_background() {
        let mut img = RgbaImage::new(1, 1);
        img.put_pixel(0, 0, Rgba([255, 255, 255, 255]));

        let out = process_image(img.into()).to_rgba8();
        assert_eq!(out.get_pixel(0, 0)[3], 0);
    }

    #[test]
    fn keeps_transparent_pixel_transparent() {
        let mut img = RgbaImage::new(1, 1);
        img.put_pixel(0, 0, Rgba([12, 34, 56, 0]));

        let out = process_image(img.into()).to_rgba8();
        assert_eq!(out.get_pixel(0, 0)[3], 0);
    }

    #[test]
    fn preserves_enclosed_background_island() {
        let mut img = RgbaImage::from_pixel(7, 7, Rgba([255, 255, 255, 255]));
        let fg = Rgba([0, 0, 0, 255]);

        for x in 1..=5 {
            img.put_pixel(x, 1, fg);
            img.put_pixel(x, 5, fg);
        }
        for y in 1..=5 {
            img.put_pixel(1, y, fg);
            img.put_pixel(5, y, fg);
        }

        let out = process_image(img.into()).to_rgba8();

        // Outer white background is edge-connected and should be removed.
        assert_eq!(out.get_pixel(0, 0)[3], 0);
        // Enclosed white island is not edge-connected and should be preserved.
        assert_eq!(out.get_pixel(3, 3)[3], 255);
        // Foreground ring should be preserved.
        assert_eq!(out.get_pixel(1, 1)[3], 255);
    }

    #[test]
    fn handles_single_row_image() {
        let mut img = RgbaImage::new(3, 1);
        img.put_pixel(0, 0, Rgba([255, 255, 255, 255]));
        img.put_pixel(1, 0, Rgba([255, 255, 255, 255]));
        img.put_pixel(2, 0, Rgba([255, 255, 255, 255]));

        let out = process_image(img.into()).to_rgba8();
        assert_eq!(out.get_pixel(0, 0)[3], 0);
        assert_eq!(out.get_pixel(1, 0)[3], 0);
        assert_eq!(out.get_pixel(2, 0)[3], 0);
    }

    #[test]
    fn handles_single_column_image() {
        let mut img = RgbaImage::new(1, 3);
        img.put_pixel(0, 0, Rgba([255, 255, 255, 255]));
        img.put_pixel(0, 1, Rgba([255, 255, 255, 255]));
        img.put_pixel(0, 2, Rgba([255, 255, 255, 255]));

        let out = process_image(img.into()).to_rgba8();
        assert_eq!(out.get_pixel(0, 0)[3], 0);
        assert_eq!(out.get_pixel(0, 1)[3], 0);
        assert_eq!(out.get_pixel(0, 2)[3], 0);
    }
}
