use std::{cmp::Ordering, path::Path};

use geo_clipper::{Clipper, EndType, JoinType};
use geo_types::{Coord, LineString, Polygon};
use image::RgbImage;
use imageproc::{contours::Contour, point::Point as ImagePoint};
use ort::{inputs, session::Session, value::Tensor};

use super::{
    base::{first_input_name, load_session, BuilderFn},
    error::OcrError,
    result::{Point, TextBox},
    scale::ScaleParam,
    utils,
};

const MEAN_VALUES: [f32; 3] = [0.485 * 255.0, 0.456 * 255.0, 0.406 * 255.0];
const NORM_VALUES: [f32; 3] = [
    1.0 / 0.229 / 255.0,
    1.0 / 0.224 / 255.0,
    1.0 / 0.225 / 255.0,
];
const MAX_SIDE_THRESHOLD: f32 = 3.0;

#[derive(Debug, Clone, Copy)]
pub struct DetectionParams {
    pub box_score_threshold: f32,
    pub box_threshold: f32,
    pub unclip_ratio: f32,
}

pub struct DbNet {
    session: Session,
    input_name: String,
}

impl DbNet {
    pub fn load(path: &Path, configure: BuilderFn) -> Result<Self, OcrError> {
        let session = load_session(path, configure)?;
        let input_name = first_input_name(&session)?;
        Ok(Self {
            session,
            input_name,
        })
    }

    /// Detects text quadrilaterals. Points are returned in `image` pixel space.
    pub fn detect(
        &mut self,
        image: &RgbImage,
        scale: &ScaleParam,
        params: DetectionParams,
    ) -> Result<Vec<TextBox>, OcrError> {
        // Difference from the origin: an already aligned input is fed as-is
        // instead of being resampled to identical dimensions.
        let resized;
        let input = if scale.is_identity() {
            image
        } else {
            resized = image::imageops::resize(
                image,
                scale.dst_width,
                scale.dst_height,
                image::imageops::FilterType::Triangle,
            );
            &resized
        };
        let rows = input.height();
        let cols = input.width();
        let tensor =
            Tensor::from_array(utils::normalize_bgr_nchw(input, &MEAN_VALUES, &NORM_VALUES))?;
        let outputs = self
            .session
            .run(inputs![self.input_name.clone() => tensor]?)?;
        let (_, output) = outputs
            .iter()
            .next()
            .ok_or_else(|| OcrError::ModelOutput("检测模型没有输出".into()))?;
        let view = output.try_extract_tensor::<f32>()?;
        let expected = rows as usize * cols as usize;
        if view.len() != expected {
            return Err(OcrError::ModelOutput(format!(
                "检测输出长度 {} 与输入 {}×{} 不匹配",
                view.len(),
                cols,
                rows
            )));
        }
        let probabilities: Vec<f32> = view.iter().copied().collect();
        Ok(Self::boxes_from_probabilities(
            probabilities,
            rows,
            cols,
            scale,
            params,
        ))
    }

