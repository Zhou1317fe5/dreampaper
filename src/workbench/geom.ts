// Source-pixel geometry. Mirrors src-tauri/src/core/workbench/geom.rs: the
// rounding, clamping and ellipse rules here must produce the same integers the
// Rust side does, so a committed rectangle means the same pixels everywhere.

import type { PixelRect } from './types';

export interface FloatRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Round a float rectangle (any corner order) into a committed integer one. */
export function normalizeRect(
  x0: number,
  y0: number,
  x1: number,
  y1: number,
  imageWidth: number,
  imageHeight: number
): PixelRect | null {
  if (imageWidth === 0 || imageHeight === 0 || ![x0, y0, x1, y1].every(Number.isFinite)) return null;
  const left = Math.min(x0, x1);
  const right = Math.max(x0, x1);
  const top = Math.min(y0, y1);
  const bottom = Math.max(y0, y1);
  if (left >= imageWidth || top >= imageHeight || right < 0 || bottom < 0) return null;
  let l = Math.round(left);
  let t = Math.round(top);
  let r = Math.round(right);
  let b = Math.round(bottom);
  if (r <= l) r = l + 1;
  if (b <= t) b = t + 1;
  l = clamp(l, 0, Math.max(0, imageWidth - 1));
  t = clamp(t, 0, Math.max(0, imageHeight - 1));
  r = clamp(r, l + 1, Math.max(imageWidth, l + 1));
  b = clamp(b, t + 1, Math.max(imageHeight, t + 1));
  if (l >= imageWidth || t >= imageHeight) return null;
  return { x: l, y: t, width: r - l, height: b - t };
}

export function roundRect(rect: FloatRect, imageWidth: number, imageHeight: number): PixelRect {
  return (
    normalizeRect(rect.x, rect.y, rect.x + rect.width, rect.y + rect.height, imageWidth, imageHeight) ?? {
      x: 0,
      y: 0,
      width: 1,
      height: 1
    }
  );
}

export function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

/** Move a crop frame by a displayed-coordinate drag while preserving its size. */
export function moveCrop(
  crop: PixelRect,
  start: { x: number; y: number },
  current: { x: number; y: number },
  imageWidth: number,
  imageHeight: number
): PixelRect {
  return {
    ...crop,
    x: clamp(Math.round(crop.x + current.x - start.x), 0, imageWidth - crop.width),
    y: clamp(Math.round(crop.y + current.y - start.y), 0, imageHeight - crop.height)
  };
}

export function rectsEqual(a: PixelRect, b: PixelRect): boolean {
  return a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;
}

/** Pixel-centre membership for the ellipse inscribed in `rect`. */
export function ellipseContains(rect: PixelRect, px: number, py: number): boolean {
  if (rect.width < 1 || rect.height < 1) return false;
  const rx = rect.width / 2;
  const ry = rect.height / 2;
  const dx = (px + 0.5 - (rect.x + rx)) / rx;
  const dy = (py + 0.5 - (rect.y + ry)) / ry;
  return dx * dx + dy * dy <= 1;
}

/** Hit test in continuous source coordinates (used by the canvas). */
export function pointInShape(shape: 'rect' | 'ellipse', rect: PixelRect, x: number, y: number): boolean {
  if (x < rect.x || y < rect.y || x > rect.x + rect.width || y > rect.y + rect.height) return false;
  if (shape === 'rect') return true;
  const rx = rect.width / 2;
  const ry = rect.height / 2;
  const dx = (x - (rect.x + rx)) / rx;
  const dy = (y - (rect.y + ry)) / ry;
  return dx * dx + dy * dy <= 1;
}

/** Keep `rect` inside the image by moving it, then shrinking if it must. */
export function containRect(rect: FloatRect, imageWidth: number, imageHeight: number): FloatRect {
  let { x, y, width, height } = rect;
  width = Math.min(Math.max(width, 1), imageWidth);
  height = Math.min(Math.max(height, 1), imageHeight);
  x = clamp(x, 0, imageWidth - width);
  y = clamp(y, 0, imageHeight - height);
  return { x, y, width, height };
}

/**
 * Map a child rectangle when its parent moves from `from` to `to`: the same
 * relative position and the same relative size.
 */
export function scaleWithin(child: FloatRect, from: PixelRect, to: FloatRect): FloatRect {
  const sx = to.width / from.width;
  const sy = to.height / from.height;
  return {
    x: to.x + (child.x - from.x) * sx,
    y: to.y + (child.y - from.y) * sy,
    width: child.width * sx,
    height: child.height * sy
  };
}

