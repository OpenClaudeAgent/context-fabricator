//! Session-owned, warm-before-use embedding worker with foreground admission.
use ctxfab_inference::{
    Embedder, Embedding, EmbeddingError, Input,
    supervisor::{ManagedWorker, Profile, Runtime},
};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Instant;

struct Candidate {
    worker: Arc<dyn Embedder>,
    managed: Option<Arc<ManagedWorker>>,
    ready: bool,
}

pub struct EmbeddingController {
    assets: PathBuf,
    worker_profile: Profile,
    runtime: Arc<Runtime>,
    candidate: Mutex<Option<Candidate>>,
    fallback: Option<Arc<dyn Embedder>>,
    admission: Mutex<()>,
    waiting_queries: AtomicUsize,
    stopped: AtomicBool,
    selecting: AtomicBool,
    enabled: AtomicBool,
    changed: Arc<tokio::sync::Notify>,
}

pub(super) struct EmbeddingSelection(Arc<EmbeddingController>);

impl Drop for EmbeddingSelection {
    fn drop(&mut self) {
        self.0.selecting.store(false, Ordering::Release);
    }
}

impl EmbeddingController {
    #[must_use]
    pub fn configured(assets: &Path, fallback: Option<Arc<dyn Embedder>>) -> Option<Self> {
        let runtime = Arc::new(Runtime::preview(assets));
        if !runtime.is_available() && fallback.is_none() {
            return None;
        }
        Some(Self::with_runtime(assets, fallback, runtime))
    }

    #[must_use]
    pub fn with_runtime(
        assets: &Path,
        fallback: Option<Arc<dyn Embedder>>,
        runtime: Arc<Runtime>,
    ) -> Self {
        Self::with_worker_profile(
            assets,
            fallback,
            runtime,
            Profile::Embedding,
            Arc::default(),
        )
    }

    #[must_use]
    pub(super) fn with_worker_profile(
        assets: &Path,
        fallback: Option<Arc<dyn Embedder>>,
        runtime: Arc<Runtime>,
        worker_profile: Profile,
        changed: Arc<tokio::sync::Notify>,
    ) -> Self {
        let controller = Self {
            assets: assets.to_path_buf(),
            worker_profile,
            runtime,
            candidate: Mutex::new(None),
            fallback,
            admission: Mutex::new(()),
            waiting_queries: AtomicUsize::new(0),
            stopped: AtomicBool::new(false),
            selecting: AtomicBool::new(false),
            enabled: AtomicBool::new(true),
            changed,
        };
        if let Some(worker) = ManagedWorker::with_runtime(
            assets,
            controller.worker_profile.clone(),
            controller.runtime.clone(),
        ) && let Ok(mut candidate) = controller.candidate.lock()
        {
            let worker = Arc::new(worker);
            *candidate = Some(Candidate {
                worker: worker.clone(),
                managed: Some(worker),
                ready: false,
            });
        }
        controller
    }

