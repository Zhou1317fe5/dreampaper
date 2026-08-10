import { useEffect, useId, useState, type ReactNode } from 'react';
import { createJob, listTemplates, uploadAsset } from '../api';
import { JobPanel, copy, type Lang } from '../app';
import type { AssetUpload, JobRecord, TemplateSummary } from '../types';
import type { DesktopCopy } from './copy';

type Copy = (typeof copy)[Lang];

/**
 * 桌面版的两个工作页（科研图 / 幻灯片）。
 *
 * 为什么不复用 `app.tsx` 的 PaperFigure / PptSlide：
 * 那两个组件是按网页版的单列长表单长出来的，桌面版只能靠 CSS 把它们
 * 掰成两列——高度对不上就只能上 ResizeObserver 拿 JS 硬同步，接缝处
 * 还要靠「上块去掉圆底、下块去掉圆顶」焊起来。改了三轮仍然「凌乱、
 * 过渡生硬」，根因是外壳在改写它管不着的 DOM。
 *
 * 这里换成自己的骨架：一行两列，两列都是 min-height:0 的 flex 卡片，
 * 高度由 grid 拉伸决定而不是由内容决定，边界因此天然对齐；
 * 卡片内部 head / body / foot 三段，body 自己滚动，分隔线明确区分边界。
 *
 * 数据层（api.ts）与结果面板（JobPanel）仍然共用，不重复实现。
 */

const MAX_TEMPLATES = 3;

/* ============ 骨架 ============ */

function Card({
  title,
  aside,
  foot,
  children,
  flush
}: {
  title?: ReactNode;
  aside?: ReactNode;
  foot?: ReactNode;
  children: ReactNode;
  /** 图库这类自带内边距的内容用 flush，省掉双层留白 */
  flush?: boolean;
}) {
  return (
    <section className="dp-card">
      {(title || aside) && (
        <header className="dp-card-head">
          {typeof title === 'string' ? <h2>{title}</h2> : title}
          {aside ? <div className="dp-card-head-aside">{aside}</div> : null}
        </header>
      )}
      <div className={`dp-card-body${flush ? ' flush' : ''}`}>{children}</div>
      {foot ? <footer className="dp-card-foot">{foot}</footer> : null}
    </section>
  );
}

/** 分段控件：右列在「选模板」与「看结果」之间切换，
 *  结果面板因此不必另起一块把两列高度撑歪。 */
function Seg<T extends string>({
  value,
  items,
  onChange
}: {
  value: T;
  items: Array<{ key: T; label: string; dot?: boolean }>;
  onChange: (key: T) => void;
}) {
  return (
    <div className="dp-seg" role="tablist">
      {items.map((item) => (
        <button
          key={item.key}
          type="button"
          role="tab"
          aria-selected={value === item.key}
          className={value === item.key ? 'active' : undefined}
          onClick={() => onChange(item.key)}
        >
          {item.label}
          {item.dot && <span className="dp-seg-dot" aria-hidden="true" />}
        </button>
      ))}
    </div>
  );
}

function DField({
  label,
  children,
  span
}: {
  label: string;
  children: ReactNode;
  span?: boolean;
}) {
  return (
    <label className={`dp-field${span ? ' span' : ''}`}>
      <span className="dp-field-label">{label}</span>
      {children}
    </label>
  );
}

function DPick({
  accept,
  label,
  value,
  multiple,
  onChange,
  children
}: {
  accept: string;
  label: string;
  value: string;
  multiple?: boolean;
  onChange: (files: FileList | null) => void;
  /**
   * 「选择」按钮右边那块空位。传了就用它顶掉 value 文案——
   * 幻灯片页把已选附件的 chip 放这儿，省掉下面单独一行。
   */
  children?: ReactNode;
}) {
  const id = useId();
  return (
    <div className="dp-pick">
      <input id={id} type="file" accept={accept} multiple={multiple} onChange={(event) => onChange(event.target.files)} />
      <label htmlFor={id}>{label}</label>
      {children ??
        (value ? (
          <span className="dp-pick-name" title={value}>
            {value}
          </span>
        ) : null)}
    </div>
  );
}

