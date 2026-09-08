import { describe, expect, it } from 'vitest';
import {
  defaultText,
  findLayer,
  HISTORY_LIMIT,
  initialState,
  isDirty,
  MERGE_WINDOW_MS,
  outgoingDoc,
  reduce,
  type EditorState
} from './state';
import type { ProjectDetail, ProjectDoc, RegionAnalysis, RepairGroup, TextLayer } from './types';

function doc(): ProjectDoc {
  return {
    schema_version: 1,
    id: 'p1',
    revision: 3,
    name: 'figure · 编辑 1',
    source: {
      asset_id: 'a1',
      filename: 'figure.png',
      width: 1000,
      height: 600,
      orientation_normalized: true,
      working_color_space: 'srgb'
    },
    viewport: { crop: { x: 0, y: 0, width: 1000, height: 600 }, flip_x: false, flip_y: false },
    layers: [],
    created_at: 't',
    updated_at: 't'
  };
}

function detail(document = doc()): ProjectDetail {
  return {
    summary: {
      id: document.id,
      name: document.name,
      asset_id: 'a1',
      filename: 'figure.png',
      source_url: 'dp-workbench://localhost/asset/a1',
      thumbnail: 'dp-workbench://localhost/asset/a1?w=400',
      source_width: 1000,
      source_height: 600,
      export_width: 1000,
      export_height: 600,
      revision: document.revision,
      created_at: 't',
      updated_at: 't'
    },
    document
  };
}

const analysis: RegionAnalysis = { color: '#ffffff', coverage: 0.97, uneven: false, candidates: [], samples: 100 };

function text(id: string, rect = { x: 110, y: 110, width: 100, height: 30 }): TextLayer {
  return { ...defaultText(rect, `text ${id}`, 'Noto Sans SC', '#000000'), id, origin: 'ocr', edited: false };
}

function withGroup(state: EditorState, children: TextLayer[] = [text('t1'), text('t2', { x: 110, y: 150, width: 80, height: 30 })]) {
  let next = reduce(state, { type: 'pending_start', shape: 'rect', rect: { x: 100, y: 100, width: 200, height: 100 }, requestId: 1 });
  next = reduce(next, { type: 'pending_analysis', requestId: 1, analysis });
  next = reduce(next, { type: 'pending_ocr', requestId: 1, ocr: 'done', ocrStatus: 'found', texts: children });
  return reduce(next, { type: 'pending_apply', groupId: 'g1' });
}

function group(state: EditorState, id = 'g1'): RepairGroup {
  const located = findLayer(state.doc, id);
  if (!located || located.layer.kind !== 'repair') throw new Error('group missing');
  return located.layer;
}

describe('pending repair', () => {
  it('only writes to the document on apply, as one undo step', () => {
    let state = initialState(detail());
    state = reduce(state, { type: 'pending_start', shape: 'ellipse', rect: { x: 10, y: 10, width: 50, height: 20 }, requestId: 7 });
    expect(state.doc.layers).toHaveLength(0);
    expect(isDirty(state)).toBe(false);
    // Stale results are dropped.
    state = reduce(state, { type: 'pending_analysis', requestId: 6, analysis });
    expect(state.pending?.analysis).toBeNull();
    state = reduce(state, { type: 'pending_analysis', requestId: 7, analysis });
    state = reduce(state, { type: 'pending_ocr', requestId: 7, ocr: 'done', ocrStatus: 'found', texts: [text('t1')] });
    const cancelled = reduce(state, { type: 'pending_cancel' });
    expect(cancelled.pending).toBeNull();
    expect(cancelled.doc.layers).toHaveLength(0);

    state = reduce(state, { type: 'pending_apply', groupId: 'g1' });
    expect(state.pending).toBeNull();
    expect(state.doc.layers).toHaveLength(1);
    expect(group(state).children).toHaveLength(1);
    expect(group(state).fill).toEqual({ auto: '#ffffff', current: '#ffffff', source: 'auto', uneven: false, coverage: 0.97 });
    expect(state.past).toHaveLength(1);
    expect(isDirty(state)).toBe(true);
    const undone = reduce(state, { type: 'undo' });
    expect(undone.doc.layers).toHaveLength(0);
    expect(reduce(undone, { type: 'redo' }).doc.layers).toHaveLength(1);
  });
});

