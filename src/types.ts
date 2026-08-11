export type ModelRole = 'design' | 'implement' | 'search';
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
  active_search_profile?: string;
  proxy_url?: string | null;
  ppt_page_plan_concurrency?: number | null;
  ppt_image_concurrency?: number | null;
  model_profiles: ModelProfile[];
}

export interface TemplateSummary {
  id: string;
  source_id: string;
  kind: 'diagram' | 'plot' | 'master';
  category?: string | null;
  rounded_ratio?: string | null;
  visual_intent: string;
  content_summary: string;
  image_url: string;
}

export interface TemplatePackSummary {
  id: string;
  name: string;
  version?: string | null;
  template_count: number;
}

export interface AssetUpload {
  id: string;
  filename: string;
  mime_type: string;
  url: string;
}

export interface JobEvent {
  stage: string;
  message: string;
  status: 'pending' | 'running' | 'succeeded' | 'failed' | 'cancelled';
  timestamp: string;
}

export interface JobRecord {
  id: string;
  mode: Mode;
  status: 'queued' | 'running' | 'succeeded' | 'failed' | 'cancelled';
  message?: string | null;
  stage?: string;
  created_at: string;
  updated_at: string;
  images: Array<{ name: string; url: string }>;
  events?: JobEvent[];
}