/**
 * 铺满剩余高度的 textarea。
 *
 * 走过两版弯路：
 *   1. `resize: vertical` —— 右下角挂着浏览器画的斜纹手柄，跟圆角 chip 打架，
 *      用户拉高还会把左列撑出滚动条、破坏两列等高。
 *   2. JS 量 scrollHeight 自适应 —— 高度随内容长，短文案时下方剩一大片空白，
 *      两个输入框各自一个高度，看着更不齐。
 *
 * 现在交给 flex：字段本身是 flex 项，`grow` 决定它在卡片剩余高度里分几份，
 * textarea 在字段内部 100% 撑满。于是高度由窗口决定而不是由内容决定——
 * 卡片永远填满，多个输入框的比例恒定，内容超出就在框内滚。
 *
 * 提示文字走 placeholder 而不是框下面那行 small：那行会白占一行高度、
 * 把输入框往上挤，且用户开始输入后它就成了废话。
 */
function GrowField({
  label,
  hint,
  value,
  onChange,
  grow = 1
}: {
  label: string;
  /** 落在框内当 placeholder */
  hint?: string;
  value: string;
  onChange: (value: string) => void;
  /** 在卡片剩余高度里占的份数：资料/方法给 2，约束给 1 */
  grow?: number;
}) {
  return (
    <label className="dp-field span dp-field-grow" style={{ flexGrow: grow }}>
      <span className="dp-field-label">{label}</span>
      <textarea
        className="dp-ta-fill"
        placeholder={hint}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      />
    </label>
  );
}

/** 数字步进器。原生 input[type=number] 的上下箭头样式不可控，
 *  CSS 里已经摘掉，这里补上自绘的加减按钮。 */
function DStep({
  value,
  min,
  max,
  onChange
}: {
  value: number;
  min: number;
  max: number;
  onChange: (value: number) => void;
}) {
  const clamp = (next: number) => Math.min(max, Math.max(min, next));
  return (
    <div className="dp-step">
      <button type="button" disabled={value <= min} aria-label="-" onClick={() => onChange(clamp(value - 1))}>
        −
      </button>
      <input
        type="number"
        min={min}
        max={max}
        value={value}
        onChange={(event) => {
          // 输入过程中允许空串/半成品，只在能解析出数字时才回写
          const next = Number(event.target.value);
          if (Number.isFinite(next)) onChange(clamp(next));
        }}
      />
      <button type="button" disabled={value >= max} aria-label="+" onClick={() => onChange(clamp(value + 1))}>
        +
      </button>
    </div>
  );
}

/** 模板/母版网格。单选（母版）与多选（科研图）共用一份。 */
function TileGrid({
  items,
  selected,
  onToggle,
  empty
}: {
  items: TemplateSummary[];
  selected: string[];
  onToggle: (id: string) => void;
  empty: ReactNode;
}) {
  if (items.length === 0) return <div className="dp-empty">{empty}</div>;
  return (
    <div className="dp-tiles">
      {items.map((item) => {
        const on = selected.includes(item.id);
        return (
          <button
            key={item.id}
            type="button"
            className={`dp-tile${on ? ' on' : ''}`}
            aria-pressed={on}
            onClick={() => onToggle(item.id)}
            title={item.visual_intent || item.category || item.kind}
          >
            <img src={item.image_url} alt="" loading="lazy" />
            <span className="dp-tile-cap">{item.category || item.kind}</span>
            {on && <span className="dp-tile-mark" aria-hidden="true" />}
          </button>
        );
      })}
    </div>
  );
}

function ResultPane({
  job,
  t,
  emptyText,
  onJob,
  onMessage
}: {
  job: JobRecord | null;
  t: Copy;
  emptyText: string;
  onJob: (job: JobRecord) => void;
  onMessage: (text: string, tone?: 'info' | 'error') => void;
}) {
  if (!job) return <div className="dp-empty">{emptyText}</div>;
  return (
    <JobPanel
      job={job}
      t={t}
      onCancelled={(next) => {
        onJob(next);
        onMessage(t.result.stopped);
      }}
      onError={(message) => onMessage(message, 'error')}
    />
  );
}

/* ============ 科研图 ============ */

export type FigureFormState = {
  kind: string;
  query: string;
  selected: string[];
  title: string;
  description: string;
  aspectRatio: string;
  layoutFidelity: 'strict' | 'balanced' | 'loose';
  styleStrength: 'high' | 'medium' | 'low';
  custom: string;
};

