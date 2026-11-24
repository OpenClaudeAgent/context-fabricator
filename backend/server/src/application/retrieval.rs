use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use ctxfab_inference::{Embedder, EmbeddingError, Input};
use serde::{Deserialize, Serialize};

use super::{SearchHit, SearchQuery, ports::Repository, telemetry::FusionRanking};

mod index;
mod ranking;

pub use index::{
    IndexDeletion, IndexState, Passage, Progress, VectorIndex, VectorMatch, VectorRanking,
};

pub const MAX_RESULTS: usize = 20;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EmbeddingCapture {
    profile: String,
    indexed: i64,
    total: i64,
    input: String,
    vector: Vec<f32>,
    nearest: Vec<VectorMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reformulation: Option<EmbeddingQuery>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddingQuery {
    input: String,
    vector: Vec<f32>,
    nearest: Vec<VectorMatch>,
}

pub(crate) struct RankedSearch {
    pub original: Vec<SearchHit>,
    pub auxiliary: Vec<SearchHit>,
    pub fusion: Option<FusionRanking>,
    pub embedding: Option<EmbeddingCapture>,
}

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
    #[error("The local embedding index is disabled.")]
    Disabled,
    #[error("The local embedding index is incomplete for the active profile.")]
    Incomplete,
    #[error("The local embedding runtime or rebuild is busy.")]
    Busy,
    #[error("The GitHub logical storage limit blocks this embedding write.")]
    StorageLimit,
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

type EmbeddingPreparation = dyn Fn(&str) -> Result<Box<dyn Send>, RetrievalError> + Send + Sync;
type EmbeddingResolver = dyn Fn(&str) -> Option<EmbeddingProfile> + Send + Sync;

#[derive(Clone)]
pub(crate) struct EmbeddingProfile {
    pub id: String,
    pub worker: Arc<dyn Embedder>,
    pub state_changed: Option<Arc<dyn Fn(bool) + Send + Sync>>,
}

impl EmbeddingProfile {
    fn notify(&self, enabled: bool) {
        if let Some(listener) = &self.state_changed {
            listener(enabled);
        }
    }
}

pub struct Retrieval {
    lexical: Arc<dyn Repository>,
    index: Arc<dyn VectorIndex>,
    active: Mutex<EmbeddingProfile>,
    resolve: Option<Arc<EmbeddingResolver>>,
    rebuilding: Mutex<()>,
    index_transition: Mutex<()>,
    prepare: Option<Arc<EmbeddingPreparation>>,
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
            active: Mutex::new(EmbeddingProfile {
                id: profile.to_owned(),
                worker: embedder,
                state_changed: None,
            }),
            resolve: None,
            rebuilding: Mutex::new(()),
            index_transition: Mutex::new(()),
            prepare: None,
        }
    }

    #[must_use]
    pub fn with_state_listener(mut self, listener: impl Fn(bool) + Send + Sync + 'static) -> Self {
        if let Ok(active) = self.active.get_mut() {
            active.state_changed = Some(Arc::new(listener));
        }
        self
    }

    #[must_use]
    pub(crate) fn with_profiles(
        mut self,
        resolve: impl Fn(&str) -> Option<EmbeddingProfile> + Send + Sync + 'static,
        prepare: impl Fn(&str) -> Result<Box<dyn Send>, RetrievalError> + Send + Sync + 'static,
        active: &str,
    ) -> Self {
        if let Some(selected) = resolve(active) {
            self.active = Mutex::new(selected);
        }
        self.resolve = Some(Arc::new(resolve));
        self.prepare = Some(Arc::new(prepare));
        self
    }

    fn active_embedding(&self) -> Result<EmbeddingProfile, RetrievalError> {
        self.active
            .lock()
            .map(|active| active.clone())
            .map_err(|_| RetrievalError::Storage)
    }

    pub(crate) fn for_corpus(
        &self,
        lexical: Arc<dyn Repository>,
        index: Arc<dyn VectorIndex>,
    ) -> Result<Self, RetrievalError> {
        let mut profile = self.active_embedding()?;
        if !self.index.state(&profile.id)?.enabled {
            return Err(RetrievalError::Disabled);
        }
        if !profile.worker.is_ready() {
            return Err(RetrievalError::Runtime);
        }
        profile.state_changed = None;
        Ok(Self {
            lexical,
            index,
            active: Mutex::new(profile),
            resolve: None,
            rebuilding: Mutex::new(()),
            index_transition: Mutex::new(()),
            prepare: None,
        })
    }

    // Keep the preparation lease alive until the selection transaction publishes the target.
    pub(super) fn prepare_embedding(
        &self,
        id: &str,
    ) -> Result<Option<Box<dyn Send>>, RetrievalError> {
        if self.active_embedding()?.id == id {
            return Ok(None);
        }
        self.prepare.as_ref().ok_or(RetrievalError::NotConfigured)?(id).map(Some)
    }

    /// # Errors
    /// Returns an error if the index state is unavailable.
    pub fn index_state(&self) -> Result<IndexState, RetrievalError> {
        self.index.state(&self.active_embedding()?.id)
    }
    /// # Errors
    /// Returns an error if the state transition cannot be committed.
    pub fn set_index_enabled(&self, enabled: bool) -> Result<IndexState, RetrievalError> {
        let _transition = self
            .index_transition
            .lock()
            .map_err(|_| RetrievalError::Storage)?;
        self.set_index_policy(enabled)
    }

    pub(super) fn complete_setup_index(
        &self,
        target: &str,
        commit: impl FnOnce(&str, &str) -> Result<Option<IndexState>, RetrievalError>,
    ) -> Result<bool, RetrievalError> {
        let _transition = self
            .index_transition
            .lock()
            .map_err(|_| RetrievalError::Storage)?;
        let mut active = self.active.lock().map_err(|_| RetrievalError::Storage)?;
        let next = if active.id == target {
            None
        } else {
            Some(
                self.resolve
                    .as_ref()
                    .and_then(|resolve| resolve(target))
                    .ok_or(RetrievalError::NotConfigured)?,
            )
        };
        let Some(state) = commit(&active.id, target)? else {
            return Ok(false);
        };
        if let Some(next) = next {
            active.notify(false);
            *active = next;
        }
        active.notify(state.enabled);
        Ok(true)
    }

    fn set_index_policy(&self, enabled: bool) -> Result<IndexState, RetrievalError> {
        let profile = self.active_embedding()?;
        let state = self.index.set_enabled(&profile.id, enabled)?;
        profile.notify(enabled);
        Ok(state)
    }
    /// # Errors
    /// Returns an error if the index deletion cannot be committed.
    pub fn delete_index(&self) -> Result<IndexDeletion, RetrievalError> {
        let _transition = self
            .index_transition
            .lock()
            .map_err(|_| RetrievalError::Storage)?;
        let profile = self.active_embedding()?;
        let result = self.index.delete_index(&profile.id)?;
        profile.notify(false);
        Ok(result)
    }
    /// # Errors
    /// Returns a typed error if the index is unavailable.
    pub fn progress(&self) -> Result<Progress, RetrievalError> {
        self.profile_progress(&self.active_embedding()?)
    }

    fn profile_progress(&self, profile: &EmbeddingProfile) -> Result<Progress, RetrievalError> {
        if !self.index.state(&profile.id)?.enabled {
            let mut progress = self.index.progress(&profile.id, None)?;
            progress.state = "disabled";
            return Ok(progress);
        }
        if !profile.worker.is_ready() {
            return Ok(Progress::not_configured());
        }
        self.index.progress(&profile.id, None)
    }

    /// Processes at most one bounded rebuild batch. Persisted rows are usable after restart.
    ///
    /// # Errors
    /// Returns a typed error if a rebuild or inference cannot proceed.
    pub fn rebuild(&self, limit: u16) -> Result<Progress, RetrievalError> {
        let profile = self.active_embedding()?;
        self.rebuild_batch(&profile, limit.clamp(1, 32))?;
        self.profile_progress(&profile)
    }

    /// Processes at most one passage without a full progress count.
    ///
    /// # Errors
    /// Returns a typed error if the worker, index, or rebuild admission is unavailable.
    pub fn rebuild_one(&self) -> Result<bool, RetrievalError> {
        self.rebuild_batch(&self.active_embedding()?, 1)
    }

    fn rebuild_batch(
        &self,
        profile: &EmbeddingProfile,
        limit: u16,
    ) -> Result<bool, RetrievalError> {
        if !self.index.state(&profile.id)?.enabled {
            return Err(RetrievalError::Disabled);
        }
        if !profile.worker.is_ready() {
            return Err(RetrievalError::Runtime);
        }
        let _owner = self
            .rebuilding
            .try_lock()
            .map_err(|_| RetrievalError::Busy)?;
        let passages = self.index.pending(&profile.id, limit)?;
        let found = !passages.is_empty();
        for passage in passages {
            let vector = profile.worker.embed(Input::Passage {
                title: &passage.evidence.title,
                content: &passage.content,
            })?;
            self.index.save_vector(&profile.id, &passage, &vector)?;
        }
        Ok(found)
    }

    /// # Errors
    /// Returns a typed error if the chosen mode cannot use complete current evidence.
    pub fn search(
        &self,
        query: &SearchQuery,
        mode: Mode,
    ) -> Result<Vec<SearchHit>, RetrievalError> {
        self.search_ranked(query, mode, false, false, || None)
            .map(|ranking| ranking.original)
    }

    pub(crate) fn search_ranked(
        &self,
        query: &SearchQuery,
        mode: Mode,
        observe: bool,
        inspect: bool,
        formulate: impl FnOnce() -> Option<String>,
    ) -> Result<RankedSearch, RetrievalError> {
        let bm25 = || {
            self.lexical
                .search(query)
                .map_err(|_| RetrievalError::Storage)
        };
        if mode == Mode::Bm25 {
            return bm25().map(|original| RankedSearch {
                original,
                auxiliary: Vec::new(),
                fusion: None,
                embedding: None,
            });
        }
        let profile = self.active_embedding()?;
        if !self.index.state(&profile.id)?.enabled {
            return Err(RetrievalError::Disabled);
        }
        if !profile.worker.is_ready() {
            return Err(RetrievalError::Runtime);
        }
        let progress = self.index.progress(&profile.id, query.scope())?;
        if progress.indexed != progress.total {
            return Err(RetrievalError::Incomplete);
        }
        if progress.total == 0 {
            return Ok(RankedSearch {
                original: Vec::new(),
                auxiliary: Vec::new(),
                fusion: None,
                embedding: None,
            });
        }
        let vector = profile.worker.embed(Input::Query(query.original()))?;
        let formulation = formulate()
            .filter(|alternate| !alternate.eq_ignore_ascii_case(query.original().trim()))
            .and_then(|alternate| {
                let alternate = SearchQuery::parse(&alternate).ok()?;
                let deadline = Instant::now() + Duration::from_secs(1);
                let vector = profile
                    .worker
                    .embed_warm(Input::Query(alternate.original()), deadline)
                    .ok()?;
                Some((vector, deadline, alternate))
            });
        if !self.index.state(&profile.id)?.enabled {
            return Err(RetrievalError::Disabled);
        }
        let lexical = if mode == Mode::Hybrid {
            bm25()?
        } else {
            Vec::new()
        };
        let ranking = self.index.rank(
            &profile.id,
            &vector,
            lexical,
            query.scope(),
            formulation
                .as_ref()
                .map(|(vector, deadline, _)| (vector, *deadline)),
            inspect,
        )?;
        let (original, fusion) = if mode == Mode::Hybrid {
            ranking::fuse(query, ranking.lexical, ranking.dense, observe)
        } else {
            (ranking.dense, None)
        };
        let embedding = if inspect {
            let reformulation = formulation
                .map(|(vector, _, query)| {
                    Ok::<_, EmbeddingError>(EmbeddingQuery {
                        input: Input::Query(query.original()).qwen_text()?,
                        vector: vector.values().to_vec(),
                        nearest: ranking.reformulated_nearest,
                    })
                })
                .transpose()?;
            Some(EmbeddingCapture {
                profile: profile.id,
                indexed: progress.indexed,
                total: progress.total,
                input: Input::Query(query.original()).qwen_text()?,
                vector: vector.values().to_vec(),
                nearest: ranking.nearest,
                reformulation,
            })
        } else {
            None
        };
        Ok(RankedSearch {
            original,
            auxiliary: ranking.formulation,
            fusion,
            embedding,
        })
    }
}