    /// Register verified weights as a candidate. Warmup occurs on the background worker.
    /// # Errors
    /// Returns an error if weights are missing or the controller is stopped.
    pub fn stage(&self) -> Result<(), EmbeddingError> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(EmbeddingError::Runtime);
        }
        let worker = Arc::new(
            ManagedWorker::with_runtime(
                &self.assets,
                self.worker_profile.clone(),
                self.runtime.clone(),
            )
            .ok_or(EmbeddingError::Runtime)?,
        );
        let mut state = self.candidate.lock().map_err(|_| EmbeddingError::Runtime)?;
        if self.stopped.load(Ordering::Acquire) || !worker.weights_exist() {
            return Err(EmbeddingError::Runtime);
        }
        if state.is_none() {
            *state = Some(Candidate {
                worker: worker.clone(),
                managed: Some(worker),
                ready: false,
            });
        }
        self.changed.notify_one();
        Ok(())
    }

    pub(super) fn change_signal(&self) -> Arc<tokio::sync::Notify> {
        self.changed.clone()
    }

    pub(super) fn unload_if_inactive(&self) {
        let Ok(_permit) = self.admission.try_lock() else {
            return;
        };
        if self.enabled.load(Ordering::Acquire) || self.selecting.load(Ordering::Acquire) {
            return;
        }
        if let Ok(mut state) = self.candidate.lock()
            && let Some(candidate) = state.as_mut()
            && let Some(worker) = &candidate.managed
        {
            candidate.ready = false;
            worker.unload();
        }
    }

    pub fn runtime_published(&self) {
        self.changed.notify_one();
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
        self.changed.notify_one();
    }

    /// Warm the currently staged candidate using one bounded test inference.
    /// # Errors
    /// Returns an error when admission, warmup, or session state is unavailable.
    pub fn warm(&self) -> Result<bool, EmbeddingError> {
        self.warm_candidate(false)
    }

    pub(super) fn warm_for_selection(
        self: &Arc<Self>,
    ) -> Result<EmbeddingSelection, EmbeddingError> {
        self.selecting
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| EmbeddingError::Busy)?;
        let lease = EmbeddingSelection(self.clone());
        self.stage()?;
        self.warm_candidate(true)?;
        if self
            .candidate
            .lock()
            .map_err(|_| EmbeddingError::Runtime)?
            .as_ref()
            .is_some_and(|candidate| candidate.ready)
        {
            return Ok(lease);
        }
        Err(EmbeddingError::Runtime)
    }

    fn warm_candidate(&self, selecting: bool) -> Result<bool, EmbeddingError> {
        if !selecting && !self.enabled.load(Ordering::Acquire) {
            return Ok(false);
        }
        if self.waiting_queries.load(Ordering::Acquire) > 0 {
            return Err(EmbeddingError::Busy);
        }
        let _permit = self
            .admission
            .try_lock()
            .map_err(|_| EmbeddingError::Busy)?;
        let worker = {
            let state = self.candidate.lock().map_err(|_| EmbeddingError::Runtime)?;
            match &*state {
                Some(candidate)
                    if !candidate.ready
                        && (candidate.managed.is_none() || self.runtime.is_available()) =>
                {
                    candidate.worker.clone()
                }
                _ => return Ok(false),
            }
        };
        worker.embed(Input::Query("Local embedding readiness check"))?;
        let mut state = self.candidate.lock().map_err(|_| EmbeddingError::Runtime)?;
        if self.stopped.load(Ordering::Acquire)
            || (!selecting && !self.enabled.load(Ordering::Acquire))
        {
            return Err(EmbeddingError::Runtime);
        }
        if let Some(candidate) = &mut *state
            && Arc::ptr_eq(&candidate.worker, &worker)
        {
            candidate.ready = true;
            return Ok(true);
        }
        Ok(false)
    }

    #[must_use]
    pub fn in_use(&self) -> bool {
        self.selecting.load(Ordering::Acquire)
            || (self.enabled.load(Ordering::Acquire)
                && self
                    .candidate
                    .lock()
                    .is_ok_and(|state| state.as_ref().is_some_and(|candidate| candidate.ready)))
    }

    /// Remove only an idle candidate, fencing warmup and any later registration.
    /// # Errors
    /// Returns busy if warmup or a ready worker owns the embedding weights.
    pub fn with_inactive_removal<T, E>(
        &self,
        remove: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, EmbeddingError> {
        let _permit = self
            .admission
            .try_lock()
            .map_err(|_| EmbeddingError::Busy)?;
        let mut state = self.candidate.lock().map_err(|_| EmbeddingError::Runtime)?;
        if self.selecting.load(Ordering::Acquire)
            || (self.enabled.load(Ordering::Acquire)
                && state.as_ref().is_some_and(|candidate| candidate.ready))
        {
            return Err(EmbeddingError::Busy);
        }
        let result = remove();
        if state.as_ref().is_some_and(|candidate| {
            result.is_ok()
                || candidate
                    .managed
                    .as_ref()
                    .is_some_and(|worker| !worker.weights_exist())
        }) && let Some(candidate) = state.take()
            && let Some(worker) = candidate.managed
        {
            worker.shutdown();
        }
        Ok(result)
    }

    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        if let Ok(mut state) = self.candidate.lock()
            && let Some(candidate) = state.take()
            && let Some(worker) = candidate.managed
        {
            worker.shutdown();
        }
    }

    fn ready_worker(&self) -> Result<Arc<dyn Embedder>, EmbeddingError> {
        if self.stopped.load(Ordering::Acquire) || !self.enabled.load(Ordering::Acquire) {
            return Err(EmbeddingError::Runtime);
        }
        self.candidate
            .lock()
            .map_err(|_| EmbeddingError::Runtime)?
            .as_ref()
            .filter(|candidate| {
                candidate.ready
                    && candidate
                        .managed
                        .as_ref()
                        .is_none_or(|worker| worker.weights_exist())
            })
            .map(|candidate| candidate.worker.clone())
            .or_else(|| self.fallback.clone())
            .ok_or(EmbeddingError::Runtime)
    }
}

