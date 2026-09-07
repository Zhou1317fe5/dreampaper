//! Source-pixel geometry shared by analysis, compositing and the frontend.
//!
//! Every committed rectangle in a project is an integer rectangle in the
//! orientation-normalized source image. The frontend keeps floats while a
//! drag is in flight and calls the same rules as `normalize` before it
//! commits, so both sides agree on rounding, minimum size and clamping. The
//! ellipse membership rule below is the one test vector both renderers share.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PixelRect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl PixelRect {
    pub const fn new(x: i64, y: i64, width: i64, height: i64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn right(&self) -> i64 {
        self.x + self.width
    }

    pub const fn bottom(&self) -> i64 {
        self.y + self.height
    }

    pub fn is_valid(&self) -> bool {
        self.width >= 1 && self.height >= 1
    }

    /// Whether this rectangle lies entirely inside a `width`×`height` image.
    pub fn within(&self, width: u32, height: u32) -> bool {
        self.is_valid()
            && self.x >= 0
            && self.y >= 0
            && self.right() <= i64::from(width)
            && self.bottom() <= i64::from(height)
    }

    /// Intersection with an image of the given size, if any pixels remain.
    pub fn clamped(&self, width: u32, height: u32) -> Option<Self> {
        let x0 = self.x.max(0);
        let y0 = self.y.max(0);
        let x1 = self.right().min(i64::from(width));
        let y1 = self.bottom().min(i64::from(height));
        (x1 > x0 && y1 > y0).then(|| Self::new(x0, y0, x1 - x0, y1 - y0))
    }
}

/// Turn a floating rectangle (any corner order) into a committed integer one.
///
/// Edges round to the nearest pixel boundary, the result is clamped to the
/// image and is at least 1×1. Returns `None` when nothing of it is inside.
pub fn normalize(x0: f64, y0: f64, x1: f64, y1: f64, width: u32, height: u32) -> Option<PixelRect> {
    if width == 0 || height == 0 || ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) {
        return None;
    }
    let (left, right) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
    let (top, bottom) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
    if left >= f64::from(width) || top >= f64::from(height) || right < 0.0 || bottom < 0.0 {
        return None;
    }
    let mut l = left.round() as i64;
    let mut t = top.round() as i64;
    let mut r = right.round() as i64;
    let mut b = bottom.round() as i64;
    if r <= l {
        r = l + 1;
    }
    if b <= t {
        b = t + 1;
    }
    let w = i64::from(width);
    let h = i64::from(height);
    l = l.clamp(0, (w - 1).max(0));
    t = t.clamp(0, (h - 1).max(0));
    r = r.clamp(l + 1, w.max(l + 1));
    b = b.clamp(t + 1, h.max(t + 1));
    if l >= w || t >= h {
        return None;
    }
    Some(PixelRect::new(l, t, r - l, b - t))
}

/// Pixel-centre membership test for an ellipse inscribed in `rect`.
///
/// A pixel belongs to the ellipse when its centre `(px + 0.5, py + 0.5)` is
/// inside the inscribed ellipse. Used for colour sampling (exact pixels) and
/// mirrored by the frontend's hit test; the anti-aliased fill in compositing
/// uses the same ellipse so the two disagree only on boundary coverage.
pub fn ellipse_contains(rect: &PixelRect, px: i64, py: i64) -> bool {
    if !rect.is_valid() {
        return false;
    }
    let rx = rect.width as f64 / 2.0;
    let ry = rect.height as f64 / 2.0;
    let cx = rect.x as f64 + rx;
    let cy = rect.y as f64 + ry;
    let dx = (px as f64 + 0.5 - cx) / rx;
    let dy = (py as f64 + 0.5 - cy) / ry;
    dx * dx + dy * dy <= 1.0
}

