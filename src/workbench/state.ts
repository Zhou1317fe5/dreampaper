// Editor state: the in-memory project document, a 100-step history, the
// selection, the pending (not yet applied) repair preview and save bookkeeping.
//
// The document is immutable data; every edit produces a new object and the
// previous one goes on the history stack. Drags run as a "transient" so the
// pointer-up commits one history step for the whole gesture, and typing into
// a field merges consecutive keystrokes into one step.

import { containRect, roundRect, scaleWithin } from './geom';
import type {
  Layer,
  OcrStatus,
  PixelRect,
  ProjectDetail,
  ProjectDoc,
  RegionAnalysis,
  RepairGroup,
  Shape,
  TextLayer,
  Viewport
} from './types';

export const HISTORY_LIMIT = 100;
/** Consecutive edits with the same key inside this window share one undo step. */
export const MERGE_WINDOW_MS = 1000;

export type Tool = 'select' | 'rect' | 'ellipse' | 'eyedropper' | 'text' | 'crop';

export interface Selection {
  id: string | null;
  parentId: string | null;
}

export type PendingOcr = 'idle' | 'running' | 'done' | 'unavailable' | 'failed';

export interface PendingRepair {
  requestId: number;
  shape: Shape;
  rect: PixelRect;
  analysis: RegionAnalysis | null;
  analyzing: boolean;
  ocr: PendingOcr;
  ocrStatus: OcrStatus;
  texts: TextLayer[];
}

export interface EditorState {
  projectId: string;
  doc: ProjectDoc;
  /** The document as last written to disk at `revision`. */
  savedDoc: ProjectDoc;
  revision: number;
  past: ProjectDoc[];
  future: ProjectDoc[];
  selection: Selection;
  tool: Tool;
  pending: PendingRepair | null;
  transientBase: ProjectDoc | null;
  lastEdit: { key: string; at: number } | null;
}

export type Action =
  | { type: 'load'; detail: ProjectDetail }
  | { type: 'set_tool'; tool: Tool }
  | { type: 'select'; id: string | null; parentId?: string | null }
  | { type: 'begin_transient' }
  | { type: 'set_layer_rect'; id: string; rect: PixelRect; transient?: boolean; now?: number }
  | { type: 'end_transient' }
  | { type: 'cancel_transient' }
  | { type: 'update_text'; id: string; patch: Partial<Omit<TextLayer, 'kind' | 'id'>>; mergeKey?: string; now?: number }
  | { type: 'set_fill'; id: string; color: string }
  | { type: 'reset_fill'; id: string }
  | { type: 'add_text'; text: TextLayer; parentId: string | null }
  | { type: 'remove_layer'; id: string }
  | { type: 'duplicate_layer'; id: string; newIds: string[] }
  | { type: 'reorder_layer'; id: string; direction: 'up' | 'down' | 'top' | 'bottom' }
  | { type: 'toggle_visible'; id: string }
  | { type: 'link_text'; textId: string; groupId: string }
  | { type: 'unlink_text'; textId: string }
  | { type: 'merge_texts'; groupId: string; newId: string }
  | { type: 'set_viewport'; viewport: Viewport }
  | { type: 'restore_original' }
  | { type: 'undo' }
  | { type: 'redo' }
  | { type: 'pending_start'; shape: Shape; rect: PixelRect; requestId: number }
  | { type: 'pending_rect'; rect: PixelRect; requestId: number }
  | { type: 'pending_analysis'; requestId: number; analysis: RegionAnalysis }
  | { type: 'pending_ocr'; requestId: number; ocr: PendingOcr; ocrStatus: OcrStatus; texts: TextLayer[] }
  | { type: 'pending_cancel' }
  | { type: 'pending_apply'; groupId: string }
  | {
      type: 'reanalyzed';
      groupId: string;
      analysis: RegionAnalysis;
      texts: TextLayer[] | null;
      ocrStatus: OcrStatus | null;
      expectDoc: ProjectDoc | null;
      expectedGroup: RepairGroup;
    }
  | { type: 'reocr'; groupId: string; expectedGroup: RepairGroup; texts: TextLayer[]; ocrStatus: OcrStatus }
  | { type: 'saved'; base: ProjectDoc; document: ProjectDoc }
  | { type: 'renamed'; name: string; revision: number; updatedAt: string };

