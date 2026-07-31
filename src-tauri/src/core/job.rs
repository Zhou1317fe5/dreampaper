use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

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
        Ok(record)
    }

    pub fn list_jobs(&self, limit: usize, offset: usize) -> AppResult<Vec<JobRecord>> {
        let conn = self.store.connection()?;
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
        }
        Ok(records)
    }

    fn events_for_job(&self, job_id: &str) -> AppResult<Vec<JobEvent>> {
        let conn = self.store.connection()?;
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
