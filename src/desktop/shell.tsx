import { useEffect, useMemo, useState, type JSX } from 'react';
import { createJob, deleteJob, getConfig, getJob, listJobs, listTemplates, saveConfig } from '../api';
import { copy, emptyConfig, isJobSettled, Settings, useJobPolling, type Lang } from '../app';
import type { AppConfig, JobRecord } from '../types';
import { desktopCopy, type DesktopCopy } from './copy';
import {
  defaultFigureForm,
  defaultSlideForm,
  FigureForm,
  SettingsPane,
  SlideForm,
  type FigureFormState,
  type SlideFormState
} from './forms';
import { TemplateLibrary } from './templates';

type Page = 'paper' | 'ppt' | 'templates' | 'history' | 'settings';
type Copy = (typeof copy)[Lang];

const supportsViewTransitions = typeof document !== 'undefined' && 'startViewTransition' in document;

export function DesktopApp() {
  const [page, setPage] = useState<Page>('paper');
  const [lang, setLang] = useState<Lang>('zh');
  const [config, setConfig] = useState<AppConfig>(emptyConfig);
  const [toast, setToast] = useState<{ id: number; text: string; tone: 'info' | 'error' } | null>(null);
  const [paperJob, setPaperJob] = useState<JobRecord | null>(null);
  const [pptJob, setPptJob] = useState<JobRecord | null>(null);
  const [figureState, setFigureState] = useState<FigureFormState>(defaultFigureForm);
  const [slideState, setSlideState] = useState<SlideFormState>(defaultSlideForm);
  async function deleteSettled(job: JobRecord, refresh?: () => void) {
    try {
      await deleteJob(job.id);
      refresh?.();
    } catch (error) {
      showMessage(
        error instanceof Error ? error.message : d.recent.deleteFailed,
        'error'
      );
    }
  }

  async function rerunJob(job: JobRecord) {
    try {
      // List rows are kept light; fetch the full record for the stored payload.
      const full = await getJob(job.id);
      if (!full.payload) {
        showMessage(d.recent.rerunUnavailable, 'error');
        return;
      }
      const created = await createJob(full.payload);
      if (created.mode === 'ppt_slide') {
        setPptJob(created);
        transitionToPage('ppt');
      } else {
        setPaperJob(created);
        transitionToPage('paper');
      }
    } catch (error) {
      showMessage(
        error instanceof Error ? error.message : d.recent.rerunFailed,
        'error'
      );
    }
  }

  const t = copy[lang];
  const d = desktopCopy[lang];

  const transitionToPage = (nextPage: Page) => {
    if (supportsViewTransitions && !window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      (document as any).startViewTransition(() => {
        setPage(nextPage);
      });
    } else {
      setPage(nextPage);
    }
  };

  const showMessage = useMemo(
    () => (text: string, tone: 'info' | 'error' = 'info') => setToast({ id: Date.now(), text, tone }),
    []
  );

  useEffect(() => {
    getConfig()
      .then(setConfig)
      .catch((error) => showMessage(error.message, 'error'));
  }, [showMessage]);

  useJobPolling(paperJob, setPaperJob, (message) => showMessage(message, 'error'));
  useJobPolling(pptJob, setPptJob, (message) => showMessage(message, 'error'));

  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(() => setToast(null), 3200);
    return () => window.clearTimeout(timer);
  }, [toast]);



  async function persistConfig(next: AppConfig) {
    try {
      showMessage(t.common.saving);
      const saved = await saveConfig(next);
      setConfig(saved);
      showMessage(t.common.saved);
    } catch (error) {
      showMessage(error instanceof Error ? error.message : 'Save failed', 'error');
    }
  }

  async function openJob(summary: JobRecord) {
    try {
      const job = await getJob(summary.id);
      if (job.mode === 'ppt_slide') {
        setPptJob(job);
        fillSlideForm(job.payload?.payload);
        transitionToPage('ppt');
      } else {
        setPaperJob(job);
        fillFigureForm(job.payload?.payload);
        transitionToPage('paper');
      }
    } catch (error) {
      showMessage(error instanceof Error ? error.message : 'Load failed', 'error');
    }
  }

  // Restores the original inputs so an old job can be tweaked and resubmitted.
  // Unknown/missing fields keep whatever the user already typed.
  function fillFigureForm(payload: unknown) {
    if (!payload || typeof payload !== 'object') return;
    const p = payload as Record<string, unknown>;
    const ids = Array.isArray(p.template_ids)
      ? (p.template_ids as unknown[]).filter((id): id is string => typeof id === 'string')
      : [];
    const apply = (existing: string[]) =>
      setFigureState((current) => ({
        ...current,
        title: typeof p.figure_title === 'string' ? p.figure_title : current.title,
        description:
          typeof p.section_description === 'string' ? p.section_description : current.description,
        selected: ids.length > 0 ? existing : current.selected,
        aspectRatio: typeof p.aspect_ratio === 'string' ? p.aspect_ratio : current.aspectRatio,
        layoutFidelity:
          p.layout_fidelity === 'strict' || p.layout_fidelity === 'balanced' || p.layout_fidelity === 'loose'
            ? p.layout_fidelity
            : current.layoutFidelity,
        styleStrength:
          p.style_strength === 'high' || p.style_strength === 'medium' || p.style_strength === 'low'
            ? p.style_strength
            : current.styleStrength,
        custom: typeof p.custom_prompt === 'string' ? p.custom_prompt : ''
      }));
    if (ids.length === 0) {
      apply([]);
      return;
    }
    // Drop ids whose templates no longer exist so the picker and the ready
    // flag stay truthful.
    listTemplates('all', '')
      .then((templates) => {
        const available = new Set(templates.map((item) => item.id));
        apply(ids.filter((id) => available.has(id)));
      })
      .catch(() => apply(ids));
  }

  function fillSlideForm(payload: unknown) {
    if (!payload || typeof payload !== 'object') return;
    const p = payload as Record<string, unknown>;
    setSlideState((current) => ({
      ...current,
      material: typeof p.material_text === 'string' ? p.material_text : current.material,
      pages: typeof p.page_count === 'number' && p.page_count > 0 ? p.page_count : current.pages,
      custom: typeof p.custom_prompt === 'string' ? p.custom_prompt : ''
    }));
  }

  const navItems: Array<{ key: Page; label: string; icon: JSX.Element }> = [
    { key: 'paper', label: d.nav.paper, icon: <IconFigure /> },
    { key: 'ppt', label: d.nav.ppt, icon: <IconSlide /> },
    { key: 'templates', label: d.nav.templates, icon: <IconTemplates /> },
    { key: 'history', label: d.nav.history, icon: <IconHistory /> }
  ];

  const head = pageHead[page];

  return (
    <div className="desktop-shell">
      <aside className="desktop-rail">
        <div className="desktop-rail-brand">
          <img src="/favor.png" alt="DreamPaper" />
        </div>

        <nav className="desktop-rail-nav" aria-label="Primary">
          {navItems.map((item) => (
            <button
              key={item.key}
              type="button"
              className={`rail-btn${page === item.key ? ' active' : ''}`}
              data-tip={item.label}
              aria-label={item.label}
              aria-current={page === item.key ? 'page' : undefined}
              onClick={() => transitionToPage(item.key)}
            >
              {item.icon}
            </button>
          ))}

        </nav>

        <span className="rail-spacer" />

        <button
          type="button"
          className={`rail-btn${page === 'settings' ? ' active' : ''}`}
          data-tip={d.nav.settings}
          aria-label={d.nav.settings}
          aria-current={page === 'settings' ? 'page' : undefined}
          onClick={() => transitionToPage('settings')}
        >
          <IconGear />
        </button>

        <button
          type="button"
          className="rail-btn"
          data-tip={lang === 'zh' ? 'English' : '中文'}
          aria-label={lang === 'zh' ? 'Switch to English' : '切换到中文'}
          onClick={() => setLang(lang === 'zh' ? 'en' : 'zh')}
        >
          <span className="rail-lang">{lang === 'zh' ? 'EN' : '中'}</span>
        </button>
      </aside>

      <main className="desktop-main">
        <div className="desktop-drag-strip" data-tauri-drag-region />

        <div className="desktop-head" data-tauri-drag-region="deep">
          <div className="desktop-head-text">
            <h1>{head.title(t, d)}</h1>
            <p>{head.intro(t, d)}</p>
          </div>
        </div>

        <div key={page} className={supportsViewTransitions ? 'desktop-page' : 'desktop-page page-enter'}>
          {page === 'paper' && (
            <FigureForm
              state={figureState}
              onState={setFigureState}
              job={paperJob}
              onJob={setPaperJob}
              onMessage={showMessage}
              onGoTemplates={() => transitionToPage('templates')}
              t={t}
              d={d}
            />
          )}
          {page === 'ppt' && (
            <SlideForm
              state={slideState}
              onState={setSlideState}
              job={pptJob}
              onJob={setPptJob}
              onMessage={showMessage}
              onGoTemplates={() => transitionToPage('templates')}
              t={t}
              d={d}
            />
          )}
          {page === 'templates' && <TemplateLibrary t={d} onMessage={showMessage} />}
          {page === 'history' && (
            <HistoryPage
              d={d}
              onOpen={openJob}
              onDelete={(job, refresh) => deleteSettled(job, refresh)}
              onRerun={rerunJob}
            />
          )}
          {page === 'settings' && (
            <SettingsPane>
              <Settings config={config} onChange={setConfig} onSave={persistConfig} t={t} />
            </SettingsPane>
          )}
        </div>
      </main>

      {toast && (
        <div className={`toast toast-${toast.tone}`} role="status" aria-live="polite" key={toast.id}>
          <span>{toast.text}</span>
          <button type="button" className="toast-close" aria-label="Close" onClick={() => setToast(null)}>
            ×
          </button>
        </div>
      )}
    </div>
  );
}

