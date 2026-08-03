import { useEffect, useState } from 'react';
import { importTemplateImage, importTemplatePack, listTemplates, pickDirectory } from '../api';
import { Field, FilePicker } from '../app';
import type { TemplateSummary } from '../types';
import { PAPER_BANANA_BENCH_URL, type DesktopCopy } from './copy';

type Kind = 'all' | 'diagram' | 'plot';

export function TemplateLibrary({
  t,
  onMessage
}: {
  t: DesktopCopy;
  onMessage: (text: string, tone?: 'info' | 'error') => void;
}) {
  const [templates, setTemplates] = useState<TemplateSummary[]>([]);
  const [kind, setKind] = useState<Kind>('all');
  const [query, setQuery] = useState('');
  const [busy, setBusy] = useState(false);

  // 导入表单
  const [file, setFile] = useState<File | null>(null);
  const [formKind, setFormKind] = useState<'diagram' | 'plot'>('diagram');
  const [category, setCategory] = useState('');
  const [visualIntent, setVisualIntent] = useState('');
  const [contentSummary, setContentSummary] = useState('');

  async function refresh() {
    try {
      setTemplates(await listTemplates(kind, query));
    } catch (error) {
      onMessage(error instanceof Error ? error.message : 'Load failed', 'error');
    }
  }

  useEffect(() => {
    let cancelled = false;
    listTemplates(kind, query)
      .then((items) => {
        if (!cancelled) setTemplates(items);
      })
      .catch((error) => onMessage(error instanceof Error ? error.message : 'Load failed', 'error'));
    return () => {
      cancelled = true;
    };
  }, [kind, query]);

  async function submitImage() {
    if (!file) {
      onMessage(t.templates.needFile, 'error');
      return;
    }
    setBusy(true);
    try {
      onMessage(t.templates.importing);
      await importTemplateImage(file, formKind, category, visualIntent, contentSummary);
      setFile(null);
      setCategory('');
      setVisualIntent('');
      setContentSummary('');
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
      // 用户取消选择不是错误，静默返回
      if (!path) return;
      onMessage(t.templates.importing);
      const pack = await importTemplatePack(path);
      await refresh();
      onMessage(t.templates.importedPack(pack.template_count));
    } catch (error) {
      onMessage(error instanceof Error ? error.message : 'Import failed', 'error');
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="clay-panel page-panel">
      <header className="page-head">
        <h2>{t.templates.title}</h2>
        <p>{t.templates.intro}</p>
      </header>

      <div className="tpl-import-row">
        <div className="tpl-import-card">
          <h3>{t.templates.importImage}</h3>
          <Field label={t.templates.file}>
            <FilePicker
              accept="image/*"
              label={t.templates.choose}
              value={file?.name ?? ''}
              compact
              onChange={(files) => setFile(files?.[0] ?? null)}
            />
          </Field>
          <div className="field-row two">
            <Field label={t.templates.kind}>
              <select value={formKind} onChange={(event) => setFormKind(event.target.value as 'diagram' | 'plot')}>
                <option value="diagram">{t.templates.diagram}</option>
                <option value="plot">{t.templates.plot}</option>
              </select>
            </Field>
            <Field label={t.templates.category} hint={t.templates.categoryHint}>
              <input value={category} onChange={(event) => setCategory(event.target.value)} />
            </Field>
          </div>
          <Field label={t.templates.visualIntent} hint={t.templates.visualIntentHint}>
            <input value={visualIntent} onChange={(event) => setVisualIntent(event.target.value)} />
          </Field>
          <Field label={t.templates.contentSummary} hint={t.templates.contentSummaryHint}>
            <input value={contentSummary} onChange={(event) => setContentSummary(event.target.value)} />
          </Field>
          <button type="button" className="primary" disabled={busy} onClick={submitImage}>
            {busy ? t.templates.importing : t.templates.submit}
          </button>
        </div>

        <div className="tpl-import-card">
          <h3>{t.templates.importPack}</h3>
          <p className="tpl-hint">{t.templates.packHint}</p>
          <button type="button" className="primary" disabled={busy} onClick={submitPack}>
            {busy ? t.templates.importing : t.templates.pickPack}
          </button>
          <a className="tpl-link" href={PAPER_BANANA_BENCH_URL} target="_blank" rel="noreferrer">
            {t.templates.emptyLink} ↗
          </a>
        </div>
      </div>

      <div className="toolbar field-row two">
        <Field label={t.templates.kind}>
          <select value={kind} onChange={(event) => setKind(event.target.value as Kind)}>
            <option value="all">{t.templates.all}</option>
            <option value="diagram">{t.templates.diagram}</option>
            <option value="plot">{t.templates.plot}</option>
          </select>
        </Field>
        <Field label={t.templates.search}>
          <input value={query} onChange={(event) => setQuery(event.target.value)} />
        </Field>
      </div>

      {templates.length === 0 ? (
        <div className="tpl-empty">
          <h3>{t.templates.emptyTitle}</h3>
          <p>{t.templates.emptyBody}</p>
          <a href={PAPER_BANANA_BENCH_URL} target="_blank" rel="noreferrer">
            {t.templates.emptyLink} ↗
          </a>
        </div>
      ) : (
        <>
          <p className="tpl-count">{t.templates.count(templates.length)}</p>
          <div className="tpl-grid">
            {templates.map((item) => (
              <figure key={item.id} className="tpl-card">
                <img src={item.image_url} alt={item.visual_intent} loading="lazy" />
                <figcaption>
                  <span className={`tpl-badge tpl-badge-${item.kind}`}>{item.kind}</span>
                  {item.category ? <span className="tpl-cat">{item.category}</span> : null}
                  <span className="tpl-intent" title={item.visual_intent}>
                    {item.visual_intent}
                  </span>
                </figcaption>
              </figure>
            ))}
          </div>
        </>
      )}
    </section>
  );
}
