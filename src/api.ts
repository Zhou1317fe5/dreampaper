import type { AppConfig, AssetUpload, JobRecord, TemplateSummary } from './types';

async function request<T>(url: string, init?: RequestInit): Promise<T> {
  const response = await fetch(url, init);
  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `HTTP ${response.status}`);
  }
  return response.json() as Promise<T>;
}

export function getConfig() {
  return request<AppConfig>('/api/config/models');
}

export function saveConfig(config: AppConfig) {
  return request<AppConfig>('/api/config/models', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(config)
  });
}

export function listTemplates(kind: string, q: string) {
  const params = new URLSearchParams();
  if (kind) params.set('kind', kind);
  if (q) params.set('q', q);
  params.set('limit', '60');
  return request<TemplateSummary[]>(`/api/templates?${params.toString()}`);
}

export async function uploadAsset(file: File) {
  const data = new FormData();
  data.append('file', file);
  return request<AssetUpload>('/api/assets', { method: 'POST', body: data });
}

export function createJob(body: unknown) {
  return request<JobRecord>('/api/jobs', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body)
  });
}

export function getJob(id: string) {
  return request<JobRecord>(`/api/jobs/${id}`);
}

