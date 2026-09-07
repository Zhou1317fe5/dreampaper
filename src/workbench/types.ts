// Frontend mirror of the Rust workbench contract (src-tauri/src/core/workbench).
// Field names are the serde names; geometry is integer source pixels.

export interface PixelRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type Shape = 'rect' | 'ellipse';
export type FillSource = 'auto' | 'manual';
export type OcrStatus = 'pending' | 'none' | 'found' | 'unreliable' | 'unavailable' | 'failed';
export type Align = 'left' | 'center' | 'right';
export type VAlign = 'top' | 'middle' | 'bottom';

export interface Fill {
  auto: string | null;
  current: string;
  source: FillSource;
  uneven: boolean;
  coverage: number | null;
}

export interface OcrState {
  status: OcrStatus | '';
  edited: boolean;
  stale: boolean;
}

export interface FontSpec {
  family: string;
  size: number;
  weight: number;
  italic: boolean;
}

export interface TextLayer {
  kind: 'text';
  id: string;
  visible: boolean;
  text: string;
  rect: PixelRect;
  font: FontSpec;
  color: string;
  align: Align;
  valign: VAlign;
  line_height: number;
  letter_spacing: number;
  auto_fit: boolean;
  angle: number;
  origin: 'ocr' | 'manual';
  edited: boolean;
  score: number | null;
}

export interface RepairGroup {
  kind: 'repair';
  id: string;
  visible: boolean;
  shape: Shape;
  rect: PixelRect;
  fill: Fill;
  ocr: OcrState;
  children: TextLayer[];
}

export type Layer = RepairGroup | TextLayer;

export interface SourceRef {
  asset_id: string;
  filename: string;
  width: number;
  height: number;
  orientation_normalized: boolean;
  working_color_space: string;
}

export interface Viewport {
  crop: PixelRect;
  flip_x: boolean;
  flip_y: boolean;
}

export interface ProjectDoc {
  schema_version: 1;
  id: string;
  revision: number;
  name: string;
  source: SourceRef;
  viewport: Viewport;
  layers: Layer[];
  created_at: string;
  updated_at: string;
}

export interface ProjectSummary {
  id: string;
  name: string;
  asset_id: string;
  filename: string;
  source_url: string;
  thumbnail: string;
  source_width: number;
  source_height: number;
  export_width: number;
  export_height: number;
  revision: number;
  created_at: string;
  updated_at: string;
}

export interface ExportRecord {
  id: string;
  project_id: string;
  filename: string;
  path: string;
  width: number;
  height: number;
  created_at: string;
  available: boolean;
}

export interface ProjectDetail {
  summary: ProjectSummary;
  document: ProjectDoc;
}

export interface OpenResult {
  detail: ProjectDetail;
  resumed: boolean;
}

export type ProjectSource =
  | { kind: 'asset'; asset_id: string }
  | { kind: 'file'; path: string }
  | { kind: 'bytes'; filename: string; bytes: number[] }
  | { kind: 'snapshot'; asset_id: string };

export interface ColorCandidate {
  color: string;
  coverage: number;
}

export interface RegionAnalysis {
  color: string;
  coverage: number;
  uneven: boolean;
  candidates: ColorCandidate[];
  samples: number;
}

export interface TextSpec {
  text: string;
  width: number;
  height: number;
  family: string;
  size: number;
  weight: number;
  italic: boolean;
  line_height: number;
  letter_spacing: number;
  align: Align;
  valign: VAlign;
  auto_fit: boolean;
}

export interface LineBox {
  text: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface TextLayout {
  lines: LineBox[];
  content_width: number;
  content_height: number;
  offset_y: number;
  font_size: number;
  overflow: boolean;
  family_used: string;
  missing_font: boolean;
  /** Horizontal fake-bold width in source px when the face has no bold cut; 0 otherwise. */
  synthetic_bold: number;
}

export interface FontInfo {
  family: string;
  covers: boolean | null;
  bundled: boolean;
}

export interface ExportPreview {
  width: number;
  height: number;
  missing_fonts: string[];
  color_note: string | null;
  had_alpha: boolean;
}

export interface DeleteResult {
  asset_unreferenced: boolean;
}

export interface StorageStats {
  project_count: number;
  project_bytes: number;
  asset_count: number;
  asset_bytes: number;
  unreferenced_count: number;
  unreferenced_bytes: number;
}

export interface CleanupResult {
  removed: number;
  freed_bytes: number;
}

export interface OcrProgress {
  file: string;
  received: number;
  total: number;
  overall_received: number;
  overall_total: number;
  state: 'downloading' | 'verifying' | 'done' | 'failed' | 'cancelled';
  message: string | null;
}

/** The bundled native engine: Rust sidecar + ONNX Runtime, shipped with the app. */
export interface OcrEngineStatus {
  available: boolean;
  version: string;
  runtime_version: string;
  program: string | null;
  runtime: string | null;
  problem: string | null;
  running: boolean;
  tripped: boolean;
}

export interface OcrPackageStatus {
  installed: boolean;
  version: string;
  download_bytes: number;
  installed_bytes: number;
  directory: string;
  downloading: boolean;
  progress: OcrProgress | null;
  error: string | null;
  det_model_name: string;
  cls_model_name: string;
  rec_model_name: string;
  files: Array<{ name: string; bytes: number }>;
  license: string;
  sources: string[];
  engine: OcrEngineStatus;
}

/** One recognised line as the sidecar reports it, in region-crop pixels. */
export interface OcrItem {
  poly: Array<{ x: number; y: number }>;
  text: string;
  score: number;
  box_score?: number;
  angle?: number;
}

export interface RecognizeResult {
  items: OcrItem[];
  elapsed_ms: number;
  padded_width: number;
  padded_height: number;
}
