//! Project documents on disk plus their SQLite index.
//!
//! The JSON under `workbench/projects/<id>.json` is the truth. Saves carry
//! the revision the client last saw: a stale save is refused rather than
//! merged, which is what keeps an out-of-order autosave from overwriting
//! newer work. Every write goes through a sibling temp file and rename, and
//! `repair_index` re-derives index rows from the files at startup, so a
//! crash between the two steps costs nothing.

use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::store::Store;
use crate::error::{AppError, AppResult};

use super::asset::{rename_replace, AssetStore, WorkbenchAsset};
use super::doc::{ProjectDoc, SourceRef, Viewport, SCHEMA_VERSION};
use super::geom::PixelRect;

/// Width the project list asks the thumbnail cache for.
const LIST_THUMB_WIDTH: u32 = 400;

#[derive(Clone, Debug, Serialize)]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    pub asset_id: String,
    pub filename: String,
    pub source_url: String,
    pub thumbnail: String,
    pub source_width: u32,
    pub source_height: u32,
    pub export_width: u32,
    pub export_height: u32,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProjectDetail {
    pub summary: ProjectSummary,
    pub document: ProjectDoc,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportRecord {
    pub id: String,
    pub project_id: String,
    pub filename: String,
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub created_at: String,
    /// Whether the file is still where the user saved it. Computed on read.
    pub available: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeleteResult {
    /// The source snapshot has no remaining project and may be cleaned up.
    pub asset_unreferenced: bool,
}

pub struct ProjectStore<'a> {
    store: &'a Store,
    app_data: &'a Path,
}

impl<'a> ProjectStore<'a> {
    pub fn new(store: &'a Store, app_data: &'a Path) -> Self {
        Self { store, app_data }
    }

    pub fn dir(&self) -> PathBuf {
        self.app_data.join("workbench").join("projects")
    }

    fn doc_path(&self, id: &str) -> PathBuf {
        self.dir().join(format!("{id}.json"))
    }

    fn assets(&self) -> AssetStore<'_> {
        AssetStore::new(self.store, self.app_data)
    }

    pub fn list(&self) -> AppResult<Vec<ProjectSummary>> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare(&format!("{SELECT_SUMMARY} ORDER BY p.updated_at DESC"))?;
        let rows = stmt.query_map([], summary_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn summary(&self, id: &str) -> AppResult<ProjectSummary> {
        let conn = self.store.connection()?;
        conn.query_row(
            &format!("{SELECT_SUMMARY} WHERE p.id = ?1"),
            params![id],
            summary_from_row,
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => not_found(),
            other => other.into(),
        })
    }

    /// Most recently edited project on a snapshot, so re-entering from the
    /// same image resumes work instead of piling up projects.
    pub fn latest_for_asset(&self, asset_id: &str) -> AppResult<Option<ProjectSummary>> {
        let conn = self.store.connection()?;
        conn.query_row(
            &format!("{SELECT_SUMMARY} WHERE p.asset_id = ?1 ORDER BY p.updated_at DESC LIMIT 1"),
            params![asset_id],
            summary_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    /// A fresh project on `asset`: full-image crop, no layers.
    pub fn create(&self, asset: &WorkbenchAsset) -> AppResult<ProjectDetail> {
        let doc = self.blank_doc(asset)?;
        self.insert(doc)
    }

    /// A new project sharing the snapshot and copying every edit parameter.
    pub fn copy(&self, project_id: &str) -> AppResult<ProjectDetail> {
        let current = self.get(project_id)?;
        let asset = self.assets().get(&current.summary.asset_id)?;
        let mut doc = self.blank_doc(&asset)?;
        doc.viewport = current.document.viewport.clone();
        doc.layers = current.document.layers.clone();
        doc.source = current.document.source.clone();
        self.insert(doc)
    }

    fn blank_doc(&self, asset: &WorkbenchAsset) -> AppResult<ProjectDoc> {
        let now = Utc::now().to_rfc3339();
        Ok(ProjectDoc {
            schema_version: SCHEMA_VERSION,
            id: Uuid::new_v4().to_string(),
            revision: 1,
            name: self.next_name(asset)?,
            source: SourceRef {
                asset_id: asset.id.clone(),
                filename: asset.filename.clone(),
                width: asset.width,
                height: asset.height,
                orientation_normalized: true,
                working_color_space: "srgb".to_string(),
            },
            viewport: Viewport {
                crop: PixelRect::new(0, 0, i64::from(asset.width), i64::from(asset.height)),
                flip_x: false,
                flip_y: false,
            },
            layers: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
        })
    }

    /// `<stem> · 编辑 N`, where N is one past the highest N already used for
    /// this snapshot (renamed projects do not consume a number).
    fn next_name(&self, asset: &WorkbenchAsset) -> AppResult<String> {
        let stem = Path::new(&asset.filename)
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("图片")
            .to_string();
        let prefix = format!("{stem} · 编辑 ");
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare("SELECT name FROM workbench_projects WHERE asset_id = ?1")?;
        let names = stmt.query_map(params![asset.id], |row| row.get::<_, String>(0))?;
        let mut highest = 0u32;
        for name in names.flatten() {
            if let Some(n) = name
                .strip_prefix(&prefix)
                .and_then(|rest| rest.trim().parse::<u32>().ok())
            {
                highest = highest.max(n);
            }
        }
        Ok(format!("{prefix}{}", highest + 1))
    }

    fn insert(&self, doc: ProjectDoc) -> AppResult<ProjectDetail> {
        doc.validate()?;
        write_doc(&self.doc_path(&doc.id), &doc)?;
        let conn = self.store.connection()?;
        conn.execute(
            "INSERT INTO workbench_projects(id, asset_id, name, path, schema_version, revision, export_width, export_height, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                doc.id,
                doc.source.asset_id,
                doc.name,
                format!("{}.json", doc.id),
                doc.schema_version,
                doc.revision as i64,
                doc.viewport.crop.width,
                doc.viewport.crop.height,
                doc.created_at,
                doc.updated_at
            ],
        )?;
        drop(conn);
        self.detail(doc)
    }

    pub fn get(&self, id: &str) -> AppResult<ProjectDetail> {
        let path = self.doc_path(id);
        let doc = read_doc(&path)?;
        if self.summary(id).is_err() {
            // The JSON landed but the index did not; heal it on the spot.
            self.repair_index()?;
        }
        self.detail(doc)
    }

    fn detail(&self, doc: ProjectDoc) -> AppResult<ProjectDetail> {
        let summary = self.summary(&doc.id)?;
        Ok(ProjectDetail {
            summary,
            document: doc,
        })
    }

    /// Persist `doc` if `base_revision` is still current. The returned
    /// document carries the new revision the client must send next time.
    pub fn save(
        &self,
        id: &str,
        base_revision: u64,
        mut doc: ProjectDoc,
    ) -> AppResult<ProjectDetail> {
        if doc.id != id {
            return Err(AppError::new("project_id_mismatch", "工程 id 不匹配"));
        }
        let current = self.summary(id)?;
        if current.revision != base_revision {
            return Err(AppError::with_detail(
                "project_revision_conflict",
                "工程已被更新，请刷新后重试",
                serde_json::json!({ "current": current.revision, "base": base_revision }),
            ));
        }
        doc.revision = current.revision + 1;
        doc.updated_at = Utc::now().to_rfc3339();
        doc.source.asset_id = current.asset_id.clone();
        doc.validate()?;
        write_doc(&self.doc_path(id), &doc)?;
        self.update_index(&doc)?;
        self.detail(doc)
    }

    fn update_index(&self, doc: &ProjectDoc) -> AppResult<()> {
        let conn = self.store.connection()?;
        conn.execute(
            "UPDATE workbench_projects SET name = ?2, revision = ?3, export_width = ?4, export_height = ?5, updated_at = ?6, schema_version = ?7 \
             WHERE id = ?1",
            params![
                doc.id,
                doc.name,
                doc.revision as i64,
                doc.viewport.crop.width,
                doc.viewport.crop.height,
                doc.updated_at,
                doc.schema_version
            ],
        )?;
        Ok(())
    }

    /// Rename without touching the edit state or bumping the client-visible
    /// revision handshake: the client's document is updated in place and its
    /// revision advanced like any other save.
    pub fn rename(&self, id: &str, name: &str) -> AppResult<ProjectSummary> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::new("project_name_empty", "工程名称不能为空"));
        }
        let mut doc = read_doc(&self.doc_path(id))?;
        doc.name = name.to_string();
        doc.revision += 1;
        doc.updated_at = Utc::now().to_rfc3339();
        write_doc(&self.doc_path(id), &doc)?;
        self.update_index(&doc)?;
        self.summary(id)
    }

    pub fn delete(&self, id: &str) -> AppResult<DeleteResult> {
        let summary = self.summary(id)?;
        let conn = self.store.connection()?;
        conn.execute(
            "DELETE FROM workbench_exports WHERE project_id = ?1",
            params![id],
        )?;
        conn.execute("DELETE FROM workbench_projects WHERE id = ?1", params![id])?;
        let remaining: i64 = conn.query_row(
            "SELECT count(*) FROM workbench_projects WHERE asset_id = ?1",
            params![summary.asset_id],
            |row| row.get(0),
        )?;
        drop(conn);
        match std::fs::remove_file(self.doc_path(id)) {
            Ok(()) | Err(_) => {}
        }
        Ok(DeleteResult {
            asset_unreferenced: remaining == 0,
        })
    }

    /// Bring the index back in line with the files: rows for documents that
    /// exist without one, newer revisions read from disk, and rows dropped
    /// for documents that are gone.
    pub fn repair_index(&self) -> AppResult<()> {
        let dir = self.dir();
        std::fs::create_dir_all(&dir)?;
        let conn = self.store.connection()?;
        let mut known: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT id, revision FROM workbench_projects")?;
            for row in stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })? {
                let (id, revision) = row?;
                known.insert(id, revision);
            }
        }
        let mut seen = std::collections::HashSet::new();
        for entry in std::fs::read_dir(&dir)?.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Ok(doc) = read_doc(&path) else {
                continue;
            };
            seen.insert(doc.id.clone());
            let asset_exists: bool = conn
                .query_row(
                    "SELECT 1 FROM workbench_assets WHERE id = ?1",
                    params![doc.source.asset_id],
                    |_| Ok(true),
                )
                .optional()?
                .unwrap_or(false);
            if !asset_exists {
                continue;
            }
            match known.get(&doc.id) {
                None => {
                    conn.execute(
                        "INSERT INTO workbench_projects(id, asset_id, name, path, schema_version, revision, export_width, export_height, created_at, updated_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        params![
                            doc.id,
                            doc.source.asset_id,
                            doc.name,
                            format!("{}.json", doc.id),
                            doc.schema_version,
                            doc.revision as i64,
                            doc.viewport.crop.width,
                            doc.viewport.crop.height,
                            doc.created_at,
                            doc.updated_at
                        ],
                    )?;
                }
                Some(revision) if (*revision as u64) < doc.revision => {
                    conn.execute(
                        "UPDATE workbench_projects SET name = ?2, revision = ?3, export_width = ?4, export_height = ?5, updated_at = ?6 WHERE id = ?1",
                        params![
                            doc.id,
                            doc.name,
                            doc.revision as i64,
                            doc.viewport.crop.width,
                            doc.viewport.crop.height,
                            doc.updated_at
                        ],
                    )?;
                }
                Some(_) => {}
            }
        }
        for id in known.keys() {
            if !seen.contains(id) {
                conn.execute(
                    "DELETE FROM workbench_exports WHERE project_id = ?1",
                    params![id],
                )?;
                conn.execute("DELETE FROM workbench_projects WHERE id = ?1", params![id])?;
            }
        }
        Ok(())
    }

    /// Describe a finished export. Nothing is persisted: the workbench keeps
    /// no export log (PRD L1), the legacy `workbench_exports` table only gets
    /// cleaned up when a project is deleted.
    pub fn export_record(
        &self,
        project_id: &str,
        path: &Path,
        width: u32,
        height: u32,
    ) -> ExportRecord {
        ExportRecord {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_string(),
            filename: path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("export.png")
                .to_string(),
            path: path.to_string_lossy().to_string(),
            width,
            height,
            created_at: Utc::now().to_rfc3339(),
            available: true,
        }
    }

    /// Bytes used by project documents on disk.
    pub fn usage(&self) -> AppResult<(usize, u64)> {
        let mut count = 0;
        let mut bytes = 0;
        if let Ok(entries) = std::fs::read_dir(self.dir()) {
            for entry in entries.flatten() {
                if entry.path().extension().and_then(|value| value.to_str()) == Some("json") {
                    count += 1;
                    bytes += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                }
            }
        }
        Ok((count, bytes))
    }
}

