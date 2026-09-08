// The interaction canvas. Konva draws the preview and provides hit testing and
// transform handles; every coordinate that leaves this component is in source
// pixels, and nothing drawn here is ever exported.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type Konva from 'konva';
import { Ellipse, Group, Image as KonvaImage, Layer as KonvaLayer, Line, Rect, Stage, Text as KonvaText, Transformer } from 'react-konva';
import { clamp, containRect, constrainCrop, fitView, moveCrop, normalizeRect, roundRect, toSource, unflipSourcePoint, zoomAt, type FloatRect, type ViewTransform } from './geom';
import type { PendingRepair, Selection, Tool } from './state';
import type { Layer, PixelRect, ProjectDoc, RepairGroup, TextLayer, TextLayout } from './types';

export interface CropDraft {
  crop: PixelRect;
  flip_x: boolean;
  flip_y: boolean;
  ratio: number | null;
}

export interface CanvasProps {
  doc: ProjectDoc;
  image: HTMLImageElement | null;
  view: ViewTransform;
  onView: (view: ViewTransform) => void;
  /** Bump to fit the image into the container (also fires once per image). */
  fitRequest: number;
  tool: Tool;
  selection: Selection;
  pending: PendingRepair | null;
  cropDraft: CropDraft | null;
  showOriginal: boolean;
  compare: number | null;
  layouts: ReadonlyMap<string, TextLayout>;
  onSelect: (id: string | null, parentId: string | null) => void;
  onDraw: (shape: 'rect' | 'ellipse', rect: PixelRect) => void;
  onPendingRect: (rect: PixelRect) => void;
  onLayerRect: (id: string, rect: PixelRect, phase: 'move' | 'end') => void;
  onTextDraw: (rect: PixelRect) => void;
  onEyedrop: (x: number, y: number) => void;
  onCropDraft: (crop: PixelRect) => void;
  onCompare: (fraction: number) => void;
}

export const PENDING_ID = '__pending__';
type Handle = 'nw' | 'n' | 'ne' | 'e' | 'se' | 's' | 'sw' | 'w';
const HANDLES: Handle[] = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];

// `fauxItalic` means Rust found no italic face and shears the glyphs itself;
// the node is then skewed instead of asking the browser for an italic it
// may not synthesise for CJK families.
function fontStyle(text: TextLayer, fauxItalic = false): string {
  const parts: string[] = [];
  if (text.font.italic && !fauxItalic) parts.push('italic');
  if (text.font.weight >= 600) parts.push('bold');
  else if (text.font.weight !== 400) parts.push(String(text.font.weight));
  return parts.length ? parts.join(' ') : 'normal';
}