    /// DB post-processing on a raw probability map. Public so gate probes and
    /// golden tests can feed reference maps without running the model.
    pub fn boxes_from_probabilities(
        probabilities: Vec<f32>,
        rows: u32,
        cols: u32,
        scale: &ScaleParam,
        params: DetectionParams,
    ) -> Vec<TextBox> {
        // Reference `segmentation = pred > thresh` on the float map; PaddleX
        // defaults to `use_dilation=False`, so contours come straight from it.
        let binary: Vec<u8> = probabilities
            .iter()
            .map(|pixel| {
                if *pixel > params.box_threshold {
                    255
                } else {
                    0
                }
            })
            .collect();
        let probability_map: image::ImageBuffer<image::Luma<f32>, Vec<f32>> =
            image::ImageBuffer::from_vec(cols, rows, probabilities).expect("shape matches");
        let binary = image::GrayImage::from_vec(cols, rows, binary).expect("shape matches");
        let contours: Vec<Contour<i32>> = imageproc::contours::find_contours(&binary);

        let mut boxes = Vec::new();
        for contour in contours {
            if contour.points.len() <= 2 {
                continue;
            }
            let mut max_side = 0.0;
            let min_box = Self::mini_box(&contour.points, &mut max_side);
            if max_side < MAX_SIDE_THRESHOLD {
                continue;
            }
            // Reference default is `score_mode: fast`: average the probability
            // inside the minimum-area box, not inside the raw contour.
            let score = Self::score_fast(&min_box, &probability_map);
            if score < params.box_score_threshold {
                continue;
            }
            let clipped = Self::unclip(&min_box, params.unclip_ratio);
            if clipped.is_empty() {
                continue;
            }
            let mut max_side_clip = 0.0;
            let clip_box = Self::mini_box(&clipped, &mut max_side_clip);
            if max_side_clip < MAX_SIDE_THRESHOLD + 2.0 {
                continue;
            }
            // Reference `clip_det_res` keeps points inside `[0, side - 1]`.
            let points: Vec<Point> = clip_box
                .into_iter()
                .map(|item| Point {
                    x: ((item.x / scale.scale_width).round().max(0.0) as u32)
                        .min(scale.src_width.saturating_sub(1)),
                    y: ((item.y / scale.scale_height).round().max(0.0) as u32)
                        .min(scale.src_height.saturating_sub(1)),
                })
                .collect();
            // Reference `filter_det_res` drops boxes that collapsed to <= 3 px after clipping.
            if edge_length(&points[0], &points[1]) <= 3 || edge_length(&points[0], &points[3]) <= 3
            {
                continue;
            }
            boxes.push(TextBox { score, points });
        }
        boxes
    }

    fn mini_box(contour: &[ImagePoint<i32>], min_edge: &mut f32) -> Vec<ImagePoint<f32>> {
        let rect = imageproc::geometry::min_area_rect(contour);
        let mut rect_points: Vec<ImagePoint<f32>> = rect
            .iter()
            .map(|p| ImagePoint::new(p.x as f32, p.y as f32))
            .collect();

        let width = ((rect_points[0].x - rect_points[1].x).powi(2)
            + (rect_points[0].y - rect_points[1].y).powi(2))
        .sqrt();
        let height = ((rect_points[1].x - rect_points[2].x).powi(2)
            + (rect_points[1].y - rect_points[2].y).powi(2))
        .sqrt();
        *min_edge = width.min(height);

        rect_points.sort_by(|a, b| {
            if a.x > b.x {
                Ordering::Greater
            } else if a.x == b.x {
                Ordering::Equal
            } else {
                Ordering::Less
            }
        });

        let (index_1, index_4) = if rect_points[1].y > rect_points[0].y {
            (0, 1)
        } else {
            (1, 0)
        };
        let (index_2, index_3) = if rect_points[3].y > rect_points[2].y {
            (2, 3)
        } else {
            (3, 2)
        };
        vec![
            rect_points[index_1],
            rect_points[index_2],
            rect_points[index_3],
            rect_points[index_4],
        ]
    }

    /// `box_score_fast`: mean probability inside the (floor/ceil bounded) box polygon.
    fn score_fast(
        box_points: &[ImagePoint<f32>],
        probability_map: &image::ImageBuffer<image::Luma<f32>, Vec<f32>>,
    ) -> f32 {
        let width = probability_map.width() as i32;
        let height = probability_map.height() as i32;
        let mut xmin = i32::MAX;
        let mut xmax = i32::MIN;
        let mut ymin = i32::MAX;
        let mut ymax = i32::MIN;
        for point in box_points {
            xmin = xmin.min(point.x.floor() as i32);
            xmax = xmax.max(point.x.ceil() as i32);
            ymin = ymin.min(point.y.floor() as i32);
            ymax = ymax.max(point.y.ceil() as i32);
        }
        xmin = xmin.clamp(0, width - 1);
        xmax = xmax.clamp(0, width - 1);
        ymin = ymin.clamp(0, height - 1);
        ymax = ymax.clamp(0, height - 1);
        let roi_width = xmax - xmin + 1;
        let roi_height = ymax - ymin + 1;
        if roi_width <= 0 || roi_height <= 0 {
            return 0.0;
        }

        let mut mask = image::GrayImage::new(roi_width as u32, roi_height as u32);
        let points: Vec<ImagePoint<i32>> = box_points
            .iter()
            .map(|point| ImagePoint::new(point.x as i32 - xmin, point.y as i32 - ymin))
            .collect();
        imageproc::drawing::draw_polygon_mut(&mut mask, &points, image::Luma([255]));
        let cropped = image::imageops::crop_imm(
            probability_map,
            xmin as u32,
            ymin as u32,
            roi_width as u32,
            roi_height as u32,
        )
        .to_image();
        utils::mean_with_mask(&cropped, &mask)
    }

