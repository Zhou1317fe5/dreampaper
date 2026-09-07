// @vitest-environment jsdom
import { act, createElement, useState, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { WorkbenchCanvas, type CanvasProps, type CropDraft } from './canvas';
import type { ProjectDoc } from './types';

interface DragEvent {
  target: { getStage: () => { getPointerPosition: () => { x: number; y: number } } };
}
interface NodeProps {
  children?: ReactNode;
  clipFunc?: (ctx: { rect: (x: number, y: number, width: number, height: number) => void }) => void;
  draggable?: boolean;
  stroke?: string;
  onDragStart?: (event: DragEvent) => void;
  onDragMove?: (event: DragEvent) => void;
  onDragEnd?: (event: DragEvent) => void;
}
const nodes = vi.hoisted(() => ({ groups: [] as NodeProps[], rects: [] as NodeProps[] }));
vi.mock('react-konva', async () => {
  const { createElement } = await import('react');
  const container = (props: NodeProps) => createElement('div', null, props.children);
  return {
    Stage: container, Layer: container,
    Group: (props: NodeProps) => { nodes.groups.push(props); return container(props); },
    Rect: (props: NodeProps) => { nodes.rects.push(props); return null; },
    Ellipse: () => null, Image: () => null, Line: () => null, Text: () => null, Transformer: () => null
  };
});

const crop = { x: 100, y: 80, width: 400, height: 300 };
const doc: ProjectDoc = {
  schema_version: 1, id: 'p', revision: 1, name: '测试',
  source: { asset_id: 'a', filename: 'a.png', width: 1000, height: 800, orientation_normalized: true, working_color_space: 'srgb' },
  viewport: { crop, flip_x: false, flip_y: false }, layers: [], created_at: '', updated_at: ''
};
let root: Root;
let host: HTMLDivElement;

beforeEach(() => {
  nodes.groups.length = 0;
  nodes.rects.length = 0;
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  vi.unstubAllGlobals();
});

function props(flipX: boolean, flipY: boolean, scale: number): CanvasProps {
  return {
    doc: { ...doc, viewport: { crop, flip_x: flipX, flip_y: flipY } },
    image: null, view: { x: 20, y: 40, scale }, onView: vi.fn(), fitRequest: 0,
    tool: 'select', selection: { id: null, parentId: null }, pending: null, cropDraft: null,
    showOriginal: false, compare: null, layouts: new Map(), onSelect: vi.fn(), onDraw: vi.fn(),
    onPendingRect: vi.fn(), onLayerRect: vi.fn(), onTextDraw: vi.fn(), onEyedrop: vi.fn(),
    onCropDraft: vi.fn(), onCompare: vi.fn()
  };
}

const flips = [[false, false], [true, false], [false, true], [true, true]] as const;

describe('cropped canvas interactions', () => {
  it.each(flips)('keeps comparison clipping aligned with the displayed line (flip %s/%s)', async (flipX, flipY) => {
    for (const fraction of [0, 0.25, 0.5, 1]) {
      nodes.groups.length = 0;
      await act(async () => root.render(createElement(WorkbenchCanvas, { ...props(flipX, flipY, 0.5), compare: fraction })));
      const clip = nodes.groups.find(group => group.clipFunc)?.clipFunc;
      const rect = vi.fn();
      expect(clip).toBeDefined();
      clip!({ rect });
      const [x, , width] = rect.mock.calls[0];
      const pivot = 2 * crop.x + crop.width;
      for (const sourceX of [100.5, 249.5, 250.5, 399.5, 499.5]) {
        const displayedX = flipX ? pivot - sourceX : sourceX;
        expect(sourceX >= x && sourceX < x + width).toBe(displayedX >= fraction * doc.source.width);
      }
    }
  });

  it.each(flips)('drags the unmirrored crop frame in display coordinates (flip %s/%s)', async (flipX, flipY) => {
    for (const scale of [0.1, 1, 3]) {
      const base = props(flipX, flipY, scale);
      let current = crop;
      function Harness() {
        const [draft, setDraft] = useState<CropDraft>({ crop, flip_x: flipX, flip_y: flipY, ratio: null });
        return createElement(WorkbenchCanvas, { ...base, tool: 'crop', cropDraft: draft, onCropDraft: next => {
          current = next;
          setDraft({ ...draft, crop: next });
        } });
      }
      nodes.rects.length = 0;
      await act(async () => root.render(createElement(Harness, { key: scale })));
      const event = (x: number, y: number) => ({ target: { getStage: () => ({
        getPointerPosition: () => ({ x: base.view.x + x * scale, y: base.view.y + y * scale })
      }) } });
      const frame = () => nodes.rects.filter(rect => rect.draggable && rect.stroke === '#ffffff').at(-1)!;
      await act(async () => frame().onDragStart!(event(150, 120)));
      await act(async () => frame().onDragMove!(event(173.4, 91.6)));
      expect(current).toEqual({ ...crop, x: 123, y: 52 });
      await act(async () => frame().onDragEnd!(event(180, 100)));
      expect(current).toEqual({ ...crop, x: 130, y: 60 });
    }
  });
});