export function initialState(detail: ProjectDetail): EditorState {
  return {
    projectId: detail.summary.id,
    doc: detail.document,
    savedDoc: detail.document,
    revision: detail.summary.revision,
    past: [],
    future: [],
    selection: { id: null, parentId: null },
    tool: 'select',
    pending: null,
    transientBase: null,
    lastEdit: null
  };
}

export function isDirty(state: EditorState): boolean {
  return state.doc !== state.savedDoc;
}

// ---- lookup helpers ---------------------------------------------------------

export interface Located {
  layer: Layer;
  index: number;
  parent: RepairGroup | null;
  childIndex: number;
}

export function findLayer(doc: ProjectDoc, id: string): Located | null {
  for (let index = 0; index < doc.layers.length; index += 1) {
    const layer = doc.layers[index];
    if (layer.id === id) return { layer, index, parent: null, childIndex: -1 };
    if (layer.kind === 'repair') {
      const childIndex = layer.children.findIndex((child) => child.id === id);
      if (childIndex >= 0) return { layer: layer.children[childIndex], index, parent: layer, childIndex };
    }
  }
  return null;
}

function mapLayer(doc: ProjectDoc, id: string, fn: (layer: Layer, parent: RepairGroup | null) => Layer): ProjectDoc {
  let changed = false;
  const layers = doc.layers.map((layer) => {
    if (layer.id === id) {
      const next = fn(layer, null);
      if (next !== layer) changed = true;
      return next;
    }
    if (layer.kind === 'repair' && layer.children.some((child) => child.id === id)) {
      let childChanged = false;
      const children = layer.children.map((child) => {
        if (child.id !== id) return child;
        const next = fn(child, layer) as TextLayer;
        childChanged = next !== child;
        return next;
      });
      if (!childChanged) return layer;
      changed = true;
      return { ...layer, children };
    }
    return layer;
  });
  return changed ? { ...doc, layers } : doc;
}

function markParentEdited(doc: ProjectDoc, childId: string): ProjectDoc {
  const located = findLayer(doc, childId);
  if (!located?.parent) return doc;
  const parentId = located.parent.id;
  return {
    ...doc,
    layers: doc.layers.map((layer) =>
      layer.id === parentId && layer.kind === 'repair' && !layer.ocr.edited
        ? { ...layer, ocr: { ...layer.ocr, edited: true } }
        : layer
    )
  };
}

/** Move/scale a repair group and carry its children along proportionally. */
export function moveGroup(group: RepairGroup, rect: PixelRect): RepairGroup {
  if (
    rect.x === group.rect.x &&
    rect.y === group.rect.y &&
    rect.width === group.rect.width &&
    rect.height === group.rect.height
  ) {
    return group;
  }
  const scaleY = rect.height / group.rect.height;
  const children = group.children.map((child) => {
    const moved = scaleWithin(child.rect, group.rect, rect);
    const scaled = child.auto_fit
      ? {
          ...child,
          font: { ...child.font, size: Math.max(1, child.font.size * scaleY) },
          letter_spacing: child.letter_spacing * scaleY
        }
      : child;
    return { ...scaled, rect: { x: moved.x, y: moved.y, width: moved.width, height: moved.height } };
  });
  return {
    ...group,
    rect,
    children,
    ocr: group.ocr.edited ? { ...group.ocr, stale: true } : group.ocr
  };
}

/** Snap every rectangle to integer source pixels inside the image. */
export function roundDoc(doc: ProjectDoc): ProjectDoc {
  const { width, height } = doc.source;
  const roundText = (text: TextLayer): TextLayer => ({
    ...text,
    rect: roundRect(text.rect, width, height),
    font: { ...text.font, size: Math.round(text.font.size * 4) / 4 },
    letter_spacing: Math.round(text.letter_spacing * 100) / 100
  });
  return {
    ...doc,
    layers: doc.layers.map((layer) =>
      layer.kind === 'repair'
        ? { ...layer, rect: roundRect(layer.rect, width, height), children: layer.children.map(roundText) }
        : roundText(layer)
    )
  };
}

export function fullViewport(doc: ProjectDoc): Viewport {
  return {
    crop: { x: 0, y: 0, width: doc.source.width, height: doc.source.height },
    flip_x: false,
    flip_y: false
  };
}

