// The workbench page: project list, editor and dialogs. Everything that talks
// to the backend or the OCR worker lives here; the reducer stays pure.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  analyzeWorkbenchRegion,
  cancelOcrPackageInstall,
  copyWorkbenchProject,
  deleteWorkbenchProject,
  exportWorkbenchProject,
  getOcrPackageStatus,
  getWorkbenchProject,
  installOcrPackage,
  listenExportProgress,
  listenOcrProgress,
  listWorkbenchFonts,
  listWorkbenchProjects,
  measureWorkbenchText,
  workbenchStorage,
  cleanupWorkbenchAssets,
  removeOcrPackage,
  openWorkbenchProject,
  pickImageFile,
  pickSavePath,
  previewWorkbenchExport,
  renameWorkbenchProject
} from '../api';
import type { Lang } from '../app';
import { WorkbenchCanvas, type CropDraft } from './canvas';
import { formatBytes, workbenchCopy, type WorkbenchCopy } from './copy';
import { judgeLine } from './filter';
import { aspectRatio, constrainCrop, polyBounds, roundRect, zoomAt, type AspectPreset, type ViewTransform } from './geom';
import { OcrSkipped, ocrClient } from './ocr';
import { Sidebar, type SidebarProps, type SidebarTab } from './panel';
import { useAutosave, type SaveController } from './save';
import {
  defaultText,
  findLayer,
  fullViewport,
  initialState,
  newId,
  outgoingDoc,
  reduce,
  type Action,
  type EditorState,
  type Tool
} from './state';
import type {
  ExportPreview,
  OcrItem,
  OcrPackageStatus,
  OcrStatus,
  PixelRect,
  ProjectDetail,
  ProjectSummary,
  RegionAnalysis,
  RepairGroup,
  Shape,
  TextLayer,
  TextLayout,
  TextSpec
} from './types';
import './styles.css';

/** An image handed over from the results or history page. */
export interface WorkbenchRequest {
  assetId: string;
  token: number;
}

export type LeaveGuard = () => Promise<boolean>;

export interface WorkbenchPageProps {
  lang: Lang;
  request: WorkbenchRequest | null;
  onRequestHandled: () => void;
  onMessage: (text: string, tone?: 'info' | 'error') => void;
  registerLeaveGuard: (guard: LeaveGuard | null) => void;
}

export function WorkbenchPage({ lang, request, onRequestHandled, onMessage, registerLeaveGuard }: WorkbenchPageProps) {
  const c = workbenchCopy[lang];
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [detail, setDetail] = useState<ProjectDetail | null>(null);
  const [collapsed, setCollapsed] = useState(false);
  const [sidebarTab, setSidebarTab] = useState<SidebarTab>('projects');
  const guard = useRef<LeaveGuard | null>(null);

  const refreshProjects = useCallback(async () => {
    try {
      setProjects(await listWorkbenchProjects());
    } catch (error) {
      onMessage(error instanceof Error ? error.message : c.projects.loadFailed, 'error');
    }
  }, [onMessage, c.projects.loadFailed]);

  useEffect(() => {
    void refreshProjects();
  }, [refreshProjects]);

  useEffect(() => {
    registerLeaveGuard(() => (guard.current ? guard.current() : Promise.resolve(true)));
    return () => registerLeaveGuard(null);
  }, [registerLeaveGuard]);

  const leaveCurrent = useCallback(async (): Promise<boolean> => (guard.current ? guard.current() : true), []);
  const onGuard = useCallback((next: LeaveGuard | null) => { guard.current = next; }, []);

  const showDetail = useCallback(
    (next: ProjectDetail) => {
      setDetail(next);
      void refreshProjects();
    },
    [refreshProjects]
  );

  const openById = useCallback(
    async (id: string) => {
      if (detail?.summary.id === id) return;
      if (!(await leaveCurrent())) return;
      try {
        showDetail(await getWorkbenchProject(id));
      } catch (error) {
        onMessage(error instanceof Error ? error.message : c.projects.openFailed, 'error');
      }
    },
    [detail?.summary.id, leaveCurrent, onMessage, showDetail, c.projects.openFailed]
  );

  // Images handed over from other pages.
  const handledToken = useRef<number | null>(null);
  const handlingToken = useRef<number | null>(null);
  useEffect(() => {
    if (!request || handledToken.current === request.token || handlingToken.current === request.token) return;
    (async () => {
      handlingToken.current = request.token;
      if (!(await leaveCurrent())) {
        handlingToken.current = null;
        return;
      }
      try {
        const opened = await openWorkbenchProject({ kind: 'asset', asset_id: request.assetId });
        handledToken.current = request.token;
        showDetail(opened.detail);
        onMessage(opened.resumed ? c.projects.resumed : c.projects.created);
        onRequestHandled();
      } catch (error) {
        handledToken.current = request.token;
        onMessage(error instanceof Error ? error.message : c.projects.openFailed, 'error');
        onRequestHandled();
      } finally {
        handlingToken.current = null;
      }
    })();
  }, [request, leaveCurrent, onMessage, onRequestHandled, showDetail, c.projects.resumed, c.projects.created, c.projects.openFailed]);

  const importImage = async () => {
    const path = await pickImageFile(c.projects.import);
    if (!path) return;
    if (!(await leaveCurrent())) return;
    try {
      const opened = await openWorkbenchProject({ kind: 'file', path });
      showDetail(opened.detail);
      onMessage(opened.resumed ? c.projects.resumed : c.projects.created);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : c.projects.openFailed, 'error');
    }
  };

  const blankProject = async () => {
    if (!detail || !(await leaveCurrent())) return;
    try {
      const opened = await openWorkbenchProject({ kind: 'snapshot', asset_id: detail.summary.asset_id }, true);
      showDetail(opened.detail);
      onMessage(c.projects.created);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : c.projects.openFailed, 'error');
    }
  };

  const copyProject = async () => {
    if (!detail || !(await leaveCurrent())) return;
    try {
      showDetail(await copyWorkbenchProject(detail.summary.id));
      onMessage(c.messages.copyDone);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : c.projects.openFailed, 'error');
    }
  };

  const renameProject = async (id: string, name: string) => {
    if (detail?.summary.id === id && !(await leaveCurrent())) return;
    try {
      const summary = await renameWorkbenchProject(id, name);
      setProjects((current) => current.map((project) => (project.id === id ? summary : project)));
      if (detail?.summary.id === id) {
        setDetail({ ...detail, summary, document: { ...detail.document, name: summary.name, revision: summary.revision } });
      }
    } catch (error) {
      onMessage(error instanceof Error ? error.message : c.projects.openFailed, 'error');
    }
  };

  const removeProject = async (id: string) => {
    if (detail?.summary.id === id && !(await leaveCurrent())) return;
    try {
      await deleteWorkbenchProject(id);
      if (detail?.summary.id === id) setDetail(null);
      onMessage(c.projects.removed);
      await refreshProjects();
    } catch (error) {
      onMessage(error instanceof Error ? error.message : c.projects.removeFailed, 'error');
    }
  };

  const sidebar: Omit<SidebarProps, 'inspector'> = {
    tab: sidebarTab,
    onTab: setSidebarTab,
    collapsed,
    onToggle: () => setCollapsed((value) => !value),
    c,
    projects: {
      projects,
      activeId: detail?.summary.id ?? null,
      onOpen: (id) => void openById(id),
      onImport: () => void importImage(),
      onBlank: () => void blankProject(),
      onCopy: () => void copyProject(),
      onRename: (id, name) => void renameProject(id, name),
      onDelete: (id) => void removeProject(id)
    }
  };

  return (
    <section className="wb-root">
      {detail ? (
        <Editor
          key={detail.summary.id}
          detail={detail}
          c={c}
          sidebar={sidebar}
          onMessage={onMessage}
          onGuard={onGuard}
          onSaved={refreshProjects}
        />
      ) : (
        <>
          <Sidebar {...sidebar} tab="projects" inspector={null} />
          <div className="wb-center wb-placeholder">
            <p>{c.projects.empty}</p>
            <button type="button" className="wb-btn wb-primary" onClick={() => void importImage()}>
              {c.projects.import}
            </button>
          </div>
        </>
      )}
    </section>
  );
}

