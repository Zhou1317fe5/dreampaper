import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { createJob, getConfig, getJob, listTemplates, saveConfig, uploadAsset } from './api';
import type { AppConfig, AssetUpload, JobRecord, ModelProfile, TemplateSummary } from './types';

type Lang = 'zh' | 'en';
type UiMode = 'paper' | 'ppt' | 'settings';
type PaperFigureState = {
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
type PptSlideState = {
  asset: AssetUpload | null;
  materials: AssetUpload[];
  material: string;
  pages: number;
  custom: string;
};
type StateUpdater<T> = (next: T | ((current: T) => T)) => void;

const defaultPaperState: PaperFigureState = {
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

const defaultPptState: PptSlideState = {
  asset: null,
  materials: [],
  material: '',
  pages: 1,
  custom: ''
};

const emptyConfig: AppConfig = {
  version: 1,
  active_design_profile: 'design-default',
  active_implement_profile: 'implement-default',
  active_search_profile: 'search-default',
  proxy_url: 'http://127.0.0.1:7890',
  ppt_page_plan_concurrency: null,
  ppt_image_concurrency: null,
  model_profiles: []
};

const copy = {
  zh: {
    nav: { paper: '科研图', ppt: '幻灯片', settings: '设置' },
    brandSub: 'figure / slide',
    common: {
      protocol: '协议',
      baseUrl: 'URL',
      model: '模型',
      apiKey: '密钥',
      save: '保存',
      saved: '已保存',
      saving: '保存中...',
      redacted: '保存后脱敏',
      submitFailed: '提交失败',
      uploadFailed: '上传失败',
      choose: '选择',
      noFile: '未选择',
      uploadedFiles: '已上传'
    },
    settings: { design: 'Design', implement: 'Implement', search: 'Search', searchHint: '幻灯片视觉素材检索。duckduckgo_html 免密钥；tavily 填 API key；openai_chat 可用 Grok/OpenAI 兼容 search model。', proxyAndConcurrency: '代理与并发', proxy: '代理', proxyHint: '本地代理地址，例如 http://127.0.0.1:7890；留空表示不指定代理。', concurrency: '幻灯片并发', concurrencyHint: '留空表示跟随本次输入的幻灯片页数；实际并发不会超过页数。制图建议先设为 1，降低网关 502。', pagePlanConcurrency: '规划并发', imageConcurrency: '制图并发', defaultByPages: '默认=页数', size: '尺寸', quality: '质量', format: '格式', ratio: '比例', clarity: '清晰度', tendency: '倾向', version: '版本', timeout: '超时(秒)', timeoutHint: '同步出图可能较久，implement 建议 600–900。', retries: '重试次数', maxResults: '结果数', keySet: '密钥已配置', keyNone: '未配置密钥', notSet: '未设置' },
    paper: { title: '科研图', intro: '选择 template 作为 few-shot 风格参考。', figureTitle: '标题', description: '方法', ratio: '比例', fidelity: '布局', strength: '风格', custom: '约束', customHint: '可选，用于补充禁用元素、强调风格、文字限制或审稿要求。', generate: '生成', search: '搜索', kind: '类型', inherited: '继承', submitted: '科研图任务已提交' },
    ppt: { title: '幻灯片', intro: '上传 template，分析母版，再批量生成页面。', template: '母版', pages: '页数', material: '资料', materialFile: '附件', materialHint: '可输入文字，也可上传 pdf、docx、txt、md、csv 等资料。', custom: '约束', customHint: '可选，用于补充页数结构、禁用元素、术语、颜色或展示重点。', generate: '生成', submitted: '幻灯片任务已提交', uploading: '上传中...', uploaded: '已上传' },
    result: { title: 'Result', waiting: '等待中', progress: '进度', current: '当前', step: '当前步骤', failed: '失败', completed: '完成', queued: '排队中', running: '运行中', preview: '预览', download: '下载', of: '/' }
  },
  en: {
    nav: { paper: 'Figure', ppt: 'Slide', settings: 'Settings' },
    brandSub: 'figure / slide',
    common: {
      protocol: 'Protocol',
      baseUrl: 'URL',
      model: 'Model',
      apiKey: 'Key',
      save: 'Save',
      saved: 'Saved',
      saving: 'Saving...',
      redacted: 'Redacted after save',
      submitFailed: 'Submit failed',
      uploadFailed: 'Upload failed',
      choose: 'Choose',
      noFile: 'No file',
      uploadedFiles: 'Uploaded'
    },
    settings: { design: 'Design', implement: 'Implement', search: 'Search', searchHint: 'Slide visual grounding search. duckduckgo_html needs no key; tavily needs API key; openai_chat is an OpenAI-compatible search model (e.g. Grok endpoint).', proxyAndConcurrency: 'Proxy & concurrency', proxy: 'Proxy', proxyHint: 'Local proxy URL, e.g. http://127.0.0.1:7890. Leave empty to disable explicit proxy.', concurrency: 'Slide concurrency', concurrencyHint: 'Leave empty to follow the current Slide page count; actual concurrency will not exceed pages. Prefer image concurrency = 1 to reduce 502s.', pagePlanConcurrency: 'Plan workers', imageConcurrency: 'Image workers', defaultByPages: 'default=pages', size: 'Size', quality: 'Quality', format: 'Format', ratio: 'Ratio', clarity: 'Sharpness', tendency: 'Quality', version: 'Version', timeout: 'Timeout (s)', timeoutHint: 'Sync image APIs can be slow; implement often needs 600–900s.', retries: 'Retries', maxResults: 'Results', keySet: 'Key set', keyNone: 'No key', notSet: 'Not set' },
    paper: { title: 'Figure', intro: 'Choose templates as few-shot visual references.', figureTitle: 'Title', description: 'Method', ratio: 'Ratio', fidelity: 'Layout', strength: 'Style', custom: 'Rules', customHint: 'Optional constraints for banned elements, style emphasis, text limits, or review requirements.', generate: 'Generate', search: 'Search', kind: 'Type', inherited: 'Template', submitted: 'Figure job submitted' },
    ppt: { title: 'Slide', intro: 'Upload a template, analyze the master, then generate pages.', template: 'Master', pages: 'Pages', material: 'Material', materialFile: 'Files', materialHint: 'Enter text or upload pdf, docx, txt, md, csv, and other common materials.', custom: 'Rules', customHint: 'Optional constraints for page structure, banned elements, terms, colors, or focus.', generate: 'Generate', submitted: 'Slide job submitted', uploading: 'Uploading...', uploaded: 'Uploaded' },
    result: { title: 'Result', waiting: 'Waiting', progress: 'Progress', current: 'Current', step: 'Current step', failed: 'Failed', completed: 'Completed', queued: 'Queued', running: 'Running', preview: 'Preview', download: 'Download', of: '/' }
  }
} as const;

const PAPER_STAGE_WEIGHTS: Record<string, number> = {
  queued: 4,
  started: 8,
  paper_validate: 12,
  paper_templates: 16,
  paper_structure_prompt: 20,
  paper_structure: 32,
  paper_structure_parse: 38,
  paper_prompt: 44,
  paper_design: 58,
  paper_parse: 68,
  paper_implement: 86,
  paper_save: 94,
  completed: 100,
  failed: 100
};

const PPT_STAGE_WEIGHTS: Record<string, number> = {
  queued: 3,
  started: 6,
  ppt_validate: 10,
  ppt_template: 14,
  ppt_material: 18,
  ppt_visual_assets: 22,
  ppt_analyze: 30,
  ppt_parse_template: 36,
  ppt_outline_prompt: 40,
  ppt_outline: 48,
  ppt_parse_outline: 52,
  ppt_page_plan_queue: 56,
  ppt_merge_pages: 72,
  ppt_implement_queue: 76,
  completed: 100,
  failed: 100
};

function useJobPolling(job: JobRecord | null, setJob: (job: JobRecord) => void, onError: (message: string) => void) {
  useEffect(() => {
    if (!job || job.status === 'succeeded' || job.status === 'failed') return;
    const timer = window.setInterval(() => {
      getJob(job.id)
        .then(setJob)
        .catch((error) => onError(error instanceof Error ? error.message : String(error)));
    }, 1800);
    return () => window.clearInterval(timer);
  }, [job, setJob, onError]);
}

export function App() {
  const [mode, setMode] = useState<UiMode>('paper');
  const [lang, setLang] = useState<Lang>('zh');
  const [config, setConfig] = useState<AppConfig>(emptyConfig);
  const [toast, setToast] = useState<{ id: number; text: string; tone: 'info' | 'error' } | null>(null);
  const [paperJob, setPaperJob] = useState<JobRecord | null>(null);
  const [pptJob, setPptJob] = useState<JobRecord | null>(null);
  const [paperState, setPaperState] = useState<PaperFigureState>(defaultPaperState);
  const [pptState, setPptState] = useState<PptSlideState>(defaultPptState);
  const t = copy[lang];

  const showMessage = useMemo(() => {
    return (text: string, tone: 'info' | 'error' = 'info') => {
      setToast({ id: Date.now(), text, tone });
    };
  }, []);

  useEffect(() => {
    getConfig().then(setConfig).catch((error) => showMessage(error.message, 'error'));
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

  return (
    <main className="shell">
      <header className="topbar clay-panel">
        <div className="brand">
          <div className="brand-mark"><img src="/favor.png" alt="" /></div>
          <div className="brand-text">
            <h1>DREAMPAPER</h1>
          </div>
        </div>
        <div className="top-row">
          <nav className="nav" aria-label="Primary">
            <button type="button" className={mode === 'paper' ? 'active' : ''} onClick={() => setMode('paper')}>{t.nav.paper}</button>
            <button type="button" className={mode === 'ppt' ? 'active' : ''} onClick={() => setMode('ppt')}>{t.nav.ppt}</button>
          </nav>
          <div className="top-tools">
            <button
              type="button"
              className={`tool-button tool-icon-only ${mode === 'settings' ? 'active' : ''}`}
              onClick={() => setMode('settings')}
              aria-label={t.nav.settings}
              title={t.nav.settings}
            >
              <span className="tool-icon" aria-hidden="true">⚙</span>
            </button>
            <button type="button" className="lang-toggle" onClick={() => setLang(lang === 'zh' ? 'en' : 'zh')}>{lang === 'zh' ? 'EN' : '中'}</button>
          </div>
        </div>
      </header>
      <section className="workspace">
        {mode === 'settings' && (
          <Settings config={config} onChange={setConfig} onSave={persistConfig} t={t} />
        )}
        {mode === 'paper' && (
          <>
            <PaperFigure state={paperState} onState={setPaperState} onJob={setPaperJob} onMessage={showMessage} t={t} />
            {paperJob && <JobPanel job={paperJob} t={t} />}
          </>
        )}
        {mode === 'ppt' && (
          <>
            <PptSlide state={pptState} onState={setPptState} onJob={setPptJob} onMessage={showMessage} t={t} />
            {pptJob && <JobPanel job={pptJob} t={t} />}
          </>
        )}
      </section>
      {toast && (
        <div className={`toast toast-${toast.tone}`} role="status" aria-live="polite" key={toast.id}>
          <span>{toast.text}</span>
          <button type="button" className="toast-close" aria-label="Close" onClick={() => setToast(null)}>
            ×
          </button>
        </div>
      )}
    </main>
  );
}

const PROTOCOL_LABELS: Record<string, string> = {
  openai_responses: 'OpenAI Responses',
  openai_chat: 'OpenAI Chat',
  anthropic_messages: 'Anthropic Messages',
  image2: 'image2',
  banana2: 'banana2',
  duckduckgo_html: 'duckduckgo_html',
  tavily: 'tavily'
};

function protocolLabel(value: string) {
  return PROTOCOL_LABELS[value] || value;
}

function Settings({ config, onChange, onSave, t }: { config: AppConfig; onChange: (config: AppConfig) => void; onSave: (config: AppConfig) => void; t: typeof copy[Lang] }) {
  const design = config.model_profiles.find((item) => item.role === 'design') || defaultDesign();
  const implement = config.model_profiles.find((item) => item.role === 'implement') || defaultImplement();
  const search = config.model_profiles.find((item) => item.role === 'search') || defaultSearch();
  // 各段独立开合：配置时常需要来回对照 design / implement，单开手风琴会误关正在编辑的段
  const [open, setOpen] = useState<string[]>(['design']);

  function toggle(key: string) {
    setOpen((current) => (current.includes(key) ? current.filter((item) => item !== key) : [...current, key]));
  }

  function upsert(profile: ModelProfile) {
    const rest = config.model_profiles.filter((item) => item.id !== profile.id);
    const next = { ...config, model_profiles: [...rest, profile] };
    if (profile.role === 'design') next.active_design_profile = profile.id;
    if (profile.role === 'implement') next.active_implement_profile = profile.id;
    if (profile.role === 'search') next.active_search_profile = profile.id;
    onChange(next);
  }

  function patchConfig(values: Partial<AppConfig>) {
    onChange({ ...config, ...values });
  }

  function patchConcurrency(key: 'ppt_page_plan_concurrency' | 'ppt_image_concurrency', value: string) {
    const trimmed = value.trim();
    const nextValue = trimmed ? Math.min(20, Math.max(1, Number.parseInt(trimmed, 10) || 1)) : null;
    patchConfig({ [key]: nextValue } as Partial<AppConfig>);
  }

  function modelSummary(profile: ModelProfile) {
    return [protocolLabel(profile.protocol), profile.model].filter(Boolean).join(' · ');
  }

  return (
    <div className="settings-stack">
      <SettingsSection
        title={t.settings.design}
        summary={modelSummary(design)}
        badge={design.has_api_key ? t.settings.keySet : t.settings.keyNone}
        badgeTone={design.has_api_key ? 'ok' : 'warn'}
        open={open.includes('design')}
        onToggle={() => toggle('design')}
      >
        <ModelEditor profile={design} onChange={upsert} t={t} />
      </SettingsSection>

      <SettingsSection
        title={t.settings.implement}
        summary={modelSummary(implement)}
        badge={implement.has_api_key ? t.settings.keySet : t.settings.keyNone}
        badgeTone={implement.has_api_key ? 'ok' : 'warn'}
        open={open.includes('implement')}
        onToggle={() => toggle('implement')}
      >
        <ModelEditor profile={implement} onChange={upsert} t={t} />
      </SettingsSection>

      <SettingsSection
        title={t.settings.search}
        summary={modelSummary(search)}
        badge={search.protocol === 'duckduckgo_html' ? undefined : search.has_api_key ? t.settings.keySet : t.settings.keyNone}
        badgeTone={search.has_api_key ? 'ok' : 'warn'}
        open={open.includes('search')}
        onToggle={() => toggle('search')}
      >
        <ModelEditor profile={search} onChange={upsert} t={t} hint={t.settings.searchHint} />
      </SettingsSection>

      <SettingsSection
        title={t.settings.proxyAndConcurrency}
        summary={config.proxy_url || t.settings.notSet}
        open={open.includes('runtime')}
        onToggle={() => toggle('runtime')}
      >
        <div className="settings-side-block">
          <Field label={t.settings.proxy}>
            <input
              value={config.proxy_url ?? ''}
              placeholder="http://127.0.0.1:7890"
              onChange={(event) => patchConfig({ proxy_url: event.target.value })}
            />
          </Field>
          <p className="settings-side-note">{t.settings.proxyHint}</p>
        </div>
        <div className="settings-side-divider" aria-hidden="true" />
        <div className="settings-side-block">
          <h3 className="settings-side-subtitle">{t.settings.concurrency}</h3>
          <div className="field-row two">
            <Field label={t.settings.pagePlanConcurrency}>
              <input
                type="number"
                min="1"
                max="20"
                placeholder={t.settings.defaultByPages}
                value={config.ppt_page_plan_concurrency ?? ''}
                onChange={(event) => patchConcurrency('ppt_page_plan_concurrency', event.target.value)}
              />
            </Field>
            <Field label={t.settings.imageConcurrency}>
              <input
                type="number"
                min="1"
                max="20"
                placeholder={t.settings.defaultByPages}
                value={config.ppt_image_concurrency ?? ''}
                onChange={(event) => patchConcurrency('ppt_image_concurrency', event.target.value)}
              />
            </Field>
          </div>
          <p className="settings-side-note">{t.settings.concurrencyHint}</p>
        </div>
      </SettingsSection>

      <div className="actions-row left settings-actions">
        <button className="primary" onClick={() => onSave(config)}>{t.common.save}</button>
      </div>
    </div>
  );
}

function SettingsSection({
  title,
  summary,
  badge,
  badgeTone,
  open,
  onToggle,
  children
}: {
  title: string;
  summary: string;
  badge?: string;
  badgeTone?: 'ok' | 'warn';
  open: boolean;
  onToggle: () => void;
  children: React.ReactNode;
}) {
  return (
    <section className={`clay-panel settings-section${open ? ' open' : ''}`}>
      <button type="button" className="settings-section-header" onClick={onToggle} aria-expanded={open}>
        <span className="settings-section-title">{title}</span>
        <span className="settings-section-summary" title={summary}>{summary}</span>
        {badge ? <span className={`settings-section-badge ${badgeTone || 'ok'}`}>{badge}</span> : null}
        <span className="settings-section-chevron" aria-hidden="true">›</span>
      </button>
      <div className="settings-section-panel">
        <div className="settings-section-panel-inner">
          {/* 始终渲染：条件渲染会让收起方向没有过渡可言 */}
          <div className="settings-section-body" inert={!open}>{children}</div>
        </div>
      </div>
    </section>
  );
}

function ModelEditor({ profile, onChange, t, hint }: { profile: ModelProfile; onChange: (profile: ModelProfile) => void; t: typeof copy[Lang]; hint?: string }) {
  const isImplement = profile.role === 'implement';
  const isSearch = profile.role === 'search';
  const defaults = profile.output_defaults || {};
  function patch(values: Partial<ModelProfile>) {
    onChange({ ...profile, ...values });
  }
  function patchDefaults(values: Record<string, string>) {
    patch({ output_defaults: { ...defaults, ...values } });
  }
  return (
    <div className="model-fields">
      {hint && <p className="model-panel-hint">{hint}</p>}
      <Field label={t.common.protocol}>
        <select value={profile.protocol} onChange={(event) => patch({ protocol: event.target.value })}>
          {profile.role === 'design' ? (
            <>
              <option value="openai_responses">OpenAI Responses</option>
              <option value="openai_chat">OpenAI Chat</option>
              <option value="anthropic_messages">Anthropic Messages</option>
            </>
          ) : profile.role === 'search' ? (
            <>
              <option value="duckduckgo_html">duckduckgo_html</option>
              <option value="tavily">tavily</option>
              <option value="openai_chat">openai_chat (search model)</option>
            </>
          ) : (
            <>
              <option value="image2">image2</option>
              <option value="banana2">banana2</option>
            </>
          )}
        </select>
      </Field>
      <Field label={t.common.baseUrl}>
        <input
          value={profile.base_url}
          placeholder={isSearch && profile.protocol === 'tavily' ? 'https://api.tavily.com' : isSearch && profile.protocol === 'openai_chat' ? 'https://api.x.ai/v1' : undefined}
          onChange={(event) => patch({ base_url: event.target.value })}
        />
      </Field>
      <Field label={t.common.model}>
        <input
          value={profile.model}
          placeholder={isSearch && profile.protocol === 'duckduckgo_html' ? 'duckduckgo-html' : isSearch && profile.protocol === 'openai_chat' ? 'grok-3' : undefined}
          onChange={(event) => patch({ model: event.target.value })}
        />
      </Field>
      <Field label={t.common.apiKey}>
        <input
          type="password"
          disabled={isSearch && profile.protocol === 'duckduckgo_html'}
          placeholder={isSearch && profile.protocol === 'duckduckgo_html' ? '—' : (profile.api_key_hint || t.common.redacted)}
          onChange={(event) => patch({ api_key: event.target.value })}
        />
      </Field>
      <div className="field-row two">
        <Field label={t.settings.timeout} hint={isImplement ? t.settings.timeoutHint : undefined}>
          <IntegerInput
            value={profile.timeout_seconds}
            min={5}
            max={1800}
            fallback={isImplement ? 600 : isSearch ? 20 : 120}
            onCommit={(next) => patch({ timeout_seconds: next })}
          />
        </Field>
        <Field label={t.settings.retries}>
          <IntegerInput
            value={profile.max_retries}
            min={0}
            max={8}
            fallback={0}
            onCommit={(next) => patch({ max_retries: next })}
          />
        </Field>
      </div>
      {isSearch && (
        <Field label={t.settings.maxResults}>
          <IntegerInput
            value={Number.parseInt(defaults.max_results || '3', 10) || 3}
            min={1}
            max={8}
            fallback={3}
            onCommit={(next) => patchDefaults({ max_results: String(next) })}
          />
        </Field>
      )}
      {isImplement && profile.protocol === 'banana2' && (
        <Field label={t.settings.version}>
          <input value={profile.api_version || 'v1beta'} onChange={(event) => patch({ api_version: event.target.value })} />
        </Field>
      )}
      {isImplement && profile.protocol === 'image2' && (
        <div className="field-row four">
          <Field label={t.settings.size}>
            <select value={defaults.size || '1200x675'} onChange={(event) => patchDefaults({ size: event.target.value })}>
              <option value="auto">auto</option>
              <option value="16:9">16:9</option>
              <option value="1024x1024">1024x1024</option>
              <option value="1200x675">1200x675</option>
              <option value="928x1664">928x1664</option>
              <option value="3000x1000">3000x1000</option>
            </select>
          </Field>
          <Field label={t.settings.quality}>
            <select value={defaults.quality || 'auto'} onChange={(event) => patchDefaults({ quality: event.target.value })}>
              <option value="auto">auto</option>
              <option value="low">low</option>
              <option value="medium">medium</option>
              <option value="high">high</option>
              <option value="hd">hd</option>
            </select>
          </Field>
          <Field label={t.settings.format}>
            <select value={defaults.output_format || 'png'} onChange={(event) => patchDefaults({ output_format: event.target.value })}>
              <option value="png">png</option>
              <option value="jpeg">jpeg</option>
              <option value="webp">webp</option>
            </select>
          </Field>
          <Field label="response">
            <select value={defaults.response_format || 'url'} onChange={(event) => patchDefaults({ response_format: event.target.value })}>
              <option value="url">url</option>
              <option value="b64_json">b64_json</option>
            </select>
          </Field>
        </div>
      )}
      {isImplement && profile.protocol === 'banana2' && (
        <div className="field-row four">
          <Field label={t.settings.ratio}>
            <select value={defaults.aspect_ratio || '16:9'} onChange={(event) => patchDefaults({ aspect_ratio: event.target.value })}>
              <option value="16:9">16:9</option>
              <option value="4:3">4:3</option>
              <option value="1:1">1:1</option>
              <option value="3:2">3:2</option>
            </select>
          </Field>
          <Field label={t.settings.clarity}>
            <select value={defaults.image_size || '4K'} onChange={(event) => patchDefaults({ image_size: event.target.value })}>
              <option value="1K">1K</option>
              <option value="2K">2K</option>
              <option value="4K">4K</option>
            </select>
          </Field>
          <Field label={t.settings.tendency}>
            <select value={defaults.thinking_level || 'high'} onChange={(event) => patchDefaults({ thinking_level: event.target.value })}>
              <option value="minimal">minimal</option>
              <option value="high">high</option>
            </select>
          </Field>
          <Field label={t.settings.format}>
            <select value={defaults.mime_type || 'image/png'} onChange={(event) => patchDefaults({ mime_type: event.target.value })}>
              <option value="image/png">image/png</option>
              <option value="image/jpeg">image/jpeg</option>
              <option value="image/webp">image/webp</option>
            </select>
          </Field>
        </div>
      )}
    </div>
  );
}

function PaperFigure({ state, onState, onJob, onMessage, t }: { state: PaperFigureState; onState: StateUpdater<PaperFigureState>; onJob: (job: JobRecord) => void; onMessage: (message: string, tone?: 'info' | 'error') => void; t: typeof copy[Lang] }) {
  const [templates, setTemplates] = useState<TemplateSummary[]>([]);
  const formBodyRef = useRef<HTMLDivElement>(null);
  const galleryRef = useRef<HTMLDivElement>(null);
  const { kind, query, selected, title, description, aspectRatio, layoutFidelity, styleStrength, custom } = state;

  function patch(values: Partial<PaperFigureState>) {
    onState((current) => ({ ...current, ...values }));
  }

  useEffect(() => {
    listTemplates(kind, query).then(setTemplates).catch(console.error);
  }, [kind, query]);

  // 模板库整体高度与左侧表单（至约束说明）底边对齐
  useEffect(() => {
    const form = formBodyRef.current;
    const gallery = galleryRef.current;
    if (!form || !gallery || typeof ResizeObserver === 'undefined') return;

    const syncHeight = () => {
      const next = Math.round(form.getBoundingClientRect().height);
      if (next > 0) gallery.style.height = `${next}px`;
    };

    syncHeight();
    const observer = new ResizeObserver(syncHeight);
    observer.observe(form);
    window.addEventListener('resize', syncHeight);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', syncHeight);
      gallery.style.height = '';
    };
  }, []);

  function toggle(id: string) {
    onState((current) => ({
      ...current,
      selected: current.selected.includes(id) ? current.selected.filter((item) => item !== id) : [...current.selected, id].slice(0, 3)
    }));
  }

  async function submit() {
    try {
      onMessage(t.paper.submitted);
      const job = await createJob({
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
      onJob(job);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : t.common.submitFailed, 'error');
    }
  }

  return (
    <section className="clay-panel page-panel">
      <Header title={t.paper.title} text={t.paper.intro} />
      <div className="figure-layout">
        <div className="form-panel workflow-form figure-form-body" ref={formBodyRef}>
          <Field label={t.paper.figureTitle}>
            <input value={title} onChange={(event) => patch({ title: event.target.value })} />
          </Field>
          <Field label={t.paper.description}>
            <textarea className="figure-method-textarea" value={description} onChange={(event) => patch({ description: event.target.value })} />
          </Field>
          <div className="field-row three">
            <Field label={t.paper.ratio}>
              <select value={aspectRatio} onChange={(event) => patch({ aspectRatio: event.target.value })}>
                <option value="inherit">{t.paper.inherited}</option>
                <option value="16:9">16:9</option>
                <option value="4:3">4:3</option>
                <option value="1:1">1:1</option>
                <option value="3:2">3:2</option>
              </select>
            </Field>
            <Field label={t.paper.fidelity}>
              <select value={layoutFidelity} onChange={(event) => patch({ layoutFidelity: event.target.value as PaperFigureState['layoutFidelity'] })}>
                <option value="strict">strict</option>
                <option value="balanced">balanced</option>
                <option value="loose">loose</option>
              </select>
            </Field>
            <Field label={t.paper.strength}>
              <select value={styleStrength} onChange={(event) => patch({ styleStrength: event.target.value as PaperFigureState['styleStrength'] })}>
                <option value="high">high</option>
                <option value="medium">medium</option>
                <option value="low">low</option>
              </select>
            </Field>
          </div>
          <Field label={t.paper.custom} hint={t.paper.customHint}>
            <textarea className="figure-custom-textarea" value={custom} onChange={(event) => patch({ custom: event.target.value })} />
          </Field>
        </div>
        <div className="gallery-panel" ref={galleryRef}>
          <div className="toolbar field-row two">
            <Field label={t.paper.kind}>
              <select value={kind} onChange={(event) => patch({ kind: event.target.value })}>
                <option value="diagram">diagram</option>
                <option value="plot">plot</option>
              </select>
            </Field>
            <Field label={t.paper.search}>
              <input placeholder={t.paper.search} value={query} onChange={(event) => patch({ query: event.target.value })} />
            </Field>
          </div>
          <div className="template-grid">
            {templates.map((template) => (
              <button key={template.id} className={`template-tile ${selected.includes(template.id) ? 'selected' : ''}`} onClick={() => toggle(template.id)}>
                <img src={template.image_url} alt="" />
                <span>{template.category || template.kind}</span>
              </button>
            ))}
          </div>
        </div>
        <div className="actions-row left figure-actions">
          <button className="primary" disabled={!title || !description || selected.length === 0} onClick={submit}>{t.paper.generate}</button>
        </div>
      </div>
    </section>
  );
}

function PptSlide({ state, onState, onJob, onMessage, t }: { state: PptSlideState; onState: StateUpdater<PptSlideState>; onJob: (job: JobRecord) => void; onMessage: (message: string, tone?: 'info' | 'error') => void; t: typeof copy[Lang] }) {
  const { asset, materials, material, pages, custom } = state;

  function patch(values: Partial<PptSlideState>) {
    onState((current) => ({ ...current, ...values }));
  }

  async function onTemplateFile(file?: File) {
    if (!file) return;
    try {
      onMessage(t.ppt.uploading);
      const uploaded = await uploadAsset(file);
      patch({ asset: uploaded });
      onMessage(t.ppt.uploaded);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : t.common.uploadFailed, 'error');
    }
  }

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
    if (!asset) return;
    try {
      onMessage(t.ppt.submitted);
      const job = await createJob({
        mode: 'ppt_slide',
        payload: {
          template_asset_id: asset.id,
          material_text: material,
          material_asset_ids: materials.map((item) => item.id),
          page_count: pages,
          custom_prompt: custom || null
        }
      });
      onJob(job);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : t.common.submitFailed, 'error');
    }
  }

  return (
    <section className="clay-panel page-panel">
      <Header title={t.ppt.title} text={t.ppt.intro} />
      <div className="slide-layout">
        <div className="slide-master-head">
          <Field label={t.ppt.template}>
            <FilePicker
              compact
              accept="image/*"
              label={t.common.choose}
              value={asset?.filename || ''}
              onChange={(files) => onTemplateFile(files?.[0])}
            />
          </Field>
        </div>
        <div className="slide-meta-head">
          <Field label={t.ppt.pages}>
            <input
              className="pages-input"
              type="number"
              min={1}
              max={20}
              value={pages}
              onChange={(event) => patch({ pages: Number(event.target.value) })}
            />
          </Field>
          <Field label={t.ppt.materialFile}>
            <FilePicker
              compact
              accept=".pdf,.doc,.docx,.txt,.md,.markdown,.csv,.tsv,.json,application/pdf,text/*"
              label={t.common.choose}
              value={materials.length ? `${t.common.uploadedFiles} ${materials.length}` : ''}
              multiple
              onChange={onMaterialFiles}
            />
          </Field>
        </div>

        <div className={`slide-preview-wrap ${asset ? 'has-asset' : ''}`}>
          {asset ? (
            <div className="template-preview-frame has-image">
              <img className="template-preview" src={asset.url} alt="" />
            </div>
          ) : (
            <div className="template-preview-frame">
              <span className="template-preview-empty">{t.common.noFile}</span>
            </div>
          )}
        </div>

        <div className="slide-material-wrap">
          {materials.length > 0 && (
            <div className="file-list">
              {materials.map((item) => <span key={item.id}>{item.filename}</span>)}
            </div>
          )}
          <Field label={t.ppt.material} hint={t.ppt.materialHint}>
            <textarea className="material-textarea" value={material} onChange={(event) => patch({ material: event.target.value })} />
          </Field>
        </div>

        <div className="slide-custom-wrap">
          <Field label={t.ppt.custom} hint={t.ppt.customHint}>
            <textarea className="custom-textarea" value={custom} onChange={(event) => patch({ custom: event.target.value })} />
          </Field>
        </div>

        <div className="actions-row right slide-actions">
          <button className="primary" disabled={!asset || (!material.trim() && materials.length === 0)} onClick={submit}>{t.ppt.generate}</button>
        </div>
      </div>
    </section>
  );
}

function JobPanel({ job, t }: { job: JobRecord; t: typeof copy[Lang] }) {
  const images = useMemo(() => job.images || [], [job.images]);
  const events = job.events || [];
  const latest = events.length > 0 ? events[events.length - 1] : null;
  const displayMessage = latest?.message?.trim() || job.message?.trim() || statusText(job.status, t);
  const displayStage = latest?.stage || job.stage || job.status;
  const progress = useMemo(() => computeJobProgress(job), [job]);
  const [stepAnimKey, setStepAnimKey] = useState(0);

  useEffect(() => {
    setStepAnimKey((key) => key + 1);
  }, [displayStage, displayMessage, latest?.timestamp]);

  return (
    <section className="clay-panel result-panel">
      <Header title={t.result.title} text={`${statusText(job.status, t)} · ${progress}%`} />
      <div className={`progress-card progress-card-live ${job.status}`}>
        <div className="progress-main-row">
          <CircularProgress value={progress} status={job.status} />
          <div className="progress-main-body">
            <div className="progress-meta-row">
              <span className="progress-label">{t.result.progress}</span>
              <strong className="progress-percent">{progress}%</strong>
              <span className={`status-pill ${job.status}`}>{statusText(job.status, t)}</span>
              <span className="progress-step-count">
                {events.length > 0 ? `${events.length}` : '0'}
              </span>
            </div>
            <div className={`progress-track ${job.status}`} role="progressbar" aria-valuenow={progress} aria-valuemin={0} aria-valuemax={100}>
              <span style={{ width: `${progress}%` }} />
            </div>
            <div className="step-log" aria-live="polite">
              <span className="step-log-label">{t.result.step}</span>
              <div key={stepAnimKey} className={`step-log-panel ${latest?.status || job.status}`}>
                <div className="step-log-dot" />
                <div className="step-log-text">
                  <strong>{displayMessage}</strong>
                  <small>{displayStage}</small>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
      {images.length > 0 && (
        <div className="result-grid">
          {images.map((image) => (
            <div key={image.url} className="result-card">
              <a className="result-preview" href={image.url} target="_blank" rel="noreferrer" aria-label={`${t.result.preview} ${image.name}`}>
                <img src={image.url} alt={image.name} />
              </a>
              <div className="result-card-footer">
                <span>{image.name}</span>
                <a className="download-button" href={image.url} download={image.name}>{t.result.download}</a>
              </div>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

function CircularProgress({ value, status }: { value: number; status: JobRecord['status'] }) {
  const size = 72;
  const stroke = 7;
  const radius = (size - stroke) / 2;
  const circumference = 2 * Math.PI * radius;
  const offset = circumference * (1 - Math.min(100, Math.max(0, value)) / 100);
  return (
    <div className={`circular-progress ${status}`} aria-hidden="true">
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`}>
        <circle className="circular-progress-bg" cx={size / 2} cy={size / 2} r={radius} strokeWidth={stroke} />
        <circle
          className="circular-progress-fg"
          cx={size / 2}
          cy={size / 2}
          r={radius}
          strokeWidth={stroke}
          strokeDasharray={circumference}
          strokeDashoffset={offset}
        />
      </svg>
      <span className="circular-progress-value">{value}%</span>
    </div>
  );
}

function statusText(status: JobRecord['status'], t: typeof copy[Lang]) {
  if (status === 'queued') return t.result.queued;
  if (status === 'running') return t.result.running;
  if (status === 'succeeded') return t.result.completed;
  return t.result.failed;
}

function computeJobProgress(job: JobRecord): number {
  if (job.status === 'succeeded') return 100;
  if (job.status === 'queued') return 4;

  const stage = (job.stage || job.events?.[job.events.length - 1]?.stage || 'started').toLowerCase();
  const pagePlan = stage.match(/^ppt_page_(?:prompt|plan)_(\d+)$/);
  if (pagePlan) {
    const page = Number.parseInt(pagePlan[1], 10) || 1;
    // 页面规划约占 56% → 72%
    return Math.min(71, 56 + Math.min(page, 12) * 1.2);
  }
  const pageImpl = stage.match(/^ppt_implement_(\d+)$/);
  if (pageImpl) {
    const page = Number.parseInt(pageImpl[1], 10) || 1;
    // 制图约占 76% → 96%
    return Math.min(96, 76 + Math.min(page, 12) * 1.5);
  }

  const weights = stage.startsWith('paper_') ? PAPER_STAGE_WEIGHTS : stage.startsWith('ppt_') || stage in PPT_STAGE_WEIGHTS ? PPT_STAGE_WEIGHTS : { ...PAPER_STAGE_WEIGHTS, ...PPT_STAGE_WEIGHTS };
  if (stage in weights) {
    const base = weights[stage];
    if (job.status === 'failed') return Math.min(96, Math.max(8, base));
    return base;
  }

  // 未知 stage：按事件数平滑推进，避免卡死观感
  const eventCount = job.events?.length || 1;
  const fallback = Math.min(92, 10 + eventCount * 4);
  return job.status === 'failed' ? Math.min(96, fallback) : fallback;
}

function Header({ title, text }: { title: string; text: string }) {
  return (
    <div className="section-header">
      <h2>{title}</h2>
      <p>{text}</p>
    </div>
  );
}

function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className={`field${hint ? ' has-hint' : ''}`}>
      <span className="field-label">{label}</span>
      <div className="field-control">{children}</div>
      {hint ? <small className="field-hint">{hint}</small> : null}
    </div>
  );
}

/** 整数输入：允许清空后完整键入，失焦时再 clamp，避免 onChange 强制最小值导致无法改数 */
function IntegerInput({
  value,
  min,
  max,
  fallback,
  onCommit
}: {
  value: number;
  min: number;
  max: number;
  fallback: number;
  onCommit: (value: number) => void;
}) {
  const [text, setText] = useState(String(value));
  const [focused, setFocused] = useState(false);

  useEffect(() => {
    if (!focused) setText(String(value));
  }, [value, focused]);

  function commit(raw: string) {
    const trimmed = raw.trim();
    if (trimmed === '' || !/^\d+$/.test(trimmed)) {
      onCommit(fallback);
      setText(String(fallback));
      return;
    }
    const parsed = Number.parseInt(trimmed, 10);
    const next = Math.min(max, Math.max(min, parsed));
    onCommit(next);
    setText(String(next));
  }

  return (
    <input
      type="text"
      inputMode="numeric"
      autoComplete="off"
      value={text}
      onFocus={() => setFocused(true)}
      onChange={(event) => {
        const raw = event.target.value;
        if (raw === '' || /^\d+$/.test(raw)) setText(raw);
      }}
      onBlur={() => {
        setFocused(false);
        commit(text);
      }}
      onKeyDown={(event) => {
        if (event.key === 'Enter') {
          event.currentTarget.blur();
        }
      }}
    />
  );
}

function FilePicker({
  accept,
  label,
  value,
  multiple,
  compact,
  onChange
}: {
  accept: string;
  label: string;
  value: string;
  multiple?: boolean;
  compact?: boolean;
  onChange: (files: FileList | null) => void;
}) {
  const id = useId();
  return (
    <div className={`file-picker${compact ? ' file-picker-compact' : ''}`}>
      <input id={id} type="file" accept={accept} multiple={multiple} onChange={(event) => onChange(event.target.files)} />
      <label htmlFor={id}>{label}</label>
      {!compact && value ? <span>{value}</span> : null}
      {compact && value ? <span className="file-picker-name" title={value}>{value}</span> : null}
    </div>
  );
}

function defaultDesign(): ModelProfile {
  return { id: 'design-default', role: 'design', name: 'Design model', protocol: 'openai_responses', base_url: 'https://api.openai.com', model: 'gpt-5.4', headers: {}, timeout_seconds: 120, max_retries: 2, output_defaults: {} };
}

function defaultImplement(): ModelProfile {
  return { id: 'implement-default', role: 'implement', name: 'Implement model', protocol: 'image2', base_url: 'https://api.openai.com', model: 'gpt-image-2', headers: {}, timeout_seconds: 600, max_retries: 3, output_defaults: { size: '1200x675', quality: 'auto', output_format: 'png', response_format: 'url', aspect_ratio: '16:9', image_size: '4K', thinking_level: 'high', mime_type: 'image/png' } };
}

function defaultSearch(): ModelProfile {
  return {
    id: 'search-default',
    role: 'search',
    name: 'Search model',
    protocol: 'duckduckgo_html',
    base_url: 'https://duckduckgo.com',
    model: 'duckduckgo-html',
    headers: {},
    timeout_seconds: 15,
    max_retries: 1,
    output_defaults: { max_results: '3' }
  };
}
