//! Local navigation over an explicitly selected repository, branch and Rust source root.
use crate::domain::{
    corpus::snapshot::Location,
    github::{Error, valid_branch},
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub branch: String,
    pub root: Option<String>,
    pub symbol: Option<String>,
}

impl Query {
    pub(super) fn valid(&self) -> bool {
        valid_branch(&self.branch)
            && match (&self.root, &self.symbol) {
                (None, None) => true,
                (Some(root), Some(symbol)) => {
                    root.len() <= 1024
                        && root.split('/').all(|p| !matches!(p, "" | "." | ".."))
                        && !root.chars().any(char::is_control)
                        && !symbol.is_empty()
                        && symbol.len() <= 256
                        && symbol
                            .chars()
                            .all(|c| c.is_alphanumeric() || matches!(c, '_' | ':'))
                }
                _ => false,
            }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub repository_id: i64,
    pub branch: String,
    pub access: &'static str,
    pub snapshot: Option<Snapshot>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub revision: String,
    pub profile: String,
    pub coverage: String,
    pub roots: Vec<Root>,
    pub hits: Vec<Hit>,
    pub truncated: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    pub path: String,
    pub parsed_files: usize,
    pub reachable_files: usize,
    pub unsupported_files: usize,
    pub parse_errors: usize,
    pub limited_files: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    pub role: String,
    pub resolution: String,
    pub site: Location,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Location>,
}

pub trait Store: Send + Sync {
    /// # Errors
    /// Returns a storage error or rejects an unconfigured repository.
    fn graph(&self, repository: i64, query: &Query) -> Result<State, Error>;
}