/// Map a rectangle through the export viewport: crop translation, then flips.
///
/// Flipping is a pure pixel permutation, so a rectangle maps to a rectangle.
/// Returns `None` for rectangles that fall completely outside the crop.
pub fn viewport_rect(
    rect: &PixelRect,
    crop: &PixelRect,
    flip_x: bool,
    flip_y: bool,
) -> Option<PixelRect> {
    let x0 = rect.x.max(crop.x);
    let y0 = rect.y.max(crop.y);
    let x1 = rect.right().min(crop.right());
    let y1 = rect.bottom().min(crop.bottom());
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let mut out = PixelRect::new(x0 - crop.x, y0 - crop.y, x1 - x0, y1 - y0);
    if flip_x {
        out.x = crop.width - out.right();
    }
    if flip_y {
        out.y = crop.height - out.bottom();
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rounds_clamps_and_keeps_a_minimum_size() {
        assert_eq!(
            normalize(10.4, 20.6, 30.5, 40.49, 100, 100),
            Some(PixelRect::new(10, 21, 21, 19))
        );
        // Reverse corner order is fine.
        assert_eq!(
            normalize(30.0, 40.0, 10.0, 20.0, 100, 100),
            Some(PixelRect::new(10, 20, 20, 20))
        );
        // Degenerate drags become one pixel.
        assert_eq!(
            normalize(5.2, 5.2, 5.3, 5.3, 100, 100),
            Some(PixelRect::new(5, 5, 1, 1))
        );
        // Clamped to the image.
        assert_eq!(
            normalize(-20.0, -20.0, 10.0, 10.0, 100, 100),
            Some(PixelRect::new(0, 0, 10, 10))
        );
        assert_eq!(
            normalize(90.0, 90.0, 400.0, 400.0, 100, 100),
            Some(PixelRect::new(90, 90, 10, 10))
        );
        // Fully outside.
        assert_eq!(normalize(200.0, 200.0, 300.0, 300.0, 100, 100), None);
        assert_eq!(normalize(-20.0, -20.0, -10.0, -10.0, 100, 100), None);
        assert_eq!(normalize(f64::NAN, 0.0, 10.0, 10.0, 100, 100), None);
        assert_eq!(normalize(0.0, 0.0, 10.0, 10.0, 0, 100), None);
    }

    #[test]
    fn ellipse_membership_uses_pixel_centres() {
        let rect = PixelRect::new(0, 0, 10, 6);
        assert!(ellipse_contains(&rect, 5, 3));
        assert!(ellipse_contains(&rect, 0, 3) || ellipse_contains(&rect, 1, 3));
        assert!(!ellipse_contains(&rect, 0, 0));
        assert!(!ellipse_contains(&rect, 9, 5));
        // Shared golden vector: a 4x4 ellipse covers exactly the centre 2x2
        // plus the four edge-midpoint pixels.
        let small = PixelRect::new(0, 0, 4, 4);
        let inside: Vec<(i64, i64)> = (0..4)
            .flat_map(|y| (0..4).map(move |x| (x, y)))
            .filter(|(x, y)| ellipse_contains(&small, *x, *y))
            .collect();
        assert_eq!(
            inside,
            vec![
                (1, 0),
                (2, 0),
                (0, 1),
                (1, 1),
                (2, 1),
                (3, 1),
                (0, 2),
                (1, 2),
                (2, 2),
                (3, 2),
                (1, 3),
                (2, 3)
            ]
        );
    }

    #[test]
    fn viewport_mapping_crops_then_flips() {
        let crop = PixelRect::new(10, 10, 100, 50);
        let rect = PixelRect::new(20, 15, 30, 10);
        assert_eq!(
            viewport_rect(&rect, &crop, false, false),
            Some(PixelRect::new(10, 5, 30, 10))
        );
        assert_eq!(
            viewport_rect(&rect, &crop, true, false),
            Some(PixelRect::new(60, 5, 30, 10))
        );
        assert_eq!(
            viewport_rect(&rect, &crop, false, true),
            Some(PixelRect::new(10, 35, 30, 10))
        );
        // Partially outside is clipped; fully outside is dropped.
        assert_eq!(
            viewport_rect(&PixelRect::new(0, 0, 20, 20), &crop, false, false),
            Some(PixelRect::new(0, 0, 10, 10))
        );
        assert_eq!(
            viewport_rect(&PixelRect::new(200, 200, 5, 5), &crop, false, false),
            None
        );
    }
}
