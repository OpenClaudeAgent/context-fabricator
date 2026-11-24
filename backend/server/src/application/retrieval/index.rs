use std::time::Instant;

use ctxfab_inference::{DIMENSIONS, Embedding};
use serde::Serialize;

use super::RetrievalError;
use crate::application::{CodeScope, SearchHit};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub dimensions: usize,
    pub indexed: i64,
    pub total: i64,
}

impl Progress {
    #[must_use]
    pub const fn not_configured() -> Self {
        Self {
            state: "notConfigured",
            profile: None,
            dimensions: 0,
            indexed: 0,
            total: 0,
        }
    }

    #[must_use]
    pub fn new(profile: &str, indexed: i64, total: i64) -> Self {
        Self {
            state: if indexed == total {
                "ready"
            } else {
                "incomplete"
            },
            profile: Some(profile.to_owned()),
            dimensions: DIMENSIONS,
            indexed,
            total,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexState {
    pub profile: String,
    pub enabled: bool,
    pub epoch: i64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexDeletion {
    #[serde(flatten)]
    pub state: IndexState,
    pub removed_vectors: i64,
    pub removed_bytes: i64,
}

pub struct Passage {
    pub id: i64,
    pub index_epoch: i64,
    pub collection: String,
    pub generation: u32,
    pub evidence: SearchHit,
    pub content: String,
}

pub trait VectorIndex: Send + Sync {
    /// # Errors
    /// Returns an error when the stored index state cannot be read.
    fn state(&self, profile: &str) -> Result<IndexState, RetrievalError>;
    /// # Errors
    /// Returns an error when the state transition cannot be committed.
    fn set_enabled(&self, profile: &str, enabled: bool) -> Result<IndexState, RetrievalError>;
    /// # Errors
    /// Returns an error when the index cannot be disabled and deleted atomically.
    fn delete_index(&self, profile: &str) -> Result<IndexDeletion, RetrievalError>;
    /// # Errors
    /// Returns an error if the selected profile cannot be inspected.
    fn progress(
        &self,
        profile: &str,
        scope: Option<&CodeScope>,
    ) -> Result<Progress, RetrievalError>;

    /// # Errors
    /// Returns an error if a bounded batch of unindexed passages cannot be read.
    fn pending(&self, profile: &str, limit: u16) -> Result<Vec<Passage>, RetrievalError>;

    /// # Errors
    /// Returns an error if the vector cannot be persisted. A changed passage returns false.
    fn save_vector(
        &self,
        profile: &str,
        passage: &Passage,
        vector: &Embedding,
    ) -> Result<bool, RetrievalError>;

    /// Ranks a primary vector and an optional auxiliary vector in the same scoped snapshot.
    ///
    /// # Errors
    /// Returns an error for an incomplete profile or unavailable storage.
    fn rank(
        &self,
        profile: &str,
        vector: &Embedding,
        lexical: Vec<SearchHit>,
        scope: Option<&CodeScope>,
        formulation: Option<(&Embedding, Instant)>,
        inspect: bool,
    ) -> Result<VectorRanking, RetrievalError>;
}
pub struct VectorRanking {
    pub dense: Vec<SearchHit>,
    pub lexical: Vec<SearchHit>,
    pub formulation: Vec<SearchHit>,
    pub nearest: Vec<VectorMatch>,
    pub reformulated_nearest: Vec<VectorMatch>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorMatch {
    pub evidence: SearchHit,
    pub similarity: f32,
    pub text: String,
    pub vector: Vec<f32>,
}
