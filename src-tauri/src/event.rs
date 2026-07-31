use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct JobEventPayload {
    pub job_id: String,
    pub status: String,
    pub stage: String,
    pub message: String,
    pub timestamp: DateTime<Utc>,
}
