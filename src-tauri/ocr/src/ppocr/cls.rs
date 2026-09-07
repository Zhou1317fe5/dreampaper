use std::path::Path;

use image::RgbImage;
use ndarray::{s, Array4};
use ort::{inputs, session::Session, value::Tensor};

use super::{
    base::{first_input_name, load_session, BuilderFn},
    error::OcrError,
    result::Angle,
    utils,
};

const MEAN_VALUES: [f32; 3] = [127.5, 127.5, 127.5];
const NORM_VALUES: [f32; 3] = [1.0 / 127.5, 1.0 / 127.5, 1.0 / 127.5];
const ANGLE_WIDTH: u32 = 192;
const ANGLE_HEIGHT: u32 = 48;
const ANGLE_CLASSES: usize = 2;

/// 0°/180° text line orientation classifier (`ch_ppocr_mobile_v2.0_cls`).
pub struct AngleNet {
    session: Session,
    input_name: String,
}

impl AngleNet {
    pub fn load(path: &Path, configure: BuilderFn) -> Result<Self, OcrError> {
        let session = load_session(path, configure)?;
        let input_name = first_input_name(&session)?;
        Ok(Self {
            session,
            input_name,
        })
    }

    pub fn classify_all(&mut self, lines: &[RgbImage]) -> Result<Vec<Angle>, OcrError> {
        lines.iter().map(|line| self.classify(line)).collect()
    }

    pub fn classify(&mut self, line: &RgbImage) -> Result<Angle, OcrError> {
        // Reference `resize_norm_img`: keep the aspect ratio, scale to the
        // classifier height and left-align on a zero canvas (zero is the
        // normalised value of mid grey) instead of stretching to full width.
        let resized_width = resized_width(line);
        let resized = image::imageops::resize(
            line,
            resized_width,
            ANGLE_HEIGHT,
            image::imageops::FilterType::Triangle,
        );
        let mut input = Array4::<f32>::zeros((1, 3, ANGLE_HEIGHT as usize, ANGLE_WIDTH as usize));
        input
            .slice_mut(s![0, .., .., ..resized_width as usize])
            .assign(
                &utils::normalize_bgr_nchw(&resized, &MEAN_VALUES, &NORM_VALUES).slice(s![
                    0,
                    ..,
                    ..,
                    ..
                ]),
            );
        let tensor = Tensor::from_array(input)?;
        let outputs = self
            .session
            .run(inputs![self.input_name.clone() => tensor]?)?;
        let (_, output) = outputs
            .iter()
            .next()
            .ok_or_else(|| OcrError::ModelOutput("方向模型没有输出".into()))?;
        let view = output.try_extract_tensor::<f32>()?;
        let mut angle = Angle::default();
        let mut max_value = f32::MIN;
        for (index, value) in view.iter().take(ANGLE_CLASSES).enumerate() {
            if index == 0 || *value > max_value {
                max_value = *value;
                angle.index = index as i32;
            }
        }
        angle.score = max_value;
        Ok(angle)
    }
}

fn resized_width(line: &RgbImage) -> u32 {
    let ratio = line.width() as f32 / line.height().max(1) as f32;
    ((ANGLE_HEIGHT as f32 * ratio).ceil() as u32).clamp(1, ANGLE_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_lines_keep_their_aspect_ratio() {
        assert_eq!(resized_width(&RgbImage::new(60, 60)), 48);
        assert_eq!(resized_width(&RgbImage::new(100, 40)), 120);
    }

    #[test]
    fn wide_lines_are_clamped_to_the_classifier_width() {
        assert_eq!(resized_width(&RgbImage::new(4000, 40)), ANGLE_WIDTH);
    }
}
