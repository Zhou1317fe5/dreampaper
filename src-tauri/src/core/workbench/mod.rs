//! The image workbench: non-destructive repair, text and crop on a single
//! raster image, with an authoritative Rust export.
//!
//! `WorkbenchService` is the one entry point the command layer uses. It owns
//! nothing itself; the long-lived pieces (decoded-source cache, font system,
//! OCR download state) live on `Core` and are borrowed per call.

pub mod asset;
pub mod color;
pub mod doc;
pub mod geom;
pub mod ocr;
pub mod project;
pub mod render;
pub mod sidecar;
pub mod source;
pub mod text;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::store::Store;
use crate::error::{AppError, AppResult};

use self::asset::{AssetStore, CleanupResult, WorkbenchAsset};
use self::color::RegionAnalysis;
use self::doc::{ProjectDoc, Shape};
use self::geom::PixelRect;
use self::project::{DeleteResult, ExportRecord, ProjectDetail, ProjectStore, ProjectSummary};
use self::render::ExportPreview;
use self::source::SourceCache;
use self::text::{FontHandle, FontInfo, TextLayout, TextSpec};

/// Where a new project's image comes from.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectSource {
    /// An image in the generation asset table (results page, history).
    Asset { asset_id: String },
    /// A file the user picked.
    File { path: String },
    /// Bytes uploaded from the webview.
    Bytes { filename: String, bytes: Vec<u8> },
    /// An existing workbench snapshot (new blank project on the same image).
    Snapshot { asset_id: String },
}

#[derive(Clone, Debug, Serialize)]
pub struct StorageStats {
    pub project_count: usize,
    pub project_bytes: u64,
    pub asset_count: usize,
    pub asset_bytes: u64,
    pub unreferenced_count: usize,
    pub unreferenced_bytes: u64,
}

/// Result of opening an image: either an existing project was resumed or a
/// new one was created.
#[derive(Clone, Debug, Serialize)]
pub struct OpenResult {
    pub detail: ProjectDetail,
    pub resumed: bool,
}

pub struct WorkbenchService<'a> {
    store: &'a Store,
    app_data: &'a Path,
    font_root: &'a Path,
    sources: &'a SourceCache,
    fonts: &'a FontHandle,
}

impl<'a> WorkbenchService<'a> {
    pub fn new(
        store: &'a Store,
        app_data: &'a Path,
        font_root: &'a Path,
        sources: &'a SourceCache,
        fonts: &'a FontHandle,
    ) -> Self {
        Self {
            store,
            app_data,
            font_root,
            sources,
            fonts,
        }
    }

