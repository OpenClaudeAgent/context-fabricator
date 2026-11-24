use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ctxfab_inference::{
    ALTERNATE_EMBEDDING_PROFILE, Embedder, PROFILE,
    generation::{Generation, Model},
    llama_cpp::LocalWorker,
    supervisor::{ALTERNATE_EMBEDDING_FILE, EMBEDDING_FILE, Profile, Runtime},
};

use super::embedding::EmbeddingController;
use crate::adapters::outbound::sqlite::SqliteStore;
use crate::application::{
    answers::Answers,
    model_management::{ModelInventory, ModelUsage},
    retrieval::{EmbeddingProfile, Retrieval, RetrievalError},
};
use crate::domain::model::{
    Artifact, ArtifactKind, DownloadError, discovery::approved_imported_embedding,
};

pub struct EmbeddingWorkers {
    assets: PathBuf,
    runtime: Arc<Runtime>,
    workers: Mutex<HashMap<String, Arc<EmbeddingController>>>,
    changed: Arc<tokio::sync::Notify>,
}

impl EmbeddingWorkers {
    fn new(assets: &Path, runtime: Arc<Runtime>, primary: Arc<EmbeddingController>) -> Self {
        let changed = primary.change_signal();
        let alternate = Arc::new(EmbeddingController::with_worker_profile(
            assets,
            None,
            runtime.clone(),
            Profile::EmbeddingF16,
            changed.clone(),
        ));
        Self {
            assets: assets.to_path_buf(),
            runtime,
            workers: Mutex::new(HashMap::from([
                (PROFILE.into(), primary),
                (ALTERNATE_EMBEDDING_PROFILE.into(), alternate),
            ])),
            changed,
        }
    }

    fn register(&self, artifact: &Artifact) -> Result<Arc<EmbeddingController>, DownloadError> {
        if !approved_imported_embedding(artifact) {
            return Err(DownloadError::Unavailable);
        }
        let id = artifact.profile.as_ref();
        let profile = Profile::imported_embedding(id).ok_or(DownloadError::Unavailable)?;
        let mut workers = self
            .workers
            .lock()
            .map_err(|_| DownloadError::Unavailable)?;
        Ok(workers
            .entry(id.into())
            .or_insert_with(|| {
                let worker = Arc::new(EmbeddingController::with_worker_profile(
                    &self.assets,
                    None,
                    self.runtime.clone(),
                    profile,
                    self.changed.clone(),
                ));
                worker.set_enabled(false);
                worker
            })
            .clone())
    }

    pub(super) fn get(&self, id: &str) -> Option<Arc<EmbeddingController>> {
        self.workers.lock().ok()?.get(id).cloned()
    }

    fn entries(&self) -> Vec<(String, Arc<EmbeddingController>)> {
        self.workers.lock().map_or_default(|workers| {
            workers
                .iter()
                .map(|(id, worker)| (id.clone(), worker.clone()))
                .collect()
        })
    }

    pub(super) fn unload_inactive(&self, active: &str) {
        for (id, worker) in self.entries() {
            if id != active {
                worker.unload_if_inactive();
            }
        }
    }

    pub(super) fn change_signal(&self) -> Arc<tokio::sync::Notify> {
        self.changed.clone()
    }
}

pub struct Inference {
    embeddings: Arc<EmbeddingWorkers>,
    generation: Option<Arc<Generation>>,
    pub retrieval: Option<Arc<Retrieval>>,
    pub answers: Option<Arc<Answers>>,
}

impl ModelUsage for Inference {
    fn installed(&self, artifact: &Artifact) -> Result<(), DownloadError> {
        if artifact.kind == Some(ArtifactKind::Embedding) {
            let worker = if matches!(
                (artifact.profile.as_ref(), artifact.file.as_ref()),
                (PROFILE, EMBEDDING_FILE) | (ALTERNATE_EMBEDDING_PROFILE, ALTERNATE_EMBEDDING_FILE)
            ) {
                self.embeddings
                    .get(&artifact.profile)
                    .ok_or(DownloadError::Unavailable)?
            } else {
                self.embeddings.register(artifact)?
            };
            return worker.stage().map_err(|_| DownloadError::Unavailable);
        }
        if !matches!(artifact.kind, Some(ArtifactKind::Generation)) {
            return Ok(());
        }
        let Some(generation) = &self.generation else {
            return Ok(());
        };
        let model = Model::managed(
            &artifact.profile,
            artifact.label.as_deref().unwrap_or(""),
            &artifact.sha256,
        )
        .ok_or(DownloadError::Unavailable)?;
        generation
            .register(model)
            .map_err(|_| DownloadError::Unavailable)
    }

