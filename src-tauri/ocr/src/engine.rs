use std::{path::Path, time::Instant};

use image::RgbImage;
use ort::{
    execution_providers::CPUExecutionProvider,
    session::builder::{GraphOptimizationLevel, SessionBuilder},
};
use thiserror::Error;

use crate::{
    ppocr::{
        cls::AngleNet,
        det::{DbNet, DetectionParams},
        rec::CrnnNet,
        scale::ScaleParam,
        utils, OcrError,
    },
    protocol::{AnalyzeResult, Line, Point, StageTimes},
};

pub use crate::pad::DET_MIN_PAD;
/// Longest detection side (PaddleOCR 2.x `det_limit_side_len=960, limit_type=max`,
/// confirmed 2026-09-07 for the P2 gate). Larger inputs are downscaled for
/// detection only; recognition always crops from the full-resolution input.
pub const DET_MAX_SIDE_LEN: u32 = 960;
/// DB post-processing constants from the locked model's own `det.yml`
/// (`thresh 0.2`, `box_thresh 0.45`, `unclip_ratio 1.4`); Q1 calibrates on the
/// training split only.
pub const DETECTION: DetectionParams = DetectionParams {
    box_score_threshold: 0.45,
    box_threshold: 0.2,
    unclip_ratio: 1.4,
};
const ANGLE_THRESHOLD: f32 = 0.9;
const MAX_LINES: usize = 3000;
const MAX_TEXT_BYTES: usize = 1024 * 1024;

