use chrono::Utc;
use tauri::{AppHandle, Emitter, State};

use crate::core::asset::AssetUpload;
use crate::core::config::AppConfig;
use crate::core::doc::{DocumentChunkHit, DocumentSummary};
use crate::core::job::JobRecord;
use crate::core::tpl::{TemplatePackSummary, TemplateSummary};
use crate::error::{AppError, AppResult};
use crate::event::JobEventPayload;
use crate::state::AppState;

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> AppResult<AppConfig> {
    state.core().get_config()
}

#[tauri::command]
pub fn save_config(state: State<'_, AppState>, config: AppConfig) -> AppResult<AppConfig> {
    state.core().save_config(config)
}

#[tauri::command]
pub fn list_templates(
    state: State<'_, AppState>,
    kind: String,
    query: String,
) -> AppResult<Vec<TemplateSummary>> {
    state.core().list_templates(kind, query)
}

#[tauri::command]
pub fn import_asset(
    state: State<'_, AppState>,
    filename: String,
    mime_type: String,
    bytes: Vec<u8>,
) -> AppResult<AssetUpload> {
    state.core().import_asset(filename, mime_type, bytes)
}

#[tauri::command]
pub fn import_template_image(
    state: State<'_, AppState>,
    filename: String,
    mime_type: String,
    bytes: Vec<u8>,
    kind: String,
    category: Option<String>,
    visual_intent: Option<String>,
    content_summary: Option<String>,
) -> AppResult<TemplateSummary> {
    state.core().import_template_image(
        filename,
        mime_type,
        bytes,
        kind,
        category,
        visual_intent,
        content_summary,
    )
}

#[tauri::command]
pub fn import_template_pack(
    state: State<'_, AppState>,
    path: String,
) -> AppResult<TemplatePackSummary> {
    state.core().import_template_pack(path)
}

#[tauri::command]
pub fn import_document(state: State<'_, AppState>, path: String) -> AppResult<DocumentSummary> {
    state.core().import_document(path)
}

#[tauri::command]
pub fn import_document_asset(
    state: State<'_, AppState>,
    asset_id: String,
) -> AppResult<DocumentSummary> {
    state.core().import_document_asset(asset_id)
}

#[tauri::command]
pub fn search_documents(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> AppResult<Vec<DocumentChunkHit>> {
    state.core().search_documents(query, limit.unwrap_or(8))
}

#[tauri::command]
pub fn create_job(
    app: AppHandle,
    state: State<'_, AppState>,
    payload: serde_json::Value,
) -> AppResult<JobRecord> {
    let record = state.core().create_job(payload)?;
    let _ = app.emit(
        "job://queued",
        JobEventPayload {
            job_id: record.id.clone(),
            status: record.status.clone(),
            stage: record.stage.clone().unwrap_or_else(|| "queued".to_string()),
            message: record
                .message
                .clone()
                .unwrap_or_else(|| "任务已排队".to_string()),
            timestamp: Utc::now(),
        },
    );
    crate::core::pipeline::execute::spawn(app, state.core_arc(), record.id.clone());
    Ok(record)
}

#[tauri::command]
pub fn get_job(state: State<'_, AppState>, id: String) -> AppResult<JobRecord> {
    state.core().get_job(id)
}

#[tauri::command]
pub fn list_jobs(
    state: State<'_, AppState>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> AppResult<Vec<JobRecord>> {
    state
        .core()
        .list_jobs(limit.unwrap_or(50), offset.unwrap_or(0))
}

/// Open a generated image with the OS default viewer.
///
/// The webview cannot do this on its own: inside Tauri a plain
/// `<a target="_blank">` is a silent no-op (no window.open handler, so no
/// navigation and no error), so the result preview has to round-trip through
/// the backend. `open_path` is called from Rust, which bypasses the plugin ACL
/// scope — that only gates calls arriving from the frontend, and the path here
/// comes from our own asset table rather than from the webview.
#[tauri::command]
pub fn open_artifact(state: State<'_, AppState>, artifact_id: String) -> AppResult<()> {
    let file = state.core().asset_file(&artifact_id)?;
    if !file.path.exists() {
        return Err(AppError::new("asset_file_missing", "资源文件已不在磁盘上"));
    }
    tauri_plugin_opener::open_path(&file.path, None::<&str>)
        .map_err(|error| AppError::new("open_artifact_failed", error.to_string()))
}


#[tauri::command]
pub fn cancel_job(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> AppResult<JobRecord> {
    let record = state.core().cancel_job(id)?;
    let _ = app.emit(
        "job://stage",
        JobEventPayload {
            job_id: record.id.clone(),
            status: record.status.clone(),
            stage: record
                .stage
                .clone()
                .unwrap_or_else(|| "cancelled".to_string()),
            message: record
                .message
                .clone()
                .unwrap_or_else(|| "任务已停止".to_string()),
            timestamp: Utc::now(),
        },
    );
    Ok(record)
}

#[tauri::command]
pub fn delete_templates(state: State<'_, AppState>, ids: Vec<String>) -> AppResult<usize> {
    state.core().delete_templates(ids)
}

#[tauri::command]
pub fn delete_job(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state.core().delete_job(id)
}

#[tauri::command]
pub fn save_asset(state: State<'_, AppState>, asset_id: String, path: String) -> AppResult<()> {
    state.core().export_asset(&asset_id, std::path::Path::new(&path))
}
