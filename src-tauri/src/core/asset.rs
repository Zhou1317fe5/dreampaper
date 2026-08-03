use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::params;
use serde::Serialize;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

use super::store::Store;

#[derive(Clone, Debug, Serialize)]
pub struct AssetUpload {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub url: String,
}

#[derive(Clone, Debug)]
pub struct AssetFile {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub path: PathBuf,
}

pub struct AssetService<'a> {
    store: &'a Store,
    app_data: &'a Path,
}

impl<'a> AssetService<'a> {
    pub fn new(store: &'a Store, app_data: &'a Path) -> Self {
        Self { store, app_data }
    }

    pub fn import_asset(
        &self,
        filename: String,
        mime_type: String,
        bytes: Vec<u8>,
    ) -> AppResult<AssetUpload> {
        let id = Uuid::new_v4().to_string();
        let safe_name = sanitize_filename(&filename);
        let suffix = Path::new(&safe_name)
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| format!(".{value}"))
            .unwrap_or_else(|| ".bin".to_string());
        let stored_name = format!("{id}{suffix}");
        let upload_dir = self.app_data.join("uploads");
        std::fs::create_dir_all(&upload_dir)?;
        let path = upload_dir.join(&stored_name);
        std::fs::write(&path, &bytes)?;
        let detected_mime = if mime_type.trim().is_empty() || mime_type == "application/octet-stream" {
            guess_mime(&path)
        } else {
            mime_type
        };

        let conn = self.store.connection()?;
        conn.execute(
            "INSERT INTO assets(id, kind, filename, mime, path, bytes, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                "upload",
                filename,
                detected_mime,
                path.to_string_lossy().to_string(),
                bytes.len() as i64,
                Utc::now().to_rfc3339()
            ],
        )?;

        Ok(AssetUpload {
            id: id.clone(),
            filename,
            mime_type: detected_mime,
            url: format!("dp-asset://localhost/{id}"),
        })
    }

    pub fn asset_file(&self, id: &str) -> AppResult<AssetFile> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare("SELECT id, filename, mime, path FROM assets WHERE id = ?1")?;
        stmt.query_row(params![id], |row| {
            let path: String = row.get(3)?;
            Ok(AssetFile {
                id: row.get(0)?,
                filename: row.get(1)?,
                mime_type: row.get(2)?,
                path: PathBuf::from(path),
            })
        })
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => {
                AppError::new("asset_not_found", "Asset not found")
            }
            other => other.into(),
        })
    }

    /// 生成结果落 `outputs/{job_id}/`，并登记为 asset —— `dp-asset://` 协议按 id 取文件，
    /// 所以出图必须进 assets 表才在前端可见。
    pub fn save_job_image(
        &self,
        job_id: &str,
        filename: &str,
        bytes: &[u8],
    ) -> AppResult<AssetUpload> {
        let id = Uuid::new_v4().to_string();
        let job_dir = self.app_data.join("outputs").join(sanitize_filename(job_id));
        std::fs::create_dir_all(&job_dir)?;
        let path = job_dir.join(sanitize_filename(filename));
        std::fs::write(&path, bytes)?;
        let mime_type = guess_mime(&path);

        let conn = self.store.connection()?;
        conn.execute(
            "INSERT INTO assets(id, kind, filename, mime, path, bytes, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                "output",
                filename,
                mime_type,
                path.to_string_lossy().to_string(),
                bytes.len() as i64,
                Utc::now().to_rfc3339()
            ],
        )?;

        Ok(AssetUpload {
            id: id.clone(),
            filename: filename.to_string(),
            mime_type,
            url: format!("dp-asset://localhost/{id}"),
        })
    }
}

pub fn guess_mime(path: &Path) -> String {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "txt" | "md" | "markdown" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
    .to_string()
}

pub fn sanitize_filename(name: &str) -> String {
    let value = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if value.trim_matches('_').is_empty() {
        "upload.bin".to_string()
    } else {
        value
    }
}
