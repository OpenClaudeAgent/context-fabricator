//! Opt-in local journal of completed, verified answers.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{Failure, answers::Answer};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub revision: u32,
    pub enabled: bool,
    pub retention_days: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            revision: 0,
            enabled: false,
            retention_days: 30,
        }
    }
}

impl Settings {
    /// # Errors
    /// Returns an error if retention is outside 1 to 365 days.
    pub fn validate(&self) -> Result<(), Failure> {
        if !(1..=365).contains(&self.retention_days) {
            return Err(Failure::InvalidAnswerHistoryRetention);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actions_run_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actions_run_attempt: Option<u32>,
}

pub struct Capture {
    pub(crate) epoch: i64,
    pub(crate) purge_sequence: i64,
}
pub struct Record<'a> {
    pub question: &'a str,
    pub scope: &'a Scope,
    pub answer: &'a Answer,
    pub completed_at_ms: i64,
    pub trace_id: Option<Uuid>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: i64,
    pub question: String,
    pub scope: Scope,
    pub completed_at_ms: i64,
    pub expires_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    pub answer: serde_json::Value,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub id: i64,
    pub question: String,
    pub completed_at_ms: i64,
    pub outcome: String,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub entries: Vec<Summary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_before_id: Option<i64>,
}

pub trait Journal: Send + Sync {
    /// # Errors
    /// Returns an error if settings cannot be read.
    fn settings(&self) -> Result<Settings, Failure>;
    /// # Errors
    /// Returns an error for invalid settings, a revision conflict, or unavailable storage.
    fn update(&self, input: Settings, now_ms: i64) -> Result<Settings, Failure>;
    /// # Errors
    /// Returns an error if the capture guard cannot be read.
    fn begin(&self) -> Result<Option<Capture>, Failure>;
    /// # Errors
    /// Returns an error if the completed answer cannot be stored.
    fn save(&self, capture: Capture, record: Record<'_>) -> Result<(), Failure>;
    /// # Errors
    /// Returns an error if a page cannot be read.
    fn list(&self, before_id: Option<i64>, now_ms: i64) -> Result<Page, Failure>;
    /// # Errors
    /// Returns an error if the entry cannot be read or decoded.
    fn get(&self, id: i64, now_ms: i64) -> Result<Option<Entry>, Failure>;
    /// # Errors
    /// Returns an error if the entry for the execution cannot be read or decoded.
    fn get_by_trace(&self, trace_id: Uuid, now_ms: i64) -> Result<Option<Entry>, Failure>;
    /// # Errors
    /// Returns an error if history cannot be cleared.
    fn clear(&self) -> Result<(), Failure>;
    /// # Errors
    /// Returns an error if expired history cannot be removed.
    fn prune(&self, now_ms: i64) -> Result<(), Failure>;
}
