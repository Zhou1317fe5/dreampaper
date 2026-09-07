import { beforeEach, describe, expect, it, vi } from 'vitest';
import { OcrClient, OcrSkipped } from './ocr';
import type { OcrPackageStatus } from './types';

const api = vi.hoisted(() => ({ recognizeWorkbenchRegion: vi.fn(), cancelWorkbenchOcr: vi.fn() }));
vi.mock('../api', async () => {
  const actual = await vi.importActual<typeof import('../api')>('../api');
  return { ...actual, recognizeWorkbenchRegion: api.recognizeWorkbenchRegion, cancelWorkbenchOcr: api.cancelWorkbenchOcr };
});

import { IpcError } from '../api';

const status: OcrPackageStatus = {
  installed: true, version: 'test', download_bytes: 0, installed_bytes: 0,
  directory: '', downloading: false, progress: null, error: null,
  det_model_name: 'det', cls_model_name: 'cls', rec_model_name: 'rec', files: [], license: '', sources: [],
  engine: { available: true, version: '0.1.0', runtime_version: '1.29.0', program: '/p', runtime: '/r', problem: null, running: false, tripped: false }
};
const rect = { x: 10, y: 20, width: 30, height: 40 };

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.clearAllMocks();
  api.cancelWorkbenchOcr.mockResolvedValue(true);
});

describe('native OCR client', () => {
  it('forwards coordinates only and returns the host items', async () => {
    api.recognizeWorkbenchRegion.mockResolvedValue({ items: [{ poly: [], text: 'A', score: 0.9 }], elapsed_ms: 1, padded_width: 64, padded_height: 64 });
    const client = new OcrClient();
    const id = client.nextRequestId();
    const items = await client.recognize(status, 'p1', rect, '#ffffff', id);
    expect(items).toEqual([{ poly: [], text: 'A', score: 0.9 }]);
    expect(api.recognizeWorkbenchRegion).toHaveBeenCalledWith('p1', rect, '#ffffff', id);
    expect(client.lastFailure).toBeNull();
  });

  it('refuses to run without models or engine before touching the host', async () => {
    const client = new OcrClient();
    await expect(client.recognize({ ...status, installed: false }, 'p1', rect, null, 1)).rejects.toThrow('OCR 组件尚未安装');
    await expect(
      client.recognize({ ...status, engine: { ...status.engine, available: false, problem: '缺少运行时' } }, 'p1', rect, null, 2)
    ).rejects.toThrow('缺少运行时');
    expect(api.recognizeWorkbenchRegion).not.toHaveBeenCalled();
  });

  it('turns superseded and cancelled answers into OcrSkipped without recording a failure', async () => {
    api.recognizeWorkbenchRegion.mockRejectedValueOnce(new IpcError('replaced', 'ocr_superseded', null));
    api.recognizeWorkbenchRegion.mockRejectedValueOnce(new IpcError('engine died', 'ocr_engine_failed', null));
    const client = new OcrClient();
    await expect(client.recognize(status, 'p1', rect, null, 1)).rejects.toBeInstanceOf(OcrSkipped);
    expect(client.lastFailure).toBeNull();
    await expect(client.recognize(status, 'p1', rect, null, 2)).rejects.toThrow('engine died');
    expect(client.lastFailure).toBe('engine died');
  });

  it('cancels only requests that are still in flight', async () => {
    const pending = deferred<{ items: never[]; elapsed_ms: number; padded_width: number; padded_height: number }>();
    api.recognizeWorkbenchRegion.mockReturnValueOnce(pending.promise);
    const client = new OcrClient();
    const run = client.recognize(status, 'p1', rect, null, 7);
    await client.cancel(7);
    expect(api.cancelWorkbenchOcr).toHaveBeenCalledWith(7);
    pending.resolve({ items: [], elapsed_ms: 0, padded_width: 32, padded_height: 32 });
    await run;
    await client.cancel(7);
    expect(api.cancelWorkbenchOcr).toHaveBeenCalledTimes(1);
  });

  it('dispose cancels everything in flight and swallows host errors', async () => {
    const first = deferred<never>();
    const second = deferred<never>();
    api.recognizeWorkbenchRegion.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    api.cancelWorkbenchOcr.mockRejectedValue(new Error('gone'));
    const client = new OcrClient();
    const a = client.recognize(status, 'p1', rect, null, 1).catch(() => undefined);
    const b = client.recognize(status, 'p1', rect, null, 2).catch(() => undefined);
    await client.dispose();
    expect(api.cancelWorkbenchOcr).toHaveBeenCalledTimes(2);
    first.reject(new IpcError('cancelled', 'ocr_cancelled', null));
    second.reject(new IpcError('cancelled', 'ocr_cancelled', null));
    await Promise.all([a, b]);
  });
});