export interface WorkbenchSettingsProps {
  lang: Lang;
  onMessage: (text: string, tone?: 'info' | 'error') => void;
}

export function WorkbenchSettings({ lang, onMessage }: WorkbenchSettingsProps) {
  const c = workbenchCopy[lang];
  const [storage, setStorage] = useState<import('./types').StorageStats | null>(null);
  const [ocr, setOcr] = useState<OcrPackageStatus | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [nextStorage, nextOcr] = await Promise.all([workbenchStorage(), getOcrPackageStatus()]);
      setStorage(nextStorage);
      setOcr(nextOcr);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : c.settings.refresh, 'error');
    }
  }, [c.settings.refresh, onMessage]);

  useEffect(() => {
    void refresh();
    let stop: (() => void) | null = null;
    let disposed = false;
    void listenOcrProgress((progress) => {
      setOcr((current) => (current ? { ...current, downloading: progress.state === 'downloading' || progress.state === 'verifying', progress } : current));
      if (progress.state === 'done' || progress.state === 'failed' || progress.state === 'cancelled') void refresh();
    }).then((unsubscribe) => {
      if (disposed) unsubscribe();
      else stop = unsubscribe;
    }).catch((error) => onMessage(String(error), 'error'));
    return () => { disposed = true; stop?.(); };
  }, [refresh]);

  const run = async (key: string, action: () => Promise<void>, success?: string) => {
    setBusy(key);
    try {
      await action();
      if (success) onMessage(success);
      await refresh();
    } catch (error) {
      onMessage(error instanceof Error ? error.message : c.settings.refresh, 'error');
    } finally {
      setBusy(null);
    }
  };

  const install = () => run('ocr', async () => { await installOcrPackage(); });
  const remove = () => {
    if (!window.confirm(c.settings.ocrRemoveConfirm)) return;
    return run('ocr', async () => { await ocrClient.dispose(); await removeOcrPackage(); });
  };
  const redownload = () => run('ocr', async () => {
    await ocrClient.dispose();
    if (ocr?.installed) await removeOcrPackage();
    await installOcrPackage();
  });
  const cleanup = () => {
    if (!window.confirm(c.settings.cleanupConfirm)) return;
    return run('cleanup', async () => {
      const result = await cleanupWorkbenchAssets();
      onMessage(c.settings.cleaned(result.removed, formatBytes(result.freed_bytes)));
    });
  };

  return (
    <section className="wb-settings">
      <div className="wb-settings-head">
        <h3>{c.settings.title}</h3>
        <button type="button" className="wb-btn" disabled={busy !== null} onClick={() => void refresh()}>{c.settings.refresh}</button>
      </div>
      <div className="wb-settings-card">
        <strong>{c.settings.storage}</strong>
        {storage ? (
          <>
            <p>{c.settings.storageLine(storage.project_count, formatBytes(storage.project_bytes), storage.asset_count, formatBytes(storage.asset_bytes))}</p>
            <p className="wb-muted">{c.settings.unreferenced(storage.unreferenced_count, formatBytes(storage.unreferenced_bytes))}</p>
            <button type="button" className="wb-btn" disabled={busy !== null || storage.unreferenced_count === 0} onClick={() => void cleanup()}>{c.settings.cleanup}</button>
          </>
        ) : <p className="wb-muted">{c.settings.refresh}</p>}
      </div>
      <div className="wb-settings-card">
        <strong>{c.settings.ocr}</strong>
        {ocr && (
          <p className={ocr.engine.available ? undefined : 'wb-warning'}>
            {ocr.engine.available
              ? c.settings.ocrEngine(ocr.engine.version, ocr.engine.runtime_version, ocr.engine.running)
              : c.settings.ocrEngineMissing(ocr.engine.problem ?? '')}
          </p>
        )}
        <p>
          {ocr?.installed
            ? c.settings.ocrInstalled(ocr.version, formatBytes(ocr.installed_bytes))
            : c.settings.ocrMissing(formatBytes(ocr?.download_bytes ?? 0))}
        </p>
        {ocr?.downloading && ocr.progress && (
          <div className="wb-progress">
            <div className="wb-progress-track"><span style={{ width: `${Math.round((ocr.progress.overall_received / Math.max(1, ocr.progress.overall_total)) * 100)}%` }} /></div>
            <p className="wb-muted">{formatBytes(ocr.progress.overall_received)} / {formatBytes(ocr.progress.overall_total)}</p>
          </div>
        )}
        {ocr && <p className="wb-muted">{c.ocrDialog.location(ocr.directory)} · {c.ocrDialog.license(ocr.license)}</p>}
        {ocr?.progress?.message && <p className="wb-warning">{ocr.progress.message}</p>}
        <div className="wb-settings-row">
          {!ocr?.installed && <button type="button" className="wb-btn wb-primary" disabled={busy !== null || ocr?.downloading} onClick={() => void install()}>{c.settings.ocrInstall}</button>}
          {ocr?.installed && <button type="button" className="wb-btn" disabled={busy !== null || ocr.downloading} onClick={() => void redownload()}>{c.settings.ocrRedownload}</button>}
          {ocr?.installed && <button type="button" className="wb-btn wb-danger" disabled={busy !== null || ocr.downloading} onClick={() => void remove()}>{c.settings.ocrRemove}</button>}
          {ocr?.downloading && <button type="button" className="wb-btn" onClick={() => void run('ocr', async () => { await cancelOcrPackageInstall(); })}>{c.ocrDialog.cancel}</button>}
          <button type="button" className="wb-btn" disabled={busy !== null || ocr?.downloading} onClick={() => void refresh()}>{c.settings.ocrCheck}</button>
        </div>
      </div>
    </section>
  );
}

