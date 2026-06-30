import { useEffect, useMemo, useState } from 'react';
import { createJob, getConfig, getJob, listTemplates, saveConfig, uploadAsset } from './api';
import type { AppConfig, AssetUpload, JobRecord, ModelProfile, TemplateSummary } from './types';

const emptyConfig: AppConfig = {
  version: 1,
  active_design_profile: 'design-default',
  active_implement_profile: 'implement-default',
  model_profiles: []
};

export function App() {
  const [mode, setMode] = useState<'paper' | 'ppt' | 'settings'>('paper');
  const [config, setConfig] = useState<AppConfig>(emptyConfig);
  const [message, setMessage] = useState('');
  const [job, setJob] = useState<JobRecord | null>(null);

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
      setMessage('保存配置中...');
      const saved = await saveConfig(next);
      setConfig(saved);
      setMessage('配置已保存');
    } catch (error) {
      setMessage(error instanceof Error ? error.message : '配置保存失败');
    }
  }

  return (
    <main className="shell">
      <aside className="sidebar clay-panel">
        <div className="brand">
          <div className="brand-mark">dp</div>
          <div>
            <h1>dreampaper</h1>
            <p>paper figure / PPT slide</p>
          </div>
        </div>
        <nav className="nav">
          <button className={mode === 'paper' ? 'active' : ''} onClick={() => setMode('paper')}>Paper figure</button>
          <button className={mode === 'ppt' ? 'active' : ''} onClick={() => setMode('ppt')}>PPT slide</button>
          <button className={mode === 'settings' ? 'active' : ''} onClick={() => setMode('settings')}>模型配置</button>
        </nav>
        <StatusPanel config={config} />
      </aside>
      <section className="workspace">
        {message && <div className="notice">{message}</div>}
        {mode === 'settings' && <Settings config={config} onChange={setConfig} onSave={persistConfig} />}
        {mode === 'paper' && <PaperFigure onJob={setJob} onMessage={setMessage} />}
        {mode === 'ppt' && <PptSlide onJob={setJob} onMessage={setMessage} />}
        {job && <JobPanel job={job} />}
      </section>
    </main>
  );
}

function StatusPanel({ config }: { config: AppConfig }) {
  const design = config.model_profiles.find((item) => item.id === config.active_design_profile);
  const implement = config.model_profiles.find((item) => item.id === config.active_implement_profile);
  return (
    <div className="status-soft">
      <span>Design</span>
      <strong>{design?.protocol || '未配置'}</strong>
      <span>Implement</span>
      <strong>{implement?.protocol || '未配置'}</strong>
    </div>
  );
}

function Settings({ config, onChange, onSave }: { config: AppConfig; onChange: (config: AppConfig) => void; onSave: (config: AppConfig) => void }) {
  const design = config.model_profiles.find((item) => item.role === 'design') || defaultDesign();
  const implement = config.model_profiles.find((item) => item.role === 'implement') || defaultImplement();

  function upsert(profile: ModelProfile) {
    const rest = config.model_profiles.filter((item) => item.id !== profile.id);
    const next = { ...config, model_profiles: [...rest, profile] };
    next.active_design_profile = profile.role === 'design' ? profile.id : next.active_design_profile;
    next.active_implement_profile = profile.role === 'implement' ? profile.id : next.active_implement_profile;
    onChange(next);
  }

  return (
    <div className="grid two">
      <ModelEditor title="Design model" profile={design} onChange={upsert} />
      <ModelEditor title="Implement model" profile={implement} onChange={upsert} />
      <div className="actions-row full">
        <button className="primary" onClick={() => onSave(config)}>保存配置</button>
      </div>
    </div>
  );
}

