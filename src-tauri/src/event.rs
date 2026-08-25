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

/// What one design-model call reports about itself while it runs.
///
/// The pipeline talks in these; `execute` turns them into `job://design`
/// payloads and, on `End`, a row the job can be reopened from. A step is
/// identified by the same string the progress bar uses as its stage, so the UI
/// can highlight the card belonging to the current step without a second map.
pub enum DesignLog<'a> {
    /// The call started. The UI adds an empty card for it.
    Begin { step: &'a str, label: &'a str },
    /// Streamed text, appended in arrival order.
    Delta { step: &'a str, text: &'a str },
    /// Everything streamed for this step so far is void. Sent when a transport
    /// retry or a JSON repair round restarts the same call — without it the card
    /// would show two attempts concatenated.
    Reset { step: &'a str },
    /// The call settled. `text` is authoritative and replaces whatever streamed,
    /// which matters when the deltas and the collected result disagree (an
    /// upstream that only sends a terminal `response.completed`, say) and when
    /// nothing streamed at all because the profile has streaming off.
    End {
        step: &'a str,
        label: &'a str,
        text: &'a str,
        ok: bool,
    },
}

pub type DesignSink<'a> = &'a (dyn Fn(DesignLog<'_>) + Send + Sync);

#[derive(Clone, Debug, Serialize)]
pub struct DesignLogPayload {
    pub job_id: String,
    /// `begin` | `delta` | `reset` | `end`
    pub kind: String,
    pub step: String,
    pub label: String,
    pub text: String,
    pub status: String,
    pub timestamp: DateTime<Utc>,
}
