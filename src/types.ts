export type ModelRole = 'design' | 'implement';
export type Mode = 'paper_figure' | 'ppt_slide';

export interface ModelProfile {
  id: string;
  role: ModelRole;
  name: string;
  protocol: string;
  base_url: string;
  model: string;
  api_key?: string | null;
  api_version?: string | null;
  headers: Record<string, string>;
  timeout_seconds: number;
  max_retries: number;
  output_defaults: Record<string, string>;
  has_api_key?: boolean;
  api_key_hint?: string | null;
}

export interface AppConfig {
  version: number;
  active_design_profile: string;
  active_implement_profile: string;
  model_profiles: ModelProfile[];
}

export interface TemplateSummary {
  id: string;
  source_id: string;
  kind: 'diagram' | 'plot';
  category?: string | null;
  rounded_ratio?: string | null;
  visual_intent: string;
  content_summary: string;
  image_url: string;
}

export interface AssetUpload {
  id: string;
  filename: string;
  mime_type: string;
  url: string;
}

export interface JobRecord {
  id: string;
  mode: Mode;
  status: 'queued' | 'running' | 'succeeded' | 'failed';
  message?: string | null;
  created_at: string;
  updated_at: string;
  images: Array<{ name: string; url: string }>;
}

