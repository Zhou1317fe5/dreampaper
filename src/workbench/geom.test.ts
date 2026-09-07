import { describe, expect, it } from 'vitest';
import { constrainCrop, ellipseContains, fitView, flipPoint, moveCrop, normalizeRect, polyBounds, scaleWithin, toSource, unflipSourcePoint, zoomAt } from './geom';

describe('normalizeRect', () => {
  // Same vectors as geom.rs: both renderers must agree on these integers.
  it('rounds, clamps and keeps a minimum size', () => {
    expect(normalizeRect(10.4, 20.6, 30.5, 40.49, 100, 100)).toEqual({ x: 10, y: 21, width: 21, height: 19 });
    expect(normalizeRect(30, 40, 10, 20, 100, 100)).toEqual({ x: 10, y: 20, width: 20, height: 20 });
    expect(normalizeRect(5.2, 5.2, 5.3, 5.3, 100, 100)).toEqual({ x: 5, y: 5, width: 1, height: 1 });
    expect(normalizeRect(-20, -20, 10, 10, 100, 100)).toEqual({ x: 0, y: 0, width: 10, height: 10 });
    expect(normalizeRect(90, 90, 400, 400, 100, 100)).toEqual({ x: 90, y: 90, width: 10, height: 10 });
    expect(normalizeRect(200, 200, 300, 300, 100, 100)).toBeNull();
  });
});

describe('ellipseContains', () => {
  it('matches the shared 4x4 golden vector', () => {
    const rect = { x: 0, y: 0, width: 4, height: 4 };
    const inside: Array<[number, number]> = [];
    for (let y = 0; y < 4; y += 1) for (let x = 0; x < 4; x += 1) if (ellipseContains(rect, x, y)) inside.push([x, y]);
    expect(inside).toEqual([[1, 0], [2, 0], [0, 1], [1, 1], [2, 1], [3, 1], [0, 2], [1, 2], [2, 2], [3, 2], [1, 3], [2, 3]]);
  });
});

describe('scaleWithin', () => {
  it('keeps relative position and size', () => {
    const child = { x: 20, y: 15, width: 30, height: 10 };
    const from = { x: 10, y: 10, width: 100, height: 50 };
    const to = { x: 110, y: 20, width: 200, height: 100 };
    expect(scaleWithin(child, from, to)).toEqual({ x: 130, y: 30, width: 60, height: 20 });
  });
});

describe('constrainCrop', () => {
  it('locks the aspect ratio on the axis the user did not edit', () => {
    expect(constrainCrop({ x: 0, y: 0, width: 400, height: 100 }, 16 / 9, 4000, 3000, 'width')).toEqual({
      x: 0,
      y: 0,
      width: 400,
      height: 225
    });
    expect(constrainCrop({ x: 0, y: 0, width: 400, height: 180 }, 16 / 9, 4000, 3000, 'height')).toEqual({
      x: 0,
      y: 0,
      width: 320,
      height: 180
    });
  });
  it('keeps the crop inside the image', () => {
    expect(constrainCrop({ x: 3900, y: 2900, width: 400, height: 300 }, null, 4000, 3000)).toEqual({
      x: 3600,
      y: 2700,
      width: 400,
      height: 300
    });
    const huge = constrainCrop({ x: 0, y: 0, width: 9000, height: 100 }, 1, 4000, 3000, 'both');
    expect(huge.width).toBe(huge.height);
    expect(huge.width).toBeLessThanOrEqual(3000);
  });
});

describe('polyBounds', () => {
  it('returns the bbox and a snapped angle', () => {
    const level = polyBounds([{ x: 10, y: 10 }, { x: 110, y: 10.5 }, { x: 110, y: 30 }, { x: 10, y: 30 }]);
    expect(level.rect).toEqual({ x: 10, y: 10, width: 100, height: 20 });
    expect(level.angle).toBe(0);
    const tilted = polyBounds([{ x: 0, y: 0 }, { x: 100, y: 100 }, { x: 90, y: 110 }, { x: -10, y: 10 }]);
    expect(tilted.angle).toBeCloseTo(45);
  });
});

describe('view transform', () => {
  it('fits the image centred and zooms about the pointer', () => {
    const view = fitView({ width: 2000, height: 1000 }, { width: 1024, height: 768 }, 12);
    expect(view.scale).toBeCloseTo(0.5);
    expect(view.x).toBeCloseTo(12);
    const before = toSource(view, 300, 200);
    const zoomed = zoomAt(view, 2, 300, 200);
    const after = toSource(zoomed, 300, 200);
    expect(after.x).toBeCloseTo(before.x);
    expect(after.y).toBeCloseTo(before.y);
    expect(zoomed.scale).toBeCloseTo(1);
  });
});

it('maps flipped screen coordinates back to source pixels at every zoom', () => {
  const source = { x: 123, y: 456 };
  for (const scale of [0.1, 1, 3]) {
    for (const flipX of [false, true]) for (const flipY of [false, true]) {
      const shown = flipPoint(source, 1000, 600, flipX, flipY);
      const view = { x: 10, y: 20, scale };
      const displayed = toSource(view, shown.x * scale + view.x, shown.y * scale + view.y);
      const hit = flipPoint(displayed, 1000, 600, flipX, flipY);
      expect(hit.x).toBeCloseTo(source.x);
      expect(hit.y).toBeCloseTo(source.y);
    }
  }
});

it('moves a crop by integer source pixels and clamps it to the image', () => {
  const crop = { x: 100, y: 80, width: 400, height: 300 };
  expect(moveCrop(crop, { x: 150, y: 120 }, { x: 173.4, y: 91.6 }, 1000, 800)).toEqual({ x: 123, y: 52, width: 400, height: 300 });
  expect(moveCrop(crop, { x: 150, y: 120 }, { x: -500, y: -500 }, 1000, 800)).toEqual({ x: 0, y: 0, width: 400, height: 300 });
  expect(moveCrop(crop, { x: 150, y: 120 }, { x: 2000, y: 2000 }, 1000, 800)).toEqual({ x: 600, y: 500, width: 400, height: 300 });
});

it('mirrors points within a cropped viewport without moving the crop', () => {
  const crop = { x: 200, y: 100, width: 400, height: 300 };
  for (const flipX of [false, true]) for (const flipY of [false, true]) {
    const source = { x: 275, y: 325 };
    const displayed = flipPoint(
      { x: source.x - crop.x, y: source.y - crop.y },
      crop.width,
      crop.height,
      flipX,
      flipY
    );
    const restored = unflipSourcePoint({ x: displayed.x + crop.x, y: displayed.y + crop.y }, crop, flipX, flipY);
    expect(restored).toEqual(source);
  }
});