export type AspectPreset = 'free' | 'original' | '1:1' | '4:3' | '16:9' | '3:4' | '9:16' | 'custom';

export function aspectRatio(
  preset: AspectPreset,
  original: { width: number; height: number },
  custom: { width: number; height: number }
): number | null {
  switch (preset) {
    case 'free':
      return null;
    case 'original':
      return original.width / original.height;
    case 'custom':
      return custom.width > 0 && custom.height > 0 ? custom.width / custom.height : null;
    default: {
      const [w, h] = preset.split(':').map(Number);
      return w / h;
    }
  }
}

/**
 * Apply an aspect ratio to a crop rectangle by adjusting the axis the user
 * did not just edit, keeping the anchor edge fixed and the result inside the
 * image. Widths and heights come back as integers.
 */
export function constrainCrop(
  rect: FloatRect,
  ratio: number | null,
  imageWidth: number,
  imageHeight: number,
  changed: 'width' | 'height' | 'both' = 'both'
): PixelRect {
  let { x, y, width, height } = rect;
  if (ratio) {
    if (changed === 'height') {
      width = height * ratio;
    } else if (changed === 'width') {
      height = width / ratio;
    } else {
      // Keep whichever fits, shrinking the other.
      if (width / height > ratio) width = height * ratio;
      else height = width / ratio;
    }
    if (width > imageWidth) {
      width = imageWidth;
      height = width / ratio;
    }
    if (height > imageHeight) {
      height = imageHeight;
      width = height * ratio;
    }
  }
  width = Math.max(1, Math.round(width));
  height = Math.max(1, Math.round(height));
  const contained = containRect({ x, y, width, height }, imageWidth, imageHeight);
  return {
    x: Math.round(contained.x),
    y: Math.round(contained.y),
    width: Math.round(contained.width),
    height: Math.round(contained.height)
  };
}

/** Bounding box and clockwise angle (degrees) of a quadrilateral. */
export function polyBounds(poly: Array<{ x: number; y: number }>): { rect: FloatRect; angle: number } {
  const xs = poly.map((p) => p.x);
  const ys = poly.map((p) => p.y);
  const rect = {
    x: Math.min(...xs),
    y: Math.min(...ys),
    width: Math.max(...xs) - Math.min(...xs),
    height: Math.max(...ys) - Math.min(...ys)
  };
  let angle = 0;
  if (poly.length >= 2) {
    const a = poly[0];
    const b = poly[1];
    angle = (Math.atan2(b.y - a.y, b.x - a.x) * 180) / Math.PI;
    if (Math.abs(angle) < 1.5) angle = 0;
  }
  return { rect, angle };
}

export interface ViewTransform {
  scale: number;
  x: number;
  y: number;
}

export function toSource(view: ViewTransform, screenX: number, screenY: number): { x: number; y: number } {
  return { x: (screenX - view.x) / view.scale, y: (screenY - view.y) / view.scale };
}

export function fitView(
  image: { width: number; height: number },
  container: { width: number; height: number },
  padding = 24
): ViewTransform {
  const usableW = Math.max(container.width - padding * 2, 1);
  const usableH = Math.max(container.height - padding * 2, 1);
  const scale = Math.min(usableW / image.width, usableH / image.height, 8);
  return {
    scale,
    x: (container.width - image.width * scale) / 2,
    y: (container.height - image.height * scale) / 2
  };
}

export function zoomAt(view: ViewTransform, factor: number, pivotX: number, pivotY: number): ViewTransform {
  const scale = clamp(view.scale * factor, 0.02, 32);
  const source = toSource(view, pivotX, pivotY);
  return { scale, x: pivotX - source.x * scale, y: pivotY - source.y * scale };
}

export function flipPoint(point: { x: number; y: number }, width: number, height: number, flipX: boolean, flipY: boolean) {
  return { x: flipX ? width - point.x : point.x, y: flipY ? height - point.y : point.y };
}

/** Convert a point from the mirrored viewport back to source coordinates. */
export function unflipSourcePoint(
  point: { x: number; y: number },
  crop: PixelRect,
  flipX: boolean,
  flipY: boolean
) {
  const local = { x: point.x - crop.x, y: point.y - crop.y };
  const mapped = flipPoint(local, crop.width, crop.height, flipX, flipY);
  return { x: mapped.x + crop.x, y: mapped.y + crop.y };
}