describe('group geometry', () => {
  it('moves children with the group and scales them proportionally', () => {
    let state = withGroup(initialState(detail()));
    state = reduce(state, { type: 'set_layer_rect', id: 'g1', rect: { x: 300, y: 200, width: 400, height: 200 } });
    const g = group(state);
    expect(g.rect).toEqual({ x: 300, y: 200, width: 400, height: 200 });
    expect(g.children[0].rect).toEqual({ x: 320, y: 220, width: 200, height: 60 });
    expect(g.children[0].font.size).toBe(42); // 21 * 2, auto-fit scales the size
    expect(g.ocr.stale).toBe(false); // untouched OCR text may be re-recognised
  });

  it('marks edited groups stale instead of losing manual text', () => {
    let state = withGroup(initialState(detail()));
    state = reduce(state, { type: 'update_text', id: 't1', patch: { text: '人工改写' } });
    expect(group(state).ocr.edited).toBe(true);
    expect(group(state).children[0].edited).toBe(true);
    state = reduce(state, { type: 'set_layer_rect', id: 'g1', rect: { x: 120, y: 100, width: 200, height: 100 } });
    expect(group(state).ocr.stale).toBe(true);
    expect(group(state).children[0].text).toBe('人工改写');
    // Automation may update the colour candidate but not the texts.
    state = reduce(state, {
      type: 'reanalyzed',
      groupId: 'g1',
      analysis: { ...analysis, color: '#eeeeee' },
      texts: [text('fresh')],
      ocrStatus: 'found',
      expectDoc: state.doc,
      expectedGroup: group(state)
    });
    expect(group(state).fill.current).toBe('#eeeeee');
    expect(group(state).children.map((c) => c.id)).toEqual(['t1', 't2']);
  });

  it('a drag is one history step and re-analysis folds into it', () => {
    let state = withGroup(initialState(detail()));
    const before = state.past.length;
    state = reduce(state, { type: 'begin_transient' });
    state = reduce(state, { type: 'set_layer_rect', id: 'g1', rect: { x: 100.4, y: 100, width: 200, height: 100 }, transient: true });
    state = reduce(state, { type: 'set_layer_rect', id: 'g1', rect: { x: 130.6, y: 100, width: 200, height: 100 }, transient: true });
    expect(state.past).toHaveLength(before);
    state = reduce(state, { type: 'end_transient' });
    expect(state.past).toHaveLength(before + 1);
    expect(group(state).rect.x).toBe(131);
    const afterMove = state.past.length;
    state = reduce(state, {
      type: 'reanalyzed',
      groupId: 'g1',
      analysis: { ...analysis, color: '#123456' },
      texts: [text('n1')],
      ocrStatus: 'found',
      expectDoc: state.doc,
      expectedGroup: group(state)
    });
    expect(state.past).toHaveLength(afterMove);
    expect(group(state).fill.current).toBe('#123456');
    expect(group(state).children[0].id).toBe('n1');
    const undone = reduce(state, { type: 'undo' });
    expect(group(undone).rect.x).toBe(100);
    expect(group(undone).fill.current).toBe('#ffffff');
    expect(group(undone).children[0].id).toBe('t1');
  });

  it('manual fill survives re-analysis and can be reset', () => {
    let state = withGroup(initialState(detail()));
    state = reduce(state, { type: 'set_fill', id: 'g1', color: '#ff0000' });
    state = reduce(state, {
      type: 'reanalyzed',
      groupId: 'g1',
      analysis: { ...analysis, color: '#00ff00' },
      texts: null,
      ocrStatus: null,
      expectDoc: null,
      expectedGroup: group(state)
    });
    expect(group(state).fill).toMatchObject({ current: '#ff0000', auto: '#00ff00', source: 'manual' });
    state = reduce(state, { type: 'reset_fill', id: 'g1' });
    expect(group(state).fill).toMatchObject({ current: '#00ff00', source: 'auto' });
  });
});

