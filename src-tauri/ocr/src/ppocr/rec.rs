use std::path::Path;

use image::RgbImage;
use ndarray::{s, Array4};
use ort::{inputs, session::Session, value::Tensor};

use super::{
    base::{first_input_name, load_session, BuilderFn},
    error::OcrError,
    result::TextLine,
    utils,
};

const MEAN_VALUES: [f32; 3] = [127.5, 127.5, 127.5];
const NORM_VALUES: [f32; 3] = [1.0 / 127.5, 1.0 / 127.5, 1.0 / 127.5];
pub const REC_HEIGHT: u32 = 48;
pub const REC_BASE_WIDTH: u32 = 320;
/// Same as RapidOCR `rec_batch_num`; lines are sorted by aspect ratio so each
/// batch pads to a similar width.
pub const REC_BATCH: usize = 6;

/// CTC text recogniser (PP-OCRv6 rec) with reference-style batching. The
/// origin crate ran one line at a time but padded every line to the widest
/// line of the whole image; batching by sorted aspect ratio follows
/// PaddleOCR/RapidOCR and removes most of that padding work.
pub struct CrnnNet {
    session: Session,
    input_name: String,
    keys: Vec<String>,
}

impl CrnnNet {
    pub fn load(path: &Path, dictionary: &Path, configure: BuilderFn) -> Result<Self, OcrError> {
        let session = load_session(path, configure)?;
        let input_name = first_input_name(&session)?;
        let keys = read_keys(dictionary)?;
        Ok(Self {
            session,
            input_name,
            keys,
        })
    }

    pub fn vocabulary_len(&self) -> usize {
        self.keys.len()
    }

