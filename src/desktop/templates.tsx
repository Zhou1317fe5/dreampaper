import { useEffect, useRef, useState } from 'react';
import {
  deleteTemplates,
  importTemplateImage,
  importTemplatePack,
  listTemplates,
  openExternal,
  pickDirectory
} from '../api';
import type { TemplateSummary } from '../types';
import { PAPER_BANANA_BENCH_URL, type DesktopCopy } from './copy';
import { DesktopCard, DesktopField, DesktopPick, DesktopSeg } from './forms';

type Tab = 'figure' | 'master';
type FigureKind = 'all' | 'diagram' | 'plot';

const listCache = new Map<string, TemplateSummary[]>();
const LIST_CACHE_MAX = 12;

function cacheKey(tab: Tab, kind: FigureKind, query: string): string {
  return `${tab}|${kind}|${query}`;
}

function putCache(key: string, items: TemplateSummary[]) {
  listCache.delete(key);
  listCache.set(key, items);
  while (listCache.size > LIST_CACHE_MAX) {
    const oldest = listCache.keys().next().value;
    if (oldest === undefined) break;
    listCache.delete(oldest);
  }
}

export function TemplateLibrary({
  t,
  onMessage
}: {
  t: DesktopCopy;
  onMessage: (text: string, tone?: 'info' | 'error') => void;
}) {
  const [tab, setTab] = useState<Tab>('figure');
  const [figureKind, setFigureKind] = useState<FigureKind>('all');
  const [query, setQuery] = useState('');
  const key = cacheKey(tab, figureKind, query);
  const [templates, setTemplates] = useState<TemplateSummary[]>(() => listCache.get(key) ?? []);
  const [pending, setPending] = useState(() => !listCache.has(key));
  const [busy, setBusy] = useState(false);

  const [importOpen, setImportOpen] = useState(false);
  const importRef = useRef<HTMLDivElement>(null);
  const [file, setFile] = useState<File | null>(null);
  const [formKind, setFormKind] = useState<'diagram' | 'plot'>('diagram');
  const [category, setCategory] = useState('');
  const [visualIntent, setVisualIntent] = useState('');
  const [contentSummary, setContentSummary] = useState('');

  const [picking, setPicking] = useState(false);
  const [chosen, setChosen] = useState<string[]>([]);
  const [confirming, setConfirming] = useState(false);
  const [removing, setRemoving] = useState(false);

  const importKind = tab === 'master' ? 'master' : formKind;

  useEffect(() => {
    if (!importOpen) return;
    function onClickAway(event: MouseEvent) {
      if (importRef.current && !importRef.current.contains(event.target as Node)) setImportOpen(false);
    }
    document.addEventListener('mousedown', onClickAway);
    return () => document.removeEventListener('mousedown', onClickAway);
  }, [importOpen]);

  async function load() {
    const kind = tab === 'master' ? 'master' : figureKind === 'all' ? 'all' : figureKind;
    const items = await listTemplates(kind, query);
    return tab === 'figure' && kind === 'all' ? items.filter((item) => item.kind !== 'master') : items;
  }

  async function refresh() {
    try {
      const items = await load();
      putCache(key, items);
      setTemplates(items);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : 'Load failed', 'error');
    }
  }

  useEffect(() => {
    let cancelled = false;
    const cached = listCache.get(key);
    setTemplates(cached ?? []);
    setPending(!cached);
    load()
      .then((items) => {
        putCache(key, items);
        if (!cancelled) {
          setTemplates(items);
          setPending(false);
        }
      })
      .catch((error) => {
        if (!cancelled) setPending(false);
        onMessage(error instanceof Error ? error.message : 'Load failed', 'error');
      });
    return () => {
      cancelled = true;
    };
  }, [key]);

  useEffect(() => {
    setChosen([]);
    setConfirming(false);
  }, [key]);

  const visibleIds = templates.map((item) => item.id);
  const chosenVisible = chosen.filter((id) => visibleIds.includes(id));
  const allChosen = visibleIds.length > 0 && chosenVisible.length === visibleIds.length;

  function toggleChoice(id: string) {
    setConfirming(false);
    setChosen((current) =>
      current.includes(id) ? current.filter((item) => item !== id) : [...current, id]
    );
  }

  function leavePicking() {
    setPicking(false);
    setChosen([]);
    setConfirming(false);
  }

  async function removeChosen() {
    if (chosenVisible.length === 0) {
      onMessage(t.templates.removeNone, 'error');
      return;
    }
    if (!confirming) {
      setConfirming(true);
      return;
    }
    setRemoving(true);
    try {
      onMessage(t.templates.removing);
      const count = await deleteTemplates(chosenVisible);
      listCache.clear();
      leavePicking();
      await refresh();
      onMessage(t.templates.removed(count));
    } catch (error) {
      onMessage(error instanceof Error ? error.message : 'Delete failed', 'error');
    } finally {
      setRemoving(false);
    }
  }

  async function submitImage() {
    if (!file) {
      onMessage(t.templates.needFile, 'error');
      return;
    }
    setBusy(true);
    try {
      onMessage(t.templates.importing);
      await importTemplateImage(file, importKind, category, visualIntent, contentSummary);
      setFile(null);
      setCategory('');
      setVisualIntent('');
      setContentSummary('');
      setImportOpen(false);
      listCache.clear();
      await refresh();
      onMessage(t.templates.imported);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : 'Import failed', 'error');
    } finally {
      setBusy(false);
    }
  }

  async function submitPack() {
    setBusy(true);
    try {
      const path = await pickDirectory(t.templates.packHint);
      if (!path) return;
      onMessage(t.templates.importing);
      const pack = await importTemplatePack(path);
      setImportOpen(false);
      listCache.clear();
      await refresh();
      onMessage(t.templates.importedPack(pack.template_count));
    } catch (error) {
      onMessage(error instanceof Error ? error.message : 'Import failed', 'error');
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="dp-work single">
      <DesktopCard
        title={
          <DesktopSeg
            value={tab}
            onChange={setTab}
            items={[
              { key: 'figure', label: t.templates.tabFigure },
              { key: 'master', label: t.templates.tabMaster }
            ]}
          />
        }
        aside={
          <div className="dp-head-tools">
            <span className="dp-foot-note">{t.templates.count(templates.length)}</span>
            {picking ? (
              <>
                <span className="dp-foot-note">{t.templates.selectedCount(chosenVisible.length)}</span>
                <button
                  type="button"
                  className="dp-ghost"
                  disabled={visibleIds.length === 0}
                  onClick={() => {
                    setConfirming(false);
                    setChosen(allChosen ? [] : visibleIds);
                  }}
                >
                  {allChosen ? t.templates.selectNone : t.templates.selectAll}
                </button>
                <button
                  type="button"
                  className={`dp-ghost dp-danger${confirming ? ' on' : ''}`}
                  disabled={removing || chosenVisible.length === 0}
                  onClick={removeChosen}
                >
                  {removing
                    ? t.templates.removing
                    : confirming
                      ? t.templates.confirmRemove(chosenVisible.length)
                      : t.templates.remove}
                </button>
                <button type="button" className="dp-ghost" disabled={removing} onClick={leavePicking}>
                  {t.templates.selectDone}
                </button>
              </>
            ) : (
              <>
                {tab === 'figure' && (
                  <select
                    className="dp-mini"
                    value={figureKind}
                    onChange={(event) => setFigureKind(event.target.value as FigureKind)}
                    aria-label={t.templates.kind}
                  >
                    <option value="all">{t.templates.all}</option>
                    <option value="diagram">{t.templates.diagram}</option>
                    <option value="plot">{t.templates.plot}</option>
                  </select>
                )}
                <input
                  className="dp-mini dp-search"
                  placeholder={t.templates.search}
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  aria-label={t.templates.search}
                />
                <button
                  type="button"
                  className="dp-ghost"
                  disabled={templates.length === 0}
                  onClick={() => setPicking(true)}
                >
                  {t.templates.select}
                </button>

                <div className="dp-pop-wrap" ref={importRef}>
                  <button
                    type="button"
                    className={`dp-ghost${importOpen ? ' on' : ''}`}
                    aria-expanded={importOpen}
                    onClick={() => setImportOpen((open) => !open)}
                  >
                    {t.templates.importTitle}
                  </button>
                  {importOpen && (
                    <div className="dp-pop" role="dialog" aria-label={t.templates.importTitle}>
                      <h3>{t.templates.importImage}</h3>
                      <DesktopField label={t.templates.file} span>
                        <DesktopPick
                          accept="image/*"
                          label={t.templates.choose}
                          value={file?.name ?? ''}
                          onChange={(files) => setFile(files?.[0] ?? null)}
                        />
                      </DesktopField>
                      <div className="dp-row two">
                        {tab === 'figure' ? (
                          <DesktopField label={t.templates.kind}>
                            <select
                              value={formKind}
                              onChange={(event) => setFormKind(event.target.value as 'diagram' | 'plot')}
                            >
                              <option value="diagram">{t.templates.diagram}</option>
                              <option value="plot">{t.templates.plot}</option>
                            </select>
                          </DesktopField>
                        ) : (
                          <DesktopField label={t.templates.kind}>
                            <input value={t.templates.master} readOnly />
                          </DesktopField>
                        )}
                        <DesktopField label={t.templates.category}>
                          <input
                            placeholder={t.templates.categoryHint}
                            value={category}
                            onChange={(event) => setCategory(event.target.value)}
                          />
                        </DesktopField>
                      </div>
                      <DesktopField label={t.templates.visualIntent} span>
                        <input
                          placeholder={t.templates.visualIntentHint}
                          value={visualIntent}
                          onChange={(event) => setVisualIntent(event.target.value)}
                        />
                      </DesktopField>
                      <DesktopField label={t.templates.contentSummary} span>
                        <input
                          placeholder={t.templates.contentSummaryHint}
                          value={contentSummary}
                          onChange={(event) => setContentSummary(event.target.value)}
                        />
                      </DesktopField>
                      <button type="button" className="dp-primary" disabled={busy} onClick={submitImage}>
                        {busy ? t.templates.importing : t.templates.submit}
                      </button>

                      {tab === 'figure' && (
                        <div className="dp-pop-alt">
                          <h3>{t.templates.importPack}</h3>
                          <p>{t.templates.packHint}</p>
                          <div className="dp-pop-alt-row">
                            <button type="button" className="dp-ghost" disabled={busy} onClick={submitPack}>
                              {t.templates.pickPack}
                            </button>
                            <a
                              href={PAPER_BANANA_BENCH_URL}
                              target="_blank"
                              rel="noreferrer"
                              onClick={(event) => {
                                event.preventDefault();
                                void openExternal(PAPER_BANANA_BENCH_URL);
                              }}
                            >
                              {t.templates.emptyLink} ↗
                            </a>
                          </div>
                        </div>
                      )}
                    </div>
                  )}
                </div>
              </>
            )}
          </div>
        }
        flush
      >
        {pending ? (
          <div className="dp-tiles wide" aria-busy="true">
            {Array.from({ length: 8 }, (_, index) => (
              <div key={index} className="dp-lib-card dp-lib-skeleton" aria-hidden="true" />
            ))}
          </div>
        ) : templates.length === 0 ? (
          <div className="dp-empty">
            <strong>{tab === 'master' ? t.templates.masterEmptyTitle : t.templates.figureEmptyTitle}</strong>
            <p>{tab === 'master' ? t.templates.masterEmptyBody : t.templates.figureEmptyBody}</p>
            <button type="button" className="dp-ghost" onClick={() => setImportOpen(true)}>
              {t.templates.importTitle}
            </button>
          </div>
        ) : (
          <div className={`dp-tiles wide${picking ? ' picking' : ''}`}>
            {templates.map((item) => {
              const on = chosen.includes(item.id);
              const caption = (
                <figcaption>
                  <span className={`dp-badge dp-badge-${item.kind}`}>
                    {item.kind === 'master'
                      ? t.templates.master
                      : item.kind === 'plot'
                        ? t.templates.plot
                        : t.templates.diagram}
                  </span>
                  {item.category ? <span className="dp-lib-cat">{item.category}</span> : null}
                  <span className="dp-lib-intent" title={item.visual_intent}>
                    {item.visual_intent}
                  </span>
                </figcaption>
              );
              if (!picking) {
                return (
                  <figure key={item.id} className="dp-lib-card">
                    <img src={item.image_url} alt={item.visual_intent} loading="lazy" />
                    {caption}
                  </figure>
                );
              }
              return (
                <figure
                  key={item.id}
                  className={`dp-lib-card dp-lib-pick${on ? ' on' : ''}`}
                  role="checkbox"
                  aria-checked={on}
                  tabIndex={0}
                  onClick={() => toggleChoice(item.id)}
                  onKeyDown={(event) => {
                    if (event.key === ' ' || event.key === 'Enter') {
                      event.preventDefault();
                      toggleChoice(item.id);
                    }
                  }}
                >
                  <img src={item.image_url} alt={item.visual_intent} loading="lazy" />
                  <span className="dp-lib-check" aria-hidden="true" />
                  {caption}
                </figure>
              );
            })}
          </div>
        )}
      </DesktopCard>
    </div>
  );
}
