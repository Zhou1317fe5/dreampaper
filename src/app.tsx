import { useEffect, useId, useMemo, useState } from 'react';
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
  proxy_url: 'http://127.0.0.1:7890',
  ppt_page_plan_concurrency: null,
  ppt_image_concurrency: null,
  model_profiles: []
};

const copy = {
  zh: {
    nav: { paper: 'Figure', ppt: 'Slide', settings: 'Model' },
    brandSub: 'figure / slide',
    status: { design: 'Design', implement: 'Implement', unset: '未配置' },
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
      config: '配置',
      choose: '选择',
      noFile: '未选择',
      uploadedFiles: '已上传'
    },
    settings: { design: 'Design', implement: 'Implement', proxy: '代理', proxyHint: '本地代理地址，留空表示不指定代理。', concurrency: 'Slide 并发', concurrencyHint: '留空表示跟随本次输入的 Slide 页数；实际并发不会超过页数。', pagePlanConcurrency: '规划并发', imageConcurrency: '制图并发', defaultByPages: '默认=页数', size: '尺寸', quality: '质量', format: '格式', ratio: '比例', clarity: '清晰度', tendency: '倾向', version: '版本' },
    paper: { title: 'Figure', intro: '选择 template 作为 few-shot 风格参考。', figureTitle: '标题', description: '方法', ratio: '比例', fidelity: '布局', strength: '风格', custom: '约束', customHint: '可选，用于补充禁用元素、强调风格、文字限制或审稿要求。', generate: '生成', search: '搜索', inherited: '继承', submitted: 'Figure 任务已提交' },
    ppt: { title: 'Slide', intro: '上传 template，分析母版，再批量生成页面。', template: 'template', pages: '页数', material: '资料', materialFile: '文件', materialHint: '可输入文字，也可上传 pdf、docx、txt、md、csv 等资料。', custom: '约束', customHint: '可选，用于补充页数结构、禁用元素、术语、颜色或展示重点。', generate: '生成', submitted: 'Slide 任务已提交', uploading: '上传中...', uploaded: '已上传', rulesTitle: 'A/B/C', rulesIntro: '系统自动选择页面骨架，不需要手动选择。', ruleA: 'A：封面、章节、成果、结论。', ruleB: 'B：正文、方法、路线、三模块。', ruleC: 'C：流程、矩阵、图文、自适应。' },
    result: { title: 'Result', waiting: '等待中', progress: '进度', current: '当前', failed: '失败', completed: '完成', queued: '排队中', running: '运行中' }
  },
  en: {
    nav: { paper: 'Figure', ppt: 'Slide', settings: 'Model' },
    brandSub: 'figure / slide',
    status: { design: 'Design', implement: 'Implement', unset: 'Unset' },
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
      config: 'Config',
      choose: 'Choose',
      noFile: 'No file',
      uploadedFiles: 'Uploaded'
    },
    settings: { design: 'Design', implement: 'Implement', proxy: 'Proxy', proxyHint: 'Local proxy URL. Leave empty to disable explicit proxy.', concurrency: 'Slide concurrency', concurrencyHint: 'Leave empty to follow the current Slide page count; actual concurrency will not exceed pages.', pagePlanConcurrency: 'Plan workers', imageConcurrency: 'Image workers', defaultByPages: 'default=pages', size: 'Size', quality: 'Quality', format: 'Format', ratio: 'Ratio', clarity: 'Sharpness', tendency: 'Quality', version: 'Version' },
    paper: { title: 'Figure', intro: 'Choose templates as few-shot visual references.', figureTitle: 'Title', description: 'Method', ratio: 'Ratio', fidelity: 'Layout', strength: 'Style', custom: 'Rules', customHint: 'Optional constraints for banned elements, style emphasis, text limits, or review requirements.', generate: 'Generate', search: 'Search', inherited: 'Template', submitted: 'Figure job submitted' },
    ppt: { title: 'Slide', intro: 'Upload a template, analyze the master, then generate pages.', template: 'template', pages: 'Pages', material: 'Material', materialFile: 'File', materialHint: 'Enter text or upload pdf, docx, txt, md, csv, and other common materials.', custom: 'Rules', customHint: 'Optional constraints for page structure, banned elements, terms, colors, or focus.', generate: 'Generate', submitted: 'Slide job submitted', uploading: 'Uploading...', uploaded: 'Uploaded', rulesTitle: 'A/B/C', rulesIntro: 'Internal page skeleton rules; no manual selection required.', ruleA: 'A: cover, section, results, conclusion.', ruleB: 'B: body, method, route, three modules.', ruleC: 'C: flow, matrix, image-text, adaptive.' },
    result: { title: 'Result', waiting: 'Waiting', progress: 'Progress', current: 'Current', failed: 'Failed', completed: 'Completed', queued: 'Queued', running: 'Running' }
  }
} as const;

