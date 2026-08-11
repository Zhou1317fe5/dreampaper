import { useEffect, useMemo, useRef, useState, type JSX } from 'react';
import { getConfig, getJob, listJobs, saveConfig } from '../api';
import { copy, emptyConfig, Settings, useJobPolling, type Lang } from '../app';
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

type Page = 'paper' | 'ppt' | 'templates' | 'settings';
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
  const [recent, setRecent] = useState<JobRecord[]>([]);

  const [recentOpen, setRecentOpen] = useState(false);
  const recentRef = useRef<HTMLDivElement>(null);

  const t = copy[lang];
  const d = desktopCopy[lang];

  useEffect(() => {
    if (!recentOpen) return;
    function handleClick(event: MouseEvent) {
      if (recentRef.current && !recentRef.current.contains(event.target as Node)) {
        setRecentOpen(false);
      }
    }
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [recentOpen]);

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

  useEffect(() => {
    let cancelled = false;
    listJobs(20)
      .then((jobs) => {
        if (!cancelled) setRecent(jobs);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [paperJob?.status, paperJob?.id, pptJob?.status, pptJob?.id]);

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
        transitionToPage('ppt');
      } else {
        setPaperJob(job);
        transitionToPage('paper');
      }
    } catch (error) {
      showMessage(error instanceof Error ? error.message : 'Load failed', 'error');
    }
  }

  const navItems: Array<{ key: Page; label: string; icon: JSX.Element }> = [
    { key: 'paper', label: d.nav.paper, icon: <IconFigure /> },
    { key: 'ppt', label: d.nav.ppt, icon: <IconSlide /> },
    { key: 'templates', label: d.nav.templates, icon: <IconTemplates /> }
  ];

  const head = pageHead[page];
  const hasLiveJob = recent.some((job) => job.status === 'running' || job.status === 'queued');

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

          <div className="rail-pop-wrap" ref={recentRef}>
            {recentOpen && (
              <div className="rail-pop" role="dialog" aria-label={d.recent.title}>
                <h2>{d.recent.title}</h2>
                {recent.length === 0 ? (
                  <p className="rail-pop-empty">{d.recent.empty}</p>
                ) : (
                  <ul>
                    {recent.map((job) => (
                      <li key={job.id}>
                        <button
                          type="button"
                          onClick={() => {
                            setRecentOpen(false);
                            openJob(job);
                          }}
                          title={job.message ?? job.id}
                        >
                          <span className={`job-dot job-dot-${job.status}`} aria-hidden="true" />
                          <span className="desktop-recent-mode">
                            {job.mode === 'ppt_slide' ? d.nav.ppt : d.nav.paper}
                          </span>
                          <span className="desktop-recent-time">{shortTime(job.created_at)}</span>
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            )}
            <button
              type="button"
              className={`rail-btn${recentOpen ? ' active' : ''}`}
              data-tip={d.recent.title}
              aria-label={d.recent.title}
              aria-expanded={recentOpen}
              onClick={() => setRecentOpen((open) => !open)}
            >
              <IconRecent />
              {hasLiveJob && <span className="rail-badge" aria-hidden="true" />}
            </button>
          </div>
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

const pageHead: Record<
  Page,
  { title: (t: Copy, d: DesktopCopy) => string; intro: (t: Copy, d: DesktopCopy) => string }
> = {
  paper: { title: (t) => t.paper.title, intro: (t) => t.paper.intro },
  ppt: { title: (t) => t.ppt.title, intro: (t) => t.ppt.intro },
  templates: { title: (_t, d) => d.templates.title, intro: (_t, d) => d.templates.intro },
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

function IconRecent() {
  return (
    <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <circle cx="10" cy="10" r="7.25" />
      <path d="M10 6.1V10l2.7 1.9" strokeLinecap="round" strokeLinejoin="round" />
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