impl Embedder for EmbeddingController {
    fn is_ready(&self) -> bool {
        !self.stopped.load(Ordering::Acquire)
            && self.enabled.load(Ordering::Acquire)
            && (self.fallback.is_some()
                || self.candidate.lock().is_ok_and(|state| {
                    state.as_ref().is_some_and(|item| {
                        item.ready
                            && item
                                .managed
                                .as_ref()
                                .is_none_or(|worker| worker.weights_exist())
                    })
                }))
    }
    fn embed(&self, input: Input<'_>) -> Result<Embedding, EmbeddingError> {
        let query = matches!(input, Input::Query(_));
        if query {
            self.waiting_queries.fetch_add(1, Ordering::AcqRel);
        }
        let permit = if query {
            self.admission.lock().map_err(|_| EmbeddingError::Runtime)
        } else if self.waiting_queries.load(Ordering::Acquire) > 0 {
            Err(EmbeddingError::Busy)
        } else {
            self.admission.try_lock().map_err(|_| EmbeddingError::Busy)
        };
        if query {
            self.waiting_queries.fetch_sub(1, Ordering::AcqRel);
        }
        let _permit = permit?;
        if !query && self.waiting_queries.load(Ordering::Acquire) > 0 {
            return Err(EmbeddingError::Busy);
        }
        self.ready_worker()?.embed(input)
    }

