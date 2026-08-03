import { useEffect, useMemo, useState } from 'react';
import { getConfig, getJob, listJobs, saveConfig } from '../api';
import {
  copy,
  defaultPaperState,
  defaultPptState,
  emptyConfig,
  JobPanel,
  PaperFigure,
  PptSlide,
  Settings,
  useJobPolling,
  type Lang,
  type PaperFigureState,
  type PptSlideState
} from '../app';
import type { AppConfig, JobRecord } from '../types';
import { desktopCopy } from './copy';
import { TemplateLibrary } from './templates';

type Page = 'paper' | 'ppt' | 'templates' | 'settings';

/**
 * 桌面外壳。
 *
 * 与网页版的区别只在骨架：左侧固定侧栏 + 铺满窗口的内容区，
 * 表单与结果面板直接复用 `app.tsx` 的组件。桌面窗口是固定尺寸的，
 * 网页版那套 `min(100%, 1120px)` 居中在 1360px 窗口里会两边留白、
 * 白白浪费掉本来就不宽的横向空间。
 */
export function DesktopApp() {
  const [page, setPage] = useState<Page>('paper');
  const [lang, setLang] = useState<Lang>('zh');
  const [config, setConfig] = useState<AppConfig>(emptyConfig);
  const [toast, setToast] = useState<{ id: number; text: string; tone: 'info' | 'error' } | null>(null);
  const [paperJob, setPaperJob] = useState<JobRecord | null>(null);
  const [pptJob, setPptJob] = useState<JobRecord | null>(null);
  const [paperState, setPaperState] = useState<PaperFigureState>(defaultPaperState);
  const [pptState, setPptState] = useState<PptSlideState>(defaultPptState);
  const [recent, setRecent] = useState<JobRecord[]>([]);

  const t = copy[lang];
  const d = desktopCopy[lang];

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

  // 近期任务跟着当前任务状态刷新：任务一结束，侧栏立刻反映出来
  useEffect(() => {
    let cancelled = false;
    listJobs(20)
      .then((jobs) => {
        if (!cancelled) setRecent(jobs);
      })
      .catch(() => {
        /* 侧栏是附属信息，取不到就空着，不打扰用户 */
      });
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
        setPage('ppt');
      } else {
        setPaperJob(job);
        setPage('paper');
      }
    } catch (error) {
      showMessage(error instanceof Error ? error.message : 'Load failed', 'error');
    }
  }

  const navItems: Array<{ key: Page; label: string }> = [
    { key: 'paper', label: d.nav.paper },
    { key: 'ppt', label: d.nav.ppt },
    { key: 'templates', label: d.nav.templates },
    { key: 'settings', label: d.nav.settings }
  ];

  return (
    <div className="desktop-shell">
      <aside className="desktop-side">
        <div className="desktop-brand">
          <img src="/favor.png" alt="" />
          <span>DREAMPAPER</span>
        </div>

        <nav className="desktop-nav" aria-label="Primary">
          {navItems.map((item) => (
            <button
              key={item.key}
              type="button"
              className={page === item.key ? 'active' : ''}
              onClick={() => setPage(item.key)}
            >
              {item.label}
            </button>
          ))}
        </nav>

        <div className="desktop-recent">
          <h2>{d.recent.title}</h2>
          {recent.length === 0 ? (
            <p className="desktop-recent-empty">{d.recent.empty}</p>
          ) : (
            <ul>
              {recent.map((job) => (
                <li key={job.id}>
                  <button type="button" onClick={() => openJob(job)} title={job.message ?? job.id}>
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

        <button
          type="button"
          className="desktop-lang"
          onClick={() => setLang(lang === 'zh' ? 'en' : 'zh')}
        >
          {lang === 'zh' ? 'EN' : '中'}
        </button>
      </aside>

      <main className="desktop-main">
        {page === 'paper' && (
          <>
            <PaperFigure
              state={paperState}
              onState={setPaperState}
              onJob={setPaperJob}
              onMessage={showMessage}
              t={t}
            />
            {paperJob && <JobPanel job={paperJob} t={t} />}
          </>
        )}
        {page === 'ppt' && (
          <>
            <PptSlide state={pptState} onState={setPptState} onJob={setPptJob} onMessage={showMessage} t={t} />
            {pptJob && <JobPanel job={pptJob} t={t} />}
          </>
        )}
        {page === 'templates' && <TemplateLibrary t={d} onMessage={showMessage} />}
        {page === 'settings' && (
          <Settings config={config} onChange={setConfig} onSave={persistConfig} t={t} />
        )}
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
