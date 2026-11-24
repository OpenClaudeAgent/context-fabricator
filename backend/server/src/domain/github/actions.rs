//! Bounded workflow and run observations for one tracked branch.
use serde::{Deserialize, Serialize};

const MAX_RETAINED_ATTEMPTS: usize = 500;

#[derive(Clone, Eq, PartialEq)]
pub struct Scope {
    pub repository_id: i64,
    pub full_name: String,
    pub branch: String,
    pub generation: i64,
    pub credential: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub state: String,
    pub definition_blob: Option<String>,
    pub definition: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: i64,
    pub attempt: u32,
    pub workflow_id: i64,
    pub event: String,
    pub head_sha: String,
    pub head_branch: Option<String>,
    pub head_repository_id: Option<i64>,
    pub commit_provenance: String,
    pub created_at: String,
    pub updated_at: String,
    pub status: String,
    pub conclusion: Option<String>,
}

/// Keep the newest observations. Input and output use (run ID, attempt) order.
pub(crate) fn retain_recent_attempts(mut attempts: Vec<Run>) -> Vec<Run> {
    if attempts.len() > MAX_RETAINED_ATTEMPTS {
        attempts.sort_unstable_by(|left, right| {
            (&right.updated_at, right.id, right.attempt).cmp(&(
                &left.updated_at,
                left.id,
                left.attempt,
            ))
        });
        attempts.truncate(MAX_RETAINED_ATTEMPTS);
        attempts.sort_unstable_by_key(|run| (run.id, run.attempt));
    }
    attempts
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub repository_id: i64,
    pub repository: String,
    pub branch: String,
    pub definition_commit: String,
    pub workflows: Vec<Workflow>,
    pub workflow_coverage: String,
    pub executed_definition: String,
    pub runs: Vec<Run>,
    pub run_coverage: String,
    pub attempt_coverage: String,
}

#[derive(Serialize)]
pub struct State {
    pub availability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Snapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<Schedule>,
}

pub use super::{Schedule, ScheduleDue as Due};
