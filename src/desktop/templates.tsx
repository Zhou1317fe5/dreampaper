import { useEffect, useRef, useState } from 'react';
import {
  deleteTemplates,
  importTemplateImage,
  importTemplatePack,
  listTemplates,
  pickDirectory
} from '../api';
import type { TemplateSummary } from '../types';
import { PAPER_BANANA_BENCH_URL, type DesktopCopy } from './copy';
import { DesktopCard, DesktopField, DesktopPick, DesktopSeg } from './forms';

/** 模板库分两类：科研绘图模板（diagram + plot）与幻灯片母版（master）。
 *  两类分别喂给两条管道，混在一起选会串味，所以顶栏用分段控件硬分开。 */
type Tab = 'figure' | 'master';
type FigureKind = 'all' | 'diagram' | 'plot';

/**
 * 列表缓存。
 *
 * shell 用 key={page} 换页，TemplateLibrary 每次切页都是整个重建，
 * state 跟着清空 —— 于是切回模板库先画一格空网格，等 IPC 回来才刷出图，
 * 就是用户看到的「先空页面再加载」。缓存必须挂在模块级、活在组件之外
 * 才接得住这次卸载；组件里的 useState/useRef 都会一起没。
 *
 * 命中缓存时先原样画旧结果、再在后台重新拉一次覆盖（stale-while-revalidate）：
 * 图片 URL 不变，配合 protocol.rs 那条 immutable 缓存头，浏览器直接复用
 * 已解码的位图，不再重新读盘解码。
 */
const listCache = new Map<string, TemplateSummary[]>();
/** 搜索框每敲一个字就是一个新 key，不设上限会一直涨 */
const LIST_CACHE_MAX = 12;

function cacheKey(tab: Tab, kind: FigureKind, query: string): string {
  return `${tab}|${kind}|${query}`;
}

function putCache(key: string, items: TemplateSummary[]) {
  listCache.delete(key);
  listCache.set(key, items);
  // Map 按插入序迭代，队头就是最久没用到的那条
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
  // 初值就取缓存：等到 effect 里再 setState 已经晚了一帧，那一帧就是空网格
  const [templates, setTemplates] = useState<TemplateSummary[]>(() => listCache.get(key) ?? []);
  const [pending, setPending] = useState(() => !listCache.has(key));
  const [busy, setBusy] = useState(false);

  // 导入表单收进悬浮弹窗：常驻两张导入卡会跟图库抢高度，且左右填不满
  const [importOpen, setImportOpen] = useState(false);
  const importRef = useRef<HTMLDivElement>(null);
  const [file, setFile] = useState<File | null>(null);
  const [formKind, setFormKind] = useState<'diagram' | 'plot'>('diagram');
  const [category, setCategory] = useState('');
  const [visualIntent, setVisualIntent] = useState('');
  const [contentSummary, setContentSummary] = useState('');

  /**
   * 批量删除。
   *
   * 单独一个「选择」模式，而不是每张卡常挂一个删除角标：图库里一屏几十张，
   * 常挂的删除键太容易误触，而删除是不可撤销的。
   *
   * 确认走按钮自身的二段式（「删除」→「确认删除 N 张」），不用 window.confirm——
   * 那是系统模态，在无边框窗口里样式不受控，且会打断这一列的操作节奏。
   */
  const [picking, setPicking] = useState(false);
  const [chosen, setChosen] = useState<string[]>([]);
  const [confirming, setConfirming] = useState(false);
  const [removing, setRemoving] = useState(false);

  // 母版页导入的一定是母版，类型不再让用户选
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
    // 科研图那一档要 diagram + plot 两类，后端只支持单类或全部，
    // 所以取全部再在前端剔掉母版，省一次往返。
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
    // 有缓存就先把旧结果摆上，后台再校验；没有才允许露出空状态
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

  // 换了页签/筛选/搜索词，选中的那些多半已经不在眼前了：
  // 留着它们会导致「看不见的模板被删掉」
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
    // 第一下只是把按钮换成确认态，第二下才真删
    if (!confirming) {
      setConfirming(true);
      return;
    }
    setRemoving(true);
    try {
      onMessage(t.templates.removing);
      const count = await deleteTemplates(chosenVisible);
      // 删除会命中所有筛选组合，跟导入一样整片作废
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
      // 新模板会落进哪些筛选组合说不准（类型/分类/搜索词都可能命中），
      // 与其逐个推算，不如整片作废重拉——导入是低频操作
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
      // 用户取消选择不是错误，静默返回
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
              // 选择模式下把筛选/搜索/导入让位给删除工具：
              // 这一行装不下两套控件，而且选中态在换筛选后本来就得清空
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
                            <a href={PAPER_BANANA_BENCH_URL} target="_blank" rel="noreferrer">
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
          // 首次拉取期间画骨架而不是空状态：空状态写着「还没有模板」，
          // 在数据还没回来时显示等于撒谎，而且会和随后刷出的网格闪一下
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
              // 选择模式下整张卡是一个 checkbox 按钮：只在角标上开点击热区，
              // 手要瞄那 18px 的小方块，一屏几十张时太难点
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