    fn deletion_block(&self, profile: &str) -> Result<Option<&'static str>, DownloadError> {
        if self
            .embeddings
            .get(profile)
            .is_some_and(|worker| worker.in_use())
        {
            return Ok(Some("The embedding model is in use for this session."));
        }
        if let Some(worker) = &self.generation {
            let active = worker
                .active_profile()
                .map_err(|_| DownloadError::Unavailable)?;
            if active.as_deref() == Some(profile) {
                return Ok(Some(
                    "Switch to another answer model before deleting this model.",
                ));
            }
        }
        Ok(None)
    }

    fn with_removal(
        &self,
        profile: &str,
        remove: &dyn Fn() -> Result<(), DownloadError>,
    ) -> Result<(), DownloadError> {
        if let Some(worker) = self.embeddings.get(profile) {
            return worker
                .with_inactive_removal(remove)
                .map_err(|_| DownloadError::InUse)?;
        }
        if let Some(worker) = &self.generation {
            return worker
                .with_inactive_model(profile, remove)
                .map_err(|_| DownloadError::InUse)?;
        }
        remove()
    }
}

impl Inference {
    #[must_use]
    pub fn asset_directory(directory: &Path) -> PathBuf {
        std::env::var_os("CTXFAB_INFERENCE_DIRECTORY")
            .map_or_else(|| directory.join("inference"), PathBuf::from)
    }

    #[must_use]
    pub fn configured(assets: &Path, store: &Arc<SqliteStore>) -> Self {
        Self::configured_with_models(assets, store, Vec::new())
    }

    #[must_use]
    pub fn configured_with_models(
        assets: &Path,
        store: &Arc<SqliteStore>,
        imported: Vec<Model>,
    ) -> Self {
        let fallback = LocalWorker::configured()
            .ok()
            .flatten()
            .map(|worker| Arc::new(worker) as Arc<dyn Embedder>);
        let runtime = Arc::new(Runtime::preview(assets));
        let generation =
            Generation::configured_persistent(assets, imported, store.clone()).map(Arc::new);
        Self::with_workers(assets, store, runtime, fallback, generation)
    }

    #[must_use]
    pub fn managed(
        assets: &Path,
        store: &Arc<SqliteStore>,
        imported: Vec<Model>,
        runtime: Arc<Runtime>,
    ) -> Self {
        let generation = Arc::new(Generation::with_runtime(
            assets,
            imported,
            Some(store.clone()),
            runtime.clone(),
        ));
        Self::with_workers(assets, store, runtime, None, Some(generation))
    }

    fn with_workers(
        assets: &Path,
        store: &Arc<SqliteStore>,
        runtime: Arc<Runtime>,
        fallback: Option<Arc<dyn Embedder>>,
        generation: Option<Arc<Generation>>,
    ) -> Self {
        let primary = Arc::new(EmbeddingController::with_runtime(
            assets,
            fallback,
            runtime.clone(),
        ));
        let embeddings = Arc::new(EmbeddingWorkers::new(assets, runtime, primary.clone()));
        if let Ok(retained) = store.load_models() {
            for entry in retained {
                if entry.artifact.kind == Some(ArtifactKind::Embedding) {
                    let _ = embeddings.register(&entry.artifact);
                }
            }
        }
        let resolve = embeddings.clone();
        let prepare = embeddings.clone();
        let selected = store
            .selected_embedding()
            .unwrap_or_else(|_| PROFILE.to_owned());
        let service = Retrieval::new(store.clone(), store.clone(), primary, PROFILE).with_profiles(
            move |id| {
                let worker = resolve.get(id)?;
                let observed = worker.clone();
                Some(EmbeddingProfile {
                    id: id.to_owned(),
                    worker,
                    state_changed: Some(Arc::new(move |enabled| observed.set_enabled(enabled))),
                })
            },
            move |id| {
                prepare
                    .get(id)
                    .ok_or(RetrievalError::NotConfigured)?
                    .warm_for_selection()
                    .map(|lease| Box::new(lease) as Box<dyn Send>)
                    .map_err(RetrievalError::from)
            },
            &selected,
        );
        let index = service.index_state().ok();
        for (id, worker) in embeddings.entries() {
            worker.set_enabled(
                index
                    .as_ref()
                    .is_some_and(|state| state.profile == id && state.enabled),
            );
        }
        let retrieval = Some(Arc::new(service));
        let answers = generation.as_ref().map(|worker| {
            Arc::new(
                Answers::new(
                    store.clone(),
                    store.clone(),
                    retrieval.clone(),
                    worker.clone(),
                )
                .with_history(store.clone())
                .with_actions(store.clone()),
            )
        });
        Self {
            embeddings,
            generation,
            retrieval,
            answers,
        }
    }

    pub fn runtime_published(&self) {
        self.embeddings.changed.notify_one();
    }

    #[must_use]
    pub fn background_embedding(&self) -> Option<(Arc<EmbeddingWorkers>, Arc<Retrieval>)> {
        Some((self.embeddings.clone(), self.retrieval.clone()?))
    }

    pub fn shutdown(&self) {
        for (_, worker) in self.embeddings.entries() {
            worker.shutdown();
        }
        if let Some(generation) = &self.generation {
            generation.shutdown();
        }
    }
}
