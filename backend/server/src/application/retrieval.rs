use std::sync::{Arc, Mutex};

use ctxfab_inference::{Embedder, EmbeddingError, Input};
use serde::{Deserialize, Serialize};

use super::{SearchHit, SearchQuery, ports::Repository};

mod index;
mod ranking;

pub use index::{Passage, Progress, VectorIndex};

pub const MAX_RESULTS: usize = 20;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Bm25,
    Semantic,
    Hybrid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RetrievalError {
    #[error("The local embedding runtime is not configured.")]
    NotConfigured,
    #[error("The local embedding runtime is unavailable.")]
    Runtime,
    #[error("The local embedding index is incomplete for the active profile.")]
    Incomplete,
    #[error("The local embedding runtime or rebuild is busy.")]
    Busy,
    #[error("The local embedding index could not be read or saved.")]
    Storage,
    #[error("The embedding vector is incompatible with the active profile.")]
    Vector,
    #[error("A source passage exceeds the embedding profile input budget.")]
    PassageTooLarge,
}

impl From<EmbeddingError> for RetrievalError {
    fn from(error: EmbeddingError) -> Self {
        match error {
            EmbeddingError::Runtime => Self::Runtime,
            EmbeddingError::Busy => Self::Busy,
            EmbeddingError::Vector => Self::Vector,
            EmbeddingError::PassageTooLarge => Self::PassageTooLarge,
        }
    }
}

pub struct Retrieval {
    lexical: Arc<dyn Repository>,
    index: Arc<dyn VectorIndex>,
    embedder: Arc<dyn Embedder>,
    profile: &'static str,
    rebuilding: Mutex<()>,
}

impl Retrieval {
    #[must_use]
    pub fn new(
        lexical: Arc<dyn Repository>,
        index: Arc<dyn VectorIndex>,
        embedder: Arc<dyn Embedder>,
        profile: &'static str,
    ) -> Self {
        Self {
            lexical,
            index,
            embedder,
            profile,
            rebuilding: Mutex::new(()),
        }
    }

    /// # Errors
    /// Returns a typed error if the index is unavailable.
    pub fn progress(&self) -> Result<Progress, RetrievalError> {
        self.index.progress(self.profile, None)
    }

    /// Processes at most one bounded rebuild batch. Persisted rows are usable after restart.
    ///
    /// # Errors
    /// Returns a typed error if a rebuild or inference cannot proceed.
    pub fn rebuild(&self, limit: u16) -> Result<Progress, RetrievalError> {
        let _owner = self
            .rebuilding
            .try_lock()
            .map_err(|_| RetrievalError::Busy)?;
        for passage in self.index.pending(self.profile, limit.clamp(1, 32))? {
            let vector = self.embedder.embed(Input::Passage {
                title: &passage.evidence.title,
                content: &passage.content,
            })?;
            self.index.save_vector(self.profile, &passage, &vector)?;
        }
        self.progress()
    }

    /// # Errors
    /// Returns a typed error if the chosen mode cannot use complete current evidence.
    pub fn search(
        &self,
        query: &SearchQuery,
        mode: Mode,
    ) -> Result<Vec<SearchHit>, RetrievalError> {
        let bm25 = || {
            self.lexical
                .search(query)
                .map_err(|_| RetrievalError::Storage)
        };
        if mode == Mode::Bm25 {
            return bm25();
        }
        let progress = self.index.progress(self.profile, query.scope())?;
        if progress.indexed != progress.total {
            return Err(RetrievalError::Incomplete);
        }
        if progress.total == 0 {
            return Ok(Vec::new());
        }
        let vector = self.embedder.embed(Input::Query(query.original()))?;
        let lexical = if mode == Mode::Hybrid {
            bm25()?
        } else {
            Vec::new()
        };
        let (dense, lexical) = self
            .index
            .rank(self.profile, &vector, lexical, query.scope())?;
        Ok(if mode == Mode::Hybrid {
            ranking::fuse(lexical, dense)
        } else {
            dense
        })
    }
}