// ---- history ----------------------------------------------------------------

function commit(state: EditorState, doc: ProjectDoc, mergeKey?: string, now = Date.now()): EditorState {
  if (doc === state.doc) return state;
  const merged =
    mergeKey !== undefined &&
    state.lastEdit !== null &&
    state.lastEdit.key === mergeKey &&
    now - state.lastEdit.at < MERGE_WINDOW_MS &&
    state.future.length === 0;
  const past = merged ? state.past : [...state.past.slice(-(HISTORY_LIMIT - 1)), state.doc];
  return {
    ...state,
    doc,
    past,
    future: [],
    lastEdit: mergeKey !== undefined ? { key: mergeKey, at: now } : null
  };
}

function selectionStillValid(doc: ProjectDoc, selection: Selection): Selection {
  if (!selection.id) return selection;
  const located = findLayer(doc, selection.id);
  if (!located) return { id: null, parentId: null };
  return { id: selection.id, parentId: located.parent?.id ?? null };
}

// ---- reducer ----------------------------------------------------------------

export function reduce(state: EditorState, action: Action): EditorState {
  switch (action.type) {
    case 'load':
      return initialState(action.detail);

    case 'set_tool':
      return { ...state, tool: action.tool };

    case 'select':
      return { ...state, selection: { id: action.id, parentId: action.parentId ?? null } };

    case 'begin_transient':
      return state.transientBase ? state : { ...state, transientBase: state.doc };

    case 'set_layer_rect': {
      const doc = mapLayer(state.doc, action.id, (layer) =>
        layer.kind === 'repair' ? moveGroup(layer, action.rect) : { ...layer, rect: action.rect, edited: true }
      );
      const marked = markParentEdited(doc, action.id);
      if (action.transient) {
        return { ...state, doc: marked, transientBase: state.transientBase ?? state.doc };
      }
      return commit(state, roundDoc(marked), undefined, action.now);
    }

    case 'end_transient': {
      if (!state.transientBase) return state;
      const base = state.transientBase;
      const doc = roundDoc(state.doc);
      const next = { ...state, transientBase: null, doc: base };
      return commit(next, doc);
    }

    case 'cancel_transient':
      return state.transientBase ? { ...state, doc: state.transientBase, transientBase: null } : state;

    case 'update_text': {
      const located = findLayer(state.doc, action.id);
      if (!located || located.layer.kind !== 'text') return state;
      const patchKeys = Object.keys(action.patch).filter((key) => key !== 'visible');
      let doc = mapLayer(state.doc, action.id, (layer) => ({
        ...(layer as TextLayer),
        ...action.patch,
        edited: patchKeys.length > 0 ? true : (layer as TextLayer).edited
      }));
      if (patchKeys.length > 0) doc = markParentEdited(doc, action.id);
      return commit(state, doc, action.mergeKey, action.now);
    }

    case 'set_fill': {
      const doc = mapLayer(state.doc, action.id, (layer) =>
        layer.kind === 'repair'
          ? { ...layer, fill: { ...layer.fill, current: action.color, source: 'manual' } }
          : layer
      );
      return commit(state, doc, `fill:${action.id}`);
    }

    case 'reset_fill': {
      const doc = mapLayer(state.doc, action.id, (layer) =>
        layer.kind === 'repair'
          ? { ...layer, fill: { ...layer.fill, current: layer.fill.auto ?? layer.fill.current, source: 'auto' } }
          : layer
      );
      return commit(state, doc);
    }

    case 'add_text': {
      let doc: ProjectDoc;
      if (action.parentId) {
        doc = mapLayer(state.doc, action.parentId, (layer) =>
          layer.kind === 'repair'
            ? { ...layer, children: [...layer.children, action.text], ocr: { ...layer.ocr, edited: true } }
            : layer
        );
        if (doc === state.doc) return state;
      } else {
        doc = { ...state.doc, layers: [...state.doc.layers, action.text] };
      }
      return { ...commit(state, doc), selection: { id: action.text.id, parentId: action.parentId } };
    }

    case 'remove_layer': {
      const located = findLayer(state.doc, action.id);
      if (!located) return state;
      let doc: ProjectDoc;
      if (located.parent) {
        const parentId = located.parent.id;
        doc = mapLayer(state.doc, parentId, (layer) =>
          layer.kind === 'repair'
            ? {
                ...layer,
                children: layer.children.filter((child) => child.id !== action.id),
                ocr: { ...layer.ocr, edited: true }
              }
            : layer
        );
      } else {
        doc = { ...state.doc, layers: state.doc.layers.filter((layer) => layer.id !== action.id) };
      }
      const next = commit(state, doc);
      return { ...next, selection: selectionStillValid(doc, next.selection) };
    }

    case 'duplicate_layer': {
      const located = findLayer(state.doc, action.id);
      if (!located) return state;
      const { width, height } = state.doc.source;
      const ids = [...action.newIds];
      const nextId = () => ids.shift() ?? newId();
      const shift = (rect: PixelRect): PixelRect => roundRect(containRect({ ...rect, x: rect.x + 12, y: rect.y + 12 }, width, height), width, height);
      if (located.parent) {
        const copy: TextLayer = { ...(located.layer as TextLayer), id: nextId(), rect: shift(located.layer.rect) };
        const parentId = located.parent.id;
        const doc = mapLayer(state.doc, parentId, (layer) => {
          if (layer.kind !== 'repair') return layer;
          const children = [...layer.children];
          children.splice(located.childIndex + 1, 0, copy);
          return { ...layer, children, ocr: { ...layer.ocr, edited: true } };
        });
        return { ...commit(state, doc), selection: { id: copy.id, parentId } };
      }
      let copy: Layer;
      if (located.layer.kind === 'repair') {
        const group = located.layer;
        const rect = shift(group.rect);
        copy = {
          ...moveGroup(group, rect),
          id: nextId(),
          children: moveGroup(group, rect).children.map((child) => ({ ...child, id: nextId() }))
        };
      } else {
        copy = { ...located.layer, id: nextId(), rect: shift(located.layer.rect) };
      }
      const layers = [...state.doc.layers];
      layers.splice(located.index + 1, 0, copy);
      return { ...commit(state, { ...state.doc, layers }), selection: { id: copy.id, parentId: null } };
    }

    case 'reorder_layer': {
      const located = findLayer(state.doc, action.id);
      if (!located) return state;
      const move = <T>(list: T[], from: number): T[] => {
        const to =
          action.direction === 'up'
            ? Math.min(list.length - 1, from + 1)
            : action.direction === 'down'
              ? Math.max(0, from - 1)
              : action.direction === 'top'
                ? list.length - 1
                : 0;
        if (to === from) return list;
        const next = [...list];
        const [item] = next.splice(from, 1);
        next.splice(to, 0, item);
        return next;
      };
      if (located.parent) {
        const parentId = located.parent.id;
        const doc = mapLayer(state.doc, parentId, (layer) => {
          if (layer.kind !== 'repair') return layer;
          const children = move(layer.children, located.childIndex);
          return children === layer.children ? layer : { ...layer, children, ocr: { ...layer.ocr, edited: true } };
        });
        return commit(state, doc);
      }
      const layers = move(state.doc.layers, located.index);
      return layers === state.doc.layers ? state : commit(state, { ...state.doc, layers });
    }

    case 'toggle_visible': {
      const located = findLayer(state.doc, action.id);
      if (!located) return state;
      let doc: ProjectDoc;
      if (located.parent) {
        const parentId = located.parent.id;
        doc = mapLayer(state.doc, parentId, (layer) => {
          if (layer.kind !== 'repair') return layer;
          return {
            ...layer,
            ocr: { ...layer.ocr, edited: true },
            children: layer.children.map((child) =>
              child.id === action.id ? { ...child, visible: !child.visible, edited: true } : child
            )
          };
        });
      } else {
        doc = mapLayer(state.doc, action.id, (layer) => ({
          ...layer,
          visible: !layer.visible,
          ...(layer.kind === 'text' ? { edited: true } : {})
        }) as Layer);
      }
      return commit(state, doc);
    }

    case 'link_text': {
      const located = findLayer(state.doc, action.textId);
      const target = findLayer(state.doc, action.groupId);
      if (!located || located.parent || located.layer.kind !== 'text' || !target || target.layer.kind !== 'repair') return state;
      const text = located.layer;
      const layers = state.doc.layers
        .filter((layer) => layer.id !== action.textId)
        .map((layer) =>
          layer.id === action.groupId && layer.kind === 'repair'
            ? { ...layer, children: [...layer.children, text], ocr: { ...layer.ocr, edited: true } }
            : layer
        );
      return { ...commit(state, { ...state.doc, layers }), selection: { id: text.id, parentId: action.groupId } };
    }

    case 'unlink_text': {
      const located = findLayer(state.doc, action.textId);
      if (!located?.parent || located.layer.kind !== 'text') return state;
      const text = located.layer;
      const parentId = located.parent.id;
      const layers: Layer[] = [];
      for (const layer of state.doc.layers) {
        if (layer.id === parentId && layer.kind === 'repair') {
          layers.push({
            ...layer,
            children: layer.children.filter((child) => child.id !== action.textId),
            ocr: { ...layer.ocr, edited: true }
          });
          layers.push(text);
        } else {
          layers.push(layer);
        }
      }
      return { ...commit(state, { ...state.doc, layers }), selection: { id: text.id, parentId: null } };
    }

    case 'merge_texts': {
      const located = findLayer(state.doc, action.groupId);
      if (!located || located.layer.kind !== 'repair' || located.layer.children.length < 2) return state;
      const group = located.layer;
      const ordered = [...group.children].sort((a, b) => {
        const ay = a.rect.y + a.rect.height / 2;
        const by = b.rect.y + b.rect.height / 2;
        const tolerance = Math.min(a.rect.height, b.rect.height) / 2;
        return Math.abs(ay - by) <= tolerance ? a.rect.x - b.rect.x : ay - by;
      });
      const x = Math.min(...ordered.map((t) => t.rect.x));
      const y = Math.min(...ordered.map((t) => t.rect.y));
      const right = Math.max(...ordered.map((t) => t.rect.x + t.rect.width));
      const bottom = Math.max(...ordered.map((t) => t.rect.y + t.rect.height));
      const first = ordered[0];
      const merged: TextLayer = {
        ...first,
        id: action.newId,
        text: ordered.map((t) => t.text).join('\n'),
        rect: { x, y, width: right - x, height: bottom - y },
        auto_fit: true,
        origin: 'manual',
        edited: true,
        score: null
      };
      const doc = mapLayer(state.doc, action.groupId, (layer) =>
        layer.kind === 'repair' ? { ...layer, children: [merged], ocr: { ...layer.ocr, edited: true, stale: false } } : layer
      );
      return { ...commit(state, doc), selection: { id: merged.id, parentId: action.groupId } };
    }

    case 'set_viewport':
      return commit(state, { ...state.doc, viewport: action.viewport });

    case 'restore_original': {
      const doc = { ...state.doc, layers: [], viewport: fullViewport(state.doc) };
      return { ...commit(state, doc), selection: { id: null, parentId: null }, pending: null };
    }

    case 'undo': {
      if (state.past.length === 0) return state;
      const doc = state.past[state.past.length - 1];
      return {
        ...state,
        doc,
        past: state.past.slice(0, -1),
        future: [state.doc, ...state.future],
        lastEdit: null,
        selection: selectionStillValid(doc, state.selection),
        pending: null,
        transientBase: null
      };
    }

    case 'redo': {
      if (state.future.length === 0) return state;
      const [doc, ...future] = state.future;
      return {
        ...state,
        doc,
        past: [...state.past, state.doc],
        future,
        lastEdit: null,
        selection: selectionStillValid(doc, state.selection),
        pending: null,
        transientBase: null
      };
    }

    case 'pending_start':
      return {
        ...state,
        selection: { id: null, parentId: null },
        pending: {
          requestId: action.requestId,
          shape: action.shape,
          rect: action.rect,
          analysis: null,
          analyzing: true,
          ocr: 'idle',
          ocrStatus: 'pending',
          texts: []
        }
      };

    case 'pending_rect':
      if (!state.pending) return state;
      return {
        ...state,
        pending: { ...state.pending, rect: action.rect, requestId: action.requestId, analysis: null, analyzing: true, ocr: 'idle', ocrStatus: 'pending', texts: [] }
      };

    case 'pending_analysis':
      if (!state.pending || state.pending.requestId !== action.requestId) return state;
      return { ...state, pending: { ...state.pending, analysis: action.analysis, analyzing: false } };

    case 'pending_ocr':
      if (!state.pending || state.pending.requestId !== action.requestId) return state;
      return {
        ...state,
        pending: { ...state.pending, ocr: action.ocr, ocrStatus: action.ocrStatus, texts: action.texts }
      };

    case 'pending_cancel':
      return state.pending ? { ...state, pending: null } : state;

    case 'pending_apply': {
      const pending = state.pending;
      if (!pending || !pending.analysis || pending.analyzing) return state;
      const group: RepairGroup = {
        kind: 'repair',
        id: action.groupId,
        visible: true,
        shape: pending.shape,
        rect: pending.rect,
        fill: {
          auto: pending.analysis.color,
          current: pending.analysis.color,
          source: 'auto',
          uneven: pending.analysis.uneven,
          coverage: pending.analysis.coverage
        },
        ocr: { status: pending.ocrStatus, edited: false, stale: false },
        children: pending.texts
      };
      const doc = { ...state.doc, layers: [...state.doc.layers, group] };
      return { ...commit(state, doc), pending: null, selection: { id: group.id, parentId: null } };
    }

    case 'reanalyzed': {
      const located = findLayer(state.doc, action.groupId);
      if (!located || located.layer !== action.expectedGroup) return state;
      const doc = mapLayer(state.doc, action.groupId, (layer) => {
        if (layer.kind !== 'repair') return layer;
        const fill = {
          ...layer.fill,
          auto: action.analysis.color,
          current: layer.fill.source === 'auto' ? action.analysis.color : layer.fill.current,
          uneven: action.analysis.uneven,
          coverage: action.analysis.coverage
        };
        if (action.texts === null || layer.ocr.edited) return { ...layer, fill };
        return {
          ...layer,
          fill,
          children: action.texts,
          ocr: { ...layer.ocr, status: action.ocrStatus ?? layer.ocr.status, stale: false }
        };
      });
      if (doc === state.doc) return state;
      // Fold into the gesture that moved the region when nothing happened
      // since, so one undo restores position, colour and text together.
      if (state.doc === action.expectDoc && state.future.length === 0 && !state.transientBase) {
        return { ...state, doc };
      }
      return commit(state, doc);
    }

    case 'reocr': {
      if (findLayer(state.doc, action.groupId)?.layer !== action.expectedGroup) return state;
      const doc = mapLayer(state.doc, action.groupId, (layer) =>
        layer.kind === 'repair'
          ? { ...layer, children: action.texts, ocr: { status: action.ocrStatus, edited: false, stale: false } }
          : layer
      );
      const next = commit(state, doc);
      return { ...next, selection: selectionStillValid(doc, next.selection) };
    }

    case 'saved': {
      const savedDoc = action.document;
      const doc = state.doc === action.base ? savedDoc : {
        ...state.doc,
        revision: savedDoc.revision,
        created_at: savedDoc.created_at,
        updated_at: savedDoc.updated_at
      };
      return { ...state, doc, savedDoc, revision: savedDoc.revision };
    }

    case 'renamed': {
      const clean = state.doc === state.savedDoc;
      const metadata = { name: action.name, revision: action.revision, updated_at: action.updatedAt };
      const doc = { ...state.doc, ...metadata };
      return {
        ...state,
        doc,
        savedDoc: clean ? doc : { ...state.savedDoc, ...metadata },
        revision: action.revision
      };
    }


    default:
      return state;
  }
}

/** The document as the backend expects it on save/export: revision on disk. */
export function outgoingDoc(state: EditorState, doc: ProjectDoc = state.doc): ProjectDoc {
  return { ...doc, revision: state.revision, created_at: state.savedDoc.created_at, updated_at: state.savedDoc.updated_at };
}

export function newId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) return crypto.randomUUID();
  return `id-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

export function defaultText(rect: PixelRect, text: string, fontFamily: string, color: string): TextLayer {
  return {
    kind: 'text',
    id: newId(),
    visible: true,
    text,
    rect,
    font: { family: fontFamily, size: Math.max(6, Math.round(rect.height * 0.7)), weight: 400, italic: false },
    color,
    align: 'left',
    valign: 'middle',
    line_height: 1.2,
    letter_spacing: 0,
    auto_fit: true,
    angle: 0,
    origin: 'manual',
    edited: true,
    score: null
  };
}