    fn assets(&self) -> AssetStore<'a> {
        AssetStore::new(self.store, self.app_data)
    }

    fn projects(&self) -> ProjectStore<'a> {
        ProjectStore::new(self.store, self.app_data)
    }

    /// Runs at startup: make sure the directories exist and the index
    /// reflects the documents on disk.
    pub fn bootstrap(&self) -> AppResult<()> {
        std::fs::create_dir_all(self.assets().dir())?;
        std::fs::create_dir_all(self.projects().dir())?;
        self.projects().repair_index()
    }

    pub fn list_projects(&self) -> AppResult<Vec<ProjectSummary>> {
        self.projects().list()
    }

    /// Snapshot the source (deduplicated) and open the most recent project
    /// on it, or create the first one. `force_new` always creates.
    pub fn open(&self, source: ProjectSource, force_new: bool) -> AppResult<OpenResult> {
        let asset = self.snapshot(source)?;
        if !force_new {
            if let Some(latest) = self.projects().latest_for_asset(&asset.id)? {
                return Ok(OpenResult {
                    detail: self.projects().get(&latest.id)?,
                    resumed: true,
                });
            }
        }
        Ok(OpenResult {
            detail: self.projects().create(&asset)?,
            resumed: false,
        })
    }

    fn snapshot(&self, source: ProjectSource) -> AppResult<WorkbenchAsset> {
        let assets = self.assets();
        match source {
            ProjectSource::Asset { asset_id } => {
                let file = crate::core::asset::AssetService::new(self.store, self.app_data)
                    .asset_file(&asset_id)?;
                if !file.path.exists() {
                    return Err(AppError::new("asset_file_missing", "资源文件已不在磁盘上"));
                }
                let bytes = std::fs::read(&file.path)?;
                assets.snapshot_bytes(&file.filename, &bytes)
            }
            ProjectSource::File { path } => assets.snapshot_file(Path::new(&path)),
            ProjectSource::Bytes { filename, bytes } => assets.snapshot_bytes(&filename, &bytes),
            ProjectSource::Snapshot { asset_id } => assets.get(&asset_id),
        }
    }

    pub fn copy_project(&self, project_id: &str) -> AppResult<ProjectDetail> {
        self.projects().copy(project_id)
    }

    pub fn get_project(&self, project_id: &str) -> AppResult<ProjectDetail> {
        self.projects().get(project_id)
    }

    pub fn save_project(
        &self,
        project_id: &str,
        base_revision: u64,
        document: ProjectDoc,
    ) -> AppResult<ProjectDetail> {
        self.projects().save(project_id, base_revision, document)
    }

    pub fn rename_project(&self, project_id: &str, name: &str) -> AppResult<ProjectSummary> {
        self.projects().rename(project_id, name)
    }

    pub fn delete_project(&self, project_id: &str) -> AppResult<DeleteResult> {
        let summary = self.projects().summary(project_id)?;
        let result = self.projects().delete(project_id)?;
        if result.asset_unreferenced {
            self.sources.forget(&summary.asset_id);
        }
        Ok(result)
    }

    pub fn asset_file(&self, asset_id: &str) -> AppResult<(PathBuf, String)> {
        let asset = self.assets().get(asset_id)?;
        Ok((self.assets().file_path(&asset), asset.mime))
    }

    fn decoded(&self, asset_id: &str) -> AppResult<std::sync::Arc<source::DecodedSource>> {
        let asset = self.assets().get(asset_id)?;
        let path = self.assets().file_path(&asset);
        if !path.exists() {
            return Err(AppError::new(
                "workbench_asset_file_missing",
                "源图快照文件已丢失",
            ));
        }
        self.sources.get_or_decode(asset_id, &path)
    }

    pub fn analyze_region(
        &self,
        project_id: &str,
        rect: PixelRect,
        shape: Shape,
    ) -> AppResult<RegionAnalysis> {
        let summary = self.projects().summary(project_id)?;
        let source = self.decoded(&summary.asset_id)?;
        if rect.clamped(source.width, source.height).is_none() {
            return Err(AppError::new(
                "workbench_region_invalid",
                "选区不在图片范围内",
            ));
        }
        Ok(color::analyze(&source, &rect, shape))
    }

    /// The decoded source of a project together with `rect` clamped to it,
    /// for callers that need the pixels themselves (OCR crops).
    pub fn region_source(
        &self,
        project_id: &str,
        rect: PixelRect,
    ) -> AppResult<(std::sync::Arc<source::DecodedSource>, PixelRect)> {
        let summary = self.projects().summary(project_id)?;
        let source = self.decoded(&summary.asset_id)?;
        let rect = rect
            .clamped(source.width, source.height)
            .ok_or_else(|| AppError::new("workbench_region_invalid", "选区不在图片范围内"))?;
        Ok((source, rect))
    }

    pub fn measure_text(&self, spec: TextSpec) -> AppResult<TextLayout> {
        self.fonts
            .with(self.font_root, |fonts| fonts.measure(&spec))
    }

    pub fn list_fonts(&self, sample: Option<&str>) -> Vec<FontInfo> {
        self.fonts.with(self.font_root, |fonts| fonts.list(sample))
    }

    pub fn export_preview(
        &self,
        project_id: &str,
        document: &ProjectDoc,
    ) -> AppResult<ExportPreview> {
        let summary = self.projects().summary(project_id)?;
        let source = self.decoded(&summary.asset_id)?;
        self.fonts.with(self.font_root, |fonts| {
            render::preview(&source, document, fonts)
        })
    }

    /// Compose and write the PNG, then record where it went. The document
    /// must already be saved at `base_revision`; exporting unsaved state
    /// would let the file and the project disagree after a crash.
    pub fn export(
        &self,
        project_id: &str,
        document: &ProjectDoc,
        target: &Path,
    ) -> AppResult<ExportRecord> {
        let summary = self.projects().summary(project_id)?;
        let saved = self.projects().get(project_id)?.document;
        if document.revision != saved.revision
            || serde_json::to_value(document)? != serde_json::to_value(&saved)?
        {
            return Err(AppError::with_detail(
                "project_unsaved",
                "工程尚未保存，请先保存再导出",
                serde_json::json!({ "current": summary.revision, "document": document.revision }),
            ));
        }
        if target
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("png"))
            != Some(true)
        {
            return Err(AppError::new(
                "workbench_export_not_png",
                "工作台只导出 PNG 文件",
            ));
        }
        let source_path = self.asset_file(&summary.asset_id)?.0;
        if same_file(&source_path, target) {
            return Err(AppError::new(
                "workbench_export_overwrites_source",
                "不能覆盖源图文件",
            ));
        }
        let source = self.decoded(&summary.asset_id)?;
        let composite = self.fonts.with(self.font_root, |fonts| {
            render::compose(&source, document, fonts)
        })?;
        render::write_png(&composite, &source, target)?;
        Ok(self
            .projects()
            .export_record(project_id, target, composite.width, composite.height))
    }

    pub fn storage(&self) -> AppResult<StorageStats> {
        let (project_count, project_bytes) = self.projects().usage()?;
        let (asset_count, asset_bytes, unreferenced_count, unreferenced_bytes) =
            self.assets().usage()?;
        Ok(StorageStats {
            project_count,
            project_bytes,
            asset_count,
            asset_bytes,
            unreferenced_count,
            unreferenced_bytes,
        })
    }

    pub fn cleanup(&self) -> AppResult<CleanupResult> {
        let result = self.assets().cleanup_unreferenced()?;
        Ok(result)
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::asset::tests::{png_bytes, temp_dir};
    use super::*;

    fn service_parts(tag: &str) -> (PathBuf, Store, SourceCache, FontHandle) {
        let dir = temp_dir(tag);
        let store = Store::initialize(&dir).unwrap();
        (dir, store, SourceCache::default(), FontHandle::default())
    }

    #[test]
    fn open_resumes_the_latest_project_on_the_same_image() {
        let (dir, store, sources, fonts) = service_parts("open");
        let font_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fonts");
        let service = WorkbenchService::new(&store, &dir, &font_root, &sources, &fonts);
        service.bootstrap().unwrap();
        let bytes = png_bytes(30, 20, [255, 255, 255, 255]);
        let first = service
            .open(
                ProjectSource::Bytes {
                    filename: "a.png".into(),
                    bytes: bytes.clone(),
                },
                false,
            )
            .unwrap();
        assert!(!first.resumed);
        let again = service
            .open(
                ProjectSource::Bytes {
                    filename: "a.png".into(),
                    bytes: bytes.clone(),
                },
                false,
            )
            .unwrap();
        assert!(again.resumed);
        assert_eq!(again.detail.summary.id, first.detail.summary.id);
        let fresh = service
            .open(
                ProjectSource::Snapshot {
                    asset_id: first.detail.summary.asset_id.clone(),
                },
                true,
            )
            .unwrap();
        assert!(!fresh.resumed);
        assert_ne!(fresh.detail.summary.id, first.detail.summary.id);
        assert_eq!(service.storage().unwrap().asset_count, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_requires_a_saved_document_and_png_target() {
        let (dir, store, sources, fonts) = service_parts("export");
        let font_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fonts");
        let service = WorkbenchService::new(&store, &dir, &font_root, &sources, &fonts);
        service.bootstrap().unwrap();
        let opened = service
            .open(
                ProjectSource::Bytes {
                    filename: "a.png".into(),
                    bytes: png_bytes(30, 20, [9, 9, 9, 255]),
                },
                false,
            )
            .unwrap();
        let id = opened.detail.summary.id.clone();
        let mut doc = opened.detail.document.clone();
        doc.viewport.crop = PixelRect::new(5, 5, 10, 8);
        let unsaved = service.export(&id, &doc, &dir.join("out.png")).unwrap_err();
        assert_eq!(unsaved.code, "project_unsaved");

        let saved = service.save_project(&id, 1, doc).unwrap();
        let not_png = service
            .export(&id, &saved.document, &dir.join("out.jpg"))
            .unwrap_err();
        assert_eq!(not_png.code, "workbench_export_not_png");
        let record = service
            .export(&id, &saved.document, &dir.join("out.png"))
            .unwrap();
        assert_eq!((record.width, record.height), (10, 8));
        assert!(record.available);
        let decoded = image::open(&record.path).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (10, 8));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn region_analysis_reads_the_snapshot() {
        let (dir, store, sources, fonts) = service_parts("analyze");
        let font_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fonts");
        let service = WorkbenchService::new(&store, &dir, &font_root, &sources, &fonts);
        service.bootstrap().unwrap();
        let opened = service
            .open(
                ProjectSource::Bytes {
                    filename: "a.png".into(),
                    bytes: png_bytes(30, 20, [10, 200, 30, 255]),
                },
                false,
            )
            .unwrap();
        let id = opened.detail.summary.id.clone();
        let analysis = service
            .analyze_region(&id, PixelRect::new(2, 2, 10, 10), Shape::Rect)
            .unwrap();
        assert_eq!(analysis.color, "#0ac81e");
        assert_eq!(
            service
                .analyze_region(&id, PixelRect::new(100, 100, 5, 5), Shape::Rect)
                .unwrap_err()
                .code,
            "workbench_region_invalid"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
