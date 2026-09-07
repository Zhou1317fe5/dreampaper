//! Dominant-colour analysis for a repair region.
//!
//! The fill is explainable, not content-aware: the region's pixels are
//! binned coarsely, neighbouring bins are merged with a tolerance wide
//! enough to absorb JPEG ringing and anti-aliased edges, and the cluster
//! covering the most pixels wins. A low winning share, or a strong runner-up
//! far away in colour, is reported as "uneven" so the UI can warn while
//! still offering the candidate.

use serde::Serialize;

use super::doc::{hex_color, Shape};
use super::geom::{ellipse_contains, PixelRect};
use super::source::DecodedSource;

/// Side of a histogram bin in each channel.
const BIN: u32 = 12;
/// Clusters closer than this (Euclidean RGB) are merged.
const MERGE_DISTANCE: f64 = 34.0;
/// Bins holding less than this share of samples are noise and never seed
/// a cluster (they may still join one).
const MIN_SEED_SHARE: f64 = 0.004;
/// Below this share for the winner the background is called uneven.
const EVEN_COVERAGE: f64 = 0.6;
/// A runner-up above this share that is clearly a different colour also
/// marks the region uneven (two colour blocks, or a gradient).
const RIVAL_SHARE: f64 = 0.20;
const RIVAL_DISTANCE: f64 = 60.0;
/// Cap on sampled pixels; larger regions are strided.
const MAX_SAMPLES: usize = 250_000;

#[derive(Clone, Debug, Serialize)]
pub struct ColorCandidate {
    pub color: String,
    pub coverage: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RegionAnalysis {
    pub color: String,
    pub coverage: f64,
    pub uneven: bool,
    pub candidates: Vec<ColorCandidate>,
    pub samples: usize,
}

struct Cluster {
    sum: [f64; 3],
    count: u64,
}

impl Cluster {
    fn mean(&self) -> [f64; 3] {
        let n = self.count.max(1) as f64;
        [self.sum[0] / n, self.sum[1] / n, self.sum[2] / n]
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Analyse the pixels of `rect` (masked by `shape`) in `source`.
pub fn analyze(source: &DecodedSource, rect: &PixelRect, shape: Shape) -> RegionAnalysis {
    let Some(rect) = rect.clamped(source.width, source.height) else {
        return RegionAnalysis {
            color: "#ffffff".into(),
            coverage: 0.0,
            uneven: true,
            candidates: Vec::new(),
            samples: 0,
        };
    };
    let area = (rect.width * rect.height) as usize;
    let stride = ((area + MAX_SAMPLES - 1) / MAX_SAMPLES).max(1);
    let stride_side = (stride as f64).sqrt().ceil() as i64;

    // Bin -> (sum rgb, count). Bins are sparse, so a map beats a 3-D array.
    let mut bins: std::collections::HashMap<u32, Cluster> = std::collections::HashMap::new();
    let mut samples = 0usize;
    let mut y = rect.y;
    while y < rect.bottom() {
        let mut x = rect.x;
        while x < rect.right() {
            let inside = match shape {
                Shape::Rect => true,
                Shape::Ellipse => ellipse_contains(&rect, x, y),
            };
            if inside {
                let [r, g, b] = source.pixel(x as u32, y as u32);
                let key =
                    (u32::from(r) / BIN) << 16 | (u32::from(g) / BIN) << 8 | (u32::from(b) / BIN);
                let entry = bins.entry(key).or_insert(Cluster {
                    sum: [0.0; 3],
                    count: 0,
                });
                entry.sum[0] += f64::from(r);
                entry.sum[1] += f64::from(g);
                entry.sum[2] += f64::from(b);
                entry.count += 1;
                samples += 1;
            }
            x += stride_side;
        }
        y += stride_side;
    }
    if samples == 0 {
        return RegionAnalysis {
            color: "#ffffff".into(),
            coverage: 0.0,
            uneven: true,
            candidates: Vec::new(),
            samples: 0,
        };
    }

    // Greedy agglomeration from the heaviest bin down: deterministic, and
    // a heavy bin always seeds before a light neighbour can pull it away.
    let mut ordered: Vec<Cluster> = bins.into_values().collect();
    ordered.sort_by(|a, b| b.count.cmp(&a.count));
    let total = samples as f64;
    let mut clusters: Vec<Cluster> = Vec::new();
    for bin in ordered {
        let mean = bin.mean();
        let can_seed = (bin.count as f64) / total >= MIN_SEED_SHARE || clusters.is_empty();
        let nearest = clusters
            .iter_mut()
            .map(|cluster| (distance(cluster.mean(), mean), cluster))
            .filter(|(d, _)| *d <= MERGE_DISTANCE)
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        match nearest {
            Some((_, cluster)) => {
                cluster.sum[0] += bin.sum[0];
                cluster.sum[1] += bin.sum[1];
                cluster.sum[2] += bin.sum[2];
                cluster.count += bin.count;
            }
            None if can_seed => clusters.push(bin),
            None => {
                // Too light to seed; attach to the closest cluster anyway so
                // coverage still sums to one.
                if let Some(cluster) = clusters.iter_mut().min_by(|a, b| {
                    distance(a.mean(), mean)
                        .partial_cmp(&distance(b.mean(), mean))
                        .unwrap_or(std::cmp::Ordering::Equal)
                }) {
                    cluster.sum[0] += bin.sum[0];
                    cluster.sum[1] += bin.sum[1];
                    cluster.sum[2] += bin.sum[2];
                    cluster.count += bin.count;
                }
            }
        }
    }
    clusters.sort_by(|a, b| b.count.cmp(&a.count));

    let winner = clusters[0].mean();
    let coverage = clusters[0].count as f64 / total;
    let rival = clusters
        .get(1)
        .map(|c| (c.count as f64 / total, distance(c.mean(), winner)));
    let uneven = coverage < EVEN_COVERAGE
        || rival.is_some_and(|(share, d)| share >= RIVAL_SHARE && d >= RIVAL_DISTANCE);

    let to_hex = |mean: [f64; 3]| {
        hex_color([
            mean[0].round() as u8,
            mean[1].round() as u8,
            mean[2].round() as u8,
        ])
    };
    RegionAnalysis {
        color: to_hex(winner),
        coverage,
        uneven,
        candidates: clusters
            .iter()
            .take(4)
            .map(|cluster| ColorCandidate {
                color: to_hex(cluster.mean()),
                coverage: cluster.count as f64 / total,
            })
            .collect(),
        samples,
    }
}

#[cfg(test)]
mod tests {
    use super::super::source::PngMeta;
    use super::*;

    fn source(width: u32, height: u32, paint: impl Fn(u32, u32) -> [u8; 3]) -> DecodedSource {
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                rgb.extend_from_slice(&paint(x, y));
            }
        }
        DecodedSource {
            width,
            height,
            rgb,
            had_alpha: false,
            icc: None,
            png: PngMeta::default(),
            jfif_dpi: None,
        }
    }

