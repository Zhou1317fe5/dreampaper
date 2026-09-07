import { describe, expect, it, vi } from 'vitest';
import { Saver } from './save';
import { initialState, isDirty, outgoingDoc, reduce, type Action } from './state';
import type { ProjectDetail, ProjectDoc } from './types';

const document: ProjectDoc = {
  schema_version: 1, id: 'p1', revision: 1, name: '工程',
  source: { asset_id: 'a1', filename: 'a.png', width: 100, height: 100, orientation_normalized: true, working_color_space: 'srgb' },
  viewport: { crop: { x: 0, y: 0, width: 100, height: 100 }, flip_x: false, flip_y: false },
  layers: [], created_at: 'created', updated_at: 'created'
};
function detail(doc = document): ProjectDetail {
  return {
    document: doc, summary: {
      id: doc.id, revision: doc.revision, name: doc.name, asset_id: 'a1', filename: 'a.png',
      source_url: '', thumbnail: '', source_width: 100, source_height: 100,
      export_width: doc.viewport.crop.width, export_height: doc.viewport.crop.height,
      created_at: doc.created_at, updated_at: doc.updated_at
    }
  };
}
function session() {
  let state = initialState(detail());
  const dispatch = (action: Action) => { state = reduce(state, action); };
  const edit = (width: number) => dispatch({ type: 'set_viewport', viewport: { ...state.doc.viewport, crop: { x: 0, y: 0, width, height: 100 } } });
  return { current: () => state, dispatch, edit };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe('save/leave contract', () => {
  it('drains edits made during a save and exports the exact backend document', async () => {
    const s = session();
    const first = deferred<ProjectDetail>();
    const persist = vi.fn(async (_id: string, revision: number, doc: ProjectDoc) => detail({ ...doc, revision: revision + 1, updated_at: 'second' }));
    persist.mockImplementationOnce(() => first.promise);
    const saver = new Saver(s.current, s.dispatch, vi.fn(), persist);
    s.edit(80);
    const base = s.current().doc;
    const flush = saver.flush();
    s.edit(60);
    expect(saver.flush()).toBe(flush);
    first.resolve(detail({ ...base, revision: 2, updated_at: 'first' }));
    expect(await flush).toBe(true);
    expect(persist).toHaveBeenCalledTimes(2);
    expect(persist.mock.calls[1][1]).toBe(2);
    expect(isDirty(s.current())).toBe(false);
    expect(outgoingDoc(s.current())).toEqual(s.current().savedDoc);
    expect(outgoingDoc(s.current()).viewport.crop.width).toBe(60);
  });

  it('keeps unsaved edits and never silently overwrites a revision conflict', async () => {
    const s = session();
    const persist = vi.fn().mockRejectedValue(Object.assign(new Error('工程已被更新'), { code: 'project_revision_conflict' }));
    const load = vi.fn();
    const report = vi.fn();
    const saver = new Saver(s.current, s.dispatch, report, persist, load);
    s.edit(40);
    expect(await saver.flush()).toBe(false);
    expect(isDirty(s.current())).toBe(true);
    expect(persist).toHaveBeenCalledTimes(1);
    expect(load).not.toHaveBeenCalled();
    expect(report).toHaveBeenLastCalledWith('failed', '工程已被更新');
  });

  it('does not write during a drag; explicit discard reloads only after inflight save settles', async () => {
    const s = session();
    const persist = vi.fn();
    const load = vi.fn(async () => detail());
    const saver = new Saver(s.current, s.dispatch, vi.fn(), persist, load);
    s.edit(30);
    s.dispatch({ type: 'begin_transient' });
    expect(await saver.flush()).toBe(false);
    expect(persist).not.toHaveBeenCalled();
    await saver.discard();
    expect(isDirty(s.current())).toBe(false);
    expect(s.current().doc).toEqual(document);
  });
});
