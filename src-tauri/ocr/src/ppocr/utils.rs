use image::{imageops, Rgb, RgbImage};
use imageproc::geometric_transformations::{Interpolation, Projection};
use ndarray::Array4;

use super::result::{Point, TextBox};

// Padding lives in the always-compiled `pad` module because the Tauri host
// pads before sending; it is re-exported here for the engine-side callers.
pub use crate::pad::{pad_to_multiple, Padded};

/// Converts RGB8 to a normalised NCHW tensor whose planes are in **BGR** order.
/// PaddleOCR and RapidOCR feed cv2 BGR arrays to det/cls/rec (RapidOCR
/// `LoadImage` converts every input with `COLOR_RGB2BGR`), and `mean`/`norm`
/// are indexed in that channel order. Arithmetic is `value * norm - mean * norm`.
pub fn normalize_bgr_nchw(image: &RgbImage, mean: &[f32; 3], norm: &[f32; 3]) -> Array4<f32> {
    let cols = image.width() as usize;
    let rows = image.height() as usize;
    let plane = rows * cols;
    let offset = [mean[0] * norm[0], mean[1] * norm[1], mean[2] * norm[2]];
    let mut data = vec![0.0_f32; 3 * plane];
    let (blue, rest) = data.split_at_mut(plane);
    let (green, red) = rest.split_at_mut(plane);
    for (index, pixel) in image.as_raw().chunks_exact(3).enumerate() {
        blue[index] = pixel[2] as f32 * norm[0] - offset[0];
        green[index] = pixel[1] as f32 * norm[1] - offset[1];
        red[index] = pixel[0] as f32 * norm[2] - offset[2];
    }
    Array4::from_shape_vec((1, 3, rows, cols), data).expect("shape matches buffer")
}

/// Perspective-crops every detected quadrilateral into an upright line image.
/// Degenerate boxes yield `None` instead of panicking like the origin did;
/// the caller must drop those boxes so lines stay aligned with boxes.
pub fn crop_lines(image: &RgbImage, boxes: &[TextBox]) -> Vec<Option<RgbImage>> {
    boxes
        .iter()
        .map(|text_box| rotate_crop(image, &text_box.points))
        .collect()
}

pub fn rotate_crop(image: &RgbImage, box_points: &[Point]) -> Option<RgbImage> {
    if box_points.len() != 4 {
        return None;
    }
    let mut points = box_points.to_vec();
    let (min_x, min_y, max_x, max_y) = points.iter().fold(
        (u32::MAX, u32::MAX, 0_u32, 0_u32),
        |(min_x, min_y, max_x, max_y), point| {
            (
                min_x.min(point.x),
                min_y.min(point.y),
                max_x.max(point.x),
                max_y.max(point.y),
            )
        },
    );
    if max_x <= min_x || max_y <= min_y || max_x > image.width() || max_y > image.height() {
        return None;
    }

    let crop = imageops::crop_imm(image, min_x, min_y, max_x - min_x, max_y - min_y).to_image();
    for point in &mut points {
        point.x -= min_x;
        point.y -= min_y;
    }

    let crop_width = ((points[0].x as i32 - points[1].x as i32).pow(2) as f32
        + (points[0].y as i32 - points[1].y as i32).pow(2) as f32)
        .sqrt() as u32;
    let crop_height = ((points[0].x as i32 - points[3].x as i32).pow(2) as f32
        + (points[0].y as i32 - points[3].y as i32).pow(2) as f32)
        .sqrt() as u32;
    if crop_width == 0 || crop_height == 0 {
        return None;
    }

    let source = [
        (points[0].x as f32, points[0].y as f32),
        (points[1].x as f32, points[1].y as f32),
        (points[2].x as f32, points[2].y as f32),
        (points[3].x as f32, points[3].y as f32),
    ];
    let target = [
        (0.0, 0.0),
        (crop_width as f32, 0.0),
        (crop_width as f32, crop_height as f32),
        (0.0, crop_height as f32),
    ];
    let projection = Projection::from_control_points(source, target)?;

    let mut part = RgbImage::new(crop_width, crop_height);
    imageproc::geometric_transformations::warp_into(
        &crop,
        &projection,
        Interpolation::Bilinear,
        Rgb([255, 255, 255]),
        &mut part,
    );

    // Tall lines are vertical text: rotate so the recogniser sees them horizontally.
    if part.height() >= part.width() * 3 / 2 {
        let mut rotated = RgbImage::new(part.height(), part.width());
        for (x, y, pixel) in part.enumerate_pixels() {
            rotated.put_pixel(y, part.width() - 1 - x, *pixel);
        }
        Some(rotated)
    } else {
        Some(part)
    }
}

pub fn rotate_180(image: &mut RgbImage) {
    imageops::rotate180_in_place(image);
}

pub fn mean_with_mask(
    values: &image::ImageBuffer<image::Luma<f32>, Vec<f32>>,
    mask: &image::GrayImage,
) -> f32 {
    debug_assert_eq!(values.dimensions(), mask.dimensions());
    let mut sum = 0.0_f32;
    let mut count = 0_u32;
    for (value, mask) in values.as_raw().iter().zip(mask.as_raw()) {
        if *mask > 0 {
            sum += *value;
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        sum / count as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalisation_matches_reference_formula_in_bgr_order() {
        let mut image = RgbImage::new(2, 1);
        image.put_pixel(0, 0, Rgb([0, 128, 255]));
        image.put_pixel(1, 0, Rgb([10, 20, 30]));
        let mean = [0.485 * 255.0, 0.456 * 255.0, 0.406 * 255.0];
        let norm = [
            1.0 / 0.229 / 255.0,
            1.0 / 0.224 / 255.0,
            1.0 / 0.225 / 255.0,
        ];
        let tensor = normalize_bgr_nchw(&image, &mean, &norm);
        assert_eq!(tensor.shape(), &[1, 3, 1, 2]);
        // Plane 0 holds blue, plane 2 holds red.
        for (channel, values) in [[255_u8, 30], [128, 20], [0, 10]].iter().enumerate() {
            for (column, value) in values.iter().enumerate() {
                let expected = *value as f32 * norm[channel] - mean[channel] * norm[channel];
                assert_eq!(tensor[[0, channel, 0, column]], expected);
            }
        }
    }

    #[test]
    fn degenerate_boxes_are_skipped_instead_of_panicking() {
        let image = RgbImage::new(10, 10);
        let flat = [
            Point { x: 1, y: 5 },
            Point { x: 8, y: 5 },
            Point { x: 8, y: 5 },
            Point { x: 1, y: 5 },
        ];
        assert!(rotate_crop(&image, &flat).is_none());
        let outside = [
            Point { x: 1, y: 1 },
            Point { x: 20, y: 1 },
            Point { x: 20, y: 5 },
            Point { x: 1, y: 5 },
        ];
        assert!(rotate_crop(&image, &outside).is_none());
    }
}
