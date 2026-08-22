import type {
  AppConfig,
  AssetUpload,
  JobRecord,
  TemplatePackSummary,
  TemplateSummary
} from './types';

const isDesktop =
  typeof window !== 'undefined' &&
  (Boolean((window as { isTauri?: boolean }).isTauri) || '__TAURI_INTERNALS__' in window);

type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

let invokeFn: Promise<InvokeFn> | null = null;

async function ipc<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  invokeFn ??= import('@tauri-apps/api/core').then((mod) => mod.invoke as InvokeFn);
  const invoke = await invokeFn;
  try {
    return await invoke<T>(command, args);
  } catch (error) {
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

export function desktopAvailable() {
  return isDesktop;
}

export function listJobs(limit = 20, offset = 0) {
  return ipc<JobRecord[]>('list_jobs', { limit, offset });
}

export function cancelJob(id: string) {
  return ipc<JobRecord>('cancel_job', { id });
}

export function deleteJob(id: string) {
  return ipc<null>('delete_job', { id });
}

export function deleteTemplates(ids: string[]) {
  return ipc<number>('delete_templates', { ids });
}

export async function saveAsset(assetId: string, defaultFilename: string): Promise<void> {
  const { save } = await import('@tauri-apps/plugin-dialog');
  const path = await save({ defaultPath: defaultFilename });
  if (!path) return;
  return ipc('save_asset', { assetId, path });
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

export async function pickDirectory(title: string): Promise<string | null> {
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected = await open({ directory: true, multiple: false, title });
  return typeof selected === 'string' ? selected : null;
}

/**
 * Open a URL in the user's default browser.
 *
 * Inside the Tauri webview a plain `<a target="_blank">` is a silent no-op:
 * there is no window.open handler, so the click produces no navigation and no
 * error. The URL has to be handed to the OS instead. Scoped in
 * capabilities/default.json to https://huggingface.co/* — a URL outside that
 * scope is rejected by the ACL.
 */
export async function openExternal(url: string): Promise<void> {
  if (!isDesktop) {
    window.open(url, '_blank', 'noreferrer');
    return;
  }
  return ipc('plugin:opener|open_url', { url });
}

/**
 * Open a generated image with the OS default viewer.
 *
 * Same root cause as openExternal: the `<a target="_blank">` preview link does
 * nothing inside the webview. The backend resolves the id against the asset
 * table, so the webview never handles a filesystem path.
 */
export async function openArtifact(assetId: string): Promise<void> {
  return ipc('open_artifact', { artifactId: assetId });
}