export const defaultFigureForm: FigureFormState = {
  kind: 'diagram',
  query: '',
  selected: [],
  title: '',
  description: '',
  aspectRatio: 'inherit',
  layoutFidelity: 'balanced',
  styleStrength: 'high',
  custom: ''
};

export function FigureForm({
  state,
  onState,
  job,
  onJob,
  onMessage,
  onGoTemplates,
  t,
  d
}: {
  state: FigureFormState;
  onState: (next: (current: FigureFormState) => FigureFormState) => void;
  job: JobRecord | null;
  onJob: (job: JobRecord) => void;
  onMessage: (text: string, tone?: 'info' | 'error') => void;
  onGoTemplates: () => void;
  t: Copy;
  d: DesktopCopy;
}) {
  const [templates, setTemplates] = useState<TemplateSummary[]>([]);
  const [pane, setPane] = useState<'templates' | 'result'>('templates');
  const { kind, query, selected, title, description, aspectRatio, layoutFidelity, styleStrength, custom } = state;

  function patch(values: Partial<FigureFormState>) {
    onState((current) => ({ ...current, ...values }));
  }

  useEffect(() => {
    let cancelled = false;
    listTemplates(kind, query)
      .then((items) => {
        if (!cancelled) setTemplates(items);
      })
      .catch(() => {
        /* 图库取不到不该打断填表，页面上的空态已经说明问题 */
      });
    return () => {
      cancelled = true;
    };
  }, [kind, query]);

  // 任务一提交就切到结果，省掉用户自己找进度条
  useEffect(() => {
    if (job) setPane('result');
  }, [job?.id]);

  function toggle(id: string) {
    onState((current) => ({
      ...current,
      selected: current.selected.includes(id)
        ? current.selected.filter((item) => item !== id)
        : [...current.selected, id].slice(0, MAX_TEMPLATES)
    }));
  }

  async function submit() {
    try {
      onMessage(t.paper.submitted);
      const created = await createJob({
        mode: 'paper_figure',
        payload: {
          figure_title: title,
          section_description: description,
          template_ids: selected,
          aspect_ratio: aspectRatio,
          layout_fidelity: layoutFidelity,
          style_strength: styleStrength,
          candidate_count: 1,
          custom_prompt: custom || null
        }
      });
      onJob(created);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : t.common.submitFailed, 'error');
    }
  }

  const ready = Boolean(title.trim() && description.trim() && selected.length > 0);

  return (
    <div className="dp-work">
      <Card
        title={d.pane.form}
        foot={
          <>
            <span className="dp-foot-note">{d.pane.selected(selected.length, MAX_TEMPLATES)}</span>
            <button type="button" className="dp-primary" disabled={!ready} onClick={submit}>
              {t.paper.generate}
            </button>
          </>
        }
      >
        <DField label={t.paper.figureTitle} span>
          <input value={title} onChange={(event) => patch({ title: event.target.value })} />
        </DField>
        <GrowField
          label={t.paper.description}
          value={description}
          onChange={(next) => patch({ description: next })}
          grow={2}
        />
        <div className="dp-row three">
          <DField label={t.paper.ratio}>
            <select value={aspectRatio} onChange={(event) => patch({ aspectRatio: event.target.value })}>
              <option value="inherit">{t.paper.inherited}</option>
              <option value="16:9">16:9</option>
              <option value="4:3">4:3</option>
              <option value="1:1">1:1</option>
              <option value="3:2">3:2</option>
            </select>
          </DField>
          <DField label={t.paper.fidelity}>
            <select
              value={layoutFidelity}
              onChange={(event) => patch({ layoutFidelity: event.target.value as FigureFormState['layoutFidelity'] })}
            >
              <option value="strict">strict</option>
              <option value="balanced">balanced</option>
              <option value="loose">loose</option>
            </select>
          </DField>
          <DField label={t.paper.strength}>
            <select
              value={styleStrength}
              onChange={(event) => patch({ styleStrength: event.target.value as FigureFormState['styleStrength'] })}
            >
              <option value="high">high</option>
              <option value="medium">medium</option>
              <option value="low">low</option>
            </select>
          </DField>
        </div>
        <GrowField
          label={t.paper.custom}
          hint={t.paper.customHint}
          value={custom}
          onChange={(next) => patch({ custom: next })}
        />
      </Card>

      <Card
        title={
          <Seg
            value={pane}
            onChange={setPane}
            items={[
              { key: 'templates', label: d.pane.templates },
              { key: 'result', label: d.pane.result, dot: Boolean(job) }
            ]}
          />
        }
        aside={
          pane === 'templates' ? (
            <div className="dp-head-tools">
              <select
                className="dp-mini"
                value={kind}
                onChange={(event) => patch({ kind: event.target.value })}
                aria-label={t.paper.kind}
              >
                <option value="diagram">{d.templates.diagram}</option>
                <option value="plot">{d.templates.plot}</option>
              </select>
              <input
                className="dp-mini dp-search"
                placeholder={d.pane.search}
                value={query}
                onChange={(event) => patch({ query: event.target.value })}
                aria-label={t.paper.search}
              />
            </div>
          ) : null
        }
        flush={pane === 'templates'}
      >
        {pane === 'templates' ? (
          <TileGrid
            items={templates}
            selected={selected}
            onToggle={toggle}
            empty={
              <>
                <strong>{d.pane.figureEmpty}</strong>
                <p>{d.pane.emptyHint}</p>
                <button type="button" className="dp-ghost" onClick={onGoTemplates}>
                  {d.pane.goTemplates}
                </button>
              </>
            }
          />
        ) : (
          <ResultPane job={job} t={t} emptyText={d.pane.resultEmpty} onJob={onJob} onMessage={onMessage} />
        )}
      </Card>
    </div>
  );
}