export function App() {
  const [mode, setMode] = useState<UiMode>('paper');
  const [lang, setLang] = useState<Lang>('zh');
  const [config, setConfig] = useState<AppConfig>(emptyConfig);
  const [message, setMessage] = useState('');
  const [job, setJob] = useState<JobRecord | null>(null);
  const [paperState, setPaperState] = useState<PaperFigureState>(defaultPaperState);
  const [pptState, setPptState] = useState<PptSlideState>(defaultPptState);
  const t = copy[lang];

  useEffect(() => {
    getConfig().then(setConfig).catch((error) => setMessage(error.message));
  }, []);

  useEffect(() => {
    if (!job || job.status === 'succeeded' || job.status === 'failed') return;
    const timer = window.setInterval(() => {
      getJob(job.id).then(setJob).catch((error) => setMessage(error.message));
    }, 1800);
    return () => window.clearInterval(timer);
  }, [job]);

  async function persistConfig(next: AppConfig) {
    try {
      setMessage(t.common.saving);
      const saved = await saveConfig(next);
      setConfig(saved);
      setMessage(t.common.saved);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : 'Save failed');
    }
  }

  return (
    <main className="shell">
      <aside className="sidebar clay-panel">
        <div className="brand">
          <div className="brand-mark"><img src="/icon.png" alt="" /></div>
          <div>
            <h1>dreampaper</h1>
            <p>{t.brandSub}</p>
          </div>
        </div>
        <nav className="nav" aria-label="Primary">
          <button className={mode === 'paper' ? 'active' : ''} onClick={() => setMode('paper')}>{t.nav.paper}</button>
          <button className={mode === 'ppt' ? 'active' : ''} onClick={() => setMode('ppt')}>{t.nav.ppt}</button>
          <button className={mode === 'settings' ? 'active' : ''} onClick={() => setMode('settings')}>{t.nav.settings}</button>
        </nav>
        <div className="sidebar-spacer" />
        <button className="lang-toggle" onClick={() => setLang(lang === 'zh' ? 'en' : 'zh')}>{lang === 'zh' ? 'EN' : '中'}</button>
        <StatusPanel config={config} t={t} />
      </aside>
      <section className="workspace">
        {message && <div className="notice">{message}</div>}
        <div hidden={mode !== 'settings'}>
          <Settings config={config} onChange={setConfig} onSave={persistConfig} t={t} />
        </div>
        <div hidden={mode !== 'paper'}>
          <PaperFigure state={paperState} onState={setPaperState} onJob={setJob} onMessage={setMessage} t={t} />
        </div>
        <div hidden={mode !== 'ppt'}>
          <PptSlide state={pptState} onState={setPptState} onJob={setJob} onMessage={setMessage} t={t} />
        </div>
        {job && <JobPanel job={job} t={t} />}
      </section>
    </main>
  );
}

function StatusPanel({ config, t }: { config: AppConfig; t: typeof copy[Lang] }) {
  const design = config.model_profiles.find((item) => item.id === config.active_design_profile);
  const implement = config.model_profiles.find((item) => item.id === config.active_implement_profile);
  return (
    <div className="status-soft">
      <span>{t.status.design}</span>
      <strong>{design?.protocol || t.status.unset}</strong>
      <span>{t.status.implement}</span>
      <strong>{implement?.protocol || t.status.unset}</strong>
    </div>
  );
}