function HistoryCard({
  job,
  d,
  confirmDeleteId,
  onOpen,
  onPreview,
  onDelete,
  onRerun
}: {
  job: JobRecord;
  d: DesktopCopy;
  confirmDeleteId: string | null;
  onOpen: () => void;
  onPreview: (url: string) => void;
  onDelete: () => void;
  onRerun: () => void;
}) {
  const pill = (() => {
    switch (job.status) {
      case 'queued':
      case 'running':
        return d.recent.running;
      case 'succeeded':
        return d.recent.done;
      case 'failed':
        return d.recent.failed;
      default:
        return d.history.stopped;
    }
  })();
  return (
    <li className="history-card">
      <button
        type="button"
        className="hc-image"
        onClick={() => (job.thumbnail ? onPreview(job.thumbnail) : onOpen())}
        title={job.thumbnail ? d.history.zoom : job.title ?? job.message ?? job.id}
      >
        {job.thumbnail ? (
          <img src={job.thumbnail} alt="" loading="lazy" />
        ) : (
          <span className="hc-image-empty">
            {job.mode === 'ppt_slide' ? <IconSlide /> : <IconFigure />}
          </span>
        )}
      </button>
      <div className="hc-body">
        <button
          type="button"
          className="hc-open"
          onClick={onOpen}
          title={job.title ?? job.message ?? job.id}
        >
          <span className="hc-title-row">
            <span className={`recent-pill recent-pill-${job.status}`}>{pill}</span>
            <span className="hc-title" title={job.title ?? undefined}>
              {job.title || d.recent.untitled}
            </span>
          </span>
          <span className={`recent-summary recent-summary-${job.status}`}>
            {recentSummary(job, d)}
          </span>
        </button>
        <div className="hc-footer">
          <span className="desktop-recent-time">{shortTime(job.created_at)}</span>
          {isJobSettled(job.status) && onDelete && onRerun && (
            <span className="hc-actions">
              <button
                type="button"
                className="recent-rerun"
                aria-label={d.recent.rerun}
                title={d.recent.rerun}
                onClick={onRerun}
              >
                ⟳
              </button>
              <button
                type="button"
                className={`recent-delete${confirmDeleteId === job.id ? ' confirming' : ''}`}
                aria-label={d.recent.delete}
                title={d.recent.delete}
                onClick={onDelete}
              >
                {confirmDeleteId === job.id ? d.recent.confirmDelete : '×'}
              </button>
            </span>
          )}
        </div>
      </div>
    </li>
  );
}

