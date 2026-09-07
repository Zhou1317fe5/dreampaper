//! Immutable source snapshots for the workbench.
//!
//! A project never points at a generation output or a file the user picked:
//! outputs are deleted with their job and picked files move. The bytes are
//! copied once into `workbench/assets/<sha256>.<ext>`, deduplicated by
//! content digest, and referenced by id. Liveness is a query over projects,
//! not a counter that could drift, and cleanup re-checks before deleting.

use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::core::asset::{guess_mime, protocol_url, sanitize_filename};
use crate::core::store::Store;
use crate::error::{AppError, AppResult};

pub const PROTOCOL: &str = "dp-workbench";

#[derive(Clone, Debug, Serialize)]
pub struct WorkbenchAsset {
    pub id: String,
    pub digest: String,
    pub filename: String,
    pub mime: String,
    /// File name inside the asset directory. Relative on purpose: the app
    /// data directory may move with the user's profile.
    pub path: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub created_at: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct CleanupResult {
    pub removed: usize,
    pub freed_bytes: u64,
}

pub struct AssetStore<'a> {
    store: &'a Store,
    app_data: &'a Path,
}

impl<'a> AssetStore<'a> {
    pub fn new(store: &'a Store, app_data: &'a Path) -> Self {
        Self { store, app_data }
    }

    pub fn dir(&self) -> PathBuf {
        self.app_data.join("workbench").join("assets")
    }

    pub fn file_path(&self, asset: &WorkbenchAsset) -> PathBuf {
        self.dir().join(&asset.path)
    }

    pub fn url(id: &str) -> String {
        protocol_url(PROTOCOL, &format!("asset/{id}"))
    }