const SELECT_SUMMARY: &str = "SELECT p.id, p.name, p.asset_id, a.filename, a.width, a.height, p.export_width, p.export_height, p.revision, p.created_at, p.updated_at \
     FROM workbench_projects p JOIN workbench_assets a ON a.id = p.asset_id";

fn summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectSummary> {
    let asset_id: String = row.get(2)?;
    let source_url = AssetStore::url(&asset_id);
    Ok(ProjectSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        filename: row.get(3)?,
        thumbnail: format!("{source_url}?w={LIST_THUMB_WIDTH}"),
        source_url,
        asset_id,
        source_width: row.get::<_, i64>(4)? as u32,
        source_height: row.get::<_, i64>(5)? as u32,
        export_width: row.get::<_, i64>(6)? as u32,
        export_height: row.get::<_, i64>(7)? as u32,
        revision: row.get::<_, i64>(8)? as u64,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn not_found() -> AppError {
    AppError::new("project_not_found", "工程不存在")
}

fn read_doc(path: &Path) -> AppResult<ProjectDoc> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(not_found()),
        Err(error) => return Err(error.into()),
    };
    let doc: ProjectDoc = serde_json::from_slice(&raw).map_err(|error| {
        AppError::new("project_file_corrupt", format!("工程文件无法读取：{error}"))
    })?;
    doc.validate()?;
    Ok(doc)
}

