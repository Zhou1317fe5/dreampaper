//! Padding geometry shared by the sidecar and the Tauri host.
//!
//! The host pads a region crop with its background colour before sending it
//! (D1); the sidecar analyses exactly what it receives and returns points in
//! the padded pixel space, so both sides must agree on this one function.

use image::{imageops, Rgb, RgbImage};

/// Minimum margin callers must add around a region before sending it. The
/// sidecar itself never pads: the padded image it receives is what it analyses.
pub const DET_MIN_PAD: u32 = 24;

/// Result of [`pad_to_multiple`]: the padded image plus the offset of the
/// original pixels inside it, so callers can map coordinates back.
#[derive(Debug, Clone)]
pub struct Padded {
    pub image: RgbImage,
    pub offset_x: u32,
    pub offset_y: u32,
}

/// Pads `image` with `color` so both sides become multiples of 32 while
/// keeping at least `min_pad` pixels of margin on every edge. Inputs that stay
/// within the detection size limit then run at native resolution without any
/// resampling, which keeps returned polygons on exact source pixels.
pub fn pad_to_multiple(image: &RgbImage, min_pad: u32, color: Rgb<u8>) -> Padded {
    let width = image.width();
    let height = image.height();
    let padded_width = next_multiple_of_32(width + 2 * min_pad);
    let padded_height = next_multiple_of_32(height + 2 * min_pad);
    let offset_x = (padded_width - width) / 2;
    let offset_y = (padded_height - height) / 2;
    let mut padded = RgbImage::from_pixel(padded_width, padded_height, color);
    imageops::replace(&mut padded, image, i64::from(offset_x), i64::from(offset_y));
    Padded {
        image: padded,
        offset_x,
        offset_y,
    }
}

fn next_multiple_of_32(value: u32) -> u32 {
    value.div_ceil(32).max(1) * 32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_aligns_to_32_and_keeps_minimum_margin() {
        let image = RgbImage::from_pixel(1200, 800, Rgb([1, 2, 3]));
        let padded = pad_to_multiple(&image, 24, Rgb([9, 9, 9]));
        assert_eq!(padded.image.dimensions(), (1248, 864));
        assert_eq!((padded.offset_x, padded.offset_y), (24, 32));
        assert_eq!(padded.image.get_pixel(0, 0), &Rgb([9, 9, 9]));
        assert_eq!(padded.image.get_pixel(24, 32), &Rgb([1, 2, 3]));
        assert_eq!(padded.image.get_pixel(24 + 1199, 32 + 799), &Rgb([1, 2, 3]));
        assert_eq!(padded.image.get_pixel(1247, 863), &Rgb([9, 9, 9]));
    }

    #[test]
    fn tiny_regions_still_get_a_full_margin() {
        let image = RgbImage::from_pixel(3, 2, Rgb([1, 2, 3]));
        let padded = pad_to_multiple(&image, 24, Rgb([0, 0, 0]));
        assert_eq!(padded.image.dimensions(), (64, 64));
        assert!(padded.offset_x >= 24 && padded.offset_y >= 24);
    }
}
