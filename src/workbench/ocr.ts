// Thin client for the native OCR sidecar. Pixels never leave Rust: the
// workbench sends a project id, an integer rectangle and its request id, and
// the host crops, pads, runs the sidecar and maps the answer back. Stale and
// superseded answers are reported as `OcrSkipped` so callers can drop them
// without surfacing an error.

import { cancelWorkbenchOcr, IpcError, recognizeWorkbenchRegion } from '../api';
import type { OcrItem, OcrPackageStatus, PixelRect } from './types';

export const SKIPPED_CODES = new Set(['ocr_superseded', 'ocr_cancelled']);

export class OcrSkipped extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'OcrSkipped';
  }
}

export class OcrClient {
  private counter = 0;
  private inflight = new Set<number>();
  private failure: string | null = null;

  get lastFailure(): string | null {
    return this.failure;
  }

  /** A fresh id for one recognition round trip. */
  nextRequestId(): number {
    this.counter += 1;
    return this.counter;
  }

  async recognize(status: OcrPackageStatus, projectId: string, rect: PixelRect, background: string | null, requestId: number): Promise<OcrItem[]> {
    if (!status.installed) throw new Error('OCR 组件尚未安装');
    if (!status.engine.available) throw new Error(status.engine.problem ?? 'OCR 引擎不可用');
    this.inflight.add(requestId);
    try {
      const result = await recognizeWorkbenchRegion(projectId, rect, background, requestId);
      this.failure = null;
      return result.items;
    } catch (error) {
      if (error instanceof IpcError && SKIPPED_CODES.has(error.code)) throw new OcrSkipped(error.message);
      this.failure = error instanceof Error ? error.message : String(error);
      throw error;
    } finally {
      this.inflight.delete(requestId);
    }
  }

  /** Ask the host to drop a request that is no longer wanted (Esc, new selection). */
  async cancel(requestId: number): Promise<void> {
    if (!this.inflight.has(requestId)) return;
    try {
      await cancelWorkbenchOcr(requestId);
    } catch {
      // The request may already have finished; nothing to clean up here.
    }
  }

  /** Cancel whatever is in flight. The sidecar itself is owned by the host and
   *  exits on its own idle timer, so leaving the page costs nothing here. */
  async dispose(): Promise<void> {
    const pending = Array.from(this.inflight);
    this.inflight.clear();
    await Promise.all(pending.map((id) => cancelWorkbenchOcr(id).catch(() => false)));
  }
}

export const ocrClient = new OcrClient();
