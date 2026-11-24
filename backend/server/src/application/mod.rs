pub mod answer_history;
mod answer_language;
pub mod answers;
pub mod artifact_storage;
pub mod credentials;
pub mod github;
pub mod inference_setup;
pub mod model_management;
pub mod ports;
pub mod retrieval;
pub mod runtime_setup;
mod search_references;
pub mod sync;
pub mod telemetry;

use crate::domain::corpus::snapshot::Location;
use serde::{Deserialize, Serialize};

pub use answer_language::{AnswerLanguage, AnswerLanguageOption};

pub const DEFAULT_INFERENCE_ARTIFACT_LIMIT_GB: u16 = 16;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub revision: u32,
    #[serde(deserialize_with = "AnswerLanguage::deserialize_legacy")]
    pub answer_language: AnswerLanguage,
    pub diagnostic_retention_hours: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer_locale: Option<AnswerLanguage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer_language_options: Option<Vec<AnswerLanguageOption>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_storage_limit_mb: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inference_artifact_limit_gb: Option<u16>,
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
    #[error("Answer history retention must be between 1 and 365 days.")]
    InvalidAnswerHistoryRetention,
    #[error("Diagnostic retention must be between 1 and 168 hours.")]
    InvalidRetention,
    #[error("Diagnostic storage limit must be between 2 and 1000 MB.")]
    InvalidStorageLimit,
    #[error("Inference artifact limit must be between 1 and 1000 GB.")]
    InvalidInferenceArtifactLimit,
    #[error("Search text must contain 1 to 512 characters.")]
    InvalidQuery,
    #[error("Supply a positive GitHub repository ID and its branch name together.")]
    InvalidSearchScope,
    #[error("Supply a repository, a tracked branch and a full Git commit ID for history answers.")]
    InvalidHistoryScope,
    #[error("Supply the repository, tracked branch, run ID and attempt for an Actions answer.")]
    InvalidActionsScope,
    #[error("Select a single GitHub evidence family for each answer.")]
    ConflictingAnswerScopes,
}

impl Settings {
    #[must_use]
    pub fn preferred_answer_language(&self) -> AnswerLanguage {
        self.answer_locale.unwrap_or(self.answer_language)
    }

    pub(crate) fn with_answer_language_options(mut self) -> Self {
        self.answer_language_options = Some(AnswerLanguage::options());
        self
    }

    #[must_use]
    pub fn inference_artifact_limit_bytes(&self) -> u64 {
        u64::from(
            self.inference_artifact_limit_gb
                .unwrap_or(DEFAULT_INFERENCE_ARTIFACT_LIMIT_GB),
        ) * 1_000_000_000
    }

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
        if self
            .inference_artifact_limit_gb
            .is_some_and(|limit| !(1..=1_000).contains(&limit))
        {
            return Err(Failure::InvalidInferenceArtifactLimit);
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

    pub(crate) fn is_scoped_prose(&self) -> bool {
        self.scope.is_some() && !has_source_literal(self.original())
    }

    pub(crate) fn can_reformulate(&self) -> bool {
        self.terms.len() >= 4 && !has_source_literal(self.original())
    }
}

fn has_source_literal(query: &str) -> bool {
    query.split_whitespace().nth(1).is_none()
        || query.contains(['`', '"'])
        || query.split_whitespace().any(|word| {
            let source = word
                .trim_matches(|character: char| !character.is_alphanumeric() && character != '_');
            let camel_case = source
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_lowercase)
                && source.bytes().any(|byte| byte.is_ascii_uppercase());
            camel_case || source.contains(['_', '.', '/']) || source.contains("::")
        })
}
