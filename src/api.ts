import type {
  AppConfig,
  AssetUpload,
  JobRecord,
  TemplatePackSummary,
  TemplateSummary
} from './types';

/**
 * 桌面版走 Tauri IPC，网页版走 HTTP。
 *
 * 两个外壳共用这一份 api，是为了让 PaperFigure / PptSlide / Settings
 * 这些表单组件原样复用——否则桌面版要把整套表单连同校验再写一遍，
 * 两边的字段迟早会各改各的。外壳（布局）不同，数据层相同。
 */
const isDesktop =
  typeof window !== 'undefined' &&
  (Boolean((window as { isTauri?: boolean }).isTauri) || '__TAURI_INTERNALS__' in window);

type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

// 动态 import：网页版的产物里不会打进 @tauri-apps/api
let invokeFn: Promise<InvokeFn> | null = null;

async function ipc<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  invokeFn ??= import('@tauri-apps/api/core').then((mod) => mod.invoke as InvokeFn);
  const invoke = await invokeFn;
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    // Rust 侧抛的是 AppError 结构体，不转换的话 UI 上会显示 [object Object]
    throw new Error(errorText(error));
  }
}

function errorText(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error && typeof error === 'object' && 'message' in error) {
    return String((error as { message: unknown }).message);
  }
  return 'Unexpected error';
}

async function request<T>(url: string, init?: RequestInit): Promise<T> {
  const response = await fetch(url, init);
  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `HTTP ${response.status}`);
  }
  return response.json() as Promise<T>;
}

/**
 * Tauri 的 `Vec<u8>` 参数经 JSON 传输，会变成数字数组（约 4 倍膨胀）。
 * 模板图与母版图通常在几 MB 以内，可以接受；真正的大件（模板包）
 * 走的是目录路径而不是字节流，不经过这里。
 */
async function fileBytes(file: File): Promise<number[]> {
  return Array.from(new Uint8Array(await file.arrayBuffer()));
}

export function getConfig() {
  return isDesktop ? ipc<AppConfig>('get_config') : request<AppConfig>('/api/config/models');
}

export function saveConfig(config: AppConfig) {
  if (isDesktop) return ipc<AppConfig>('save_config', { config });
  return request<AppConfig>('/api/config/models', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(config)
  });
}

export function listTemplates(kind: string, q: string) {
  if (isDesktop) {
    return ipc<TemplateSummary[]>('list_templates', { kind: kind || 'all', query: q });
  }
  const params = new URLSearchParams();
  if (kind) params.set('kind', kind);
  if (q) params.set('q', q);
  params.set('limit', '60');
  return request<TemplateSummary[]>(`/api/templates?${params.toString()}`);
}

export async function uploadAsset(file: File) {
  if (isDesktop) {
    return ipc<AssetUpload>('import_asset', {
      filename: file.name,
      mimeType: file.type,
      bytes: await fileBytes(file)
    });
  }
  const data = new FormData();
  data.append('file', file);
  return request<AssetUpload>('/api/assets', { method: 'POST', body: data });
}

export function createJob(body: unknown) {
  if (isDesktop) return ipc<JobRecord>('create_job', { payload: body });
  return request<JobRecord>('/api/jobs', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body)
  });
}

export function getJob(id: string) {
  if (isDesktop) return ipc<JobRecord>('get_job', { id });
  return request<JobRecord>(`/api/jobs/${id}`);
}

// —— 以下仅桌面版可用：网页版的模板库由后端从 PaperBananaBench 目录直接读取 ——

export function desktopAvailable() {
  return isDesktop;
}

export function listJobs(limit = 20, offset = 0) {
  return ipc<JobRecord[]>('list_jobs', { limit, offset });
}

export async function importTemplateImage(
  file: File,
  kind: string,
  category: string,
  visualIntent: string,
  contentSummary: string
) {
  return ipc<TemplateSummary>('import_template_image', {
    filename: file.name,
    mimeType: file.type,
    bytes: await fileBytes(file),
    kind,
    category: category || null,
    visualIntent: visualIntent || null,
    contentSummary: contentSummary || null
  });
}

export function importTemplatePack(path: string) {
  return ipc<TemplatePackSummary>('import_template_pack', { path });
}

/** 打开原生目录选择器，取消时返回 null。 */
export async function pickDirectory(title: string): Promise<string | null> {
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected = await open({ directory: true, multiple: false, title });
  return typeof selected === 'string' ? selected : null;
}
