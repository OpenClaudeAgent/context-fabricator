use ctxfab_inference::{DIMENSIONS, Embedding};
use serde::Serialize;

use super::RetrievalError;
use crate::application::{CodeScope, SearchHit};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<&'static str>,
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
    pub const fn new(profile: &'static str, indexed: i64, total: i64) -> Self {
        Self {
            state: if indexed == total {
                "ready"
            } else {
                "incomplete"
            },
            profile: Some(profile),
            dimensions: DIMENSIONS,
            indexed,
            total,
        }
    }
}

pub struct Passage {
    pub id: i64,
    pub collection: String,
    pub generation: u32,
    pub evidence: SearchHit,
    pub content: String,
}

pub trait VectorIndex: Send + Sync {
    /// # Errors
    /// Returns an error if the selected profile cannot be inspected.
    fn progress(
        &self,
        profile: &'static str,
        scope: Option<&CodeScope>,
    ) -> Result<Progress, RetrievalError>;

    /// # Errors
    /// Returns an error if a bounded batch of unindexed passages cannot be read.
    fn pending(&self, profile: &'static str, limit: u16) -> Result<Vec<Passage>, RetrievalError>;

    /// # Errors
    /// Returns an error if the vector cannot be persisted. A changed passage returns false.
    fn save_vector(
        &self,
        profile: &'static str,
        passage: &Passage,
        vector: &Embedding,
    ) -> Result<bool, RetrievalError>;

    /// Returns dense ranking and the lexical candidates that are still visible.
    ///
    /// # Errors
    /// Returns an error for an incomplete profile or unavailable storage.
    fn rank(
        &self,
        profile: &'static str,
        vector: &Embedding,
        lexical: Vec<SearchHit>,
        scope: Option<&CodeScope>,
    ) -> Result<(Vec<SearchHit>, Vec<SearchHit>), RetrievalError>;
}