/* ============ 幻灯片 ============ */

export type SlideFormState = {
  /** 母版改为从模板库里选，不再上传——上传统一收进模板库页 */
  master: TemplateSummary | null;
  materials: AssetUpload[];
  material: string;
  pages: number;
  custom: string;
};

export const defaultSlideForm: SlideFormState = {
  master: null,
  materials: [],
  material: '',
  pages: 1,
  custom: ''
};

export function SlideForm({
  state,
  onState,
  job,
  onJob,
  onMessage,
  onGoTemplates,
  t,
  d
}: {
  state: SlideFormState;
  onState: (next: (current: SlideFormState) => SlideFormState) => void;
  job: JobRecord | null;
  onJob: (job: JobRecord) => void;
  onMessage: (text: string, tone?: 'info' | 'error') => void;
  onGoTemplates: () => void;
  t: Copy;
  d: DesktopCopy;
}) {
  const [masters, setMasters] = useState<TemplateSummary[]>([]);
  const [query, setQuery] = useState('');
  const [pane, setPane] = useState<'master' | 'result'>('master');
  const { master, materials, material, pages, custom } = state;

  function patch(values: Partial<SlideFormState>) {
    onState((current) => ({ ...current, ...values }));
  }

  useEffect(() => {
    let cancelled = false;
    listTemplates('master', query)
      .then((items) => {
        if (!cancelled) setMasters(items);
      })
      .catch(() => {
        /* 同科研图：空态已足够说明 */
      });
    return () => {
      cancelled = true;
    };
  }, [query]);

  useEffect(() => {
    if (job) setPane('result');
  }, [job?.id]);

  async function onMaterialFiles(files?: FileList | null) {
    if (!files?.length) return;
    try {
      onMessage(t.ppt.uploading);
      const uploaded = await Promise.all(Array.from(files).map((file) => uploadAsset(file)));
      onState((current) => ({ ...current, materials: [...current.materials, ...uploaded].slice(0, 10) }));
      onMessage(t.ppt.uploaded);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : t.common.uploadFailed, 'error');
    }
  }

  async function submit() {
    if (!master) return;
    try {
      onMessage(t.ppt.submitted);
      const created = await createJob({
        mode: 'ppt_slide',
        payload: {
          // 桌面版走 template_id（模板库里的母版）；网页版仍走 template_asset_id。
          // Rust 侧 validate_payload 两条来源取其一即可。
          template_id: master.id,
          material_text: material,
          material_asset_ids: materials.map((item) => item.id),
          page_count: pages,
          custom_prompt: custom || null
        }
      });
      onJob(created);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : t.common.submitFailed, 'error');
    }
  }

  const ready = Boolean(master && (material.trim() || materials.length > 0));
  const materialLabel = materials.length ? `${t.common.uploadedFiles} ${materials.length}` : '';

  return (
    <div className="dp-work">
      <Card
        title={d.pane.form}
        foot={
          <>
            <span className="dp-foot-note">{master ? master.category || master.kind : d.pane.masterNone}</span>
            <button type="button" className="dp-primary" disabled={!ready} onClick={submit}>
              {t.ppt.generate}
            </button>
          </>
        }
      >
        {/* 母版不在这里选：右列点缩略图即选中，左列再放一个「选择母版」入口
            等于同一件事两个按钮。选中的是哪张，看右列高亮 + 卡片底部那行就够。 */}
        <div className="dp-row two">
          <DField label={t.ppt.pages}>
            <DStep value={pages} min={1} max={20} onChange={(next) => patch({ pages: next })} />
          </DField>
          <DField label={t.ppt.materialFile}>
            <DPick
              accept=".pdf,.doc,.docx,.txt,.md,.markdown,.csv,.tsv,.json,application/pdf,text/*"
              label={t.common.choose}
              value={materialLabel}
              multiple
              onChange={onMaterialFiles}
            >
              {/* 已选附件就放在「选择」右边这块空位里，不再另起一行：
                  那一行只有几个 chip，却要占满整宽，把下面的输入框往下挤。
                  超过一行的量在这里横向滚动，卡片高度因此恒定。 */}
              {materials.length > 0 ? (
                <div className="dp-chips in-pick">
                  {materials.map((item) => (
                    <span key={item.id} className="dp-chip" title={item.filename}>
                      <span className="dp-chip-name">{item.filename}</span>
                      {/* 选错文件不该只能清空重来：叉号只从待提交列表里摘掉这一条，
                          已经上传到服务端的资源不动（下次提交不带它的 id 即可）。 */}
                      <button
                        type="button"
                        className="dp-chip-x"
                        aria-label={`${d.pane.removeFile} ${item.filename}`}
                        title={d.pane.removeFile}
                        onClick={() =>
                          onState((current) => ({
                            ...current,
                            materials: current.materials.filter((entry) => entry.id !== item.id)
                          }))
                        }
                      >
                        <svg viewBox="0 0 12 12" aria-hidden="true">
                          <path d="M3.4 3.4 8.6 8.6M8.6 3.4 3.4 8.6" />
                        </svg>
                      </button>
                    </span>
                  ))}
                </div>
              ) : null}
            </DPick>
          </DField>
        </div>

        <GrowField
          label={t.ppt.material}
          hint={t.ppt.materialHint}
          value={material}
          onChange={(next) => patch({ material: next })}
          grow={2}
        />
        <GrowField
          label={t.ppt.custom}
          hint={t.ppt.customHint}
          value={custom}
          onChange={(next) => patch({ custom: next })}
        />
      </Card>

      <Card
        title={
          <Seg
            value={pane}
            onChange={setPane}
            items={[
              { key: 'master', label: d.pane.master },
              { key: 'result', label: d.pane.result, dot: Boolean(job) }
            ]}
          />
        }
        aside={
          pane === 'master' ? (
            <div className="dp-head-tools">
              <span className="dp-foot-note">{d.pane.count(masters.length)}</span>
              <input
                className="dp-mini dp-search"
                placeholder={d.pane.search}
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                aria-label={d.pane.search}
              />
            </div>
          ) : null
        }
        flush={pane === 'master'}
      >
        {pane === 'master' ? (
          <TileGrid
            items={masters}
            selected={master ? [master.id] : []}
            // 单选：点已选中的那张就取消，点别的就换过去
            onToggle={(id) =>
              patch({ master: master?.id === id ? null : masters.find((item) => item.id === id) ?? null })
            }
            empty={
              <>
                <strong>{d.pane.masterEmpty}</strong>
                <p>{d.pane.emptyHint}</p>
                <button type="button" className="dp-ghost" onClick={onGoTemplates}>
                  {d.pane.goTemplates}
                </button>
              </>
            }
          />
        ) : (
          <ResultPane job={job} t={t} emptyText={d.pane.resultEmpty} onJob={onJob} onMessage={onMessage} />
        )}
      </Card>
    </div>
  );
}

/* ============ 设置：把共用的 Settings 包进同一套卡片骨架 ============ */

export function SettingsPane({ children }: { children: ReactNode }) {
  return (
    <div className="dp-work single">
      <Card>{children}</Card>
    </div>
  );
}

export { Card as DesktopCard, Seg as DesktopSeg, DField as DesktopField, DPick as DesktopPick };
