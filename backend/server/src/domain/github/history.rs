//! A bounded, immutable first-parent view of a tracked branch.

use serde::{Deserialize, Serialize};

pub const MAX_COMMITS: usize = 16;

#[derive(Eq, PartialEq)]
pub struct Scope {
    pub repository_id: i64,
    pub full_name: String,
    pub branch: String,
    pub generation: i64,
    pub credential: String,
}

pub struct Pin {
    pub commit: String,
    pub tree: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub id: String,
    pub parents: Vec<String>,
    pub tree: String,
    pub summary: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Boundary {
    Root,
    CommitLimit,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub branch: String,
    pub tip: String,
    pub boundary: Boundary,
    pub commits: Vec<Commit>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<super::Schedule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Snapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_trace: Option<crate::domain::telemetry::TraceLink>,
}

pub mod comparison;

pub mod hunks;

/// Immutable materialized history for one active lineage comparison.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEvidence {
    pub repository_id: i64,
    pub repository: String,
    pub branch: String,
    pub child: String,
    pub parent: Option<String>,
    pub baseline: String,
    pub coverage: String,
    pub projection_version: u32,
    pub changes: Vec<EvidenceChange>,
}

#[derive(Deserialize, Serialize)]
pub struct EvidenceChange {
    pub change: comparison::Change,
    pub projection: hunks::Projection,
}