// ---- editor ----------------------------------------------------------------

interface EditorProps {
  detail: ProjectDetail;
  c: WorkbenchCopy;
  sidebar: Omit<SidebarProps, 'inspector'>;
  onMessage: (text: string, tone?: 'info' | 'error') => void;
  onGuard: (guard: LeaveGuard | null) => void;
  onSaved: () => void;
}

type OcrDialog = { kind: 'closed' } | { kind: 'confirm' } | { kind: 'progress' };

function Editor({ detail, c, sidebar, onMessage, onGuard, onSaved }: EditorProps) {
  const [state, setState] = useState(() => initialState(detail));
  const stateRef = useRef<EditorState>(state);
  const dispatch = useCallback((action: Action) => {
    stateRef.current = reduce(stateRef.current, action);
    setState(stateRef.current);
  }, []);
  const save = useAutosave(stateRef, dispatch);
  const saveRef = useRef<SaveController>(save);
  saveRef.current = save;
  const { width: W, height: H } = state.doc.source;
  const projectId = state.projectId;

  // A rename from the project list arrives through `detail`; adopt it so the
  // next autosave does not write the old name back.
  useEffect(() => {
    if (detail.summary.name !== stateRef.current.doc.name) {
      dispatch({ type: 'renamed', name: detail.summary.name, revision: detail.summary.revision, updatedAt: detail.summary.updated_at });
    }
  }, [detail.summary.name, detail.summary.revision]);

  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const offscreen = useRef<CanvasRenderingContext2D | null>(null);
  const [view, setView] = useState<ViewTransform>({ scale: 0.2, x: 0, y: 0 });
  const [fitRequest, setFitRequest] = useState(1);
  // Selecting or creating something shows its properties; the user can switch back.
  const showProperties = sidebar.onTab;
  const [cropDraft, setCropDraft] = useState<CropDraft | null>(null);
  const [aspect, setAspect] = useState<AspectPreset>('free');
  const [custom, setCustom] = useState({ width: 16, height: 9 });
  const [showOriginal, setShowOriginal] = useState(false);
  const [compare, setCompare] = useState<number | null>(null);
  const [fonts, setFonts] = useState<import('./types').FontInfo[]>([]);
  const [ocrStatus, setOcrStatus] = useState<OcrPackageStatus | null>(null);
  const [ocrDialog, setOcrDialog] = useState<OcrDialog>({ kind: 'closed' });
  const [exportPreview, setExportPreview] = useState<ExportPreview | null>(null);
  const [exporting, setExporting] = useState(false);
  const [exportProgress, setExportProgress] = useState<{ stage: string; percent: number } | null>(null);
  const [leaveDialog, setLeaveDialog] = useState<{ resolve: (ok: boolean) => void } | null>(null);
  const [layouts, setLayouts] = useState<Map<string, { key: string; layout: TextLayout }>>(new Map());
  const requestCounter = useRef(0);
  const groupRequests = useRef(new Map<string, number>());

  // ---- source image ----
  useEffect(() => {
    const img = new Image();
    img.crossOrigin = 'anonymous';
    img.decoding = 'async';
    img.onload = () => {
      setImage(img);
      setFitRequest((value) => value + 1);
    };
    img.onerror = () => onMessage(c.projects.openFailed, 'error');
    img.src = detail.summary.source_url;
    return () => {
      img.onload = null;
      img.onerror = null;
    };
  }, [detail.summary.source_url, onMessage, c.projects.openFailed]);

  const pixels = useCallback((): CanvasRenderingContext2D | null => {
    if (offscreen.current) return offscreen.current;
    if (!image) return null;
    const canvas = document.createElement('canvas');
    canvas.width = W;
    canvas.height = H;
    const ctx = canvas.getContext('2d', { willReadFrequently: true });
    if (!ctx) return null;
    ctx.fillStyle = '#ffffff';
    ctx.fillRect(0, 0, W, H);
    ctx.drawImage(image, 0, 0);
    offscreen.current = ctx;
    return ctx;
  }, [image, W, H]);

  // ---- OCR package status & fonts ----
  const refreshOcr = useCallback(async () => {
    try {
      setOcrStatus(await getOcrPackageStatus());
    } catch {
      /* status is advisory */
    }
  }, []);
  useEffect(() => {
    void refreshOcr();
    let unlisten: (() => void) | null = null;
    let disposed = false;
    void listenOcrProgress((progress) => {
      setOcrStatus((current) => (current ? { ...current, downloading: progress.state === 'downloading' || progress.state === 'verifying', progress } : current));
      if (progress.state === 'done' || progress.state === 'failed' || progress.state === 'cancelled') void refreshOcr();
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch((error) => onMessage(String(error), 'error'));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refreshOcr, onMessage]);

  useEffect(() => () => { void ocrClient.dispose(); }, []);

  const selectedText = useMemo(() => {
    const located = state.selection.id ? findLayer(state.doc, state.selection.id) : null;
    return located?.layer.kind === 'text' ? located.layer : null;
  }, [state.selection.id, state.doc]);

  useEffect(() => {
    const sample = selectedText?.text ?? undefined;
    const timer = window.setTimeout(() => {
      listWorkbenchFonts(sample)
        .then(setFonts)
        .catch(() => {});
    }, 300);
    return () => window.clearTimeout(timer);
  }, [selectedText?.text]);

  const defaultFamily = useMemo(() => fonts.find((font) => font.bundled)?.family ?? fonts[0]?.family ?? 'Noto Sans SC', [fonts]);

  const finishCropRef = useRef<() => void>(() => {});
  const leaving = useRef<Promise<boolean> | null>(null);
  useEffect(() => {
    onGuard(() => {
      if (leaving.current) return leaving.current;
      const run = async () => {
        finishCropRef.current();
        if (stateRef.current.transientBase) dispatch({ type: 'end_transient' });
        if (await saveRef.current.flush()) return true;
        return new Promise<boolean>((resolve) => setLeaveDialog({ resolve }));
      };
      leaving.current = run().finally(() => { leaving.current = null; });
      return leaving.current;
    });
    return () => onGuard(null);
  }, [onGuard, dispatch]);

  useEffect(() => {
    if (save.status === 'saved') onSaved();
  }, [save.status, onSaved]);

  // ---- text layout calibration (Rust measurement) ----
  useEffect(() => {
    if (state.transientBase) return;
    const texts: TextLayer[] = [];
    for (const layer of state.doc.layers) {
      if (layer.kind === 'repair') texts.push(...layer.children);
      else texts.push(layer);
    }
    if (state.pending) texts.push(...state.pending.texts);
    const timer = window.setTimeout(() => {
      const missing = texts.filter((text) => {
        const key = specKey(text);
        return layouts.get(text.id)?.key !== key;
      });
      if (missing.length === 0 || missing.length > 200) return;
      Promise.all(
        missing.map(async (text) => {
          const key = specKey(text);
          try {
            const layout = await measureWorkbenchText(toSpec(text));
            return [text.id, { key, layout }] as const;
          } catch {
            return null;
          }
        })
      ).then((results) => {
        setLayouts((current) => {
          const next = new Map(current);
          for (const entry of results) if (entry) next.set(entry[0], entry[1]);
          return next;
        });
        // Auto-fit texts adopt the size Rust settled on, so the panel shows
        // the real size and export/preview agree. Pending (unapplied) texts
        // are fitted once they are committed.
        for (const entry of results) {
          if (!entry) continue;
          const located = findLayer(stateRef.current.doc, entry[0]);
          if (!located || located.layer.kind !== 'text' || !located.layer.auto_fit) continue;
          if (Math.abs(entry[1].layout.font_size - located.layer.font.size) >= 0.25) {
            dispatch({ type: 'fit_text_size', id: entry[0], size: entry[1].layout.font_size });
          }
        }
      });
    }, 220);
    return () => window.clearTimeout(timer);
  }, [state.doc, state.pending, state.transientBase, layouts, dispatch]);

  const calibrated = useMemo(() => {
    const map = new Map<string, TextLayout>();
    const consider = (text: TextLayer) => {
      const entry = layouts.get(text.id);
      if (entry && entry.key === specKey(text)) map.set(text.id, entry.layout);
    };
    for (const layer of state.doc.layers) {
      if (layer.kind === 'repair') layer.children.forEach(consider);
      else consider(layer);
    }
    state.pending?.texts.forEach(consider);
    return map;
  }, [layouts, state.doc, state.pending]);

  // ---- region analysis + OCR ----
  // The host crops the snapshot itself and pads it with the detected
  // background (D1); polygons come back relative to `rect`.
  const recognizeRegion = useCallback(
    async (rect: PixelRect, background: string, requestId: number): Promise<OcrItem[]> => {
      const status = ocrStatus;
      if (!status?.installed) throw new Error(c.pending.unavailable);
      return ocrClient.recognize(status, projectId, rect, background, requestId);
    },
    [ocrStatus, projectId, c.pending.unavailable]
  );

  const buildTexts = useCallback(
    async (items: OcrItem[], region: PixelRect, analysis: RegionAnalysis): Promise<{ texts: TextLayer[]; rejected: number }> => {
      const texts: TextLayer[] = [];
      let rejected = 0;
      for (const item of items) {
        const verdict = judgeLine(item.text, item.score);
        if (!verdict.accept) {
          if (verdict.reason !== 'empty') rejected += 1;
          continue;
        }
        const { rect: box, angle } = polyBounds(item.poly);
        const padX = Math.max(2, box.height * 0.15);
        const rect = roundRect(
          { x: region.x + box.x - padX, y: region.y + box.y - 1, width: box.width + padX * 2, height: box.height + 2 },
          W,
          H
        );
        let color = luminance(analysis.color) > 0.5 ? '#000000' : '#ffffff';
        try {
          const line = await analyzeWorkbenchRegion(projectId, rect, 'rect');
          const ink = line.candidates
            .filter((candidate) => candidate.coverage >= 0.04)
            .map((candidate) => ({ ...candidate, distance: colorDistance(candidate.color, analysis.color) }))
            .filter((candidate) => candidate.distance > 60)
            .sort((a, b) => b.distance * Math.sqrt(b.coverage) - a.distance * Math.sqrt(a.coverage))[0];
          if (ink) color = ink.color;
        } catch {
          /* keep the contrast default */
        }
        const regionCenter = region.x + region.width / 2;
        const lineCenter = rect.x + rect.width / 2;
        const centered = Math.abs(lineCenter - regionCenter) < region.width * 0.08 && rect.x - region.x > region.width * 0.12;
        texts.push({
          kind: 'text',
          id: newId(),
          visible: true,
          text: verdict.text,
          rect,
          font: { family: defaultFamily, size: Math.max(4, Math.round(box.height * 0.8 * 4) / 4), weight: 400, italic: false },
          color,
          align: centered ? 'center' : 'left',
          valign: 'middle',
          line_height: 1.2,
          letter_spacing: 0,
          auto_fit: true,
          angle,
          origin: 'ocr',
          edited: false,
          score: item.score
        });
      }
      return { texts, rejected };
    },
    [W, H, projectId, defaultFamily]
  );

  const runPending = useCallback(
    async (requestId: number, rect: PixelRect, shape: Shape) => {
      const stale = () => stateRef.current.pending?.requestId !== requestId;
      let analysis: RegionAnalysis;
      try {
        analysis = await analyzeWorkbenchRegion(projectId, rect, shape);
      } catch (error) {
        if (stale()) return;
        onMessage(error instanceof Error ? error.message : c.messages.analyzeFailed, 'error');
        dispatch({ type: 'pending_cancel' });
        return;
      }
      if (stale()) return;
      dispatch({ type: 'pending_analysis', requestId, analysis });
      if (!ocrStatus?.installed) {
        dispatch({ type: 'pending_ocr', requestId, ocr: 'unavailable', ocrStatus: 'unavailable', texts: [] });
        return;
      }
      dispatch({ type: 'pending_ocr', requestId, ocr: 'running', ocrStatus: 'pending', texts: [] });
      try {
        const items = await recognizeRegion(rect, analysis.color, requestId);
        if (stale()) return;
        const { texts, rejected } = await buildTexts(items, rect, analysis);
        if (stale()) return;
        const status: OcrStatus = texts.length > 0 ? 'found' : rejected > 0 ? 'unreliable' : 'none';
        dispatch({ type: 'pending_ocr', requestId, ocr: 'done', ocrStatus: status, texts });
      } catch (error) {
        if (stale() || error instanceof OcrSkipped) return;
        dispatch({ type: 'pending_ocr', requestId, ocr: 'failed', ocrStatus: 'failed', texts: [] });
        onMessage(error instanceof Error ? error.message : c.messages.ocrFailed, 'error');
      }
    },
    [projectId, ocrStatus?.installed, recognizeRegion, buildTexts, onMessage, c.messages.analyzeFailed, c.messages.ocrFailed]
  );

  const startPending = (shape: Shape, rect: PixelRect) => {
    const requestId = ++requestCounter.current;
    dispatch({ type: 'pending_start', shape, rect, requestId });
    dispatch({ type: 'set_tool', tool: 'select' });
    void runPending(requestId, rect, shape);
  };

  // Dropping the preview also tells the host to stop recognising it, so a
  // long OCR does not keep the CPU busy for a selection nobody wants.
  const cancelPending = useCallback(() => {
    const pending = stateRef.current.pending;
    if (pending?.ocr === 'running') void ocrClient.cancel(pending.requestId);
    dispatch({ type: 'pending_cancel' });
  }, [dispatch]);

  const movePending = (rect: PixelRect) => {
    const pending = stateRef.current.pending;
    if (!pending) return;
    const requestId = ++requestCounter.current;
    dispatch({ type: 'pending_rect', rect, requestId });
    void runPending(requestId, rect, pending.shape);
  };

  const applyPending = () => {
    dispatch({ type: 'pending_apply', groupId: newId() });
    showProperties('properties');
    window.setTimeout(() => void saveRef.current.flush(), 0);
  };

  const reanalyzeGroup = useCallback(
    async (groupId: string, allowOcr = true, fold = true) => {
      const located = findLayer(stateRef.current.doc, groupId);
      if (!located || located.layer.kind !== 'repair') return;
      const group = located.layer as RepairGroup;
      const expectDoc = fold ? stateRef.current.doc : null;
      const requestId = ++requestCounter.current;
      groupRequests.current.set(groupId, requestId);
      try {
        const analysis = await analyzeWorkbenchRegion(projectId, group.rect, group.shape);
        let texts: TextLayer[] | null = null;
        let status: OcrStatus | null = null;
        if (allowOcr && !group.ocr.edited && ocrStatus?.installed) {
          try {
            const items = await recognizeRegion(group.rect, analysis.color, requestId);
            const built = await buildTexts(items, group.rect, analysis);
            texts = built.texts;
            status = built.texts.length > 0 ? 'found' : built.rejected > 0 ? 'unreliable' : 'none';
          } catch (error) {
            if (error instanceof OcrSkipped) return;
            onMessage(error instanceof Error ? error.message : c.messages.ocrFailed, 'error');
          }
        }
        const current = findLayer(stateRef.current.doc, groupId);
        if (!current || current.layer.kind !== 'repair' || current.layer !== group || groupRequests.current.get(groupId) !== requestId) return;
        dispatch({ type: 'reanalyzed', groupId, analysis, texts, ocrStatus: status, expectDoc, expectedGroup: group });
      } catch (error) {
        onMessage(error instanceof Error ? error.message : c.messages.analyzeFailed, 'error');
      }
    },
    [projectId, ocrStatus?.installed, recognizeRegion, buildTexts, onMessage, c.messages.analyzeFailed]
  );

  const reocrGroup = async (groupId: string) => {
    const located = findLayer(stateRef.current.doc, groupId);
    if (!located || located.layer.kind !== 'repair') return;
    const group = located.layer as RepairGroup;
    if (!ocrStatus?.installed) {
      setOcrDialog({ kind: 'confirm' });
      return;
    }
    if ((group.ocr.edited || group.children.length > 0) && !window.confirm(c.inspector.reocrConfirm)) return;
    const requestId = ++requestCounter.current;
    groupRequests.current.set(groupId, requestId);
    try {
      const analysis = await analyzeWorkbenchRegion(projectId, group.rect, group.shape);
      const items = await recognizeRegion(group.rect, analysis.color, requestId);
      const built = await buildTexts(items, group.rect, analysis);
      if (groupRequests.current.get(groupId) !== requestId) return;
      dispatch({ type: 'reocr', groupId, expectedGroup: group, texts: built.texts, ocrStatus: built.texts.length > 0 ? 'found' : built.rejected > 0 ? 'unreliable' : 'none' });
    } catch (error) {
      if (error instanceof OcrSkipped) return;
      onMessage(error instanceof Error ? error.message : c.messages.ocrFailed, 'error');
    }
  };

  // ---- layer geometry from the canvas ----
  const onLayerRect = (id: string, rect: PixelRect, phase: 'move' | 'end') => {
    if (phase === 'move') {
      dispatch({ type: 'begin_transient' });
      dispatch({ type: 'set_layer_rect', id, rect, transient: true });
      return;
    }
    dispatch({ type: 'set_layer_rect', id, rect, transient: true });
    dispatch({ type: 'end_transient' });
    const located = findLayer(stateRef.current.doc, id);
    if (located?.layer.kind === 'repair') void reanalyzeGroup(id);
  };

  // ---- text creation ----
  const addTextTo = (groupId: string) => {
    const located = findLayer(state.doc, groupId);
    if (!located || located.layer.kind !== 'repair') return;
    const group = located.layer;
    const height = Math.max(8, Math.min(Math.round(group.rect.height * 0.6), 64));
    const width = Math.max(16, Math.round(group.rect.width * 0.8));
    const rect = roundRect(
      { x: group.rect.x + (group.rect.width - width) / 2, y: group.rect.y + (group.rect.height - height) / 2, width, height },
      W,
      H
    );
    const color = luminance(group.fill.current) > 0.5 ? '#000000' : '#ffffff';
    dispatch({ type: 'add_text', text: defaultText(rect, c.inspector.textLabel, defaultFamily, color), parentId: groupId });
    showProperties('properties');
  };

  const onTextDraw = (rect: PixelRect) => {
    const selected = state.selection.id ? findLayer(state.doc, state.selection.id) : null;
    const parent = selected?.layer.kind === 'repair' && intersects(rect, selected.layer.rect) ? selected.layer.id : null;
    dispatch({ type: 'add_text', text: defaultText(rect, c.inspector.textLabel, defaultFamily, '#000000'), parentId: parent });
    dispatch({ type: 'set_tool', tool: 'select' });
    showProperties('properties');
  };

  const onEyedrop = (x: number, y: number) => {
    const ctx = pixels();
    if (!ctx) return;
    const [r, g, b] = ctx.getImageData(x, y, 1, 1).data;
    const color = `#${[r, g, b].map((v) => v.toString(16).padStart(2, '0')).join('')}`;
    const located = state.selection.id ? findLayer(state.doc, state.selection.id) : null;
    if (located?.layer.kind === 'repair') dispatch({ type: 'set_fill', id: located.layer.id, color });
    else if (located?.layer.kind === 'text') dispatch({ type: 'update_text', id: located.layer.id, patch: { color } });
    dispatch({ type: 'set_tool', tool: 'select' });
  };

  // ---- crop mode ----
  const ratio = aspectRatio(aspect, { width: W, height: H }, custom);
  const setTool = (tool: Tool) => {
    if (tool === 'crop') {
      showProperties('properties');
      setCropDraft({ crop: state.doc.viewport.crop, flip_x: state.doc.viewport.flip_x, flip_y: state.doc.viewport.flip_y, ratio });
      cancelPending();
    } else if (cropDraft) {
      finishCrop();
    }
    dispatch({ type: 'set_tool', tool });
  };
  const finishCrop = () => {
    if (cropDraft) {
      const viewport = { crop: cropDraft.crop, flip_x: cropDraft.flip_x, flip_y: cropDraft.flip_y };
      const current = state.doc.viewport;
      if (
        viewport.flip_x !== current.flip_x ||
        viewport.flip_y !== current.flip_y ||
        viewport.crop.x !== current.crop.x ||
        viewport.crop.y !== current.crop.y ||
        viewport.crop.width !== current.crop.width ||
        viewport.crop.height !== current.crop.height
      ) {
        dispatch({ type: 'set_viewport', viewport });
        window.setTimeout(() => void saveRef.current.flush(), 0);
      }
    }
    setCropDraft(null);
    dispatch({ type: 'set_tool', tool: 'select' });
  };
  finishCropRef.current = () => { if (cropDraft) finishCrop(); };
  const cropControls = cropDraft
    ? {
        draft: cropDraft,
        aspect,
        custom,
        onAspect: (next: AspectPreset) => {
          setAspect(next);
          const r = aspectRatio(next, { width: W, height: H }, custom);
          setCropDraft({ ...cropDraft, ratio: r, crop: constrainCrop(cropDraft.crop, r, W, H, 'both') });
        },
        onCustom: (next: { width: number; height: number }) => {
          setCustom(next);
          const r = aspectRatio('custom', { width: W, height: H }, next);
          setCropDraft({ ...cropDraft, ratio: r, crop: constrainCrop(cropDraft.crop, r, W, H, 'both') });
        },
        onRect: (field: 'x' | 'y' | 'width' | 'height', value: number) => {
          const next = { ...cropDraft.crop, [field]: Math.round(value) };
          const changed = field === 'width' ? 'width' : field === 'height' ? 'height' : 'both';
          setCropDraft({
            ...cropDraft,
            crop: field === 'x' || field === 'y' ? constrainCrop(next, null, W, H) : constrainCrop(next, cropDraft.ratio, W, H, changed)
          });
        },
        onFlip: (axis: 'x' | 'y') => setCropDraft({ ...cropDraft, flip_x: axis === 'x' ? !cropDraft.flip_x : cropDraft.flip_x, flip_y: axis === 'y' ? !cropDraft.flip_y : cropDraft.flip_y }),
        onReset: () => {
          setAspect('free');
          setCropDraft({ ...cropDraft, ratio: null, ...fullViewport(state.doc) });
        },
        onDone: finishCrop
      }
    : null;

  // ---- restore / export / ocr install ----
  const restoreOriginal = () => {
    if (!window.confirm(c.restoreConfirm)) return;
    setCropDraft(null);
    dispatch({ type: 'restore_original' });
    onMessage(c.messages.restoreDone);
    window.setTimeout(() => void saveRef.current.flush(), 0);
  };

  const openExport = async () => {
    if (cropDraft) finishCrop();
    if (!(await saveRef.current.flush())) {
      onMessage(c.exportDialog.needSave, 'error');
      return;
    }
    try {
      setExportPreview(await previewWorkbenchExport(projectId, outgoingDoc(stateRef.current)));
    } catch (error) {
      onMessage(error instanceof Error ? error.message : c.exportDialog.failed, 'error');
    }
  };

  const confirmExport = async () => {
    const stem = state.doc.source.filename.replace(/\.[^.]+$/, '') || 'image';
    const path = await pickSavePath(c.exportDialog.defaultName(stem));
    if (!path) return;
    setExporting(true);
    setExportProgress({ stage: 'compose', percent: 0 });
    // Progress events are tagged with the project so a stale export of another
    // project (or a superseded run) cannot drive this bar.
    const stop = await listenExportProgress((progress) => {
      if (progress.project_id === projectId) setExportProgress({ stage: progress.stage, percent: progress.percent });
    });
    try {
      if (!(await saveRef.current.flush())) throw new Error(c.exportDialog.needSave);
      const record = await exportWorkbenchProject(projectId, outgoingDoc(stateRef.current), path);
      onMessage(c.exportDialog.done(`${record.filename}（${record.width} × ${record.height}）`));
      setExportPreview(null);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : c.exportDialog.failed, 'error');
    } finally {
      stop();
      setExporting(false);
      setExportProgress(null);
    }
  };

  const startInstall = async () => {
    try {
      await installOcrPackage();
      setOcrDialog({ kind: 'progress' });
      await refreshOcr();
    } catch (error) {
      onMessage(error instanceof Error ? error.message : c.ocrDialog.failed, 'error');
    }
  };

  // ---- keyboard ----
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (leaveDialog || exportPreview || ocrDialog.kind !== 'closed') return;
      const target = event.target as HTMLElement | null;
      const typing = target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement || target?.isContentEditable;
      const mod = event.metaKey || event.ctrlKey;
      if (mod && event.key.toLowerCase() === 'z') {
        if (typing) return;
        event.preventDefault();
        dispatch({ type: event.shiftKey ? 'redo' : 'undo' });
        return;
      }
      if (mod && event.key.toLowerCase() === 'y' && !typing) {
        event.preventDefault();
        dispatch({ type: 'redo' });
        return;
      }
      if (event.key === 'Escape') {
        if (leaveDialog || exportPreview || ocrDialog.kind !== 'closed') return;
        if (stateRef.current.pending) cancelPending();
        else if (cropDraft) {
          setCropDraft(null);
          dispatch({ type: 'set_tool', tool: 'select' });
        } else dispatch({ type: 'select', id: null });
        return;
      }
      if (typing || mod) return;
      const key = event.key.toLowerCase();
      if (key === 'v') setTool('select');
      else if (key === 'r') setTool('rect');
      else if (key === 'e') setTool('ellipse');
      else if (key === 'i') setTool('eyedropper');
      else if (key === 't') setTool('text');
      else if (key === 'c') setTool('crop');
      else if ((event.key === 'Delete' || event.key === 'Backspace') && stateRef.current.selection.id) {
        dispatch({ type: 'remove_layer', id: stateRef.current.selection.id });
        window.setTimeout(() => void saveRef.current.flush(), 0);
      } else if (event.key === 'Enter' && stateRef.current.pending?.analysis) {
        applyPending();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  const pending = state.pending;
  const zoomPercent = Math.round(view.scale * 100);
  const saveLabel = save.status === 'saving' ? c.status.saving : save.status === 'failed' ? c.status.failed : save.status === 'dirty' ? c.status.dirty : save.status === 'saved' ? c.status.saved : c.status.clean;

  return (
    <>
      <Sidebar
        {...sidebar}
        inspector={{
          state,
          dispatch,
          fonts,
          layouts: calibrated,
          crop: cropControls,
          c,
          onReanalyze: (groupId) => void reanalyzeGroup(groupId, false, false),
          onGroupRect: (id, rect) => {
            dispatch({ type: 'set_layer_rect', id, rect });
            void reanalyzeGroup(id);
          },
          onReocr: (groupId) => void reocrGroup(groupId),
          onAddText: addTextTo,
          onEyedropper: () => dispatch({ type: 'set_tool', tool: 'eyedropper' })
        }}
      />
      <div className="wb-center">
        <div className="wb-toolbar" role="toolbar">
          {(['select', 'rect', 'ellipse', 'eyedropper', 'text', 'crop'] as Tool[]).map((tool) => (
            <button key={tool} type="button" className={`wb-tool${state.tool === tool ? ' active' : ''}`} title={c.tools[tool]} aria-label={c.tools[tool]} onClick={() => setTool(tool)}>
              <ToolIcon tool={tool} />
            </button>
          ))}
          <span className="wb-toolbar-sep" />
          <button type="button" className="wb-tool" title={c.tools.undo} disabled={state.past.length === 0} onClick={() => dispatch({ type: 'undo' })}>
            ↶
          </button>
          <button type="button" className="wb-tool" title={c.tools.redo} disabled={state.future.length === 0} onClick={() => dispatch({ type: 'redo' })}>
            ↷
          </button>
          <span className="wb-toolbar-sep" />
          <button type="button" className="wb-tool" title={c.tools.zoomOut} onClick={() => setView((v) => zoomAt(v, 1 / 1.25, 400, 300))}>
            −
          </button>
          <button type="button" className="wb-tool" title={c.tools.zoomIn} onClick={() => setView((v) => zoomAt(v, 1.25, 400, 300))}>
            +
          </button>
          <button type="button" className="wb-tool wb-tool-text" title={c.tools.fit} onClick={() => setFitRequest((v) => v + 1)}>
            {c.tools.fit}
          </button>
          <span className="wb-toolbar-sep" />
          <button
            type="button"
            className={`wb-tool wb-tool-text${showOriginal ? ' active' : ''}`}
            title={c.tools.original}
            onPointerDown={() => setShowOriginal(true)}
            onPointerUp={() => setShowOriginal(false)}
            onPointerLeave={() => setShowOriginal(false)}
          >
            {c.tools.original}
          </button>
          <button type="button" className={`wb-tool wb-tool-text${compare !== null ? ' active' : ''}`} title={c.tools.compare} onClick={() => setCompare((value) => (value === null ? 0.5 : null))}>
            {c.tools.compare}
          </button>
          <span className="wb-toolbar-spacer" />
          <button type="button" className="wb-tool wb-tool-text" title={c.tools.restore} onClick={restoreOriginal}>
            {c.tools.restore}
          </button>
          <button type="button" className="wb-btn wb-primary" onClick={() => void openExport()}>
            {c.tools.export}
          </button>
        </div>
        <div className="wb-stage">
          <WorkbenchCanvas
            doc={state.doc}
            image={image}
            view={view}
            onView={setView}
            fitRequest={fitRequest}
            tool={state.tool}
            selection={state.selection}
            pending={pending}
            cropDraft={cropDraft}
            showOriginal={showOriginal}
            compare={compare}
            layouts={calibrated}
            onSelect={(id, parentId) => dispatch({ type: 'select', id, parentId })}
            onDraw={startPending}
            onPendingRect={movePending}
            onLayerRect={onLayerRect}
            onTextDraw={onTextDraw}
            onEyedrop={onEyedrop}
            onCropDraft={(crop) => cropDraft && setCropDraft({ ...cropDraft, crop })}
            onCompare={setCompare}
          />
          {pending && (
            <div className="wb-pending" role="status">
              <span>
                {pending.analyzing
                  ? c.pending.analyzing
                  : pending.ocr === 'running'
                    ? c.pending.ocrRunning
                    : pending.ocr === 'unavailable'
                      ? c.pending.unavailable
                      : pending.ocr === 'failed'
                        ? c.pending.failed
                        : pending.texts.length > 0
                          ? c.pending.found(pending.texts.length)
                          : pending.ocr === 'done'
                            ? c.pending.none
                            : ''}
                {pending.analysis?.uneven && <span className="wb-chip wb-chip-warn">{c.pending.uneven}</span>}
              </span>
              <span className="wb-pending-actions">
                {pending.ocr === 'unavailable' && (
                  <button type="button" className="wb-btn" onClick={() => setOcrDialog({ kind: 'confirm' })}>
                    {c.pending.install}
                  </button>
                )}
                <button type="button" className="wb-btn" onClick={cancelPending}>
                  {c.pending.cancel}
                </button>
                <button type="button" className="wb-btn wb-primary" disabled={!pending.analysis || pending.analyzing} onClick={applyPending}>
                  {c.pending.apply}
                </button>
              </span>
            </div>
          )}
        </div>
        <div className="wb-statusbar">
          <span>{c.status.zoom(zoomPercent)}</span>
          <span>{c.status.source(W, H)}</span>
          <span>{c.status.crop(cropDraft?.crop.width ?? state.doc.viewport.crop.width, cropDraft?.crop.height ?? state.doc.viewport.crop.height)}</span>
          <span className="wb-toolbar-spacer" />
          <span className={`wb-save wb-save-${save.status}`} title={save.error ?? undefined}>
            {saveLabel}
            {save.status === 'failed' && (
              <button type="button" className="wb-btn wb-small" onClick={() => void save.flush()}>
                {c.status.retry}
              </button>
            )}
          </span>
        </div>
      </div>

      {exportPreview && (
        <Modal title={c.exportDialog.title} onClose={() => setExportPreview(null)}>
          <p>{c.exportDialog.size(exportPreview.width, exportPreview.height)}</p>
          {exportPreview.missing_fonts.length > 0 && <p className="wb-warning">{c.exportDialog.missingFonts(exportPreview.missing_fonts.join(', '))}</p>}
          {exportPreview.color_note && <p className="wb-warning">{exportPreview.color_note}</p>}
          {exportPreview.had_alpha && <p className="wb-muted">{c.exportDialog.alpha}</p>}
          {exporting && exportProgress ? (
            <div className="wb-progress" role="progressbar" aria-valuenow={exportProgress.percent} aria-valuemin={0} aria-valuemax={100}>
              <div className="wb-progress-track">
                <span style={{ width: `${exportProgress.percent}%` }} />
              </div>
              <p className="wb-muted">
                {c.exportDialog.progress[exportProgress.stage] ?? exportProgress.stage} {exportProgress.percent}%
              </p>
            </div>
          ) : (
            <div className="wb-modal-actions">
              <button type="button" className="wb-btn" onClick={() => setExportPreview(null)}>
                {c.exportDialog.cancel}
              </button>
              <button type="button" className="wb-btn wb-primary" disabled={exporting || exportPreview.color_note !== null} onClick={() => void confirmExport()}>
                {c.exportDialog.confirm}
              </button>
            </div>
          )}
        </Modal>
      )}

      {ocrDialog.kind !== 'closed' && ocrStatus && (
        <Modal title={c.ocrDialog.title} onClose={() => setOcrDialog({ kind: 'closed' })}>
          <p>{c.ocrDialog.body(formatBytes(ocrStatus.download_bytes))}</p>
          <p className="wb-muted">{c.ocrDialog.location(ocrStatus.directory)}</p>
          <p className="wb-muted">{c.ocrDialog.license(ocrStatus.license)}</p>
          {ocrDialog.kind === 'progress' && ocrStatus.progress && (
            <div className="wb-progress">
              <div className="wb-progress-track">
                <span style={{ width: `${Math.round((ocrStatus.progress.overall_received / Math.max(1, ocrStatus.progress.overall_total)) * 100)}%` }} />
              </div>
              <p>
                {ocrStatus.progress.state === 'downloading' && c.ocrDialog.downloading(formatBytes(ocrStatus.progress.overall_received), formatBytes(ocrStatus.progress.overall_total))}
                {ocrStatus.progress.state === 'verifying' && c.ocrDialog.verifying}
                {ocrStatus.progress.state === 'done' && c.ocrDialog.done}
                {ocrStatus.progress.state === 'failed' && `${c.ocrDialog.failed}：${ocrStatus.progress.message ?? ''}`}
                {ocrStatus.progress.state === 'cancelled' && c.ocrDialog.cancelled}
              </p>
            </div>
          )}
          <div className="wb-modal-actions">
            {ocrStatus.downloading ? (
              <button type="button" className="wb-btn" onClick={() => void cancelOcrPackageInstall().catch(() => {})}>
                {c.ocrDialog.cancel}
              </button>
            ) : ocrStatus.installed ? (
              <button type="button" className="wb-btn wb-primary" onClick={() => setOcrDialog({ kind: 'closed' })}>
                {c.ocrDialog.close}
              </button>
            ) : (
              <>
                <button type="button" className="wb-btn" onClick={() => setOcrDialog({ kind: 'closed' })}>
                  {c.ocrDialog.cancel}
                </button>
                <button type="button" className="wb-btn wb-primary" onClick={() => void startInstall()}>
                  {ocrStatus.progress?.state === 'failed' || ocrStatus.progress?.state === 'cancelled' ? c.ocrDialog.retry : c.ocrDialog.install}
                </button>
              </>
            )}
          </div>
        </Modal>
      )}

      {leaveDialog && (
        <Modal title={c.leave.title} onClose={() => {}}>
          <p className="wb-warning">{c.leave.body(save.error ?? '')}</p>
          <div className="wb-modal-actions">
            <button
              type="button"
              className="wb-btn"
              onClick={() => {
                leaveDialog.resolve(false);
                setLeaveDialog(null);
              }}
            >
              {c.leave.stay}
            </button>
            <button
              type="button"
              className="wb-btn wb-danger"
              onClick={async () => {
                try {
                  await save.discard();
                  leaveDialog.resolve(true);
                } catch (error) {
                  onMessage(error instanceof Error ? error.message : c.status.failed, 'error');
                  leaveDialog.resolve(false);
                }
                setLeaveDialog(null);
              }}
            >
              {c.leave.discard}
            </button>
            <button
              type="button"
              className="wb-btn wb-primary"
              onClick={async () => {
                const ok = await save.flush();
                if (ok) {
                  leaveDialog.resolve(true);
                  setLeaveDialog(null);
                }
              }}
            >
              {c.leave.retry}
            </button>
          </div>
        </Modal>
      )}
    </>
  );
}

function Modal({ title, children, onClose }: { title: string; children: React.ReactNode; onClose: () => void }) {
  return (
    <div className="wb-modal-backdrop" role="dialog" aria-modal="true" aria-label={title} onClick={onClose}>
      <div className="wb-modal" onClick={(event) => event.stopPropagation()}>
        <h3>{title}</h3>
        {children}
      </div>
    </div>
  );
}

function ToolIcon({ tool }: { tool: Tool }) {
  switch (tool) {
    case 'select':
      return (
        <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
          <path d="M5 3l10 7-4.5 1L8 16z" strokeLinejoin="round" />
        </svg>
      );
    case 'rect':
      return (
        <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
          <rect x="3.5" y="5" width="13" height="10" rx="1" />
        </svg>
      );
    case 'ellipse':
      return (
        <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
          <ellipse cx="10" cy="10" rx="6.5" ry="5" />
        </svg>
      );
    case 'eyedropper':
      return (
        <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
          <path d="M12.5 4.5l3 3M4 16l1-3.5 7-7 2.5 2.5-7 7z" strokeLinejoin="round" />
        </svg>
      );
    case 'text':
      return (
        <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
          <path d="M4.5 5h11M10 5v11M7.5 16h5" strokeLinecap="round" />
        </svg>
      );
    default:
      return (
        <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
          <path d="M6 2.5v11.5h11.5M2.5 6h11.5v11.5" strokeLinecap="round" />
        </svg>
      );
  }
}

function toSpec(text: TextLayer): TextSpec {
  return {
    text: text.text,
    width: text.rect.width,
    height: text.rect.height,
    family: text.font.family,
    size: text.font.size,
    weight: text.font.weight,
    italic: text.font.italic,
    line_height: text.line_height,
    letter_spacing: text.letter_spacing,
    align: text.align,
    valign: text.valign,
    auto_fit: text.auto_fit
  };
}

function specKey(text: TextLayer): string {
  return JSON.stringify(toSpec(text));
}

function intersects(a: PixelRect, b: PixelRect): boolean {
  return a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
}

function luminance(hex: string): number {
  const value = hex.replace('#', '');
  if (value.length !== 6) return 1;
  const channel = (i: number) => parseInt(value.slice(i, i + 2), 16) / 255;
  return 0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4);
}

function colorDistance(a: string, b: string): number {
  const parse = (hex: string) => {
    const v = hex.replace('#', '');
    return [0, 2, 4].map((i) => parseInt(v.slice(i, i + 2), 16));
  };
  const [r1, g1, b1] = parse(a);
  const [r2, g2, b2] = parse(b);
  return Math.sqrt((r1 - r2) ** 2 + (g1 - g2) ** 2 + (b1 - b2) ** 2);
}