    /// Recognises every line image and returns results in input order.
    pub fn recognize_all(&mut self, lines: &[RgbImage]) -> Result<Vec<TextLine>, OcrError> {
        let mut order: Vec<usize> = (0..lines.len()).collect();
        order.sort_by(|a, b| {
            aspect_ratio(&lines[*a])
                .partial_cmp(&aspect_ratio(&lines[*b]))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut results = vec![TextLine::default(); lines.len()];
        for batch in order.chunks(REC_BATCH) {
            let images: Vec<&RgbImage> = batch.iter().map(|index| &lines[*index]).collect();
            let decoded = self.recognize_batch(&images)?;
            for (index, line) in batch.iter().zip(decoded) {
                results[*index] = line;
            }
        }
        Ok(results)
    }

    fn recognize_batch(&mut self, images: &[&RgbImage]) -> Result<Vec<TextLine>, OcrError> {
        let base_ratio = REC_BASE_WIDTH as f32 / REC_HEIGHT as f32;
        let max_ratio = images
            .iter()
            .map(|image| aspect_ratio(image))
            .fold(base_ratio, f32::max);
        let target_width = (REC_HEIGHT as f32 * max_ratio) as u32;

        let mut batch =
            Array4::<f32>::zeros((images.len(), 3, REC_HEIGHT as usize, target_width as usize));
        for (index, image) in images.iter().enumerate() {
            let resized_width = resized_width(image, target_width);
            let resized = image::imageops::resize(
                *image,
                resized_width,
                REC_HEIGHT,
                image::imageops::FilterType::Triangle,
            );
            let normalized = utils::normalize_bgr_nchw(&resized, &MEAN_VALUES, &NORM_VALUES);
            batch
                .slice_mut(s![index, .., .., ..resized_width as usize])
                .assign(&normalized.slice(s![0, .., .., ..]));
        }

        let tensor = Tensor::from_array(batch)?;
        let outputs = self
            .session
            .run(inputs![self.input_name.clone() => tensor]?)?;
        let (_, output) = outputs
            .iter()
            .next()
            .ok_or_else(|| OcrError::ModelOutput("识别模型没有输出".into()))?;
        let view = output.try_extract_tensor::<f32>()?;
        let shape = view.shape();
        if shape.len() != 3 || shape[0] != images.len() {
            return Err(OcrError::ModelOutput(format!(
                "识别输出形状无效：{shape:?}"
            )));
        }
        let timesteps = shape[1];
        let vocabulary = shape[2];
        let data: Vec<f32> = view.iter().copied().collect();
        let stride = timesteps * vocabulary;
        Ok((0..images.len())
            .map(|index| {
                decode_ctc(
                    &data[index * stride..(index + 1) * stride],
                    timesteps,
                    vocabulary,
                    &self.keys,
                )
            })
            .collect())
    }
}

fn aspect_ratio(image: &RgbImage) -> f32 {
    image.width() as f32 / image.height().max(1) as f32
}

/// PaddleOCR `resize_norm_img`: scale to the recogniser height and clamp the
/// width to the batch width instead of growing the batch.
fn resized_width(image: &RgbImage, target_width: u32) -> u32 {
    let ratio = aspect_ratio(image);
    let scaled = (REC_HEIGHT as f32 * ratio).ceil() as u32;
    scaled.clamp(1, target_width.max(1))
}

fn read_keys(path: &Path) -> Result<Vec<String>, OcrError> {
    let content = std::fs::read_to_string(path)?;
    let mut lines: Vec<&str> = content.split('\n').collect();
    // Only the trailing newline is dropped: interior empty lines are real tokens.
    if lines
        .last()
        .is_some_and(|line| line.trim_end_matches('\r').is_empty())
    {
        lines.pop();
    }
    let mut keys = Vec::with_capacity(lines.len() + 2);
    keys.push("#".to_owned());
    keys.extend(
        lines
            .iter()
            .map(|line| line.trim_end_matches('\r').to_owned()),
    );
    keys.push(" ".to_owned());
    Ok(keys)
}

/// Greedy CTC decoding shared with the origin: argmax per timestep, drop blank
/// (index 0) and repeats of the previous index, average the kept scores.
pub fn decode_ctc(
    output: &[f32],
    timesteps: usize,
    vocabulary: usize,
    keys: &[String],
) -> TextLine {
    let mut line = TextLine::default();
    let mut last_index = 0_usize;
    let mut score_sum = 0.0_f32;
    let mut score_count = 0_usize;
    for step in 0..timesteps {
        let start = step * vocabulary;
        let stop = ((step + 1) * vocabulary).min(output.len());
        if start >= stop {
            break;
        }
        let (max_index, max_value) = output[start..stop].iter().enumerate().fold(
            (0_usize, f32::MIN),
            |(max_index, max_value), (index, value)| {
                if *value > max_value {
                    (index, *value)
                } else {
                    (max_index, max_value)
                }
            },
        );
        if max_index > 0 && max_index < keys.len() && !(step > 0 && max_index == last_index) {
            line.text.push_str(&keys[max_index]);
            score_sum += max_value;
            score_count += 1;
        }
        last_index = max_index;
    }
    line.text_score = if score_count > 0 {
        score_sum / score_count as f32
    } else {
        0.0
    };
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Vec<String> {
        ["#", "a", "b", " "]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    #[test]
    fn ctc_collapses_repeats_and_blanks() {
        // timesteps: a a # b b # a
        let output = [
            0.1, 0.8, 0.1, 0.0, //
            0.1, 0.7, 0.2, 0.0, //
            0.9, 0.0, 0.1, 0.0, //
            0.1, 0.0, 0.9, 0.0, //
            0.2, 0.0, 0.8, 0.0, //
            0.9, 0.1, 0.0, 0.0, //
            0.0, 0.6, 0.4, 0.0, //
        ];
        let line = decode_ctc(&output, 7, 4, &keys());
        assert_eq!(line.text, "aba");
        assert!((line.text_score - (0.8 + 0.9 + 0.6) / 3.0).abs() < 1e-6);
    }

    #[test]
    fn ctc_without_characters_scores_zero() {
        let output = [0.9, 0.05, 0.05, 0.0, 0.9, 0.05, 0.05, 0.0];
        let line = decode_ctc(&output, 2, 4, &keys());
        assert_eq!(line.text, "");
        assert_eq!(line.text_score, 0.0);
    }

    #[test]
    fn width_is_clamped_to_batch_width() {
        let wide = RgbImage::new(4000, 40);
        assert_eq!(resized_width(&wide, 960), 960);
        let narrow = RgbImage::new(100, 50);
        assert_eq!(resized_width(&narrow, 960), 96);
    }
}
