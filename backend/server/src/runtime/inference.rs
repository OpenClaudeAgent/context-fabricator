use std::path::{Path, PathBuf};
use std::sync::Arc;

use ctxfab_inference::{
    PROFILE,
    generation::Generation,
    llama_cpp::LocalWorker,
    supervisor::{ManagedWorker, Profile},
};

use crate::adapters::outbound::sqlite::SqliteStore;
use crate::application::{answers::Answers, retrieval::Retrieval};

pub struct Inference {
    embedding: Option<Arc<ManagedWorker>>,
    generation: Option<Arc<Generation>>,
    pub retrieval: Option<Arc<Retrieval>>,
    pub answers: Option<Arc<Answers>>,
}

impl Inference {
    #[must_use]
    pub fn asset_directory(directory: &Path) -> PathBuf {
        std::env::var_os("CTXFAB_INFERENCE_DIRECTORY")
            .map_or_else(|| directory.join("inference"), PathBuf::from)
    }

    #[must_use]
    pub fn configured(assets: &Path, store: &Arc<SqliteStore>) -> Self {
        let embedding = ManagedWorker::configured(assets, Profile::Embedding).map(Arc::new);
        let generation = Generation::configured(assets).map(Arc::new);
        let retrieval = if let Some(worker) = &embedding {
            Some(Arc::new(Retrieval::new(
                store.clone(),
                store.clone(),
                worker.clone(),
                PROFILE,
            )))
        } else {
            LocalWorker::configured().ok().flatten().map(|worker| {
                Arc::new(Retrieval::new(
                    store.clone(),
                    store.clone(),
                    Arc::new(worker),
                    PROFILE,
                ))
            })
        };
        let answers = generation.as_ref().map(|worker| {
            Arc::new(Answers::new(
                store.clone(),
                store.clone(),
                retrieval.clone(),
                worker.clone(),
            ))
        });
        Self {
            embedding,
            generation,
            retrieval,
            answers,
        }
    }

    pub fn shutdown(&self) {
        if let Some(worker) = &self.embedding {
            worker.shutdown();
        }
        if let Some(generation) = &self.generation {
            generation.shutdown();
        }
    }
}