    fn embed_warm(&self, input: Input<'_>, deadline: Instant) -> Result<Embedding, EmbeddingError> {
        let _permit = self
            .admission
            .try_lock()
            .map_err(|_| EmbeddingError::Busy)?;
        if self.waiting_queries.load(Ordering::Acquire) > 0 {
            return Err(EmbeddingError::Busy);
        }
        self.ready_worker()?.embed_warm(input, deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::outbound::sqlite::SqliteStore;
    use crate::application::{ports::CorpusStore, retrieval::Retrieval};
    use crate::domain::corpus::{Batch, Document, DocumentKind};
    use crate::runtime::scheduler::Scheduler;
    use ctxfab_inference::{DIMENSIONS, PROFILE};
    use std::time::Duration;

    struct Fixture;
    impl Embedder for Fixture {
        fn embed(&self, _input: Input<'_>) -> Result<Embedding, EmbeddingError> {
            let mut values = vec![0.0; DIMENSIONS];
            values[0] = 1.0;
            Embedding::new(values)
        }
    }

    fn controller(worker: Arc<dyn Embedder>) -> EmbeddingController {
        EmbeddingController {
            assets: PathBuf::new(),
            worker_profile: Profile::Embedding,
            runtime: Arc::new(Runtime::default()),
            candidate: Mutex::new(Some(Candidate {
                worker,
                managed: None,
                ready: false,
            })),
            fallback: None,
            admission: Mutex::new(()),
            waiting_queries: AtomicUsize::new(0),
            stopped: AtomicBool::new(false),
            selecting: AtomicBool::new(false),
            enabled: AtomicBool::new(true),
            changed: Arc::default(),
        }
    }

    #[tokio::test]
    async fn scheduler_warms_then_rebuilds_in_the_same_session()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let store = Arc::new(SqliteStore::open(
            &directory.path().join("fixture.sqlite3"),
        )?);
        let docs = [Document {
            source: "fixture",
            revision: "v1",
            kind: DocumentKind::Current {
                tracked_ref: "refs/heads/main",
            },
            title: "Local",
            content: "A local source passage.",
        }];
        store.commit_batch(&Batch {
            collection: "fixture",
            generation: 0,
            previous_cursor: None,
            next_cursor: "1",
            documents: &docs,
        })?;
        let worker = Arc::new(controller(Arc::new(Fixture)));
        let retrieval = Arc::new(Retrieval::new(
            store.clone(),
            store.clone(),
            worker.clone(),
            PROFILE,
        ));
        assert_eq!(retrieval.progress()?.state, "notConfigured");
        let scheduler = Scheduler::default().with_embedding(worker.clone(), retrieval.clone());
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if retrieval.progress()?.indexed == 1 {
                    return Ok::<_, crate::application::retrieval::RetrievalError>(());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await??;
        assert!(worker.is_ready());
        assert_eq!(retrieval.progress()?.state, "ready");
        scheduler.stop().await;
        worker.shutdown();
        assert_eq!(retrieval.progress()?.state, "notConfigured");
        Ok(())
    }

    #[tokio::test]
    async fn disabled_index_prevents_scheduler_warmup() -> Result<(), Box<dyn std::error::Error>> {
        struct Counting(Arc<AtomicUsize>);
        impl Embedder for Counting {
            fn embed(&self, _: Input<'_>) -> Result<Embedding, EmbeddingError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                let mut values = vec![0.0; DIMENSIONS];
                values[0] = 1.0;
                Embedding::new(values)
            }
        }
        let directory = tempfile::tempdir()?;
        let store = Arc::new(SqliteStore::open(
            &directory.path().join("disabled.sqlite3"),
        )?);
        let count = Arc::new(AtomicUsize::new(0));
        let worker = Arc::new(controller(Arc::new(Counting(count.clone()))));
        let retrieval = Arc::new(Retrieval::new(
            store.clone(),
            store,
            worker.clone(),
            PROFILE,
        ));
        retrieval.set_index_enabled(false)?;
        let scheduler = Scheduler::default().with_embedding(worker, retrieval);
        tokio::time::sleep(Duration::from_millis(300)).await;
        scheduler.stop().await;
        assert_eq!(count.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn disabled_ready_worker_can_be_removed_when_idle() -> Result<(), Box<dyn std::error::Error>> {
        let worker = controller(Arc::new(Fixture));
        assert!(worker.warm()?);
        assert!(worker.in_use());
        assert!(worker.with_inactive_removal(|| Ok::<_, ()>(())).is_err());
        worker.set_enabled(false);
        assert!(!worker.in_use());
        assert!(worker.with_inactive_removal(|| Ok::<_, ()>(())).is_ok());
        Ok(())
    }

    #[test]
    fn pending_query_blocks_new_background_passages() -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::{Condvar, mpsc};
        struct Blocking {
            started: mpsc::SyncSender<()>,
            gate: Arc<(Mutex<bool>, Condvar)>,
        }
        impl Embedder for Blocking {
            fn embed(&self, input: Input<'_>) -> Result<Embedding, EmbeddingError> {
                if matches!(input, Input::Passage { .. }) {
                    self.started.send(()).map_err(|_| EmbeddingError::Runtime)?;
                    let (lock, changed) = &*self.gate;
                    let mut open = lock.lock().map_err(|_| EmbeddingError::Runtime)?;
                    while !*open {
                        open = changed.wait(open).map_err(|_| EmbeddingError::Runtime)?;
                    }
                }
                let mut values = vec![0.0; DIMENSIONS];
                values[0] = 1.0;
                Embedding::new(values)
            }
        }
        let (started, received) = mpsc::sync_channel(0);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker = Arc::new(controller(Arc::new(Blocking {
            started,
            gate: gate.clone(),
        })));
        worker.warm()?;
        let background = {
            let worker = worker.clone();
            std::thread::spawn(move || {
                worker.embed(Input::Passage {
                    title: "t",
                    content: "c",
                })
            })
        };
        received.recv_timeout(Duration::from_secs(2))?;
        let foreground = {
            let worker = worker.clone();
            std::thread::spawn(move || worker.embed(Input::Query("foreground")))
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while worker.waiting_queries.load(Ordering::Acquire) == 0
            && std::time::Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        let query_waiting = worker.waiting_queries.load(Ordering::Acquire) > 0;
        let background_busy = matches!(
            worker.embed(Input::Passage {
                title: "t",
                content: "c"
            }),
            Err(EmbeddingError::Busy)
        );
        let (lock, changed) = &*gate;
        *lock.lock().map_err(|_| "poisoned")? = true;
        changed.notify_all();
        background.join().map_err(|_| "background thread")??;
        foreground.join().map_err(|_| "foreground thread")??;
        assert!(query_waiting);
        assert!(background_busy);
        Ok(())
    }
}