    fn unclip(box_points: &[ImagePoint<f32>], unclip_ratio: f32) -> Vec<ImagePoint<i32>> {
        let width = ((box_points[0].x - box_points[1].x).powi(2)
            + (box_points[0].y - box_points[1].y).powi(2))
        .sqrt();
        let height = ((box_points[1].x - box_points[2].x).powi(2)
            + (box_points[1].y - box_points[2].y).powi(2))
        .sqrt();
        if height < 1.001 && width < 1.001 {
            return Vec::new();
        }

        let coords: Vec<Coord<f64>> = box_points
            .iter()
            .map(|pt| Coord {
                x: pt.x as f64,
                y: pt.y as f64,
            })
            .collect();
        let area = Self::signed_area(box_points).abs();
        let length = Self::perimeter(box_points);
        let distance = area * unclip_ratio / length as f32;

        let polygon = Polygon::new(LineString::new(coords), vec![]);
        let solution = polygon
            .offset(
                distance as f64,
                JoinType::Round(2.0),
                EndType::ClosedPolygon,
                1.0,
            )
            .0;
        let Some(first) = solution.first() else {
            return Vec::new();
        };
        first
            .exterior()
            .points()
            .map(|point| ImagePoint::new(point.x() as i32, point.y() as i32))
            .collect()
    }

    fn signed_area(points: &[ImagePoint<f32>]) -> f32 {
        let mut area = 0.0;
        for index in 0..points.len() {
            let next = points[(index + 1) % points.len()];
            area += (next.x - points[index].x) * (next.y + points[index].y) / 2.0;
        }
        area
    }

    fn perimeter(points: &[ImagePoint<f32>]) -> f64 {
        if points.is_empty() {
            return 0.0;
        }
        let mut length = 0.0;
        for index in 0..points.len() {
            let current = points[index];
            let next = points[(index + 1) % points.len()];
            let dx = next.x as f64 - current.x as f64;
            let dy = next.y as f64 - current.y as f64;
            length += (dx * dx + dy * dy).sqrt();
        }
        length
    }
}

/// Integer edge length as in the reference (`int(np.linalg.norm(a - b))`).
fn edge_length(a: &Point, b: &Point) -> u32 {
    let dx = a.x as f64 - b.x as f64;
    let dy = a.y as f64 - b.y as f64;
    (dx * dx + dy * dy).sqrt() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_length_truncates_like_the_reference() {
        assert_eq!(edge_length(&Point { x: 0, y: 0 }, &Point { x: 3, y: 4 }), 5);
        assert_eq!(edge_length(&Point { x: 0, y: 0 }, &Point { x: 3, y: 1 }), 3);
    }

    #[test]
    fn fast_score_averages_inside_the_box_only() {
        let mut map = image::ImageBuffer::<image::Luma<f32>, Vec<f32>>::new(10, 10);
        for x in 2..6 {
            for y in 2..4 {
                map.put_pixel(x, y, image::Luma([1.0]));
            }
        }
        let box_points = [
            ImagePoint::new(2.0, 2.0),
            ImagePoint::new(5.0, 2.0),
            ImagePoint::new(5.0, 3.0),
            ImagePoint::new(2.0, 3.0),
        ];
        assert_eq!(DbNet::score_fast(&box_points, &map), 1.0);
        let outside = [
            ImagePoint::new(6.0, 6.0),
            ImagePoint::new(9.0, 6.0),
            ImagePoint::new(9.0, 9.0),
            ImagePoint::new(6.0, 9.0),
        ];
        assert_eq!(DbNet::score_fast(&outside, &map), 0.0);
    }
}