    /// Store `bytes` as a snapshot, or return the existing asset with the
    /// same digest. `filename` is only remembered for display and naming.
    pub fn snapshot_bytes(&self, filename: &str, bytes: &[u8]) -> AppResult<WorkbenchAsset> {
        if bytes.is_empty() {
            return Err(AppError::new("workbench_source_empty", "图片文件为空"));
        }
        let digest = sha256_hex(bytes);
        if let Some(existing) = self.by_digest(&digest)? {
            // The file may have been deleted by hand; restore it so the
            // project keeps working instead of trusting the index blindly.
            let path = self.file_path(&existing);
            if !path.exists() {
                write_atomic(&path, bytes)?;
            }
            return Ok(existing);
        }

        let (width, height) = super::source::probe_dimensions(bytes)?;
        let safe_name = sanitize_filename(filename);
        let ext = Path::new(&safe_name)
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .filter(|value| matches!(value.as_str(), "png" | "jpg" | "jpeg" | "webp"))
            .unwrap_or_else(|| "bin".to_string());
        let stored = format!("{digest}.{ext}");
        write_atomic(&self.dir().join(&stored), bytes)?;

        let asset = WorkbenchAsset {
            id: Uuid::new_v4().to_string(),
            digest,
            filename: filename.to_string(),
            mime: guess_mime(Path::new(&stored)),
            path: stored,
            bytes: bytes.len() as u64,
            width,
            height,
            created_at: Utc::now().to_rfc3339(),
        };
        let conn = self.store.connection()?;
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO workbench_assets(id, digest, filename, mime, path, bytes, width, height, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                asset.id,
                asset.digest,
                asset.filename,
                asset.mime,
                asset.path,
                asset.bytes as i64,
                asset.width,
                asset.height,
                asset.created_at
            ],
        )?;
        if inserted == 0 {
            // Lost a race with a concurrent import of the same bytes.
            return self
                .by_digest(&asset.digest)?
                .ok_or_else(|| AppError::new("workbench_asset_missing", "快照登记失败"));
        }
        Ok(asset)
    }

    pub fn snapshot_file(&self, path: &Path) -> AppResult<WorkbenchAsset> {
        let bytes = std::fs::read(path).map_err(|error| {
            AppError::new(
                "workbench_source_unreadable",
                format!("无法读取图片：{error}"),
            )
        })?;
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("image.png");
        self.snapshot_bytes(filename, &bytes)
    }

    pub fn get(&self, id: &str) -> AppResult<WorkbenchAsset> {
        let conn = self.store.connection()?;
        conn.query_row(
            &format!("{SELECT_ASSET} WHERE id = ?1"),
            params![id],
            asset_from_row,
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => {
                AppError::new("workbench_asset_not_found", "源图快照不存在")
            }
            other => other.into(),
        })
    }

    pub fn by_digest(&self, digest: &str) -> AppResult<Option<WorkbenchAsset>> {
        let conn = self.store.connection()?;
        conn.query_row(
            &format!("{SELECT_ASSET} WHERE digest = ?1"),
            params![digest],
            asset_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    /// Snapshots no project references any more. These are safe to delete
    /// once the caller re-confirms nothing was created in between.
    pub fn unreferenced(&self) -> AppResult<Vec<WorkbenchAsset>> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare(&format!(
            "{SELECT_ASSET} WHERE NOT EXISTS (SELECT 1 FROM workbench_projects p WHERE p.asset_id = workbench_assets.id) \
             ORDER BY created_at"
        ))?;
        let rows = stmt.query_map([], asset_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Delete every unreferenced snapshot. Each row is re-checked inside the
    /// same statement that deletes it, so a project created after the
    /// listing keeps its source.
    pub fn cleanup_unreferenced(&self) -> AppResult<CleanupResult> {
        let mut result = CleanupResult::default();
        for asset in self.unreferenced()? {
            let conn = self.store.connection()?;
            let removed = conn.execute(
                "DELETE FROM workbench_assets WHERE id = ?1 \
                 AND NOT EXISTS (SELECT 1 FROM workbench_projects p WHERE p.asset_id = ?1)",
                params![asset.id],
            )?;
            if removed == 0 {
                continue;
            }
            let path = self.file_path(&asset);
            match std::fs::remove_file(&path) {
                Ok(()) => result.freed_bytes += asset.bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(AppError::new(
                        "workbench_cleanup_failed",
                        format!("删除快照失败：{error}"),
                    ))
                }
            }
            result.removed += 1;
            let _ = crate::core::thumb::forget(self.app_data, &thumb_id(&asset.id));
        }
        Ok(result)
    }

    /// Total bytes and count of stored snapshots, plus the unreferenced part.
    pub fn usage(&self) -> AppResult<(usize, u64, usize, u64)> {
        let conn = self.store.connection()?;
        let (count, bytes): (i64, i64) = conn.query_row(
            "SELECT count(*), coalesce(sum(bytes), 0) FROM workbench_assets",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let (free_count, free_bytes): (i64, i64) = conn.query_row(
            "SELECT count(*), coalesce(sum(bytes), 0) FROM workbench_assets \
             WHERE NOT EXISTS (SELECT 1 FROM workbench_projects p WHERE p.asset_id = workbench_assets.id)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok((
            count as usize,
            bytes as u64,
            free_count as usize,
            free_bytes as u64,
        ))
    }
}

/// Id under which the shared thumbnail cache keys this asset's downscales.
pub fn thumb_id(asset_id: &str) -> String {
    format!("wb-{asset_id}")
}

const SELECT_ASSET: &str =
    "SELECT id, digest, filename, mime, path, bytes, width, height, created_at FROM workbench_assets";

fn asset_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkbenchAsset> {
    Ok(WorkbenchAsset {
        id: row.get(0)?,
        digest: row.get(1)?,
        filename: row.get(2)?,
        mime: row.get(3)?,
        path: row.get(4)?,
        bytes: row.get::<_, i64>(5)? as u64,
        width: row.get::<_, i64>(6)? as u32,
        height: row.get::<_, i64>(7)? as u32,
        created_at: row.get(8)?,
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Write through a sibling temp file and rename, so a crash never leaves a
/// truncated snapshot that an index row still points at.
pub fn write_atomic(target: &Path, bytes: &[u8]) -> AppResult<()> {
    let parent = target
        .parent()
        .ok_or_else(|| AppError::new("io_error", "目标路径没有父目录"))?;
    std::fs::create_dir_all(parent)?;
    let staging = parent.join(format!(
        ".{}.{}.part",
        target
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("file"),
        Uuid::new_v4()
    ));
    let written = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::create(&staging)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = written.and_then(|_| rename_replace(&staging, target)) {
        let _ = std::fs::remove_file(&staging);
        return Err(error.into());
    }
    Ok(())
}

/// `rename` with the same retry the thumbnail cache uses: on Windows an
/// indexer holding the target for a moment makes the first attempt fail.
pub fn rename_replace(staging: &Path, target: &Path) -> std::io::Result<()> {
    let mut last = match std::fs::rename(staging, target) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    for backoff_ms in [20, 60, 150] {
        std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
        match std::fs::rename(staging, target) {
            Ok(()) => return Ok(()),
            Err(error) => last = error,
        }
    }
    Err(last)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(crate) fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dreampaper-wb-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub(crate) fn png_bytes(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut image = image::RgbaImage::new(width, height);
        for pixel in image.pixels_mut() {
            *pixel = image::Rgba(rgba);
        }
        let mut out = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut out, image::ImageFormat::Png)
            .expect("encode test png");
        out.into_inner()
    }

    #[test]
    fn snapshot_url_routes_to_the_asset_resource() {
        let url = AssetStore::url("snapshot-id");
        assert!(url.ends_with("/asset/snapshot-id"), "{url}");
    }

    #[test]
    fn same_bytes_dedupe_to_one_snapshot() {
        let dir = temp_dir("dedupe");
        let store = Store::initialize(&dir).unwrap();
        let assets = AssetStore::new(&store, &dir);
        let bytes = png_bytes(8, 4, [255, 0, 0, 255]);
        let first = assets.snapshot_bytes("图 A.png", &bytes).unwrap();
        let second = assets.snapshot_bytes("other.png", &bytes).unwrap();
        assert_eq!(first.id, second.id, "同一内容必须复用同一快照");
        assert_eq!(first.width, 8);
        assert_eq!(first.height, 4);
        assert_eq!(first.path, format!("{}.png", first.digest));
        assert!(assets.file_path(&first).exists());
        let (count, bytes_total, free, _) = assets.usage().unwrap();
        assert_eq!((count, free), (1, 1));
        assert_eq!(bytes_total, bytes.len() as u64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_only_removes_unreferenced_snapshots() {
        let dir = temp_dir("cleanup");
        let store = Store::initialize(&dir).unwrap();
        let assets = AssetStore::new(&store, &dir);
        let kept = assets
            .snapshot_bytes("kept.png", &png_bytes(2, 2, [0, 0, 255, 255]))
            .unwrap();
        let orphan = assets
            .snapshot_bytes("orphan.png", &png_bytes(3, 3, [0, 255, 0, 255]))
            .unwrap();
        let conn = store.connection().unwrap();
        conn.execute(
            "INSERT INTO workbench_projects(id, asset_id, name, path, schema_version, revision, export_width, export_height, created_at, updated_at) \
             VALUES ('p1', ?1, 'n', 'p1.json', 1, 1, 2, 2, 't', 't')",
            params![kept.id],
        )
        .unwrap();
        drop(conn);

        let result = assets.cleanup_unreferenced().unwrap();
        assert_eq!(result.removed, 1);
        assert_eq!(result.freed_bytes, orphan.bytes);
        assert!(assets.get(&kept.id).is_ok());
        assert_eq!(
            assets.get(&orphan.id).unwrap_err().code,
            "workbench_asset_not_found"
        );
        assert!(!assets.file_path(&orphan).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_undecodable_bytes() {
        let dir = temp_dir("bad");
        let store = Store::initialize(&dir).unwrap();
        let assets = AssetStore::new(&store, &dir);
        let error = assets.snapshot_bytes("x.png", b"not an image").unwrap_err();
        assert_eq!(error.code, "workbench_source_undecodable");
        assert!(assets.unreferenced().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
