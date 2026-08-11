use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

use super::asset::protocol_url;
use super::store::Store;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobImage {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobEvent {
    pub stage: String,
    pub message: String,
    pub status: String,
    pub timestamp: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobRecord {
    pub id: String,
    pub mode: String,
    pub status: String,
    pub message: Option<String>,
    pub stage: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub images: Vec<JobImage>,
    pub events: Vec<JobEvent>,
}

pub struct JobService<'a> {
    store: &'a Store,
}

impl<'a> JobService<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn create_job(&self, payload: serde_json::Value) -> AppResult<JobRecord> {
        let mode = payload
            .get("mode")
            .and_then(|value| value.as_str())
            .unwrap_or("paper_figure")
            .to_string();
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let payload_json = serde_json::to_string(&payload)?;
        let conn = self.store.connection()?;
        conn.execute(
            "INSERT INTO jobs(id, mode, status, payload_json, message, stage, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, mode, "queued", payload_json, "任务已排队", "queued", now, now],
        )?;
        conn.execute(
            "INSERT INTO job_stages(job_id, stage, message, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, "queued", "任务已排队", "queued", now],
        )?;
        self.get_job(id)
    }

    pub fn get_job(&self, id: String) -> AppResult<JobRecord> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, mode, status, message, stage, created_at, updated_at FROM jobs WHERE id = ?1",
        )?;
        let mut record = stmt
            .query_row(params![id], |row| {
                Ok(JobRecord {
                    id: row.get(0)?,
                    mode: row.get(1)?,
                    status: row.get(2)?,
                    message: row.get(3)?,
                    stage: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                    images: Vec::new(),
                    events: Vec::new(),
                })
            })
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    AppError::new("job_not_found", "Job not found")
                }
                other => other.into(),
            })?;
        record.events = self.events_for_job(&record.id)?;
        record.images = self.images_for_job(&record.id)?;
        Ok(record)
    }

    pub fn list_jobs(&self, limit: usize, offset: usize) -> AppResult<Vec<JobRecord>> {        let conn = self.store.connection()?;
        let mut stmt = conn.prepare(
            "SELECT id, mode, status, message, stage, created_at, updated_at FROM jobs ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit as i64, offset as i64], |row| {
            Ok(JobRecord {
                id: row.get(0)?,
                mode: row.get(1)?,
                status: row.get(2)?,
                message: row.get(3)?,
                stage: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                images: Vec::new(),
                events: Vec::new(),
            })
        })?;
        let mut records = rows.collect::<Result<Vec<_>, _>>()?;
        for record in &mut records {
            record.events = self.events_for_job(&record.id)?;
            record.images = self.images_for_job(&record.id)?;
        }
        Ok(records)
    }

    pub fn mark_stage(&self, job_id: &str, stage: &str, message: &str, status: &str) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.store.connection()?;
        conn.execute(
            "INSERT INTO job_stages(job_id, stage, message, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![job_id, stage, message, status, now],
        )?;
        let job_status = match status {
            "succeeded" | "failed" | "cancelled" => status,
            _ => "running",
        };
        conn.execute(
            "UPDATE jobs SET status = ?1, stage = ?2, message = ?3, updated_at = ?4 \
             WHERE id = ?5 AND status NOT IN ('succeeded', 'failed', 'cancelled')",
            params![job_status, stage, message, now, job_id],
        )?;
        Ok(())
    }

    pub fn payload(&self, job_id: &str) -> AppResult<serde_json::Value> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare("SELECT payload_json FROM jobs WHERE id = ?1")?;
        let raw: String = stmt
            .query_row(params![job_id], |row| row.get(0))
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => AppError::new("job_not_found", "Job not found"),
                other => other.into(),
            })?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn finish(&self, job_id: &str, images: &[JobImage]) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        let result = serde_json::json!({ "images": images });
        let conn = self.store.connection()?;
        conn.execute(
            "UPDATE jobs SET status = 'succeeded', stage = 'completed', message = '任务完成', \
             result_json = ?1, updated_at = ?2 \
             WHERE id = ?3 AND status NOT IN ('succeeded', 'failed', 'cancelled')",
            params![serde_json::to_string(&result)?, now, job_id],
        )?;
        drop(conn);
        self.mark_stage(job_id, "completed", "任务完成", "succeeded")
    }

    pub fn fail(&self, job_id: &str, message: &str) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        let error = serde_json::json!({ "message": message, "failed_at": now });
        let conn = self.store.connection()?;
        conn.execute(
            "UPDATE jobs SET error_json = ?1, updated_at = ?2 WHERE id = ?3",
            params![serde_json::to_string(&error)?, now, job_id],
        )?;
        drop(conn);
        self.mark_stage(job_id, "failed", message, "failed")
    }

    pub fn cancel(&self, job_id: &str, message: &str) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        let error = serde_json::json!({ "message": message, "cancelled_at": now });
        let conn = self.store.connection()?;
        conn.execute(
            "UPDATE jobs SET error_json = ?1, updated_at = ?2 WHERE id = ?3",
            params![serde_json::to_string(&error)?, now, job_id],
        )?;
        drop(conn);
        self.mark_stage(job_id, "cancelled", message, "cancelled")
    }

    fn images_for_job(&self, job_id: &str) -> AppResult<Vec<JobImage>> {
        let conn = self.store.connection()?;
        let mut stmt = conn.prepare("SELECT result_json FROM jobs WHERE id = ?1")?;
        let raw: Option<String> = stmt
            .query_row(params![job_id], |row| row.get(0))
            .unwrap_or(None);
        let Some(raw) = raw else {
            return Ok(Vec::new());
        };
        let value: serde_json::Value = serde_json::from_str(&raw)?;
        let mut images: Vec<JobImage> =
            serde_json::from_value(value["images"].clone()).unwrap_or_default();
        for image in &mut images {
            image.url = normalize_asset_url(&image.url);
        }
        Ok(images)
    }

    fn events_for_job(&self, job_id: &str) -> AppResult<Vec<JobEvent>> {        let conn = self.store.connection()?;
        let mut stmt = conn.prepare(
            "SELECT stage, message, status, created_at FROM job_stages WHERE job_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![job_id], |row| {
            Ok(JobEvent {
                stage: row.get(0)?,
                message: row.get(1)?,
                status: row.get(2)?,
                timestamp: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

/// Rebuild a stored output URL for the current platform.
///
/// `result_json` stores the full URL as built at generation time, and the URL
/// form is platform-bound (see `asset::protocol_url`). Fixing only the
/// generating side is not enough: jobs a Windows user ran before this fix, and
/// app data moved over from macOS, would keep showing broken images under
/// recent jobs. Resources are stored by id, so recovering the id from the URL
/// and rebuilding is enough — no database migration needed.
fn normalize_asset_url(url: &str) -> String {
    if !url.contains("dp-asset") {
        return url.to_string();
    }
    match url.rsplit('/').next() {
        Some(id) if !id.is_empty() => protocol_url("dp-asset", id),
        _ => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Old jobs hold the other platform's URL form; reads have to rebuild it
    /// into one this machine can load, or images under recent jobs stay broken
    /// after an upgrade. Non-dp-asset URLs pass through untouched.
    #[test]
    fn stored_image_urls_are_rebuilt_for_this_platform() {
        let expected = protocol_url("dp-asset", "abc-123");
        assert_eq!(normalize_asset_url("dp-asset://localhost/abc-123"), expected);
        assert_eq!(
            normalize_asset_url("http://dp-asset.localhost/abc-123"),
            expected
        );
        assert_eq!(
            normalize_asset_url("https://example.com/x.png"),
            "https://example.com/x.png",
            "external image links must not be rewritten"
        );
    }

    fn service_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dreampaper-job-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    #[test]
    fn a_cancelled_job_is_not_resurrected_by_late_reports() {
        let dir = service_dir("cancel");
        let store = Store::initialize(&dir).expect("初始化 store");
        let jobs = JobService::new(&store);
        let job = jobs
            .create_job(serde_json::json!({ "mode": "paper_figure" }))
            .expect("建任务");

        jobs.mark_stage(&job.id, "paper_design", "设计中", "running")
            .expect("上报阶段");
        jobs.cancel(&job.id, "任务已停止").expect("停止");

        jobs.mark_stage(&job.id, "paper_implement", "制图中", "running")
            .expect("迟到的上报");
        jobs.finish(
            &job.id,
            &[JobImage {
                name: "figure.png".to_string(),
                url: "dp-asset://x".to_string(),
            }],
        )
        .expect("迟到的收尾");

        let after = jobs.get_job(job.id.clone()).expect("读任务");
        assert_eq!(after.status, "cancelled");
        assert_eq!(after.stage.as_deref(), Some("cancelled"));
        assert!(
            after.images.is_empty(),
            "停止之后不该再把产出挂回这条任务"
        );
    }
}