function Settings({ config, onChange, onSave, t }: { config: AppConfig; onChange: (config: AppConfig) => void; onSave: (config: AppConfig) => void; t: typeof copy[Lang] }) {
  const design = config.model_profiles.find((item) => item.role === 'design') || defaultDesign();
  const implement = config.model_profiles.find((item) => item.role === 'implement') || defaultImplement();

  function upsert(profile: ModelProfile) {
    const rest = config.model_profiles.filter((item) => item.id !== profile.id);
    const next = { ...config, model_profiles: [...rest, profile] };
    next.active_design_profile = profile.role === 'design' ? profile.id : next.active_design_profile;
    next.active_implement_profile = profile.role === 'implement' ? profile.id : next.active_implement_profile;
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

  return (
    <div className="settings-layout">
      <ModelEditor title={t.settings.design} profile={design} onChange={upsert} t={t} />
      <ModelEditor title={t.settings.implement} profile={implement} onChange={upsert} t={t} />
      <section className="clay-panel form-panel proxy-panel">
        <h2>{t.settings.proxy}</h2>
        <Field label={t.settings.proxy} hint={t.settings.proxyHint}>
          <input value={config.proxy_url ?? ''} placeholder="http://127.0.0.1:7890" onChange={(event) => patchConfig({ proxy_url: event.target.value })} />
        </Field>
      </section>
      <section className="clay-panel form-panel proxy-panel">
        <h2>{t.settings.concurrency}</h2>
        <div className="field-row two">
          <Field label={t.settings.pagePlanConcurrency} hint={t.settings.concurrencyHint}>
            <input
              type="number"
              min="1"
              max="20"
              placeholder={t.settings.defaultByPages}
              value={config.ppt_page_plan_concurrency ?? ''}
              onChange={(event) => patchConcurrency('ppt_page_plan_concurrency', event.target.value)}
            />
          </Field>
          <Field label={t.settings.imageConcurrency} hint={t.settings.concurrencyHint}>
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
      </section>
      <div className="actions-row">
        <button className="primary" onClick={() => onSave(config)}>{t.common.save}</button>
      </div>
    </div>
  );
}

function ModelEditor({ title, profile, onChange, t }: { title: string; profile: ModelProfile; onChange: (profile: ModelProfile) => void; t: typeof copy[Lang] }) {
  const isImplement = profile.role === 'implement';
  const defaults = profile.output_defaults || {};
  function patch(values: Partial<ModelProfile>) {
    onChange({ ...profile, ...values });
  }
  function patchDefaults(values: Record<string, string>) {
    patch({ output_defaults: { ...defaults, ...values } });
  }
  return (
    <section className="clay-panel form-panel model-panel">
      <h2>{title}</h2>
      <Field label={t.common.protocol}>
        <select value={profile.protocol} onChange={(event) => patch({ protocol: event.target.value })}>
          {profile.role === 'design' ? (
            <>
              <option value="openai_responses">OpenAI Responses</option>
              <option value="openai_chat">OpenAI Chat</option>
              <option value="anthropic_messages">Anthropic Messages</option>
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
        <input value={profile.base_url} onChange={(event) => patch({ base_url: event.target.value })} />
      </Field>
      <Field label={t.common.model}>
        <input value={profile.model} onChange={(event) => patch({ model: event.target.value })} />
      </Field>
      <Field label={t.common.apiKey}>
        <input type="password" placeholder={profile.api_key_hint || t.common.redacted} onChange={(event) => patch({ api_key: event.target.value })} />
      </Field>
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
            <select value={defaults.response_format || 'b64_json'} onChange={(event) => patchDefaults({ response_format: event.target.value })}>
              <option value="b64_json">b64_json</option>
              <option value="url">url</option>
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
    </section>
  );
}

function PaperFigure({ state, onState, onJob, onMessage, t }: { state: PaperFigureState; onState: StateUpdater<PaperFigureState>; onJob: (job: JobRecord) => void; onMessage: (message: string) => void; t: typeof copy[Lang] }) {
  const [templates, setTemplates] = useState<TemplateSummary[]>([]);
  const { kind, query, selected, title, description, aspectRatio, layoutFidelity, styleStrength, custom } = state;

  function patch(values: Partial<PaperFigureState>) {
    onState((current) => ({ ...current, ...values }));
  }

  useEffect(() => {
    listTemplates(kind, query).then(setTemplates).catch(console.error);
  }, [kind, query]);

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
      onMessage(error instanceof Error ? error.message : t.common.submitFailed);
    }
  }

  return (
    <section className="clay-panel page-panel">
      <Header title={t.paper.title} text={t.paper.intro} />
      <div className="figure-layout">
        <div className="form-panel workflow-form">
          <Field label={t.paper.figureTitle}>
            <input value={title} onChange={(event) => patch({ title: event.target.value })} />
          </Field>
          <Field label={t.paper.description}>
            <textarea value={description} onChange={(event) => patch({ description: event.target.value })} />
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
            <textarea value={custom} onChange={(event) => patch({ custom: event.target.value })} />
          </Field>
          <button className="primary" disabled={!title || !description || selected.length === 0} onClick={submit}>{t.paper.generate}</button>
        </div>
        <div className="gallery-panel">
          <Field label={t.common.config}>
            <div className="toolbar">
              <select value={kind} onChange={(event) => patch({ kind: event.target.value })}>
                <option value="diagram">diagram</option>
                <option value="plot">plot</option>
                <option value="all">all</option>
              </select>
              <input placeholder={t.paper.search} value={query} onChange={(event) => patch({ query: event.target.value })} />
            </div>
          </Field>
          <div className="template-grid">
            {templates.map((template) => (
              <button key={template.id} className={`template-tile ${selected.includes(template.id) ? 'selected' : ''}`} onClick={() => toggle(template.id)}>
                <img src={template.image_url} alt="" />
                <span>{template.category || template.kind}</span>
              </button>
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}

function PptSlide({ state, onState, onJob, onMessage, t }: { state: PptSlideState; onState: StateUpdater<PptSlideState>; onJob: (job: JobRecord) => void; onMessage: (message: string) => void; t: typeof copy[Lang] }) {
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
      onMessage(error instanceof Error ? error.message : t.common.uploadFailed);
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
      onMessage(error instanceof Error ? error.message : t.common.uploadFailed);
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
      onMessage(error instanceof Error ? error.message : t.common.submitFailed);
    }
  }

  return (
    <section className="clay-panel page-panel">
      <Header title={t.ppt.title} text={t.ppt.intro} />
      <div className="slide-layout">
        <div className="form-panel workflow-form">
          <Field label={t.ppt.template}>
            <FilePicker accept="image/*" label={t.common.choose} value={asset?.filename || t.common.noFile} onChange={(files) => onTemplateFile(files?.[0])} />
          </Field>
          {asset && <img className="template-preview" src={asset.url} alt="" />}
          <Field label={t.ppt.pages}>
            <input type="number" min={1} max={20} value={pages} onChange={(event) => patch({ pages: Number(event.target.value) })} />
          </Field>
          <Field label={t.ppt.material} hint={t.ppt.materialHint}>
            <textarea value={material} onChange={(event) => patch({ material: event.target.value })} />
          </Field>
          <Field label={t.ppt.materialFile}>
            <FilePicker accept=".pdf,.doc,.docx,.txt,.md,.markdown,.csv,.tsv,.json,application/pdf,text/*" label={t.common.choose} value={materials.length ? `${t.common.uploadedFiles} ${materials.length}` : t.common.noFile} multiple onChange={onMaterialFiles} />
          </Field>
          {materials.length > 0 && (
            <div className="file-list">
              {materials.map((item) => <span key={item.id}>{item.filename}</span>)}
            </div>
          )}
          <Field label={t.ppt.custom} hint={t.ppt.customHint}>
            <textarea value={custom} onChange={(event) => patch({ custom: event.target.value })} />
          </Field>
          <button className="primary" disabled={!asset || (!material.trim() && materials.length === 0)} onClick={submit}>{t.ppt.generate}</button>
        </div>
        <Field label={t.common.config}>
          <div className="rules">
            <h3>{t.ppt.rulesTitle}</h3>
            <p>{t.ppt.rulesIntro}</p>
            <p>{t.ppt.ruleA}</p>
            <p>{t.ppt.ruleB}</p>
            <p>{t.ppt.ruleC}</p>
          </div>
        </Field>
      </div>
    </section>
  );
}

function JobPanel({ job, t }: { job: JobRecord; t: typeof copy[Lang] }) {
  const images = useMemo(() => job.images || [], [job.images]);
  const events = job.events || [];
  const displayMessage = job.message?.trim() || statusText(job.status, t);
  const progress = progressValue(job.status, events.length);
  return (
    <section className="clay-panel result-panel">
      <Header title={t.result.title} text={`${statusText(job.status, t)} · ${displayMessage}`} />
      <div className="progress-card">
        <div className="progress-head">
          <span>{t.result.current}</span>
          <strong>{displayMessage}</strong>
        </div>
        <div className={`progress-track ${job.status}`}>
          <span style={{ width: `${progress}%` }} />
        </div>
        <div className="event-list">
          {events.map((event) => (
            <div key={`${event.timestamp}-${event.stage}`} className={`event-item ${event.status}`}>
              <span />
              <div>
                <strong>{event.message}</strong>
                <small>{event.stage}</small>
              </div>
            </div>
          ))}
        </div>
      </div>
      <div className="result-grid">
        {images.map((image) => (
          <a key={image.url} href={image.url} target="_blank" rel="noreferrer">
            <img src={image.url} alt={image.name} />
            <span>{image.name}</span>
          </a>
        ))}
      </div>
    </section>
  );
}

function statusText(status: JobRecord['status'], t: typeof copy[Lang]) {
  if (status === 'queued') return t.result.queued;
  if (status === 'running') return t.result.running;
  if (status === 'succeeded') return t.result.completed;
  return t.result.failed;
}

function progressValue(status: JobRecord['status'], eventCount: number) {
  if (status === 'succeeded') return 100;
  if (status === 'failed') return Math.max(12, Math.min(96, eventCount * 10));
  if (status === 'queued') return 8;
  return Math.max(16, Math.min(92, eventCount * 10));
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
    <div className="field">
      <span>{label}</span>
      {children}
      {hint && <small>{hint}</small>}
    </div>
  );
}

function FilePicker({ accept, label, value, multiple, onChange }: { accept: string; label: string; value: string; multiple?: boolean; onChange: (files: FileList | null) => void }) {
  const id = useId();
  return (
    <div className="file-picker">
      <input id={id} type="file" accept={accept} multiple={multiple} onChange={(event) => onChange(event.target.files)} />
      <label htmlFor={id}>{label}</label>
      <span>{value}</span>
    </div>
  );
}

function defaultDesign(): ModelProfile {
  return { id: 'design-default', role: 'design', name: 'Design model', protocol: 'openai_responses', base_url: 'https://api.openai.com', model: 'gpt-5.4', headers: {}, timeout_seconds: 120, max_retries: 2, output_defaults: {} };
}

function defaultImplement(): ModelProfile {
  return { id: 'implement-default', role: 'implement', name: 'Implement model', protocol: 'image2', base_url: 'https://api.openai.com', model: 'gpt-image-2', headers: {}, timeout_seconds: 300, max_retries: 1, output_defaults: { size: '1200x675', quality: 'auto', output_format: 'png', response_format: 'b64_json', aspect_ratio: '16:9', image_size: '4K', thinking_level: 'high', mime_type: 'image/png' } };
}
