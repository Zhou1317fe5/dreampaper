use chrono::Utc;
use tauri::{AppHandle, Emitter, State};

use crate::core::asset::AssetUpload;
use crate::core::config::AppConfig;
use crate::core::doc::{DocumentChunkHit, DocumentSummary};
use crate::core::job::JobRecord;
use crate::core::tpl::{TemplatePackSummary, TemplateSummary};
use crate::error::AppResult;
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
    // 管道后台跑，命令立刻返回排队中的记录，前端不会卡在 invoke 上
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

#[tauri::command]
pub fn open_artifact(_state: State<'_, AppState>, _artifact_id: String) -> AppResult<()> {
    Ok(())
}

/// 停止一个在跑的任务，返回停止之后的记录，前端直接拿它刷新面板。
#[tauri::command]
pub fn cancel_job(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> AppResult<JobRecord> {
    let record = state.core().cancel_job(id)?;
    // 管道那边的 future 已经被 abort，它不会再发事件了：终态事件得由这里补上
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

/// 另存资源文件，用于「下载」按钮。前端先用原生保存对话框让用户挑路径，
/// 这个命令再把 asset 复制到那个路径，绕过 `<a download>` 对自定义协议的不认。
#[tauri::command]
pub fn save_asset(state: State<'_, AppState>, asset_id: String, path: String) -> AppResult<()> {
    state.core().export_asset(&asset_id, std::path::Path::new(&path))
}
