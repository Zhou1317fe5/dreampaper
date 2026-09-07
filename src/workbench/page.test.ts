// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { WorkbenchPage, type LeaveGuard } from './index';
import type { CanvasProps } from './canvas';
import type { SidebarProps } from './panel';
import type { ProjectDetail, ProjectDoc } from './types';

const api = vi.hoisted(() => ({
  listWorkbenchProjects: vi.fn(), openWorkbenchProject: vi.fn(), saveWorkbenchProject: vi.fn(),
  getWorkbenchProject: vi.fn(), getOcrPackageStatus: vi.fn(), listenOcrProgress: vi.fn(),
  listWorkbenchFonts: vi.fn(), analyzeWorkbenchRegion: vi.fn(), previewWorkbenchExport: vi.fn(),
  pickSavePath: vi.fn(), exportWorkbenchProject: vi.fn(),
  deleteWorkbenchProject: vi.fn()
}));
vi.mock('../api', () => api);
vi.mock('./ocr', () => ({ ocrClient: { dispose: vi.fn(async () => {}) } }));
vi.mock('./canvas', () => ({ WorkbenchCanvas: (props: CanvasProps) => createElement('button', {
  onClick: () => props.onDraw('rect', { x: 10, y: 10, width: 30, height: 20 })
}, '测试框选') }));
// The real sidebar stays (the tests drive the project list through it); a
// test button reaches the crop controls the properties column would expose.
vi.mock('./panel', async (original) => {
  const real = await original<typeof import('./panel')>();
  return { ...real, Sidebar: (props: SidebarProps) => createElement('div', null,
    createElement(real.Sidebar, props),
    createElement('button', { onClick: () => props.inspector?.crop?.onRect('width', 50) }, '测试裁剪')
  ) };
});

let root: Root;
let host: HTMLDivElement;
let guard: LeaveGuard | null;
let saved: ProjectDetail;
let messages = vi.fn<(text: string, tone?: 'info' | 'error') => void>();
const documentData: ProjectDoc = {
  schema_version: 1, id: 'p1', revision: 1, name: 'a · 编辑 1',
  source: { asset_id: 'a1', filename: 'a.png', width: 100, height: 100, orientation_normalized: true, working_color_space: 'srgb' },
  viewport: { crop: { x: 0, y: 0, width: 100, height: 100 }, flip_x: false, flip_y: false },
  layers: [], created_at: 'created', updated_at: 'created'
};
function detail(doc: ProjectDoc): ProjectDetail {
  return { document: doc, summary: {
    id: doc.id, name: doc.name, asset_id: 'a1', filename: 'a.png', source_url: 'dp-workbench://localhost/asset/a1',
    thumbnail: '', source_width: 100, source_height: 100, export_width: doc.viewport.crop.width,
    export_height: doc.viewport.crop.height, revision: doc.revision, created_at: doc.created_at, updated_at: doc.updated_at
  } };
}
async function click(label: string) {
  const button = [...host.querySelectorAll('button')].find(item => item.textContent === label || item.title === label);
  expect(button, label).toBeDefined();
  await act(async () => { button!.click(); });
}
async function mount() {
  await act(async () => root.render(createElement(WorkbenchPage, {
    lang: 'zh', request: { assetId: 'a1', token: 1 }, onRequestHandled: vi.fn(), onMessage: messages,
    registerLeaveGuard: (next) => { guard = next; }
  })));
}

beforeEach(() => {
  vi.clearAllMocks();
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  saved = detail(structuredClone(documentData));
  messages = vi.fn<(text: string, tone?: 'info' | 'error') => void>();
  guard = null;
  api.listWorkbenchProjects.mockImplementation(async () => [saved.summary]);
  api.openWorkbenchProject.mockImplementation(async () => ({ detail: saved, resumed: true }));
  api.getWorkbenchProject.mockImplementation(async () => saved);
  api.saveWorkbenchProject.mockImplementation(async (_id, revision, doc) => {
    saved = detail({ ...doc, revision: revision + 1, updated_at: `saved-${revision + 1}` });
    return saved;
  });
  api.getOcrPackageStatus.mockResolvedValue({ installed: false });
  api.listenOcrProgress.mockResolvedValue(() => {});
  api.listWorkbenchFonts.mockResolvedValue([]);
  api.analyzeWorkbenchRegion.mockResolvedValue({ color: '#ffffff', coverage: 1, uneven: false, candidates: [], samples: 10 });
  api.previewWorkbenchExport.mockImplementation(async (_id, doc) => ({ width: doc.viewport.crop.width, height: doc.viewport.crop.height, missing_fonts: [], had_alpha: false, color_note: null }));
  api.pickSavePath.mockResolvedValue('/tmp/output.png');
  api.exportWorkbenchProject.mockImplementation(async (_id, doc) => {
    expect(doc).toEqual(saved.document);
    return { filename: 'output.png' };
  });
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
});

describe('workbench orchestration', () => {
  it('opens once, previews without saving, then commits crop and exports the backend snapshot', async () => {
    await mount();
    expect(api.openWorkbenchProject).toHaveBeenCalledTimes(1);
    await click('测试框选');
    expect(api.saveWorkbenchProject).not.toHaveBeenCalled();
    await click('应用修补');
    await click('裁剪 (C)');
    await click('测试裁剪');
    await click('导出 PNG');
    expect(saved.document.layers).toHaveLength(1);
    expect(saved.document.viewport.crop.width).toBe(50);
    await click('选择位置并导出');
    expect(api.exportWorkbenchProject).toHaveBeenCalledTimes(1);
    expect(api.listWorkbenchProjects.mock.calls.length).toBeLessThan(10);
  });

  it('preserves the editor after failed saving, cancelled leaving and failed deletion', async () => {
    await mount();
    api.saveWorkbenchProject.mockRejectedValue(new Error('磁盘已满'));
    await click('裁剪 (C)');
    await click('测试裁剪');
    let leaving!: Promise<boolean>;
    await act(async () => { leaving = guard!(); await Promise.resolve(); });
    expect(host.textContent).toContain('有修改尚未保存');
    await click('留在工作台');
    expect(await leaving).toBe(false);
    expect(host.textContent).toContain('裁剪后 50 × 100');
    api.saveWorkbenchProject.mockImplementation(async (_id, revision, doc) => {
      saved = detail({ ...doc, revision: revision + 1, updated_at: 'retried' });
      return saved;
    });
    api.deleteWorkbenchProject.mockRejectedValue(new Error('删除失败'));
    await click('工程');
    await click('删除');
    await click('确认删除');
    expect(messages).toHaveBeenCalledWith('删除失败', 'error');
    expect(host.textContent).toContain('测试框选');
    expect(saved.document.viewport.crop.width).toBe(50);
  });
});