function ModelEditor({ title, profile, onChange }: { title: string; profile: ModelProfile; onChange: (profile: ModelProfile) => void }) {
  const isImplement = profile.role === 'implement';
  const defaults = profile.output_defaults || {};
  function patch(values: Partial<ModelProfile>) {
    onChange({ ...profile, ...values });
  }
  function patchDefaults(values: Record<string, string>) {
    patch({ output_defaults: { ...defaults, ...values } });
  }
  return (
    <section className="clay-panel form-panel">
      <h2>{title}</h2>
      <label>协议</label>
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
      <label>Base URL</label>
      <input value={profile.base_url} onChange={(event) => patch({ base_url: event.target.value })} />
      <label>Model</label>
      <input value={profile.model} onChange={(event) => patch({ model: event.target.value })} />
      <label>API key</label>
      <input type="password" placeholder={profile.api_key_hint || '保存后默认脱敏'} onChange={(event) => patch({ api_key: event.target.value })} />
      {isImplement && profile.protocol === 'banana2' && (
        <>
          <label>API version</label>
          <input value={profile.api_version || 'v1beta'} onChange={(event) => patch({ api_version: event.target.value })} />
        </>
      )}
      {isImplement && profile.protocol === 'image2' && (
        <div className="subgrid">
          <label>尺寸</label>
          <select value={defaults.size || '3840x2160'} onChange={(event) => patchDefaults({ size: event.target.value })}>
            <option value="1024x1024">1024x1024</option>
            <option value="2048x1152">2048x1152</option>
            <option value="3840x2160">3840x2160</option>
          </select>
          <label>质量</label>
          <select value={defaults.quality || 'high'} onChange={(event) => patchDefaults({ quality: event.target.value })}>
            <option value="auto">auto</option>
            <option value="low">low</option>
            <option value="medium">medium</option>
            <option value="high">high</option>
          </select>
          <label>格式</label>
          <select value={defaults.output_format || 'png'} onChange={(event) => patchDefaults({ output_format: event.target.value })}>
            <option value="png">png</option>
            <option value="jpeg">jpeg</option>
            <option value="webp">webp</option>
          </select>
        </div>
      )}
      {isImplement && profile.protocol === 'banana2' && (
        <div className="subgrid">
          <label>宽高比</label>
          <select value={defaults.aspect_ratio || '16:9'} onChange={(event) => patchDefaults({ aspect_ratio: event.target.value })}>
            <option value="16:9">16:9</option>
            <option value="4:3">4:3</option>
            <option value="1:1">1:1</option>
            <option value="3:2">3:2</option>
          </select>
          <label>清晰度</label>
          <select value={defaults.image_size || '4K'} onChange={(event) => patchDefaults({ image_size: event.target.value })}>
            <option value="1K">1K</option>
            <option value="2K">2K</option>
            <option value="4K">4K</option>
          </select>
          <label>质量倾向</label>
          <select value={defaults.thinking_level || 'high'} onChange={(event) => patchDefaults({ thinking_level: event.target.value })}>
            <option value="minimal">minimal</option>
            <option value="high">high</option>
          </select>
          <label>格式</label>
          <select value={defaults.mime_type || 'image/png'} onChange={(event) => patchDefaults({ mime_type: event.target.value })}>
            <option value="image/png">image/png</option>
            <option value="image/jpeg">image/jpeg</option>
            <option value="image/webp">image/webp</option>
          </select>
        </div>
      )}
    </section>
  );
}

function PaperFigure({ onJob, onMessage }: { onJob: (job: JobRecord) => void; onMessage: (message: string) => void }) {
  const [templates, setTemplates] = useState<TemplateSummary[]>([]);
  const [kind, setKind] = useState('diagram');
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState<string[]>([]);
  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');
  const [aspectRatio, setAspectRatio] = useState('inherit');
  const [layoutFidelity, setLayoutFidelity] = useState<'strict' | 'balanced' | 'loose'>('balanced');
  const [styleStrength, setStyleStrength] = useState<'high' | 'medium' | 'low'>('high');
  const [custom, setCustom] = useState('');

  useEffect(() => {
    listTemplates(kind, query).then(setTemplates).catch(console.error);
  }, [kind, query]);

  function toggle(id: string) {
    setSelected((current) => current.includes(id) ? current.filter((item) => item !== id) : [...current, id].slice(0, 3));
  }

  async function submit() {
    try {
      onMessage('Paper figure 任务已提交');
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
      onMessage(error instanceof Error ? error.message : '任务提交失败');
    }
  }

  return (
    <section className="clay-panel">
      <Header title="Paper figure" text="选择 PaperBananaBench template 作为 few-shot 风格参考。" />
      <div className="grid two">
        <div className="form-panel">
          <label>Figure title</label>
          <input value={title} onChange={(event) => setTitle(event.target.value)} />
          <label>章节/方法描述</label>
          <textarea value={description} onChange={(event) => setDescription(event.target.value)} />
          <div className="inline-fields">
            <div>
              <label>宽高比</label>
              <select value={aspectRatio} onChange={(event) => setAspectRatio(event.target.value)}>
                <option value="inherit">继承 template</option>
                <option value="16:9">16:9</option>
                <option value="4:3">4:3</option>
                <option value="1:1">1:1</option>
                <option value="3:2">3:2</option>
              </select>
            </div>
            <div>
              <label>布局跟随</label>
              <select value={layoutFidelity} onChange={(event) => setLayoutFidelity(event.target.value as 'strict' | 'balanced' | 'loose')}>
                <option value="strict">strict</option>
                <option value="balanced">balanced</option>
                <option value="loose">loose</option>
              </select>
            </div>
            <div>
              <label>风格强度</label>
              <select value={styleStrength} onChange={(event) => setStyleStrength(event.target.value as 'high' | 'medium' | 'low')}>
                <option value="high">high</option>
                <option value="medium">medium</option>
                <option value="low">low</option>
              </select>
            </div>
          </div>
          <label>定制约束</label>
          <textarea value={custom} onChange={(event) => setCustom(event.target.value)} />
          <button className="primary" disabled={!title || !description || selected.length === 0} onClick={submit}>生成 figure</button>
        </div>
        <div>
          <div className="toolbar">
            <select value={kind} onChange={(event) => setKind(event.target.value)}>
              <option value="diagram">diagram</option>
              <option value="plot">plot</option>
              <option value="all">all</option>
            </select>
            <input placeholder="搜索 template" value={query} onChange={(event) => setQuery(event.target.value)} />
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
      </div>
    </section>
  );
}