    /// Deterministic pseudo-noise in [-amp, amp].
    fn noise(x: u32, y: u32, amp: i32) -> i32 {
        let h = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)) % 1000;
        (h as i32 % (2 * amp + 1)) - amp
    }

    #[test]
    fn solid_region_is_even_and_exact() {
        let src = source(40, 20, |_, _| [30, 120, 200]);
        let result = analyze(&src, &PixelRect::new(5, 5, 20, 10), Shape::Rect);
        assert_eq!(result.color, "#1e78c8");
        assert!(!result.uneven);
        assert!((result.coverage - 1.0).abs() < 1e-9);
    }

    #[test]
    fn jpeg_like_noise_does_not_split_a_flat_background() {
        let src = source(60, 40, |x, y| {
            let n = noise(x, y, 7);
            [(235 + n) as u8, (235 + n) as u8, (240 + n) as u8]
        });
        let result = analyze(&src, &PixelRect::new(0, 0, 60, 40), Shape::Rect);
        assert!(!result.uneven, "噪点不应把同一底色拆散: {result:?}");
        assert!(result.coverage > 0.95);
        assert_eq!(result.candidates.len(), 1);
    }

    #[test]
    fn two_blocks_pick_the_larger_and_flag_uneven() {
        let src = source(100, 10, |x, _| {
            if x < 65 {
                [250, 250, 250]
            } else {
                [20, 20, 20]
            }
        });
        let result = analyze(&src, &PixelRect::new(0, 0, 100, 10), Shape::Rect);
        assert_eq!(result.color, "#fafafa");
        assert!(result.uneven);
        assert!((result.coverage - 0.65).abs() < 0.02);
        assert_eq!(result.candidates[1].color, "#141414");
    }

    #[test]
    fn dark_text_on_light_background_keeps_the_background() {
        // Thin "strokes" cover ~20% of the area.
        let src = source(50, 20, |x, y| {
            if y % 5 == 0 && x % 2 == 0 {
                [10, 10, 10]
            } else {
                [255, 255, 255]
            }
        });
        let result = analyze(&src, &PixelRect::new(0, 0, 50, 20), Shape::Rect);
        assert_eq!(result.color, "#ffffff");
        assert!(!result.uneven);
    }

    #[test]
    fn ellipse_mask_ignores_the_corners() {
        // Corners are red, the inscribed ellipse is blue.
        let rect = PixelRect::new(0, 0, 40, 20);
        let src = source(40, 20, |x, y| {
            if ellipse_contains(&rect, x as i64, y as i64) {
                [0, 0, 255]
            } else {
                [255, 0, 0]
            }
        });
        let result = analyze(&src, &rect, Shape::Ellipse);
        assert_eq!(result.color, "#0000ff");
        assert!((result.coverage - 1.0).abs() < 1e-9);
        assert!(!result.uneven);
        let as_rect = analyze(&src, &rect, Shape::Rect);
        assert!(as_rect.uneven);
    }

    #[test]
    fn gradients_are_reported_as_uneven() {
        let src = source(256, 8, |x, _| [x as u8, x as u8, x as u8]);
        let result = analyze(&src, &PixelRect::new(0, 0, 256, 8), Shape::Rect);
        assert!(result.uneven);
    }
}