export function WorkbenchCanvas(props: CanvasProps) {
  const {
    doc,
    image,
    view,
    onView,
    fitRequest,
    tool,
    selection,
    pending,
    cropDraft,
    showOriginal,
    compare,
    layouts,
    onSelect,
    onDraw,
    onPendingRect,
    onLayerRect,
    onTextDraw,
    onEyedrop,
    onCropDraft,
    onCompare
  } = props;
  const { width: W, height: H } = doc.source;
  const viewport = cropDraft ?? doc.viewport;
  const displayedPoint = (screen: { x: number; y: number }) => toSource(view, screen.x, screen.y);
  const sourcePoint = (screen: { x: number; y: number }) => {
    const displayed = displayedPoint(screen);
    return unflipSourcePoint(displayed, viewport.crop, viewport.flip_x, viewport.flip_y);
  };
  const clip = cropDraft ? {} : { clipX: doc.viewport.crop.x, clipY: doc.viewport.crop.y, clipWidth: doc.viewport.crop.width, clipHeight: doc.viewport.crop.height };
  const container = useRef<HTMLDivElement>(null);
  const stageRef = useRef<Konva.Stage>(null);
  const transformer = useRef<Konva.Transformer>(null);
  const nodes = useRef(new Map<string, Konva.Group>());
  const [size, setSize] = useState({ width: 800, height: 600 });
  const [rubber, setRubber] = useState<{ shape: 'rect' | 'ellipse' | 'text'; start: { x: number; y: number }; current: { x: number; y: number }; shift: boolean } | null>(null);
  const [spaceHeld, setSpaceHeld] = useState(false);
  const panning = useRef<{ x: number; y: number; view: ViewTransform } | null>(null);

  useEffect(() => {
    const element = container.current;
    if (!element) return;
    const observer = new ResizeObserver((entries) => {
      const box = entries[0]?.contentRect;
      if (box && box.width > 0 && box.height > 0) setSize({ width: Math.floor(box.width), height: Math.floor(box.height) });
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const lastFit = useRef(0);
  useEffect(() => {
    if (fitRequest === lastFit.current || size.width === 0) return;
    lastFit.current = fitRequest;
    onView(fitView({ width: W, height: H }, size));
  }, [fitRequest, size, W, H, onView]);

  useEffect(() => {
    const down = (event: KeyboardEvent) => {
      if (event.code === 'Space' && !(event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement)) {
        setSpaceHeld(true);
        event.preventDefault();
      }
    };
    const up = (event: KeyboardEvent) => {
      if (event.code === 'Space') setSpaceHeld(false);
    };
    window.addEventListener('keydown', down);
    window.addEventListener('keyup', up);
    return () => {
      window.removeEventListener('keydown', down);
      window.removeEventListener('keyup', up);
    };
  }, []);

  // Attach the transformer to whatever is selected (or the pending preview).
  const selectedNodeId = pending ? PENDING_ID : selection.id;
  useEffect(() => {
    const tr = transformer.current;
    if (!tr) return;
    const node = selectedNodeId ? nodes.current.get(selectedNodeId) : undefined;
    if (node && tool === 'select' && !cropDraft && !showOriginal) {
      tr.nodes([node]);
    } else {
      tr.nodes([]);
    }
    tr.getLayer()?.batchDraw();
  }, [selectedNodeId, tool, cropDraft, doc, pending, showOriginal]);

  const registerNode = useCallback((id: string) => (node: Konva.Group | null) => {
    if (node) nodes.current.set(id, node);
    else nodes.current.delete(id);
  }, []);

  const stagePointer = (): { x: number; y: number } | null => stageRef.current?.getPointerPosition() ?? null;

  const clampToImage = (p: { x: number; y: number }) => ({ x: clamp(p.x, 0, W), y: clamp(p.y, 0, H) });

  // ---- stage-level pointer handling ----
  const onPointerDown = (event: Konva.KonvaEventObject<PointerEvent>) => {
    const evt = event.evt;
    const screen = stagePointer();
    if (!screen) return;
    if (spaceHeld || evt.button === 1) {
      panning.current = { x: screen.x, y: screen.y, view };
      return;
    }
    if (evt.button !== 0) return;
    const onEmpty = event.target === event.target.getStage() || event.target.name() === 'backdrop';
    const source = clampToImage(sourcePoint(screen));
    if (cropDraft) return;
    if (tool === 'rect' || tool === 'ellipse') {
      setRubber({ shape: tool, start: source, current: source, shift: evt.shiftKey });
      return;
    }
    if (tool === 'text') {
      setRubber({ shape: 'text', start: source, current: source, shift: evt.shiftKey });
      return;
    }
    if (tool === 'eyedropper') {
      if (source.x >= 0 && source.y >= 0 && source.x < W && source.y < H) onEyedrop(Math.floor(source.x), Math.floor(source.y));
      return;
    }
    if (onEmpty && tool === 'select') onSelect(null, null);
  };

  const onPointerMove = () => {
    const screen = stagePointer();
    if (!screen) return;
    if (panning.current) {
      const start = panning.current;
      onView({ ...start.view, x: start.view.x + (screen.x - start.x), y: start.view.y + (screen.y - start.y) });
      return;
    }
    if (rubber) {
      setRubber({ ...rubber, current: clampToImage(sourcePoint(screen)) });
    }
  };

  const onPointerUp = (event: Konva.KonvaEventObject<PointerEvent>) => {
    if (panning.current) {
      panning.current = null;
      return;
    }
    if (!rubber) return;
    const shift = rubber.shift || event.evt.shiftKey;
    const box = rubberRect(rubber.start, rubber.current, shift, W, H);
    setRubber(null);
    const screenSize = Math.max(box.width, box.height) * view.scale;
    if (screenSize < 3) return;
    const rect = normalizeRect(box.x, box.y, box.x + box.width, box.y + box.height, W, H);
    if (!rect) return;
    if (rubber.shape === 'text') onTextDraw(rect);
    else onDraw(rubber.shape, rect);
  };

  const onWheel = (event: Konva.KonvaEventObject<WheelEvent>) => {
    event.evt.preventDefault();
    const screen = stagePointer();
    if (!screen) return;
    const { deltaX, deltaY, ctrlKey, metaKey, shiftKey } = event.evt;
    if (ctrlKey || metaKey) {
      const factor = Math.exp(-deltaY * 0.01);
      onView(zoomAt(view, factor, screen.x, screen.y));
    } else if (shiftKey) {
      onView({ ...view, x: view.x - deltaY - deltaX });
    } else {
      onView({ ...view, x: view.x - deltaX, y: view.y - deltaY });
    }
  };

  // ---- layer node transforms ----
  const rectFromNode = (node: Konva.Group, base: PixelRect, angle: number): FloatRect => {
    const sx = node.scaleX();
    const sy = node.scaleY();
    const width = base.width * sx;
    const height = base.height * sy;
    if (angle) {
      return { x: node.x() - width / 2, y: node.y() - height / 2, width, height };
    }
    return { x: node.x(), y: node.y(), width, height };
  };

  const handleDragMove = (id: string, base: PixelRect, angle: number) => (event: Konva.KonvaEventObject<DragEvent>) => {
    const node = event.target as Konva.Group;
    const rect = containRect(rectFromNode(node, base, angle), W, H);
    if (angle) node.position({ x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 });
    else node.position({ x: rect.x, y: rect.y });
    if (id === PENDING_ID) return;
    onLayerRect(id, { x: rect.x, y: rect.y, width: rect.width, height: rect.height }, 'move');
  };

  const handleDragEnd = (id: string, base: PixelRect, angle: number) => (event: Konva.KonvaEventObject<DragEvent>) => {
    const node = event.target as Konva.Group;
    const rect = roundRect(containRect(rectFromNode(node, base, angle), W, H), W, H);
    node.scale({ x: 1, y: 1 });
    if (id === PENDING_ID) onPendingRect(rect);
    else onLayerRect(id, rect, 'end');
  };

  const handleTransformEnd = (id: string, base: PixelRect, angle: number) => (event: Konva.KonvaEventObject<Event>) => {
    const node = event.target as Konva.Group;
    const rect = roundRect(containRect(rectFromNode(node, base, angle), W, H), W, H);
    node.scale({ x: 1, y: 1 });
    if (angle) node.position({ x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 });
    else node.position({ x: rect.x, y: rect.y });
    if (id === PENDING_ID) onPendingRect(rect);
    else onLayerRect(id, rect, 'end');
  };

  const interactive = tool === 'select' && !cropDraft && !showOriginal;

  const textNode = (text: TextLayer, parentId: string | null, preview = false) => {
    const layout = layouts.get(text.id);
    const rotated = text.angle !== 0;
    const groupProps = rotated
      ? { x: text.rect.x + text.rect.width / 2, y: text.rect.y + text.rect.height / 2, offsetX: text.rect.width / 2, offsetY: text.rect.height / 2, rotation: text.angle }
      : { x: text.rect.x, y: text.rect.y };
    const selected = selection.id === text.id && !pending;
    return (
      <Group
        key={text.id}
        ref={preview ? undefined : registerNode(text.id)}
        {...groupProps}
        draggable={interactive && selected && !preview}
        listening={interactive && !preview}
        visible={text.visible}
        onPointerDown={(event) => {
          if (!interactive || preview) return;
          event.cancelBubble = true;
          onSelect(text.id, parentId);
        }}
        onDragMove={handleDragMove(text.id, text.rect, text.angle)}
        onDragEnd={handleDragEnd(text.id, text.rect, text.angle)}
        onTransformEnd={handleTransformEnd(text.id, text.rect, text.angle)}
      >
        <Rect width={text.rect.width} height={text.rect.height} fill="rgba(0,0,0,0.001)" name="hit" />
        {layout ? (
          layout.lines.map((line, index) => (
            <KonvaText
              key={index}
              // Shear around the baseline like the export does: Konva skews
              // about the node origin, so the origin moves right by the
              // baseline's share to keep the baseline in place.
              x={line.x + (layout.synthetic_italic > 0 ? layout.synthetic_italic * layout.font_size * 0.8 : 0)}
              y={layout.offset_y + line.y}
              skewX={layout.synthetic_italic > 0 ? -layout.synthetic_italic : 0}
              text={line.text}
              fontFamily={`${layout.family_used || text.font.family}, ${text.font.family}, "Noto Sans SC", sans-serif`}
              fontSize={layout.font_size}
              fontStyle={fontStyle(text, layout.synthetic_italic > 0)}
              fill={text.color}
              // Rust emboldens faces without a bold cut by overprinting; a
              // stroke of the same width keeps the preview in step.
              stroke={layout.synthetic_bold > 0 ? text.color : undefined}
              strokeWidth={layout.synthetic_bold > 0 ? layout.synthetic_bold : 0}
              fillAfterStrokeEnabled
              letterSpacing={text.letter_spacing}
              lineHeight={text.line_height}
              wrap="none"
              listening={false}
              opacity={preview ? 0.9 : 1}
            />
          ))
        ) : (
          <KonvaText
            width={text.rect.width}
            height={text.rect.height}
            text={text.text}
            fontFamily={`${text.font.family}, "Noto Sans SC", sans-serif`}
            fontSize={text.font.size}
            fontStyle={fontStyle(text)}
            fill={text.color}
            align={text.align}
            verticalAlign={text.valign}
            letterSpacing={text.letter_spacing}
            lineHeight={text.line_height}
            wrap="word"
            listening={false}
            opacity={preview ? 0.9 : 1}
          />
        )}
      </Group>
    );
  };

  const fillNode = (id: string, shape: 'rect' | 'ellipse', rect: PixelRect, color: string, selectable: boolean, preview: boolean) => {
    const selected = preview || (selection.id === id && !pending);
    return (
      <Group
        key={id}
        ref={registerNode(id)}
        x={rect.x}
        y={rect.y}
        draggable={(interactive && selected) || (preview && !showOriginal && !cropDraft)}
        listening={(interactive && selectable) || preview}
        onPointerDown={(event) => {
          if (!interactive && !preview) return;
          event.cancelBubble = true;
          if (!preview) onSelect(id, null);
        }}
        onDragMove={handleDragMove(id, rect, 0)}
        onDragEnd={handleDragEnd(id, rect, 0)}
        onTransformEnd={handleTransformEnd(id, rect, 0)}
      >
        {shape === 'rect' ? (
          <Rect width={rect.width} height={rect.height} fill={color} strokeEnabled={preview} stroke="#b45df2" dash={[6 / view.scale, 4 / view.scale]} strokeWidth={1.5 / view.scale} />
        ) : (
          <Ellipse x={rect.width / 2} y={rect.height / 2} radiusX={rect.width / 2} radiusY={rect.height / 2} fill={color} strokeEnabled={preview} stroke="#b45df2" dash={[6 / view.scale, 4 / view.scale]} strokeWidth={1.5 / view.scale} />
        )}
      </Group>
    );
  };

  const renderLayer = (layer: Layer) => {
    if (layer.kind === 'repair') {
      const group = layer as RepairGroup;
      if (!group.visible) return null;
      return [fillNode(group.id, group.shape, group.rect, group.fill.current, true, false), ...group.children.map((child) => textNode(child, group.id))];
    }
    return textNode(layer, null);
  };

  const compareClip = useMemo(() => {
    if (compare === null) return undefined;
    const cx = compare * W;
    return (ctx: Konva.Context) => {
      if (viewport.flip_x) ctx.rect(0, 0, clamp(2 * viewport.crop.x + viewport.crop.width - cx, 0, W), H);
      else ctx.rect(cx, 0, W - cx, H);
    };
  }, [compare, W, H, viewport.flip_x, viewport.crop.x, viewport.crop.width]);

  const flip = cropDraft ? { x: cropDraft.flip_x, y: cropDraft.flip_y } : { x: doc.viewport.flip_x, y: doc.viewport.flip_y };
  const flipCrop = cropDraft?.crop ?? doc.viewport.crop;
  const flipProps = {
    scaleX: flip.x ? -1 : 1,
    scaleY: flip.y ? -1 : 1,
    x: flip.x ? flipCrop.x * 2 + flipCrop.width : 0,
    y: flip.y ? flipCrop.y * 2 + flipCrop.height : 0
  };

  const rubberBox = rubber ? rubberRect(rubber.start, rubber.current, rubber.shift, W, H) : null;
  const cursor = spaceHeld ? 'grab' : tool === 'eyedropper' ? 'crosshair' : tool === 'select' ? 'default' : 'crosshair';

  return (
    <div ref={container} className="wb-canvas" style={{ cursor }}>
      <Stage
        ref={stageRef}
        width={size.width}
        height={size.height}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerLeave={() => {
          panning.current = null;
        }}
        onWheel={onWheel}
      >
        <KonvaLayer listening={false}>
          <Rect x={0} y={0} width={size.width} height={size.height} fill="transparent" name="backdrop" />
          <Group x={view.x} y={view.y} scaleX={view.scale} scaleY={view.scale}>
            <Group {...flipProps} {...clip}>
              <Rect width={W} height={H} fill="#ffffff" />
              {image && <KonvaImage image={image} width={W} height={H} />}
              {!image && <Rect width={W} height={H} fill="#ddd" />}
            </Group>
          </Group>
        </KonvaLayer>

        <KonvaLayer>
          <Group x={view.x} y={view.y} scaleX={view.scale} scaleY={view.scale}>
            <Group {...flipProps} {...clip}>
              <Group clipFunc={compareClip} visible={!showOriginal}>
                {doc.layers.map(renderLayer)}
                {pending && pending.analysis && fillNode(PENDING_ID, pending.shape, pending.rect, pending.analysis.color, true, true)}
                {pending && !pending.analysis && fillNode(PENDING_ID, pending.shape, pending.rect, 'rgba(180,93,242,0.15)', true, true)}
                {pending && pending.texts.map((text) => textNode(text, PENDING_ID, true))}
              </Group>
            </Group>
          </Group>
        </KonvaLayer>

        <KonvaLayer>
          <Group x={view.x} y={view.y} scaleX={view.scale} scaleY={view.scale}>
            {cropDraft && (
              <CropOverlay
                draft={cropDraft}
                imageWidth={W}
                imageHeight={H}
                scale={view.scale}
                toDisplayedPoint={displayedPoint}
                onChange={onCropDraft}
              />
            )}
            <Group {...flipProps}>
              {!cropDraft && (
                <Rect x={doc.viewport.crop.x} y={doc.viewport.crop.y} width={doc.viewport.crop.width} height={doc.viewport.crop.height} stroke="rgba(180,93,242,0.6)" strokeWidth={1 / view.scale} listening={false} visible={doc.viewport.crop.width !== W || doc.viewport.crop.height !== H} />
              )}
              {rubberBox && (
                rubber?.shape === 'ellipse' ? (
                  <Ellipse x={rubberBox.x + rubberBox.width / 2} y={rubberBox.y + rubberBox.height / 2} radiusX={rubberBox.width / 2} radiusY={rubberBox.height / 2} stroke="#b45df2" strokeWidth={1.5 / view.scale} dash={[6 / view.scale, 4 / view.scale]} listening={false} />
                ) : (
                  <Rect {...rubberBox} stroke="#b45df2" strokeWidth={1.5 / view.scale} dash={[6 / view.scale, 4 / view.scale]} listening={false} />
                )
              )}
            </Group>
          </Group>
          <Transformer
            ref={transformer}
            rotateEnabled={false}
            flipEnabled={false}
            ignoreStroke
            keepRatio={false}
            anchorSize={8}
            borderStroke="#b45df2"
            anchorStroke="#b45df2"
            anchorFill="#ffffff"
            boundBoxFunc={(oldBox, newBox) => (newBox.width < 2 || newBox.height < 2 ? oldBox : newBox)}
          />
          {compare !== null && (
            <CompareLine fraction={compare} view={view} imageWidth={W} imageHeight={H} stageHeight={size.height} onChange={onCompare} />
          )}
        </KonvaLayer>
      </Stage>
    </div>
  );
}

function rubberRect(start: { x: number; y: number }, current: { x: number; y: number }, square: boolean, W: number, H: number): FloatRect {
  let dx = current.x - start.x;
  let dy = current.y - start.y;
  if (square) {
    const side = Math.max(Math.abs(dx), Math.abs(dy));
    dx = Math.sign(dx || 1) * side;
    dy = Math.sign(dy || 1) * side;
  }
  const x = clamp(Math.min(start.x, start.x + dx), 0, W);
  const y = clamp(Math.min(start.y, start.y + dy), 0, H);
  const right = clamp(Math.max(start.x, start.x + dx), 0, W);
  const bottom = clamp(Math.max(start.y, start.y + dy), 0, H);
  return { x, y, width: right - x, height: bottom - y };
}

function CropOverlay({
  draft,
  imageWidth,
  imageHeight,
  scale,
  toDisplayedPoint,
  onChange
}: {
  draft: CropDraft;
  imageWidth: number;
  imageHeight: number;
  scale: number;
  toDisplayedPoint: (screen: { x: number; y: number }) => { x: number; y: number };
  onChange: (crop: PixelRect) => void;
}) {
  const { crop } = draft;
  const handleSize = 10 / scale;
  const dragStart = useRef<{ crop: PixelRect; displayed: { x: number; y: number } } | null>(null);
  const displayedPointer = (event: Konva.KonvaEventObject<DragEvent>) => {
    const position = event.target.getStage()?.getPointerPosition();
    return position ? toDisplayedPoint(position) : null;
  };

  const positions: Record<Handle, { x: number; y: number }> = {
    nw: { x: crop.x, y: crop.y },
    n: { x: crop.x + crop.width / 2, y: crop.y },
    ne: { x: crop.x + crop.width, y: crop.y },
    e: { x: crop.x + crop.width, y: crop.y + crop.height / 2 },
    se: { x: crop.x + crop.width, y: crop.y + crop.height },
    s: { x: crop.x + crop.width / 2, y: crop.y + crop.height },
    sw: { x: crop.x, y: crop.y + crop.height },
    w: { x: crop.x, y: crop.y + crop.height / 2 }
  };

  const resize = (handle: Handle, px: number, py: number) => {
    const start = dragStart.current?.crop ?? crop;
    let x0 = start.x;
    let y0 = start.y;
    let x1 = start.x + start.width;
    let y1 = start.y + start.height;
    if (handle.includes('w')) x0 = clamp(px, 0, x1 - 1);
    if (handle.includes('e')) x1 = clamp(px, x0 + 1, imageWidth);
    if (handle.includes('n')) y0 = clamp(py, 0, y1 - 1);
    if (handle.includes('s')) y1 = clamp(py, y0 + 1, imageHeight);
    const changed: 'width' | 'height' | 'both' = handle === 'n' || handle === 's' ? 'height' : handle === 'e' || handle === 'w' ? 'width' : 'both';
    let next = constrainCrop({ x: x0, y: y0, width: x1 - x0, height: y1 - y0 }, draft.ratio, imageWidth, imageHeight, changed);
    // Anchor the opposite edge when the ratio changed the free axis.
    if (handle.includes('n')) next = { ...next, y: Math.max(0, y1 - next.height) };
    if (handle.includes('w')) next = { ...next, x: Math.max(0, x1 - next.width) };
    onChange(constrainCrop(next, null, imageWidth, imageHeight));
  };

  return (
    <>
      <Rect x={0} y={0} width={imageWidth} height={crop.y} fill="rgba(0,0,0,0.45)" listening={false} />
      <Rect x={0} y={crop.y + crop.height} width={imageWidth} height={imageHeight - crop.y - crop.height} fill="rgba(0,0,0,0.45)" listening={false} />
      <Rect x={0} y={crop.y} width={crop.x} height={crop.height} fill="rgba(0,0,0,0.45)" listening={false} />
      <Rect x={crop.x + crop.width} y={crop.y} width={imageWidth - crop.x - crop.width} height={crop.height} fill="rgba(0,0,0,0.45)" listening={false} />
      <Rect
        x={crop.x}
        y={crop.y}
        width={crop.width}
        height={crop.height}
        stroke="#ffffff"
        strokeWidth={1.5 / scale}
        fill="rgba(255,255,255,0.001)"
        draggable
        onDragStart={(event) => {
          const displayed = displayedPointer(event);
          dragStart.current = displayed ? { crop, displayed } : null;
        }}
        onDragMove={(event) => {
          const displayed = displayedPointer(event);
          const start = dragStart.current;
          if (!displayed || !start) return;
          onChange(moveCrop(start.crop, start.displayed, displayed, imageWidth, imageHeight));
        }}
        onDragEnd={(event) => {
          const displayed = displayedPointer(event);
          const start = dragStart.current;
          if (displayed && start) onChange(moveCrop(start.crop, start.displayed, displayed, imageWidth, imageHeight));
          dragStart.current = null;
        }}
      />
      {HANDLES.map((handle) => (
        <Rect
          key={handle}
          x={positions[handle].x - handleSize / 2}
          y={positions[handle].y - handleSize / 2}
          width={handleSize}
          height={handleSize}
          fill="#ffffff"
          stroke="#b45df2"
          strokeWidth={1 / scale}
          draggable
          onDragStart={(event) => {
            const displayed = displayedPointer(event);
            dragStart.current = displayed ? { crop, displayed } : null;
          }}
          onDragMove={(event) => {
            const displayed = displayedPointer(event);
            if (displayed) resize(handle, displayed.x, displayed.y);
          }}
          onDragEnd={(event) => {
            const displayed = displayedPointer(event);
            if (displayed) resize(handle, displayed.x, displayed.y);
            dragStart.current = null;
          }}
        />
      ))}
    </>
  );
}

function CompareLine({
  fraction,
  view,
  imageWidth,
  imageHeight,
  stageHeight,
  onChange
}: {
  fraction: number;
  view: ViewTransform;
  imageWidth: number;
  imageHeight: number;
  stageHeight: number;
  onChange: (fraction: number) => void;
}) {
  const x = view.x + fraction * imageWidth * view.scale;
  const top = view.y;
  const bottom = view.y + imageHeight * view.scale;
  return (
    <Group
      x={x}
      y={0}
      draggable
      dragBoundFunc={(pos) => ({ x: clamp(pos.x, view.x, view.x + imageWidth * view.scale), y: 0 })}
      onDragMove={(event) => onChange(clamp((event.target.x() - view.x) / (imageWidth * view.scale), 0, 1))}
    >
      <Line points={[0, 0, 0, stageHeight]} stroke="#b45df2" strokeWidth={2} />
      <Rect x={-14} y={(top + bottom) / 2 - 14} width={28} height={28} cornerRadius={14} fill="#b45df2" />
      <Line points={[-5, (top + bottom) / 2, 5, (top + bottom) / 2]} stroke="#fff" strokeWidth={2} />
      <Rect x={-10} y={0} width={20} height={stageHeight} fill="rgba(0,0,0,0.001)" />
    </Group>
  );
}