pub type Builder = fn(SessionBuilder) -> Result<SessionBuilder, ort::Error>;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("模型路径无效")]
    ModelPath,
    #[error("仅支持 1、2、4、6、8 或 12 个推理线程")]
    Threads,
    #[error("ONNX Runtime 初始化失败")]
    Runtime(#[source] ort::Error),
    #[error("OCR 模型初始化失败")]
    Initialize(#[source] OcrError),
    #[error("RGB 数据与图像尺寸不一致")]
    Image,
    #[error("OCR 推理失败")]
    Analyze(#[source] OcrError),
    #[error("OCR 输出超过协议上限")]
    OutputLimit,
    #[error("OCR 输出包含无效数值")]
    InvalidOutput,
}

pub struct Engine {
    detection: DbNet,
    classification: AngleNet,
    recognition: CrnnNet,
}

pub struct ModelPaths<'a> {
    pub runtime: &'a Path,
    pub detection: &'a Path,
    pub classification: &'a Path,
    pub recognition: &'a Path,
    pub dictionary: &'a Path,
}

impl Engine {
    pub fn load(paths: ModelPaths<'_>, threads: usize) -> Result<Self, EngineError> {
        for path in [
            paths.runtime,
            paths.detection,
            paths.classification,
            paths.recognition,
            paths.dictionary,
        ] {
            if !path.is_file() {
                return Err(EngineError::ModelPath);
            }
        }

        ort::init_from(paths.runtime.to_string_lossy())
            .with_name("dreampaper-ocr")
            .with_telemetry(false)
            .commit()
            .map_err(EngineError::Runtime)?;

        let builder = session_builder(threads)?;
        let arena_builder = det_session_builder(threads)?;
        let detection =
            DbNet::load(paths.detection, arena_builder).map_err(EngineError::Initialize)?;
        let classification =
            AngleNet::load(paths.classification, builder).map_err(EngineError::Initialize)?;
        let recognition = CrnnNet::load(paths.recognition, paths.dictionary, arena_builder)
            .map_err(EngineError::Initialize)?;

        Ok(Self {
            detection,
            classification,
            recognition,
        })
    }

    /// Analyses an already padded RGB8 image; returned points are in its pixel space.
    pub fn analyze(
        &mut self,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
    ) -> Result<AnalyzeResult, EngineError> {
        let image = RgbImage::from_raw(width, height, rgb).ok_or(EngineError::Image)?;
        let started = Instant::now();

        let stage = Instant::now();
        let scale = ScaleParam::for_detection(&image, DET_MAX_SIDE_LEN);
        let prepare_us = micros(stage.elapsed());

        let stage = Instant::now();
        let boxes = self
            .detection
            .detect(&image, &scale, DETECTION)
            .map_err(EngineError::Analyze)?;
        let detection_us = micros(stage.elapsed());
        if boxes.len() > MAX_LINES {
            return Err(EngineError::OutputLimit);
        }

        let stage = Instant::now();
        let mut kept_boxes = Vec::with_capacity(boxes.len());
        let mut parts = Vec::with_capacity(boxes.len());
        for (text_box, part) in boxes.iter().zip(utils::crop_lines(&image, &boxes)) {
            if let Some(part) = part {
                kept_boxes.push(text_box.clone());
                parts.push(part);
            }
        }
        let crop_us = micros(stage.elapsed());

        let stage = Instant::now();
        let angles = self
            .classification
            .classify_all(&parts)
            .map_err(EngineError::Analyze)?;
        let classification_us = micros(stage.elapsed());
        if kept_boxes.len() != parts.len() || kept_boxes.len() != angles.len() {
            return Err(EngineError::InvalidOutput);
        }

        let mut rotations = Vec::with_capacity(angles.len());
        for (part, angle) in parts.iter_mut().zip(&angles) {
            let rotate = selected_angle(angle.index, angle.score)? == 180;
            if rotate {
                utils::rotate_180(part);
            }
            rotations.push((if rotate { 180 } else { 0 }, angle.score));
        }

        let stage = Instant::now();
        let text = self
            .recognition
            .recognize_all(&parts)
            .map_err(EngineError::Analyze)?;
        let recognition_us = micros(stage.elapsed());
        if text.len() != kept_boxes.len() {
            return Err(EngineError::InvalidOutput);
        }

        let stage = Instant::now();
        let mut text_bytes = 0_usize;
        let mut lines = Vec::with_capacity(kept_boxes.len());
        for ((bbox, text), (angle, angle_score)) in kept_boxes.into_iter().zip(text).zip(rotations)
        {
            text_bytes = text_bytes
                .checked_add(text.text.len())
                .ok_or(EngineError::OutputLimit)?;
            if text_bytes > MAX_TEXT_BYTES
                || !text.text_score.is_finite()
                || !bbox.score.is_finite()
                || !angle_score.is_finite()
            {
                return Err(EngineError::InvalidOutput);
            }
            lines.push(Line {
                text: text.text,
                score: text.text_score.clamp(0.0, 1.0),
                box_score: bbox.score.clamp(0.0, 1.0),
                angle,
                angle_score: angle_score.clamp(0.0, 1.0),
                points: bbox
                    .points
                    .into_iter()
                    .map(|point| Point {
                        x: point.x.min(width),
                        y: point.y.min(height),
                    })
                    .collect(),
            });
        }
        let assemble_us = micros(stage.elapsed());

        Ok(AnalyzeResult {
            width,
            height,
            elapsed_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
            stages: StageTimes {
                prepare_us,
                detection_us,
                crop_us,
                classification_us,
                recognition_us,
                assemble_us,
            },
            lines,
        })
    }
}

fn micros(duration: std::time::Duration) -> u64 {
    duration.as_micros().try_into().unwrap_or(u64::MAX)
}

/// Reference rule: rotate only when the "180" label wins with `score > cls_thresh`.
fn selected_angle(index: i32, score: f32) -> Result<u16, EngineError> {
    if !score.is_finite() || !matches!(index, 0 | 1) {
        return Err(EngineError::InvalidOutput);
    }
    Ok(if index == 1 && score > ANGLE_THRESHOLD {
        180
    } else {
        0
    })
}

fn session_builder(threads: usize) -> Result<Builder, EngineError> {
    select_builder(threads, false)
}

/// Session options used for detection and recognition; exposed so gate probes
/// measure exactly what the sidecar runs.
pub fn det_session_builder(threads: usize) -> Result<Builder, EngineError> {
    select_builder(threads, true)
}

fn select_builder(threads: usize, arena: bool) -> Result<Builder, EngineError> {
    match (threads, arena) {
        (1, false) => Ok(configure_session::<1>),
        (2, false) => Ok(configure_session::<2>),
        (4, false) => Ok(configure_session::<4>),
        (6, false) => Ok(configure_session::<6>),
        (8, false) => Ok(configure_session::<8>),
        (12, false) => Ok(configure_session::<12>),
        (1, true) => Ok(configure_arena_session::<1>),
        (2, true) => Ok(configure_arena_session::<2>),
        (4, true) => Ok(configure_arena_session::<4>),
        (6, true) => Ok(configure_arena_session::<6>),
        (8, true) => Ok(configure_arena_session::<8>),
        (12, true) => Ok(configure_arena_session::<12>),
        _ => Err(EngineError::Threads),
    }
}

fn configure_session<const THREADS: usize>(
    builder: SessionBuilder,
) -> Result<SessionBuilder, ort::Error> {
    configure::<THREADS>(builder, CPUExecutionProvider::default())
}

fn configure_arena_session<const THREADS: usize>(
    builder: SessionBuilder,
) -> Result<SessionBuilder, ort::Error> {
    configure::<THREADS>(
        builder,
        CPUExecutionProvider::default().with_arena_allocator(),
    )
}

fn configure<const THREADS: usize>(
    builder: SessionBuilder,
    provider: CPUExecutionProvider,
) -> Result<SessionBuilder, ort::Error> {
    builder
        .with_execution_providers([provider.build()])?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(THREADS)?
        .with_inter_threads(1)?
        .with_parallel_execution(false)?
        .with_memory_pattern(true)
}

#[cfg(test)]
mod tests {
    use super::selected_angle;

    #[test]
    fn rotates_only_high_confidence_180_degree_lines() {
        assert_eq!(selected_angle(1, 0.9001).unwrap(), 180);
        assert_eq!(selected_angle(1, 0.9).unwrap(), 0);
        assert_eq!(selected_angle(0, 0.99).unwrap(), 0);
    }

    #[test]
    fn rejects_invalid_classifier_output() {
        assert!(selected_angle(2, 0.99).is_err());
        assert!(selected_angle(1, f32::NAN).is_err());
    }
}