/// Serialize to a sibling temp file, sync, then rename over the target.
fn write_doc(path: &Path, doc: &ProjectDoc) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("io_error", "工程目录无效"))?;
    std::fs::create_dir_all(parent)?;
    let staging = parent.join(format!(".{}.{}.part", doc.id, Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(doc)?;
    let result = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::create(&staging)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        rename_replace(&staging, path)
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&staging);
        return Err(AppError::new(
            "project_write_failed",
            format!("工程保存失败：{error}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::asset::tests::{png_bytes, temp_dir};
    use super::super::doc::{sample_text, Layer};
    use super::*;

    fn setup(tag: &str) -> (PathBuf, Store, WorkbenchAsset) {
        let dir = temp_dir(tag);
        let store = Store::initialize(&dir).unwrap();
        let asset = AssetStore::new(&store, &dir)
            .snapshot_bytes("figure.png", &png_bytes(64, 32, [200, 200, 200, 255]))
            .unwrap();
        (dir, store, asset)
    }

    #[test]
    fn creates_named_projects_and_reopens_them() {
        let (dir, store, asset) = setup("create");
        let projects = ProjectStore::new(&store, &dir);
        let first = projects.create(&asset).unwrap();
        let second = projects.create(&asset).unwrap();
        assert_eq!(first.summary.name, "figure · 编辑 1");
        assert_eq!(second.summary.name, "figure · 编辑 2");
        assert_eq!(first.document.viewport.crop, PixelRect::new(0, 0, 64, 32));
        assert_eq!(first.summary.export_width, 64);

        let reopened = projects.get(&first.summary.id).unwrap();
        assert_eq!(reopened.document.revision, 1);
        assert_eq!(reopened.summary.source_url, AssetStore::url(&asset.id));
        assert_eq!(projects.list().unwrap().len(), 2);
        assert_eq!(
            projects.latest_for_asset(&asset.id).unwrap().unwrap().id,
            second.summary.id
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_requires_the_current_revision() {
        let (dir, store, asset) = setup("revision");
        let projects = ProjectStore::new(&store, &dir);
        let created = projects.create(&asset).unwrap();
        let mut doc = created.document.clone();
        doc.layers
            .push(Layer::Text(sample_text("t1", PixelRect::new(1, 1, 20, 10))));

        let saved = projects.save(&doc.id, 1, doc.clone()).unwrap();
        assert_eq!(saved.document.revision, 2);
        assert_eq!(saved.document.layers.len(), 1);

        // A stale client (still on revision 1) must not overwrite revision 2.
        let stale = projects.save(&doc.id, 1, doc.clone()).unwrap_err();
        assert_eq!(stale.code, "project_revision_conflict");
        let reread = projects.get(&doc.id).unwrap();
        assert_eq!(reread.document.revision, 2);
        assert_eq!(reread.summary.revision, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copies_share_the_snapshot_and_edit_state_independently() {
        let (dir, store, asset) = setup("copy");
        let projects = ProjectStore::new(&store, &dir);
        let created = projects.create(&asset).unwrap();
        let mut doc = created.document.clone();
        doc.viewport.crop = PixelRect::new(4, 4, 20, 10);
        doc.layers
            .push(Layer::Text(sample_text("t1", PixelRect::new(1, 1, 20, 10))));
        projects.save(&created.document.id, 1, doc).unwrap();

        let copy = projects.copy(&created.summary.id).unwrap();
        assert_ne!(copy.summary.id, created.summary.id);
        assert_eq!(copy.summary.asset_id, asset.id);
        assert_eq!(copy.document.viewport.crop, PixelRect::new(4, 4, 20, 10));
        assert_eq!(copy.document.layers.len(), 1);
        assert_eq!(copy.summary.name, "figure · 编辑 2");
        assert_eq!(copy.document.revision, 1);

        let (count, _, free, _) = AssetStore::new(&store, &dir).usage().unwrap();
        assert_eq!((count, free), (1, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_releases_the_asset_only_with_the_last_project() {
        let (dir, store, asset) = setup("delete");
        let projects = ProjectStore::new(&store, &dir);
        let a = projects.create(&asset).unwrap();
        let b = projects.create(&asset).unwrap();
        assert!(!projects.delete(&a.summary.id).unwrap().asset_unreferenced);
        assert!(projects.get(&b.summary.id).is_ok());
        assert!(projects.delete(&b.summary.id).unwrap().asset_unreferenced);
        assert_eq!(
            projects.get(&b.summary.id).unwrap_err().code,
            "project_not_found"
        );
        // The snapshot itself is untouched until an explicit cleanup.
        assert!(AssetStore::new(&store, &dir).get(&asset.id).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn interrupted_saves_leave_the_last_complete_document() {
        let (dir, store, asset) = setup("interrupt");
        let projects = ProjectStore::new(&store, &dir);
        let created = projects.create(&asset).unwrap();
        let path = projects.doc_path(&created.summary.id);
        // Simulate a crash mid-write: a stray temp file beside the document.
        std::fs::write(
            projects
                .dir()
                .join(format!(".{}.deadbeef.part", created.summary.id)),
            b"{\"half\":",
        )
        .unwrap();
        let reopened = projects.get(&created.summary.id).unwrap();
        assert_eq!(reopened.document.revision, 1);
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repair_index_rebuilds_rows_from_documents() {
        let (dir, store, asset) = setup("repair");
        let projects = ProjectStore::new(&store, &dir);
        let created = projects.create(&asset).unwrap();
        // Simulate "JSON replaced, index not updated": bump the file only.
        let mut doc = created.document.clone();
        doc.revision = 5;
        doc.name = "改名".into();
        write_doc(&projects.doc_path(&doc.id), &doc).unwrap();
        // And an orphan row whose file vanished.
        let conn = store.connection().unwrap();
        conn.execute(
            "INSERT INTO workbench_projects(id, asset_id, name, path, schema_version, revision, export_width, export_height, created_at, updated_at) \
             VALUES ('ghost', ?1, 'g', 'ghost.json', 1, 1, 1, 1, 't', 't')",
            params![asset.id],
        )
        .unwrap();
        drop(conn);

        projects.repair_index().unwrap();
        let summary = projects.summary(&doc.id).unwrap();
        assert_eq!(summary.revision, 5);
        assert_eq!(summary.name, "改名");
        assert_eq!(
            projects.summary("ghost").unwrap_err().code,
            "project_not_found"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_descriptor_is_not_persisted() {
        let (dir, store, asset) = setup("exports");
        let projects = ProjectStore::new(&store, &dir);
        let created = projects.create(&asset).unwrap();
        let target = dir.join("out.png");
        let record = projects.export_record(&created.summary.id, &target, 64, 32);
        assert_eq!(record.filename, "out.png");
        assert_eq!((record.width, record.height), (64, 32));
        let rows: i64 = store
            .connection()
            .unwrap()
            .query_row("SELECT count(*) FROM workbench_exports", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0, "导出不再写入数据库");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
