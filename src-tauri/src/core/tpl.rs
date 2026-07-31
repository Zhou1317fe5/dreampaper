use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::params;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

use super::asset::{guess_mime, sanitize_filename};
use super::store::Store;

#[derive(Clone, Debug, Serialize)]
pub struct TemplateSummary {
    pub id: String,
    pub source_id: String,
    pub kind: String,
    pub category: Option<String>,
    pub rounded_ratio: Option<String>,
    pub visual_intent: String,
    pub content_summary: String,
    pub image_url: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct TemplatePackSummary {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub template_count: i64,
}

#[derive(Clone, Debug)]
pub struct TemplateFile {
    pub id: String,
    pub mime_type: String,
    pub path: PathBuf,
}

pub struct TemplateService<'a> {
    store: &'a Store,
    app_data: &'a Path,
}

impl<'a> TemplateService<'a> {
    pub fn new(store: &'a Store, app_data: &'a Path) -> Self {
        Self { store, app_data }
    }

    pub fn list_templates(&self, kind: String, query: String) -> AppResult<Vec<TemplateSummary>> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, source_id, kind, category, rounded_ratio, visual_intent, content_summary, image_path \
             FROM templates ORDER BY created_at DESC LIMIT 200",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            Ok(TemplateSummary {
                image_url: format!("dp-template://localhost/{id}"),
                id,
                source_id: row.get(1)?,
                kind: row.get(2)?,
                category: row.get(3)?,
                rounded_ratio: row.get(4)?,
                visual_intent: row.get(5)?,
                content_summary: row.get(6)?,
            })
        })?;

        let query = query.trim().to_lowercase();
        let templates = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(templates
            .into_iter()
            .filter(|template| kind == "all" || kind.trim().is_empty() || template.kind == kind)
            .filter(|template| {
                if query.is_empty() {
                    return true;
                }
                template.visual_intent.to_lowercase().contains(&query)
                    || template.content_summary.to_lowercase().contains(&query)
                    || template
                        .category
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query)
            })
            .take(60)
            .collect())
    }

    pub fn import_template_image(
        &self,
        filename: String,
        mime_type: String,
        bytes: Vec<u8>,
        kind: String,
        category: Option<String>,
        visual_intent: Option<String>,
        content_summary: Option<String>,
    ) -> AppResult<TemplateSummary> {
        let kind = normalize_kind(&kind);
        let id = Uuid::new_v4().to_string();
        let safe_name = sanitize_filename(&filename);
        let suffix = Path::new(&safe_name)
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| format!(".{value}"))
            .unwrap_or_else(|| ".png".to_string());
        let dir = self.app_data.join("templates").join(&id);
        std::fs::create_dir_all(&dir)?;
        let image_path = dir.join(format!("image{suffix}"));
        std::fs::write(&image_path, bytes)?;
        let detected_mime = if mime_type.trim().is_empty() || mime_type == "application/octet-stream" {
            guess_mime(&image_path)
        } else {
            mime_type
        };
        if !detected_mime.starts_with("image/") {
            return Err(AppError::new(
                "template_mime_unsupported",
                "Template image must be png, jpeg, webp, gif, or svg",
            ));
        }
        let visual_intent = visual_intent
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("User imported {kind} visual reference: {filename}"));
        let content_summary = content_summary
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "Imported single template image. Use it as a style/layout reference, not as a background to copy.".to_string());
        let now = Utc::now().to_rfc3339();
        let conn = self.store.connection()?;
        conn.execute(
            "INSERT INTO templates(id, pack_id, source_id, kind, category, rounded_ratio, visual_intent, content_summary, image_path, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                Option::<String>::None,
                filename,
                kind,
                category,
                Option::<String>::None,
                visual_intent,
                content_summary,
                image_path.to_string_lossy().to_string(),
                now
            ],
        )?;
        Ok(TemplateSummary {
            image_url: format!("dp-template://localhost/{id}"),
            id,
            source_id: filename,
            kind,
            category,
            rounded_ratio: None,
            visual_intent,
            content_summary,
        })
    }

    pub fn import_template_pack(&self, path: String) -> AppResult<TemplatePackSummary> {
        let source = Path::new(&path);
        if !source.exists() {
            return Err(AppError::new(
                "template_pack_not_found",
                "Template pack path does not exist",
            ));
        }
        if !source.is_dir() {
            return Err(AppError::new(
                "template_pack_unsupported",
                "Desktop template pack import expects an extracted directory path",
            ));
        }
        let id = Uuid::new_v4().to_string();
        let name = source
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("template_pack")
            .to_string();
        let target_dir = self.app_data.join("templates").join(&id);
        std::fs::create_dir_all(&target_dir)?;
        let conn = self.store.connection()?;
        conn.execute(
            "INSERT INTO template_packs(id, name, version, source_path, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, name, Option::<String>::None, path, Utc::now().to_rfc3339()],
        )?;
        let mut count = 0_i64;
        for (kind, manifest) in manifests(source) {
            count += self.import_manifest_entries(&id, &target_dir, source, &kind, &manifest)?;
        }
        Ok(TemplatePackSummary {
            id,
            name,
            version: None,
            template_count: count,
        })
    }

    pub fn template_file(&self, id: &str) -> AppResult<TemplateFile> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare("SELECT id, image_path FROM templates WHERE id = ?1")?;
        stmt.query_row(params![id], |row| {
            let path = PathBuf::from(row.get::<_, String>(1)?);
            Ok(TemplateFile {
                id: row.get(0)?,
                mime_type: guess_mime(&path),
                path,
            })
        })
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => {
                AppError::new("template_not_found", "Template not found")
            }
            other => other.into(),
        })
    }

    fn import_manifest_entries(
        &self,
        pack_id: &str,
        target_dir: &Path,
        source_dir: &Path,
        fallback_kind: &str,
        manifest_path: &Path,
    ) -> AppResult<i64> {
        let data = std::fs::read_to_string(manifest_path)?;
        let entries = serde_json::from_str::<Value>(&data)?;
        let Some(items) = entries.as_array() else {
            return Ok(0);
        };
        let mut count = 0_i64;
        let conn = self.store.connection()?;
        for item in items {
            let raw_id = string_field(item, "id").unwrap_or_else(|| Uuid::new_v4().to_string());
            let kind = normalize_kind(&string_field(item, "kind").unwrap_or_else(|| fallback_kind.to_string()));
            let Some(relative_image) = string_field(item, "path_to_gt_image")
                .or_else(|| string_field(item, "image_path"))
                .or_else(|| string_field(item, "image"))
            else {
                continue;
            };
            let image_source = source_dir.join(&kind).join(&relative_image);
            let image_source = if image_source.exists() {
                image_source
            } else {
                source_dir.join(&relative_image)
            };
            if !image_source.exists() {
                continue;
            }
            let template_id = Uuid::new_v4().to_string();
            let suffix = image_source
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| format!(".{value}"))
                .unwrap_or_else(|| ".png".to_string());
            let image_target = target_dir.join(format!("{template_id}{suffix}"));
            std::fs::copy(&image_source, &image_target)?;
            let content_summary = item
                .get("content")
                .map(|value| value.to_string())
                .or_else(|| string_field(item, "content_summary"))
                .unwrap_or_default();
            conn.execute(
                "INSERT INTO templates(id, pack_id, source_id, kind, category, rounded_ratio, visual_intent, content_summary, image_path, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    template_id,
                    pack_id,
                    raw_id,
                    kind,
                    string_field(item, "category").or_else(|| string_field(item, "original_category")),
                    item.get("additional_info")
                        .and_then(|info| info.get("rounded_ratio"))
                        .and_then(Value::as_str),
                    string_field(item, "visual_intent").unwrap_or_default(),
                    content_summary,
                    image_target.to_string_lossy().to_string(),
                    Utc::now().to_rfc3339(),
                ],
            )?;
            count += 1;
        }
        Ok(count)
    }
}

fn manifests(source: &Path) -> Vec<(String, PathBuf)> {
    [
        ("diagram".to_string(), source.join("diagram").join("ref.json")),
        ("plot".to_string(), source.join("plot").join("ref.json")),
        ("diagram".to_string(), source.join("ref.json")),
    ]
    .into_iter()
    .filter(|(_, path)| path.exists())
    .collect()
}

fn normalize_kind(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "plot" => "plot".to_string(),
        _ => "diagram".to_string(),
    }
}

fn string_field(item: &Value, key: &str) -> Option<String> {
    item.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