function PptSlide({ onJob, onMessage }: { onJob: (job: JobRecord) => void; onMessage: (message: string) => void }) {
  const [asset, setAsset] = useState<AssetUpload | null>(null);
  const [material, setMaterial] = useState('');
  const [pages, setPages] = useState(1);
  const [custom, setCustom] = useState('');

  async function onFile(file?: File) {
    if (!file) return;
    try {
      onMessage('上传 template 中...');
      const uploaded = await uploadAsset(file);
      setAsset(uploaded);
      onMessage('template 已上传');
    } catch (error) {
      onMessage(error instanceof Error ? error.message : 'template 上传失败');
    }
  }

  async function submit() {
    if (!asset) return;
    try {
      onMessage('PPT slide 任务已提交');
      const job = await createJob({
        mode: 'ppt_slide',
        payload: {
          template_asset_id: asset.id,
          material_text: material,
          page_count: pages,
          custom_prompt: custom || null
        }
      });
      onJob(job);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : '任务提交失败');
    }
  }

  return (
    <section className="clay-panel">
      <Header title="PPT slide" text="上传单页 template，系统先分析母版，再批量生成页面。" />
      <div className="grid two">
        <div className="form-panel">
          <label>单页 PPT template 图片</label>
          <input type="file" accept="image/*" onChange={(event) => onFile(event.target.files?.[0])} />
          {asset && <img className="template-preview" src={asset.url} alt="" />}
          <label>页数</label>
          <input type="number" min={1} max={20} value={pages} onChange={(event) => setPages(Number(event.target.value))} />
          <label>资料/描述</label>
          <textarea value={material} onChange={(event) => setMaterial(event.target.value)} />
          <label>定制约束</label>
          <textarea value={custom} onChange={(event) => setCustom(event.target.value)} />
          <button className="primary" disabled={!asset || !material} onClick={submit}>生成 slides</button>
        </div>
        <div className="rules">
          <h3>A/B/C 选择逻辑</h3>
          <p>A：封面、章节入口、成果总览、关键结论。</p>
          <p>B：技术正文、方法、研究路线、三模块页面。</p>
          <p>C：流程、矩阵、左右图文、中心主视觉等自适应布局。</p>
        </div>
      </div>
    </section>
  );
}

function JobPanel({ job }: { job: JobRecord }) {
  const images = useMemo(() => job.images || [], [job.images]);
  return (
    <section className="clay-panel result-panel">
      <Header title="生成结果" text={`${job.status} · ${job.message || '等待中'}`} />
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

function Header({ title, text }: { title: string; text: string }) {
  return (
    <div className="section-header">
      <h2>{title}</h2>
      <p>{text}</p>
    </div>
  );
}

function defaultDesign(): ModelProfile {
  return { id: 'design-default', role: 'design', name: 'Design model', protocol: 'openai_responses', base_url: 'https://api.openai.com', model: 'gpt-5.4', headers: {}, timeout_seconds: 120, max_retries: 2, output_defaults: {} };
}

function defaultImplement(): ModelProfile {
  return { id: 'implement-default', role: 'implement', name: 'Implement model', protocol: 'image2', base_url: 'https://api.openai.com', model: 'gpt-image-2', headers: {}, timeout_seconds: 300, max_retries: 1, output_defaults: { size: '3840x2160', quality: 'high', output_format: 'png', aspect_ratio: '16:9', image_size: '4K', thinking_level: 'high', mime_type: 'image/png' } };
}

