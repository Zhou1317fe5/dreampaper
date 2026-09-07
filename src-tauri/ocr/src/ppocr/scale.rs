/// Detection input geometry. `dst_*` are multiples of 32 as the DB head requires.
#[derive(Debug, Clone, Copy)]
pub struct ScaleParam {
    pub src_width: u32,
    pub src_height: u32,
    pub dst_width: u32,
    pub dst_height: u32,
    pub scale_width: f32,
    pub scale_height: f32,
}

impl ScaleParam {
    /// Scales the longer side to `target_size` and rounds both sides to the
    /// nearest multiple of 32 (PaddleX/RapidOCR `resize_image_type0`:
    /// `max(int(round(x / 32) * 32), 32)`). When the image already satisfies
    /// both constraints the result is the identity and detection skips resizing.
    pub fn fit_longer_side(src: &image::RgbImage, target_size: u32) -> Self {
        let src_width = src.width();
        let src_height = src.height();

        let ratio: f32 = if src_width > src_height {
            target_size as f32 / src_width as f32
        } else {
            target_size as f32 / src_height as f32
        };

        let dst_width = round_to_32((src_width as f32 * ratio) as u32);
        let dst_height = round_to_32((src_height as f32 * ratio) as u32);

        Self {
            src_width,
            src_height,
            dst_width,
            dst_height,
            scale_width: dst_width as f32 / src_width as f32,
            scale_height: dst_height as f32 / src_height as f32,
        }
    }

    /// Engine rule: never upscale, downscale only when the longer side exceeds
    /// `max_side`. Inputs already aligned to 32 therefore pass through untouched.
    pub fn for_detection(src: &image::RgbImage, max_side: u32) -> Self {
        Self::fit_longer_side(src, src.width().max(src.height()).min(max_side))
    }

    pub fn is_identity(&self) -> bool {
        self.src_width == self.dst_width && self.src_height == self.dst_height
    }
}

/// `max(int(round(value / 32) * 32), 32)` with Python's round-half-even for
/// the exact `.5` case, which is the only case where rounding modes differ.
fn round_to_32(value: u32) -> u32 {
    let quotient = value / 32;
    let remainder = value % 32;
    let rounded = match remainder.cmp(&16) {
        std::cmp::Ordering::Less => quotient,
        std::cmp::Ordering::Greater => quotient + 1,
        std::cmp::Ordering::Equal => {
            if quotient.is_multiple_of(2) {
                quotient
            } else {
                quotient + 1
            }
        }
    };
    (rounded * 32).max(32)
}

#[cfg(test)]
mod tests {
    use super::{round_to_32, ScaleParam};

    #[test]
    fn aligned_input_is_identity() {
        let image = image::RgbImage::new(928, 640);
        let scale = ScaleParam::for_detection(&image, 960);
        assert!(scale.is_identity());
        assert_eq!((scale.dst_width, scale.dst_height), (928, 640));
        assert_eq!((scale.scale_width, scale.scale_height), (1.0, 1.0));
    }

    #[test]
    fn small_input_is_not_upscaled() {
        let image = image::RgbImage::new(352, 160);
        let scale = ScaleParam::for_detection(&image, 960);
        assert!(scale.is_identity());
    }

    #[test]
    fn padded_gate_sample_matches_reference_geometry() {
        // 1200×800 padded to 1248×864; PaddleOCR 960/max yields 960×672.
        let image = image::RgbImage::new(1248, 864);
        let scale = ScaleParam::for_detection(&image, 960);
        assert_eq!((scale.dst_width, scale.dst_height), (960, 672));
        assert!(!scale.is_identity());
    }

    #[test]
    fn rounding_follows_python_round_to_multiple_of_32() {
        assert_eq!(round_to_32(664), 672);
        assert_eq!(round_to_32(1184), 1184);
        assert_eq!(round_to_32(1200), 1216);
        assert_eq!(round_to_32(1168), 1152);
        assert_eq!(round_to_32(5), 32);
    }
}