describe('layer operations', () => {
  it('links and unlinks texts without moving them', () => {
    let state = withGroup(initialState(detail()));
    const free = text('free', { x: 500, y: 500, width: 100, height: 30 });
    state = reduce(state, { type: 'add_text', text: free, parentId: null });
    expect(state.doc.layers.map((l) => l.id)).toEqual(['g1', 'free']);
    state = reduce(state, { type: 'link_text', textId: 'free', groupId: 'g1' });
    expect(state.doc.layers.map((l) => l.id)).toEqual(['g1']);
    expect(group(state).children.map((c) => c.id)).toEqual(['t1', 't2', 'free']);
    expect(findLayer(state.doc, 'free')?.layer.rect).toEqual(free.rect);
    state = reduce(state, { type: 'unlink_text', textId: 't1' });
    expect(state.doc.layers.map((l) => l.id)).toEqual(['g1', 't1']);
    expect(reduce(state, { type: 'undo' }).doc.layers.map((l) => l.id)).toEqual(['g1']);
  });

  it('reorders whole groups and keeps children above their fill', () => {
    let state = withGroup(initialState(detail()));
    state = reduce(state, { type: 'add_text', text: text('free', { x: 500, y: 500, width: 100, height: 30 }), parentId: null });
    state = reduce(state, { type: 'reorder_layer', id: 'g1', direction: 'up' });
    expect(state.doc.layers.map((l) => l.id)).toEqual(['free', 'g1']);
    state = reduce(state, { type: 'reorder_layer', id: 't1', direction: 'up' });
    expect(group(state).children.map((c) => c.id)).toEqual(['t2', 't1']);
    state = reduce(state, { type: 'reorder_layer', id: 't1', direction: 'bottom' });
    expect(group(state).children.map((c) => c.id)).toEqual(['t1', 't2']);
  });

  it('duplicates, hides and removes layers', () => {
    let state = withGroup(initialState(detail()));
    state = reduce(state, { type: 'duplicate_layer', id: 'g1', newIds: ['g2', 'c1', 'c2'] });
    expect(state.doc.layers.map((l) => l.id)).toEqual(['g1', 'g2']);
    expect(group(state, 'g2').children.map((c) => c.id)).toEqual(['c1', 'c2']);
    expect(group(state, 'g2').rect.x).toBe(112);
    expect(state.selection.id).toBe('g2');
    state = reduce(state, { type: 'toggle_visible', id: 'g2' });
    expect(group(state, 'g2').visible).toBe(false);
    state = reduce(state, { type: 'remove_layer', id: 'c1' });
    expect(group(state, 'g2').children.map((c) => c.id)).toEqual(['c2']);
    expect(group(state, 'g2').ocr.edited).toBe(true);
    state = reduce(state, { type: 'remove_layer', id: 'g2' });
    expect(state.doc.layers.map((l) => l.id)).toEqual(['g1']);
    expect(state.selection.id).toBeNull();
  });

  it('merges children into one box in reading order', () => {
    let state = withGroup(initialState(detail()), [
      text('b', { x: 110, y: 150, width: 80, height: 30 }),
      text('a', { x: 110, y: 110, width: 100, height: 30 }),
      text('a2', { x: 220, y: 112, width: 60, height: 30 })
    ]);
    state = reduce(state, { type: 'merge_texts', groupId: 'g1', newId: 'm' });
    const merged = group(state).children;
    expect(merged).toHaveLength(1);
    expect(merged[0].text).toBe('text a\ntext a2\ntext b');
    expect(merged[0].rect).toEqual({ x: 110, y: 110, width: 170, height: 70 });
    expect(merged[0].edited).toBe(true);
  });
});

