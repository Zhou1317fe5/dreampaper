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
            url: protocol_url("dp-asset", &id),
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

    pub fn export_asset(&self, id: &str, target: &Path) -> AppResult<()> {
        let file = self.asset_file(id)?;
        if !file.path.exists() {
            return Err(AppError::new("asset_file_missing", "资源文件已不在磁盘上"));
        }
        if let Some(parent) = target.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::copy(&file.path, target)?;
        Ok(())
    }

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
            url: protocol_url("dp-asset", &id),
        })
    }
}

/// Build a custom-protocol URL for a stored resource.
///
/// The two forms are not interchangeable; the platform decides:
///   - macOS / Linux: `<scheme>://localhost/<id>` — WebKit accepts the scheme.
///   - Windows / Android: `http://<scheme>.localhost/<id>`. WebView2 cannot
///     register custom schemes, so Tauri implements
///     `register_uri_scheme_protocol` by intercepting
///     `http://<scheme>.localhost/*` instead.
///
/// This used to hardcode the macOS form, which made `dp-asset://` /
/// `dp-template://` an unknown protocol to WebView2: `<img>` failed silently
/// with no request and no console error, so the template library and import
/// previews were blank on Windows while macOS worked fine.
pub fn protocol_url(scheme: &str, id: &str) -> String {
    if cfg!(windows) {
        format!("http://{scheme}.localhost/{id}")
    } else {
        format!("{scheme}://localhost/{id}")
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The URL form must follow the platform: WebView2 only accepts
    /// `http://<scheme>.localhost/`, and the macOS form makes every `<img>`
    /// there fail silently. The assertion branches on target so a Windows CI
    /// build catches a regression.
    #[test]
    fn protocol_url_follows_the_platform() {
        let url = protocol_url("dp-asset", "abc-123");
        #[cfg(windows)]
        assert_eq!(url, "http://dp-asset.localhost/abc-123");
        #[cfg(not(windows))]
        assert_eq!(url, "dp-asset://localhost/abc-123");
    }

    fn service_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dreampaper-asset-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    #[test]
    fn export_copies_the_file_and_keeps_the_original() {
        let dir = service_dir("export");
        let store = Store::initialize(&dir).expect("初始化 store");
        let assets = AssetService::new(&store, &dir);
        let saved = assets
            .save_job_image("job-1", "figure.png", b"fake-png-bytes")
            .expect("落盘产出");

        let target = dir.join("picked").join("我的图.png");
        assets.export_asset(&saved.id, &target).expect("另存");

        assert_eq!(
            std::fs::read(&target).expect("读另存出来的文件"),
            b"fake-png-bytes",
            "另存出来的内容必须和原图一致"
        );
        let original = assets.asset_file(&saved.id).expect("读 asset 记录");
        assert!(original.path.exists(), "另存是复制，不该把原文件搬走");
    }

    #[test]
    fn export_reports_a_missing_file_clearly() {
        let dir = service_dir("export-missing");
        let store = Store::initialize(&dir).expect("初始化 store");
        let assets = AssetService::new(&store, &dir);
        let saved = assets
            .save_job_image("job-1", "figure.png", b"bytes")
            .expect("落盘产出");
        let file = assets.asset_file(&saved.id).expect("读 asset 记录");
        std::fs::remove_file(&file.path).expect("手动删掉图片");

        let error = assets
            .export_asset(&saved.id, &dir.join("out.png"))
            .expect_err("文件没了应当报错");
        assert_eq!(error.code, "asset_file_missing");
    }
}