const HISTORY_PAGE_SIZE = 50;

function HistoryPage({
  d,
  onOpen,
  onDelete,
  onRerun
}: {
  d: DesktopCopy;
  onOpen: (job: JobRecord) => void;
  onDelete: (job: JobRecord, refresh: () => void) => void;
  onRerun: (job: JobRecord) => void;
}) {
  const [rows, setRows] = useState<JobRecord[]>([]);
  const [pageIndex, setPageIndex] = useState(0);
  const [modeFilter, setModeFilter] = useState<'all' | 'paper_figure' | 'ppt_slide'>('all');
  const [statusFilter, setStatusFilter] = useState<'all' | 'live' | 'succeeded' | 'failed'>('all');
  const [query, setQuery] = useState('');
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);

  useEffect(() => {
    if (!previewUrl) return;
    function onKey(event: KeyboardEvent) {
      if (event.key === 'Escape') setPreviewUrl(null);
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [previewUrl]);

  useEffect(() => {
    if (!confirmDeleteId) return;
    const timer = window.setTimeout(() => setConfirmDeleteId(null), 3000);
    return () => window.clearTimeout(timer);
  }, [confirmDeleteId]);

  useEffect(() => {
    let cancelled = false;
    listJobs(HISTORY_PAGE_SIZE, pageIndex * HISTORY_PAGE_SIZE)
      .then((jobs) => {
        if (!cancelled) setRows(jobs);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [pageIndex]);

  const refresh = () => {
    listJobs(HISTORY_PAGE_SIZE, pageIndex * HISTORY_PAGE_SIZE)
      .then(setRows)
      .catch(() => {});
  };

  // The rail popup used to poll live jobs; without it, the history page
  // keeps that duty while anything is queued or running.
  const currentLive = rows.some(
    (job) => job.status === 'queued' || job.status === 'running'
  );
  useEffect(() => {
    if (!currentLive) return;
    const timer = window.setInterval(refresh, 3000);
    return () => window.clearInterval(timer);
  }, [currentLive, pageIndex]);

  const visible = rows.filter((job) => {
    if (modeFilter !== 'all' && job.mode !== modeFilter) return false;
    if (statusFilter === 'live' && isJobSettled(job.status)) return false;
    if (statusFilter === 'succeeded' && job.status !== 'succeeded') return false;
    if (statusFilter === 'failed' && job.status !== 'failed') return false;
    if (query.trim()) {
      const haystack = `${job.title ?? ''} ${job.message ?? ''}`.toLowerCase();
      if (!haystack.includes(query.trim().toLowerCase())) return false;
    }
    return true;
  });

  const groupOf = (iso: string) => {
    const date = new Date(iso);
    const now = new Date();
    const dayStart = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
    const diffDays = Math.round((dayStart(now) - dayStart(date)) / 86_400_000);
    if (diffDays <= 0) return d.history.today;
    if (diffDays === 1) return d.history.yesterday;
    return d.history.earlier;
  };

  const grouped: Array<[string, JobRecord[]]> = [];
  for (const job of visible) {
    const label = groupOf(job.created_at);
    const bucket = grouped.find(([key]) => key === label);
    if (bucket) {
      bucket[1].push(job);
    } else {
      grouped.push([label, [job]]);
    }
  }

  const seg = (
    options: Array<{ key: string; label: string }>,
    value: string,
    onPick: (key: string) => void,
    label: string
  ) => (
    <div className="history-seg" role="group" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.key}
          type="button"
          className={`history-seg-btn${value === option.key ? ' active' : ''}`}
          onClick={() => onPick(option.key)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );

  return (
    <section className="clay-panel history-panel">
      <div className="history-toolbar">
        {seg(
          [
            { key: 'all', label: d.history.all },
            { key: 'paper_figure', label: d.nav.paper },
            { key: 'ppt_slide', label: d.nav.ppt }
          ],
          modeFilter,
          (key) => setModeFilter(key as typeof modeFilter),
          d.history.filterMode
        )}
        {seg(
          [
            { key: 'all', label: d.history.all },
            { key: 'live', label: d.recent.running },
            { key: 'succeeded', label: d.recent.done },
            { key: 'failed', label: d.recent.failed }
          ],
          statusFilter,
          (key) => setStatusFilter(key as typeof statusFilter),
          d.history.filterStatus
        )}
        <input
          className="history-search"
          placeholder={d.history.searchPlaceholder}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <span className="history-count">{d.history.count(visible.length)}</span>
      </div>
      {visible.length === 0 ? (
        <p className="rail-pop-empty history-empty">{d.history.emptyFiltered}</p>
      ) : (
        <div className="history-groups">
          {grouped.map(([label, jobs]) => (
            <section key={label} className="history-group">
              <h3 className="history-group-title">
                {label}
                <span>{jobs.length}</span>
              </h3>
              <ul className="history-list">
                {jobs.map((job) => (
                  <HistoryCard
                    key={job.id}
                    job={job}
                    d={d}
                    confirmDeleteId={confirmDeleteId}
                    onOpen={() => onOpen(job)}
                    onPreview={setPreviewUrl}
                    onDelete={() => {
                      if (confirmDeleteId !== job.id) {
                        setConfirmDeleteId(job.id);
                        return;
                      }
                      setConfirmDeleteId(null);
                      onDelete(job, refresh);
                    }}
                    onRerun={() => onRerun(job)}
                  />
                ))}
              </ul>
            </section>
          ))}
        </div>
      )}
      <div className="history-pager">
        <button
          type="button"
          className="history-page-btn"
          disabled={pageIndex === 0}
          onClick={() => setPageIndex((index) => Math.max(0, index - 1))}
        >
          {d.history.prev}
        </button>
        <span className="history-page-index">{pageIndex + 1}</span>
        <button
          type="button"
          className="history-page-btn"
          disabled={rows.length < HISTORY_PAGE_SIZE}
          onClick={() => setPageIndex((index) => index + 1)}
        >
          {d.history.next}
        </button>
      </div>
      {previewUrl && (
        <div
          className="lightbox"
          role="dialog"
          aria-label={d.history.zoom}
          onClick={() => setPreviewUrl(null)}
        >
          <button
            type="button"
            className="lightbox-close"
            aria-label={d.history.closePreview}
            onClick={() => setPreviewUrl(null)}
          >
            ×
          </button>
          <img src={previewUrl} alt="" onClick={(event) => event.stopPropagation()} />
        </div>
      )}
    </section>
  );
}

const pageHead: Record<
  Page,
  { title: (t: Copy, d: DesktopCopy) => string; intro: (t: Copy, d: DesktopCopy) => string }
> = {
  paper: { title: (t) => t.paper.title, intro: (t) => t.paper.intro },
  ppt: { title: (t) => t.ppt.title, intro: (t) => t.ppt.intro },
  templates: { title: (_t, d) => d.templates.title, intro: (_t, d) => d.templates.intro },
  history: { title: (_t, d) => d.history.title, intro: (_t, d) => d.history.intro },
  settings: { title: (_t, d) => d.nav.settings, intro: (_t, d) => d.settingsIntro }
};

function IconFigure() {
  return (
    <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <rect x="2.75" y="3.25" width="14.5" height="13.5" rx="2.5" />
      <path d="M2.75 12.5l3.6-3.4 2.9 2.6 3-3.4 4.1 4" strokeLinecap="round" strokeLinejoin="round" />
      <circle cx="7" cy="7" r="1.1" fill="currentColor" stroke="none" />
    </svg>
  );
}

function IconSlide() {
  return (
    <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <rect x="2.5" y="3.75" width="15" height="10.5" rx="2" />
      <path d="M10 14.25v2.5M7 16.75h6" strokeLinecap="round" />
    </svg>
  );
}

function IconTemplates() {
  return (
    <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <rect x="2.75" y="2.75" width="6" height="6" rx="1.8" />
      <rect x="11.25" y="2.75" width="6" height="6" rx="1.8" />
      <rect x="2.75" y="11.25" width="6" height="6" rx="1.8" />
      <rect x="11.25" y="11.25" width="6" height="6" rx="1.8" />
    </svg>
  );
}

function IconGear() {
  return (
    <svg viewBox="0 0 20 20" aria-hidden="true">
      <path
        fill="currentColor"
        fillRule="evenodd"
        clipRule="evenodd"
        d="M8.94 1.5h2.12c.5 0 .92.36 1 .85l.2 1.2c.42.15.82.35 1.18.6l1.14-.44a1.01 1.01 0 0 1 1.23.43l1.06 1.84c.25.43.16.98-.22 1.3l-.94.78c.04.23.06.47.06.71 0 .24-.02.48-.06.71l.94.78c.38.32.47.87.22 1.3l-1.06 1.84a1.01 1.01 0 0 1-1.23.43l-1.14-.43c-.36.24-.76.44-1.18.59l-.2 1.2a1.01 1.01 0 0 1-1 .85H8.94a1.01 1.01 0 0 1-1-.85l-.2-1.2a5.9 5.9 0 0 1-1.18-.6l-1.14.44a1.01 1.01 0 0 1-1.23-.43L3.13 12.1a1.01 1.01 0 0 1 .22-1.3l.94-.78A5.6 5.6 0 0 1 4.23 10c0-.24.02-.48.06-.71l-.94-.78a1.01 1.01 0 0 1-.22-1.3l1.06-1.84a1.01 1.01 0 0 1 1.23-.43l1.14.43c.36-.24.76-.44 1.18-.59l.2-1.2c.08-.49.5-.85 1-.85Zm1.06 5.55a2.95 2.95 0 1 0 0 5.9 2.95 2.95 0 0 0 0-5.9Z"
      />
    </svg>
  );
}

function IconHistory() {
  return (
    <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <path d="M3.5 4.5v11a1.5 1.5 0 0 0 1.5 1.5h10a1.5 1.5 0 0 0 1.5-1.5v-7a1.5 1.5 0 0 0-1.5-1.5H9.6L8 4.5H5Z" strokeLinejoin="round" />
      <path d="M3.5 8h12.5" strokeLinecap="round" />
    </svg>
  );
}


function shortTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return '';
  const today = new Date();
  const sameDay =
    date.getFullYear() === today.getFullYear() &&
    date.getMonth() === today.getMonth() &&
    date.getDate() === today.getDate();
  const pad = (value: number) => String(value).padStart(2, '0');
  return sameDay
    ? `${pad(date.getHours())}:${pad(date.getMinutes())}`
    : `${pad(date.getMonth() + 1)}/${pad(date.getDate())}`;
}

function recentSummary(job: JobRecord, d: DesktopCopy): string {
  const mode = job.mode === 'ppt_slide' ? d.nav.ppt : d.nav.paper;
  const message = (job.message ?? '').trim();
  const firstLine = message.split('\n')[0]?.trim() ?? '';
  if (job.status === 'succeeded') {
    const finished = shortTime(job.updated_at);
    return finished ? `${mode} · ${d.recent.finishedAt} ${finished}` : mode;
  }
  if (job.status === 'failed') {
    const clipped = firstLine.length > 60 ? `${firstLine.slice(0, 60)}…` : firstLine;
    return clipped ? `${mode} · ${clipped}` : `${mode} · ${d.recent.failed}`;
  }
  if (firstLine) {
    return `${mode} · ${firstLine}`;
  }
  return `${mode} · ${job.status === 'queued' || job.status === 'running' ? d.recent.running : d.recent.done}`;
}
