pub mod answers;
pub mod credentials;
pub mod github;
pub mod ports;
pub mod retrieval;
pub mod sync;
pub mod telemetry;

use crate::domain::corpus::snapshot::Location;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnswerLanguage {
    #[default]
    Auto,
    En,
    Fr,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub revision: u32,
    pub answer_language: AnswerLanguage,
    pub diagnostic_retention_hours: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_storage_limit_mb: Option<u16>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub status: &'static str,
    pub api_version: u8,
    pub connected_sources: u64,
    pub collected_documents: i64,
    pub indexed_documents: i64,
    pub outbound_attempts: i64,
    pub outbound_pending: i64,
    pub outbound_uncertain: i64,
    pub inference: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retrieval: Option<retrieval::Progress>,
}

#[derive(Debug, Serialize)]
pub struct Diagnostic {
    pub id: i64,
    pub timestamp: i64,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub source: String,
    pub revision: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracked_ref: Option<String>,
    pub title: String,
    pub access: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
}

impl SearchHit {
    #[must_use]
    pub fn same_evidence(&self, other: &Self) -> bool {
        self.source == other.source
            && self.revision == other.revision
            && self.kind == other.kind
            && self.tracked_ref == other.tracked_ref
            && match (&self.location, &other.location) {
                (Some(left), Some(right)) => left.same_range(right),
                (None, None) => true,
                _ => false,
            }
    }

    #[must_use]
    pub fn passage_start(&self) -> Option<u32> {
        self.location.as_ref().map(|location| location.start_byte)
    }
}

#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub enum Failure {
    #[error("The local data store is unavailable.")]
    Storage,
    #[error("Settings changed. Reload the latest revision and try again.")]
    Conflict,
    #[error("Diagnostic retention must be between 1 and 168 hours.")]
    InvalidRetention,
    #[error("Diagnostic storage limit must be between 2 and 1000 MB.")]
    InvalidStorageLimit,
    #[error("Search text must contain 1 to 512 characters.")]
    InvalidQuery,
    #[error("Supply a positive GitHub repository ID and its branch name together.")]
    InvalidSearchScope,
}

impl Settings {
    /// # Errors
    /// Returns an invalid settings error when retention or storage is outside its supported range.
    pub fn validate(&self) -> Result<(), Failure> {
        if !(1..=168).contains(&self.diagnostic_retention_hours) {
            return Err(Failure::InvalidRetention);
        }
        if self
            .diagnostic_storage_limit_mb
            .is_some_and(|limit| !(2..=1_000).contains(&limit))
        {
            return Err(Failure::InvalidStorageLimit);
        }
        Ok(())
    }
}

/// Validated literal terms for a lexical query.
#[derive(Debug, Eq, PartialEq)]
pub struct SearchQuery {
    terms: Vec<String>,
    original: String,
    scope: Option<CodeScope>,
}

/// A persisted GitHub current-code ownership scope.
#[derive(Debug, Eq, PartialEq)]
pub struct CodeScope {
    pub repository_id: i64,
    pub branch: String,
}

impl SearchQuery {
    /// # Errors
    /// Returns an invalid query error when there is no searchable term or the input is too long.
    pub fn parse(query: &str) -> Result<Self, Failure> {
        Self::parse_scoped(query, None, None)
    }

    /// # Errors
    /// Rejects invalid query text or an incomplete repository and branch pair.
    pub fn parse_scoped(
        query: &str,
        repository_id: Option<i64>,
        branch: Option<String>,
    ) -> Result<Self, Failure> {
        let scope = match (repository_id, branch) {
            (None, None) => None,
            (Some(repository_id), Some(branch))
                if repository_id > 0 && crate::domain::github::valid_branch(&branch) =>
            {
                Some(CodeScope {
                    repository_id,
                    branch,
                })
            }
            _ => return Err(Failure::InvalidSearchScope),
        };
        if query.chars().count() > 512 {
            return Err(Failure::InvalidQuery);
        }
        let terms: Vec<_> = query
            .split(|character: char| !character.is_alphanumeric())
            .filter(|term| !term.is_empty())
            .map(str::to_owned)
            .collect();
        if terms.is_empty() {
            return Err(Failure::InvalidQuery);
        }
        Ok(Self {
            terms,
            original: query.to_owned(),
            scope,
        })
    }

    #[must_use]
    pub fn original(&self) -> &str {
        &self.original
    }

    #[must_use]
    pub fn terms(&self) -> &[String] {
        &self.terms
    }

    #[must_use]
    pub const fn scope(&self) -> Option<&CodeScope> {
        self.scope.as_ref()
    }
}