describe('history and saving', () => {
  it('caps history at 100 and merges keystrokes within the window', () => {
    let state = withGroup(initialState(detail()));
    for (let i = 0; i < 130; i += 1) {
      state = reduce(state, { type: 'update_text', id: 't1', patch: { text: `v${i}` }, mergeKey: 'text:t1', now: i * (MERGE_WINDOW_MS + 1) });
    }
    expect(state.past).toHaveLength(HISTORY_LIMIT);
    const merged = reduce(
      reduce(state, { type: 'update_text', id: 't1', patch: { text: 'ab' }, mergeKey: 'text:t1', now: 10_000_000 }),
      { type: 'update_text', id: 't1', patch: { text: 'abc' }, mergeKey: 'text:t1', now: 10_000_100 }
    );
    expect(merged.past).toHaveLength(HISTORY_LIMIT);
    expect(reduce(merged, { type: 'undo' }).doc).toBe(state.doc);
  });

  it('adopts auto-fit sizes silently and ignores them for fixed-size text', () => {
    const state = withGroup(initialState(detail()));
    const before = state.past.length;
    const fitted = reduce(state, { type: 'fit_text_size', id: 't1', size: 18.25 });
    const layer = findLayer(fitted.doc, 't1')!.layer as TextLayer;
    expect(layer.font.size).toBe(18.25);
    expect(layer.edited).toBe(false);
    expect(fitted.past).toHaveLength(before);
    expect(isDirty(fitted)).toBe(true);
    // Sub-quarter differences and non-auto-fit texts leave the state untouched.
    expect(reduce(fitted, { type: 'fit_text_size', id: 't1', size: 18.4 })).toBe(fitted);
    const fixed = reduce(fitted, { type: 'update_text', id: 't1', patch: { auto_fit: false } });
    expect(reduce(fixed, { type: 'fit_text_size', id: 't1', size: 40 })).toBe(fixed);
  });

  it('tracks dirtiness against the saved snapshot and sends the disk revision', () => {
    let state = withGroup(initialState(detail()));
    expect(isDirty(state)).toBe(true);
    const base = state.doc;
    expect(outgoingDoc(state).revision).toBe(3);
    // The user keeps typing while the save is in flight.
    state = reduce(state, { type: 'update_text', id: 't1', patch: { text: 'more' } });
    state = reduce(state, { type: 'saved', base, document: { ...base, revision: 4, updated_at: 'saved-4' } });
    expect(state.revision).toBe(4);
    expect(isDirty(state)).toBe(true);
    const settled = reduce(state, { type: 'saved', base: state.doc, document: { ...state.doc, revision: 5, updated_at: 'saved-5' } });
    expect(isDirty(settled)).toBe(false);
    expect(outgoingDoc(settled)).toEqual(settled.savedDoc);
    expect(outgoingDoc(settled).updated_at).toBe('saved-5');
    const undone = reduce(settled, { type: 'undo' });
    expect(outgoingDoc(undone).updated_at).toBe('saved-5');
    const renamed = reduce(settled, { type: 'renamed', name: '新名字', revision: 6, updatedAt: 'renamed-6' });
    expect(renamed.doc.name).toBe('新名字');
    expect(isDirty(renamed)).toBe(false);
    expect(renamed.revision).toBe(6);
    expect(outgoingDoc(renamed)).toEqual(renamed.savedDoc);
  });

  it('restore to original clears layers and viewport but not the source', () => {
    let state = withGroup(initialState(detail()));
    state = reduce(state, { type: 'set_viewport', viewport: { crop: { x: 10, y: 10, width: 100, height: 50 }, flip_x: true, flip_y: false } });
    state = reduce(state, { type: 'restore_original' });
    expect(state.doc.layers).toHaveLength(0);
    expect(state.doc.viewport).toEqual({ crop: { x: 0, y: 0, width: 1000, height: 600 }, flip_x: false, flip_y: false });
    expect(state.doc.source.asset_id).toBe('a1');
    expect(reduce(state, { type: 'undo' }).doc.viewport.flip_x).toBe(true);
  });
});

 describe('async repair regressions', () => {
  it('invalidates old colour analysis when the pending box moves', () => {
    let state = initialState(detail());
    state = reduce(state, { type: 'pending_start', shape: 'rect', rect: { x: 1, y: 1, width: 50, height: 20 }, requestId: 1 });
    state = reduce(state, { type: 'pending_analysis', requestId: 1, analysis });
    state = reduce(state, { type: 'pending_rect', rect: { x: 100, y: 1, width: 50, height: 20 }, requestId: 2 });
    expect(state.pending?.analysis).toBeNull();
    expect(reduce(state, { type: 'pending_apply', groupId: 'g' }).doc.layers).toHaveLength(0);
  });

  it('rejects OCR that arrives after a manual edit or geometry change', () => {
    let state = withGroup(initialState(detail()));
    const expectedGroup = group(state);
    state = reduce(state, { type: 'update_text', id: 't1', patch: { text: '保留人工编辑' } });
    expect(reduce(state, { type: 'reocr', groupId: 'g1', expectedGroup, texts: [text('replacement')], ocrStatus: 'found' })).toBe(state);
    state = reduce(state, { type: 'set_layer_rect', id: 'g1', rect: { x: 300, y: 200, width: 200, height: 100 } });
    expect(reduce(state, { type: 'reanalyzed', groupId: 'g1', expectedGroup, expectDoc: null, analysis, texts: [], ocrStatus: 'none' })).toBe(state);
  });

  it('keeps explicit colour re-analysis as its own undo step', () => {
    const state = withGroup(initialState(detail()));
    const next = reduce(state, { type: 'reanalyzed', groupId: 'g1', expectedGroup: group(state), expectDoc: null, analysis: { ...analysis, color: '#123456' }, texts: null, ocrStatus: null });
    expect(next.past).toHaveLength(state.past.length + 1);
    expect(reduce(next, { type: 'undo' }).doc).toBe(state.doc);
  });

  it('duplicates arbitrary numbers of child layers without reusing ids', () => {
    const state = withGroup(initialState(detail()), Array.from({ length: 20 }, (_, i) => text(`t${i}`)));
    const next = reduce(state, { type: 'duplicate_layer', id: 'g1', newIds: ['g2'] });
    const ids = next.doc.layers.flatMap(layer => layer.kind === 'repair' ? [layer.id, ...layer.children.map(child => child.id)] : [layer.id]);
    expect(new Set(ids).size).toBe(ids.length);
  });
});
