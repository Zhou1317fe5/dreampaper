// The single collapsible sidebar: one of three columns is shown at a time —
// the project list, the properties of the current selection, or the
// region/layer list. Export is a toolbar action and keeps no records.

import { useEffect, useState, type ReactNode } from 'react';
import type { CropDraft } from './canvas';
import type { WorkbenchCopy } from './copy';
import type { AspectPreset } from './geom';
import { findLayer, type Action, type EditorState } from './state';
import type { FontInfo, PixelRect, ProjectSummary, RepairGroup, TextLayer, TextLayout } from './types';

export function shortTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return '';
  const pad = (value: number) => String(value).padStart(2, '0');
  const today = new Date();
  const sameDay = date.toDateString() === today.toDateString();
  return sameDay
    ? `${pad(date.getHours())}:${pad(date.getMinutes())}`
    : `${pad(date.getMonth() + 1)}/${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

// ---- sidebar shell -------------------------------------------------------------

export type SidebarTab = 'projects' | 'properties' | 'layers';

export interface ProjectListProps {
  projects: ProjectSummary[];
  activeId: string | null;
  onOpen: (id: string) => void;
  onImport: () => void;
  onBlank: () => void;
  onCopy: () => void;
  onRename: (id: string, name: string) => void;
  onDelete: (id: string) => void;
}

export interface SidebarProps {
  tab: SidebarTab;
  onTab: (tab: SidebarTab) => void;
  collapsed: boolean;
  onToggle: () => void;
  c: WorkbenchCopy;
  projects: ProjectListProps;
  /** `null` while no project is open: the properties and layer columns are then empty. */
  inspector: InspectorProps | null;
}

const SIDEBAR_TABS: SidebarTab[] = ['projects', 'properties', 'layers'];

export function Sidebar({ tab, onTab, collapsed, onToggle, c, projects, inspector }: SidebarProps) {
  if (collapsed) {
    return (
      <aside className="wb-side wb-side-collapsed">
        <button type="button" className="wb-icon-btn" title={c.projects.expand} aria-label={c.projects.expand} onClick={onToggle}>
          <SideIcon kind="expand" />
        </button>
        <span className="wb-rail-sep" />
        {SIDEBAR_TABS.map((key) => (
          <button
            key={key}
            type="button"
            className={`wb-icon-btn${tab === key ? ' active' : ''}`}
            title={c.sidebar[key]}
            aria-label={c.sidebar[key]}
            disabled={key !== 'projects' && !inspector}
            onClick={() => {
              onTab(key);
              onToggle();
            }}
          >
            <SideIcon kind={key} />
          </button>
        ))}
      </aside>
    );
  }
  return (
    <aside className="wb-side">
      <div className="wb-side-head">
        <div className="wb-seg" role="tablist">
          {SIDEBAR_TABS.map((key) => (
            <button
              key={key}
              type="button"
              role="tab"
              aria-selected={tab === key}
              className={`wb-seg-btn${tab === key ? ' active' : ''}`}
              disabled={key !== 'projects' && !inspector}
              onClick={() => onTab(key)}
            >
              {c.sidebar[key]}
            </button>
          ))}
        </div>
        <button type="button" className="wb-icon-btn" title={c.projects.collapse} aria-label={c.projects.collapse} onClick={onToggle}>
          <SideIcon kind="collapse" />
        </button>
      </div>
      {tab === 'projects' && <ProjectList c={c} {...projects} />}
      {tab === 'properties' && (
        <div className="wb-side-body">{inspector ? <Properties {...inspector} /> : <p className="wb-empty">{c.projects.empty}</p>}</div>
      )}
      {tab === 'layers' && (
        <div className="wb-side-body">{inspector ? <LayerList {...inspector} /> : <p className="wb-empty">{c.projects.empty}</p>}</div>
      )}
    </aside>
  );
}

function SideIcon({ kind }: { kind: SidebarTab | 'expand' | 'collapse' | 'import' | 'blank' | 'copy' }) {
  const common = { viewBox: '0 0 20 20', fill: 'none', stroke: 'currentColor', strokeWidth: 1.6, 'aria-hidden': true } as const;
  switch (kind) {
    case 'projects':
      return (
        <svg {...common}>
          <rect x="3" y="4" width="14" height="4" rx="1" />
          <rect x="3" y="11" width="14" height="4" rx="1" />
        </svg>
      );
    case 'properties':
      return (
        <svg {...common}>
          <path d="M4 6h12M4 10h12M4 14h7" strokeLinecap="round" />
          <circle cx="8" cy="6" r="1.6" fill="var(--panel, #fff)" />
          <circle cx="13" cy="10" r="1.6" fill="var(--panel, #fff)" />
        </svg>
      );
    case 'layers':
      return (
        <svg {...common}>
          <path d="M10 3.5l7 3.5-7 3.5-7-3.5z" strokeLinejoin="round" />
          <path d="M3 10.5l7 3.5 7-3.5M3 13.5l7 3.5 7-3.5" strokeLinejoin="round" />
        </svg>
      );
    case 'expand':
      return (
        <svg {...common}>
          <path d="M7 5l5 5-5 5" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      );
    case 'collapse':
      return (
        <svg {...common}>
          <path d="M12 5l-5 5 5 5" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      );
    case 'import':
      return (
        <svg {...common}>
          <path d="M10 3v9M6.5 8.5L10 12l3.5-3.5M4 14v2.5h12V14" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      );
    case 'blank':
      return (
        <svg {...common}>
          <rect x="3.5" y="3.5" width="13" height="13" rx="1.5" />
          <path d="M10 7v6M7 10h6" strokeLinecap="round" />
        </svg>
      );
    case 'copy':
      return (
        <svg {...common}>
          <rect x="7" y="7" width="9.5" height="9.5" rx="1.5" />
          <path d="M13 7V4.5A1.5 1.5 0 0 0 11.5 3H5A1.5 1.5 0 0 0 3.5 4.5V11A1.5 1.5 0 0 0 5 12.5h2" />
        </svg>
      );
    default:
      return null;
  }
}

// ---- project list ------------------------------------------------------------

function ProjectList({ projects, activeId, c, onOpen, onImport, onBlank, onCopy, onRename, onDelete }: ProjectListProps & { c: WorkbenchCopy }) {
  const [confirmId, setConfirmId] = useState<string | null>(null);
  const [editing, setEditing] = useState<{ id: string; value: string } | null>(null);
  useEffect(() => {
    if (!confirmId) return;
    const timer = window.setTimeout(() => setConfirmId(null), 3000);
    return () => window.clearTimeout(timer);
  }, [confirmId]);

  return (
    <>
      <div className="wb-side-actions">
        <span className="wb-side-count">{c.projects.count(projects.length)}</span>
        <span className="wb-toolbar-spacer" />
        <button type="button" className="wb-icon-btn" title={c.projects.import} aria-label={c.projects.import} onClick={onImport}>
          <SideIcon kind="import" />
        </button>
        <button type="button" className="wb-icon-btn" title={c.projects.blank} aria-label={c.projects.blank} disabled={!activeId} onClick={onBlank}>
          <SideIcon kind="blank" />
        </button>
        <button type="button" className="wb-icon-btn" title={c.projects.copy} aria-label={c.projects.copy} disabled={!activeId} onClick={onCopy}>
          <SideIcon kind="copy" />
        </button>
      </div>
      {projects.length === 0 && <p className="wb-empty">{c.projects.empty}</p>}
      <ul className="wb-project-list">
        {projects.map((project) => (
          <li key={project.id} className={`wb-project${project.id === activeId ? ' active' : ''}`}>
            <button type="button" className="wb-project-main" onClick={() => onOpen(project.id)}>
              <img src={project.thumbnail} alt="" loading="lazy" decoding="async" />
              <span className="wb-project-text">
                {editing?.id === project.id ? (
                  <input
                    className="wb-input"
                    autoFocus
                    value={editing.value}
                    onClick={(event) => event.stopPropagation()}
                    onChange={(event) => setEditing({ id: project.id, value: event.target.value })}
                    onBlur={() => {
                      if (editing.value.trim() && editing.value.trim() !== project.name) onRename(project.id, editing.value.trim());
                      setEditing(null);
                    }}
                    onKeyDown={(event) => {
                      if (event.key === 'Enter') (event.target as HTMLInputElement).blur();
                      if (event.key === 'Escape') setEditing(null);
                    }}
                  />
                ) : (
                  <span className="wb-project-name" title={project.name}>
                    {project.name}
                  </span>
                )}
                <span className="wb-project-meta">
                  {shortTime(project.updated_at)} · {c.projects.size(project.export_width, project.export_height)}
                </span>
              </span>
            </button>
            <span className="wb-project-actions">
              <button type="button" className="wb-icon-btn" title={c.projects.rename} aria-label={c.projects.rename} onClick={() => setEditing({ id: project.id, value: project.name })}>
                ✎
              </button>
              <button
                type="button"
                className={`wb-icon-btn wb-danger${confirmId === project.id ? ' confirming' : ''}`}
                title={c.projects.remove}
                aria-label={c.projects.remove}
                onClick={() => {
                  if (confirmId === project.id) {
                    setConfirmId(null);
                    onDelete(project.id);
                  } else {
                    setConfirmId(project.id);
                  }
                }}
              >
                {confirmId === project.id ? c.projects.confirmRemove : '×'}
              </button>
            </span>
          </li>
        ))}
      </ul>
    </>
  );
}

// ---- inspector columns ----------------------------------------------------------

export interface CropControls {
  draft: CropDraft;
  aspect: AspectPreset;
  custom: { width: number; height: number };
  onAspect: (aspect: AspectPreset) => void;
  onCustom: (custom: { width: number; height: number }) => void;
  onRect: (field: 'x' | 'y' | 'width' | 'height', value: number) => void;
  onFlip: (axis: 'x' | 'y') => void;
  onReset: () => void;
  onDone: () => void;
}

export interface InspectorProps {
  state: EditorState;
  dispatch: React.Dispatch<Action>;
  fonts: FontInfo[];
  layouts: ReadonlyMap<string, TextLayout>;
  crop: CropControls | null;
  c: WorkbenchCopy;
  onReanalyze: (groupId: string) => void;
  onGroupRect: (id: string, rect: PixelRect) => void;
  onReocr: (groupId: string) => void;
  onAddText: (groupId: string) => void;
  onEyedropper: () => void;
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="wb-field">
      <span className="wb-field-label">{label}</span>
      {children}
    </label>
  );
}

function NumberInput({ value, onCommit, min, max, step = 1, disabled }: { value: number; onCommit: (value: number) => void; min?: number; max?: number; step?: number; disabled?: boolean }) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);
  const commit = () => {
    const parsed = Number(text);
    if (!Number.isFinite(parsed)) {
      setText(String(value));
      return;
    }
    let next = parsed;
    if (min !== undefined) next = Math.max(min, next);
    if (max !== undefined) next = Math.min(max, next);
    if (next !== value) onCommit(next);
    else setText(String(value));
  };
  return (
    <input
      className="wb-input wb-number"
      type="number"
      value={text}
      min={min}
      max={max}
      step={step}
      disabled={disabled}
      onChange={(event) => setText(event.target.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === 'Enter') commit();
      }}
    />
  );
}

function Properties(props: InspectorProps) {
  const { state, dispatch, c, crop } = props;
  if (crop) return <CropProperties controls={crop} state={state} c={c} />;
  const located = state.selection.id ? findLayer(state.doc, state.selection.id) : null;
  if (!located) return <p className="wb-empty">{c.inspector.nothing}</p>;
  if (located.layer.kind === 'repair') return <GroupProperties group={located.layer} {...props} />;
  return <TextProperties text={located.layer} parent={located.parent} {...props} />;
}

function RectFields({ rect, onChange, c, maxW, maxH }: { rect: { x: number; y: number; width: number; height: number }; onChange: (field: 'x' | 'y' | 'width' | 'height', value: number) => void; c: WorkbenchCopy; maxW: number; maxH: number }) {
  return (
    <>
      <div className="wb-row">
        <Field label={`${c.inspector.position} X`}>
          <NumberInput value={rect.x} min={0} max={maxW - 1} onCommit={(v) => onChange('x', v)} />
        </Field>
        <Field label="Y">
          <NumberInput value={rect.y} min={0} max={maxH - 1} onCommit={(v) => onChange('y', v)} />
        </Field>
      </div>
      <div className="wb-row">
        <Field label={`${c.inspector.size} W`}>
          <NumberInput value={rect.width} min={1} max={maxW} onCommit={(v) => onChange('width', v)} />
        </Field>
        <Field label="H">
          <NumberInput value={rect.height} min={1} max={maxH} onCommit={(v) => onChange('height', v)} />
        </Field>
      </div>
    </>
  );
}

function GroupProperties({ group, state, dispatch, c, onReanalyze, onGroupRect, onReocr, onAddText, onEyedropper }: InspectorProps & { group: RepairGroup }) {
  const { width: W, height: H } = state.doc.source;
  const setRect = (field: 'x' | 'y' | 'width' | 'height', value: number) => {
    const next = { ...group.rect, [field]: Math.round(value) };
    next.width = Math.max(1, Math.min(next.width, W));
    next.height = Math.max(1, Math.min(next.height, H));
    next.x = Math.max(0, Math.min(next.x, W - next.width));
    next.y = Math.max(0, Math.min(next.y, H - next.height));
    onGroupRect(group.id, next);
  };
  return (
    <div className="wb-props">
      <Field label={c.inspector.shape}>
        <span className="wb-static">{group.shape === 'rect' ? c.inspector.rect : c.inspector.ellipse}</span>
      </Field>
      <RectFields rect={group.rect} onChange={setRect} c={c} maxW={W} maxH={H} />
      <Field label={c.inspector.fill}>
        <span className="wb-color-row">
          <input type="color" value={group.fill.current} onChange={(event) => dispatch({ type: 'set_fill', id: group.id, color: event.target.value })} />
          <code>{group.fill.current}</code>
          <span className="wb-chip">{group.fill.source === 'auto' ? c.inspector.auto : c.inspector.manual}</span>
        </span>
      </Field>
      {group.fill.auto && (
        <Field label={c.inspector.autoValue}>
          <span className="wb-color-row">
            <span className="wb-swatch" style={{ background: group.fill.auto }} />
            <code>{group.fill.auto}</code>
            {group.fill.coverage !== null && <span className="wb-muted">{Math.round(group.fill.coverage * 100)}%</span>}
          </span>
        </Field>
      )}
      {group.fill.uneven && <p className="wb-warning">{c.inspector.uneven}</p>}
      <div className="wb-row wb-wrap">
        <button type="button" className="wb-btn" onClick={() => onReanalyze(group.id)}>
          {c.inspector.reanalyze}
        </button>
        <button type="button" className="wb-btn" disabled={group.fill.source === 'auto' || !group.fill.auto} onClick={() => dispatch({ type: 'reset_fill', id: group.id })}>
          {c.inspector.resetAuto}
        </button>
        <button type="button" className="wb-btn" onClick={onEyedropper}>
          {c.inspector.eyedropper}
        </button>
      </div>
      <Field label={c.inspector.ocr}>
        <span className="wb-static">{c.inspector.ocrStatus[group.ocr.status]}</span>
      </Field>
      {group.ocr.stale && <p className="wb-warning">{c.inspector.stale}</p>}
      <div className="wb-row wb-wrap">
        <button type="button" className="wb-btn" onClick={() => onReocr(group.id)}>
          {c.inspector.reocr}
        </button>
        <button type="button" className="wb-btn" onClick={() => onAddText(group.id)}>
          {c.inspector.addText}
        </button>
        <button type="button" className="wb-btn" disabled={group.children.length < 2} onClick={() => dispatch({ type: 'merge_texts', groupId: group.id, newId: crypto.randomUUID() })}>
          {c.inspector.mergeTexts}
        </button>
      </div>
    </div>
  );
}

function TextProperties({ text, parent, state, dispatch, c, fonts, layouts }: InspectorProps & { text: TextLayer; parent: RepairGroup | null }) {
  const { width: W, height: H } = state.doc.source;
  const layout = layouts.get(text.id);
  const update = (patch: Partial<TextLayer>, mergeKey?: string) => dispatch({ type: 'update_text', id: text.id, patch, mergeKey });
  const setRect = (field: 'x' | 'y' | 'width' | 'height', value: number) => {
    const next = { ...text.rect, [field]: Math.round(value) };
    next.width = Math.max(1, Math.min(next.width, W));
    next.height = Math.max(1, Math.min(next.height, H));
    next.x = Math.max(0, Math.min(next.x, W - next.width));
    next.y = Math.max(0, Math.min(next.y, H - next.height));
    dispatch({ type: 'set_layer_rect', id: text.id, rect: next });
  };
  const covering = fonts.filter((font) => font.covers !== false);
  const others = fonts.filter((font) => font.covers === false);
  const knownFamily = fonts.some((font) => font.family === text.font.family);
  const groups = state.doc.layers.filter((layer): layer is RepairGroup => layer.kind === 'repair');
  return (
    <div className="wb-props">
      <Field label={c.inspector.content}>
        <textarea className="wb-input wb-textarea" rows={3} value={text.text} onChange={(event) => update({ text: event.target.value }, `text:${text.id}`)} />
      </Field>
      <Field label={c.inspector.font}>
        <select className="wb-input" value={text.font.family} onChange={(event) => update({ font: { ...text.font, family: event.target.value } })}>
          {!knownFamily && <option value={text.font.family}>{text.font.family}</option>}
          <optgroup label={c.inspector.fontRecommended}>
            {covering.map((font) => (
              <option key={font.family} value={font.family}>
                {font.family}
              </option>
            ))}
          </optgroup>
          {others.length > 0 && (
            <optgroup label={c.inspector.fontOther}>
              {others.map((font) => (
                <option key={font.family} value={font.family}>
                  {font.family}
                </option>
              ))}
            </optgroup>
          )}
        </select>
      </Field>
      {layout?.missing_font && <p className="wb-warning">{c.inspector.fontMissing(text.font.family)}</p>}
      <div className="wb-row">
        <Field label={text.auto_fit ? c.inspector.fontSizeAuto : c.inspector.fontSize}>
          <NumberInput
            value={layout && text.auto_fit ? layout.font_size : text.font.size}
            min={1}
            max={4096}
            step={0.5}
            disabled={text.auto_fit}
            onCommit={(v) => update({ font: { ...text.font, size: v } })}
          />
        </Field>
        <Field label={c.inspector.weight}>
          <select className="wb-input" value={text.font.weight} onChange={(event) => update({ font: { ...text.font, weight: Number(event.target.value) } })}>
            {[300, 400, 500, 600, 700, 800].map((weight) => (
              <option key={weight} value={weight}>
                {weight}
              </option>
            ))}
          </select>
        </Field>
      </div>
      <div className="wb-row wb-row-controls">
        <span className="wb-color-row">
          <input type="color" aria-label={c.inspector.color} value={text.color} onChange={(event) => update({ color: event.target.value }, `color:${text.id}`)} />
          <code>{text.color}</code>
        </span>
        <label className="wb-check" title={c.inspector.italicHint}>
          <input type="checkbox" checked={text.font.italic} onChange={(event) => update({ font: { ...text.font, italic: event.target.checked } })} />
          <span>{c.inspector.italic}</span>
        </label>
        <label className="wb-check" title={c.inspector.autoFitHint}>
          <input type="checkbox" checked={text.auto_fit} onChange={(event) => update({ auto_fit: event.target.checked })} />
          <span>{c.inspector.autoFit}</span>
        </label>
      </div>
      <div className="wb-row">
        <Field label={c.inspector.align}>
          <span className="wb-seg">
            {(['left', 'center', 'right'] as const).map((value) => (
              <button key={value} type="button" className={`wb-seg-btn${text.align === value ? ' active' : ''}`} onClick={() => update({ align: value })}>
                {c.inspector[value]}
              </button>
            ))}
          </span>
        </Field>
        <Field label={c.inspector.valign}>
          <span className="wb-seg">
            {(['top', 'middle', 'bottom'] as const).map((value) => (
              <button key={value} type="button" className={`wb-seg-btn${text.valign === value ? ' active' : ''}`} onClick={() => update({ valign: value })}>
                {c.inspector[value]}
              </button>
            ))}
          </span>
        </Field>
      </div>
      <div className="wb-row">
        <Field label={c.inspector.lineHeight}>
          <NumberInput value={text.line_height} min={0.5} max={4} step={0.05} onCommit={(v) => update({ line_height: v })} />
        </Field>
        <Field label={c.inspector.letterSpacing}>
          <NumberInput value={text.letter_spacing} min={-512} max={512} step={0.5} onCommit={(v) => update({ letter_spacing: v })} />
        </Field>
      </div>
      {layout?.overflow && <p className="wb-warning">{c.inspector.overflow}</p>}
      <RectFields rect={text.rect} onChange={setRect} c={c} maxW={W} maxH={H} />
      <div className="wb-row wb-wrap">
        {parent ? (
          <button type="button" className="wb-btn" onClick={() => dispatch({ type: 'unlink_text', textId: text.id })}>
            {c.inspector.unlink}
          </button>
        ) : (
          groups.length > 0 && (
            <select
              className="wb-input"
              value=""
              onChange={(event) => {
                if (event.target.value) dispatch({ type: 'link_text', textId: text.id, groupId: event.target.value });
              }}
            >
              <option value="">{c.inspector.link}</option>
              {groups.map((group, index) => (
                <option key={group.id} value={group.id}>
                  #{index + 1} {group.shape === 'rect' ? c.inspector.rect : c.inspector.ellipse} {group.rect.width}×{group.rect.height}
                </option>
              ))}
            </select>
          )
        )}
      </div>
    </div>
  );
}

const ASPECTS: AspectPreset[] = ['free', 'original', '1:1', '4:3', '16:9', '3:4', '9:16', 'custom'];

function CropProperties({ controls, state, c }: { controls: CropControls; state: EditorState; c: WorkbenchCopy }) {
  const { width: W, height: H } = state.doc.source;
  const label = (aspect: AspectPreset) => (aspect === 'free' ? c.inspector.aspectFree : aspect === 'original' ? c.inspector.aspectOriginal : aspect === 'custom' ? c.inspector.aspectCustom : aspect);
  return (
    <div className="wb-props">
      <strong>{c.inspector.cropTitle}</strong>
      <p className="wb-muted">{c.inspector.cropHint}</p>
      <Field label={c.inspector.aspect}>
        <select className="wb-input" value={controls.aspect} onChange={(event) => controls.onAspect(event.target.value as AspectPreset)}>
          {ASPECTS.map((aspect) => (
            <option key={aspect} value={aspect}>
              {label(aspect)}
            </option>
          ))}
        </select>
      </Field>
      {controls.aspect === 'custom' && (
        <div className="wb-row">
          <Field label="W">
            <NumberInput value={controls.custom.width} min={1} max={10000} onCommit={(v) => controls.onCustom({ ...controls.custom, width: v })} />
          </Field>
          <Field label="H">
            <NumberInput value={controls.custom.height} min={1} max={10000} onCommit={(v) => controls.onCustom({ ...controls.custom, height: v })} />
          </Field>
        </div>
      )}
      <RectFields rect={controls.draft.crop} onChange={controls.onRect} c={c} maxW={W} maxH={H} />
      <div className="wb-row wb-wrap">
        <button type="button" className={`wb-btn${controls.draft.flip_x ? ' active' : ''}`} onClick={() => controls.onFlip('x')}>
          {c.inspector.flipX}
        </button>
        <button type="button" className={`wb-btn${controls.draft.flip_y ? ' active' : ''}`} onClick={() => controls.onFlip('y')}>
          {c.inspector.flipY}
        </button>
        <button type="button" className="wb-btn" onClick={controls.onReset}>
          {c.inspector.resetCrop}
        </button>
        <button type="button" className="wb-btn wb-primary" onClick={controls.onDone}>
          {c.inspector.applyCrop}
        </button>
      </div>
    </div>
  );
}

function LayerList({ state, dispatch, c }: InspectorProps) {
  const layers = [...state.doc.layers].reverse();
  const [open, setOpen] = useState<Record<string, boolean>>({});
  if (layers.length === 0) return <p className="wb-empty">{c.inspector.layersEmpty}</p>;
  const row = (layer: RepairGroup | TextLayer, parentId: string | null, label: string) => {
    const selected = state.selection.id === layer.id;
    return (
      <div key={layer.id} className={`wb-layer${selected ? ' active' : ''}${parentId ? ' child' : ''}`}>
        <button type="button" className="wb-layer-eye" title={layer.visible ? c.inspector.hidden : c.inspector.visible} onClick={() => dispatch({ type: 'toggle_visible', id: layer.id })}>
          {layer.visible ? '●' : '○'}
        </button>
        <button type="button" className="wb-layer-name" onClick={() => dispatch({ type: 'select', id: layer.id, parentId })}>
          {layer.kind === 'repair' && (
            <span
              className="wb-layer-toggle"
              onClick={(event) => {
                event.stopPropagation();
                setOpen((current) => ({ ...current, [layer.id]: !current[layer.id] }));
              }}
            >
              {open[layer.id] ? '▾' : '▸'}
            </span>
          )}
          {label}
        </button>
        <span className="wb-layer-actions">
          <button type="button" className="wb-icon-btn" title={c.inspector.up} onClick={() => dispatch({ type: 'reorder_layer', id: layer.id, direction: 'up' })}>
            ↑
          </button>
          <button type="button" className="wb-icon-btn" title={c.inspector.down} onClick={() => dispatch({ type: 'reorder_layer', id: layer.id, direction: 'down' })}>
            ↓
          </button>
          <button type="button" className="wb-icon-btn" title={c.inspector.duplicate} onClick={() => dispatch({ type: 'duplicate_layer', id: layer.id, newIds: Array.from({ length: layer.kind === 'repair' ? layer.children.length + 1 : 1 }, () => crypto.randomUUID()) })}>
            ⧉
          </button>
          <button type="button" className="wb-icon-btn wb-danger" title={c.inspector.remove} onClick={() => dispatch({ type: 'remove_layer', id: layer.id })}>
            ×
          </button>
        </span>
      </div>
    );
  };
  return (
    <div className="wb-layers">
      {layers.map((layer) => {
        if (layer.kind === 'repair') {
          const shape = layer.shape === 'rect' ? c.inspector.rect : c.inspector.ellipse;
          return (
            <div key={layer.id}>
              {row(layer, null, c.inspector.groupLabel(shape, layer.children.length))}
              {open[layer.id] && [...layer.children].reverse().map((child) => row(child, layer.id, child.text.split('\n')[0] || c.inspector.textLabel))}
            </div>
          );
        }
        return row(layer, null, `${c.inspector.independent}: ${layer.text.split('\n')[0] || c.inspector.textLabel}`);
      })}
    </div>
  );
}
