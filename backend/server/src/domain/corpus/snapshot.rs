//! Immutable current-source snapshots prepared by a provider connector.

use serde::{Deserialize, Serialize};

pub struct Plan<'a> {
    pub collection: &'a str,
    pub generation: u32,
    /// ID of the publication observed when planning this run.
    pub predecessor: Option<i64>,
    pub revision: &'a str,
    pub tracked_ref: &'a str,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub id: i64,
    pub revision: String,
    pub tracked_ref: String,
    pub generation: u32,
    pub sequence: u32,
    pub cursor: Option<String>,
    pub phase: Phase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Staging,
    Sealed,
    Published,
}

pub struct File<'a> {
    pub source: &'a str,
    pub path: &'a str,
    pub blob: &'a str,
    pub file_url: &'a str,
    /// `None` reuses bytes already retained under this collection and blob ID.
    pub text: Option<&'a str>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub path: String,
    pub blob: String,
    pub file_url: String,
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: u32,
    pub end_line: u32,
}

impl Location {
    #[must_use]
    pub fn same_range(&self, other: &Self) -> bool {
        self.path == other.path
            && self.blob == other.blob
            && (
                self.start_byte,
                self.end_byte,
                self.start_line,
                self.end_line,
            ) == (
                other.start_byte,
                other.end_byte,
                other.start_line,
                other.end_line,
            )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SnapshotError {
    #[error("The source snapshot is stale or incomplete.")]
    Conflict,
    #[error("The collection's source access is unavailable.")]
    AccessLost,
    #[error("Invalid source snapshot data.")]
    Invalid,
    #[error("The source snapshot could not be stored.")]
    Storage,
}
