//! Low-priority bounded embedding work for the app session.
use super::Scheduler;
use crate::application::retrieval::{Retrieval, RetrievalError};
use crate::runtime::{embedding::EmbeddingController, inference::EmbeddingWorkers};
use ctxfab_inference::Embedder;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

impl Scheduler {
    #[must_use]
    pub fn with_embedding(
        self,
        embedding: Arc<EmbeddingController>,
        retrieval: Arc<Retrieval>,
    ) -> Self {
        self.with_embedding_provider(
            embedding.change_signal(),
            move |_| Some(embedding.clone()),
            retrieval,
        )
    }

    #[must_use]
    pub fn with_embedding_profiles(
        self,
        workers: Arc<EmbeddingWorkers>,
        retrieval: Arc<Retrieval>,
    ) -> Self {
        let signal = workers.change_signal();
        self.with_embedding_provider(
            signal,
            move |id| {
                workers.unload_inactive(id);
                workers.get(id)
            },
            retrieval,
        )
    }

    fn with_embedding_provider(
        mut self,
        changed: Arc<tokio::sync::Notify>,
        provider: impl Fn(&str) -> Option<Arc<EmbeddingController>> + Send + Sync + 'static,
        retrieval: Arc<Retrieval>,
    ) -> Self {
        let provider = Arc::new(provider);
        let mut stop = self.stop.subscribe();
        self.workers.spawn(async move {
            let mut last_state = "";
            let mut idle = Duration::from_secs(2);
            loop {
                if *stop.borrow() {
                    break;
                }
                let provider = provider.clone();
                let index = retrieval.clone();
                let outcome = tokio::task::spawn_blocking(move || {
                    let state = index.index_state()?;
                    let worker = provider(&state.profile).ok_or(RetrievalError::NotConfigured)?;
                    if !state.enabled {
                        return Ok(("disabled", false));
                    }
                    let warmed = worker.warm().map_err(RetrievalError::from)?;
                    if !worker.is_ready() {
                        return Ok(("unavailable", false));
                    }
                    if index.rebuild_one()? {
                        Ok(("indexing", true))
                    } else {
                        Ok(("ready", warmed))
                    }
                })
                .await;
                let (state, delay) = match outcome {
                    Ok(Ok((state, true))) => {
                        idle = Duration::from_secs(2);
                        (state, Duration::from_millis(100))
                    }
                    Ok(Ok((state, false))) => {
                        let delay = idle;
                        idle = (idle * 2).min(Duration::from_secs(30));
                        (state, delay)
                    }
                    Ok(Err(RetrievalError::Busy)) => ("waiting", Duration::from_millis(250)),
                    Ok(Err(_)) | Err(_) => ("degraded", Duration::from_secs(5)),
                };
                if state != last_state {
                    tracing::info!(
                        code = "embedding_background",
                        state,
                        "Background embedding state changed"
                    );
                    last_state = state;
                }
                tokio::select! {
                    _ = stop.wait_for(|stopping| *stopping) => break,
                    () = sleep(delay) => {}
                    () = changed.notified() => { idle = Duration::from_secs(2); }
                }
            }
        });
        self
    }
}
