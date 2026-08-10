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

    /// 把一个已登记的 asset 另存到用户挑的路径。
    ///
    /// 走后端复制而不是在前端把 URL 塞给 `<a download>`：`dp-asset://` 是自定义
    /// 协议，WKWebView 对它不认 download 属性，点下去只会把图片当页面导航过去，
    /// 表现就是整扇窗被那张图占满，而且回不去。
    ///
    /// 用 `fs::copy` 而不是先读进内存再写：出图动辄十几 MB，没必要在内存里过一手。
    pub fn export_asset(&self, id: &str, target: &Path) -> AppResult<()> {
        let file = self.asset_file(id)?;
        if !file.path.exists() {
            return Err(AppError::new("asset_file_missing", "资源文件已不在磁盘上"));
        }
        // 目标目录一般是用户家目录下已存在的路径，但「新建文件夹」后立刻保存的情况
        // 也有，父目录缺失时补一下，免得抛一个看不懂的 io 错误
        if let Some(parent) = target.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::copy(&file.path, target)?;
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// 「下载」按钮的落点：把 outputs/ 里的产出复制到用户挑的路径，
    /// 原文件留在原地（近期任务里还要能再看、再下一次）。
    #[test]
    fn export_copies_the_file_and_keeps_the_original() {
        let dir = service_dir("export");
        let store = Store::initialize(&dir).expect("初始化 store");
        let assets = AssetService::new(&store, &dir);
        let saved = assets
            .save_job_image("job-1", "figure.png", b"fake-png-bytes")
            .expect("落盘产出");

        // 目标故意放在一个还不存在的子目录里：用户在保存对话框里新建文件夹是常事
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

    /// 磁盘上的文件被手动删了，得给一句能看懂的话，而不是抛一个裸 io 错误。
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
