//! Worker admission, lazy startup, cancellation, and recovery for one profile.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::guardian::Guardian;
use super::{Client, Profile, Runtime, WorkerError};

pub struct ManagedWorker {
    runtime: Arc<Runtime>,
    model: PathBuf,
    profile: Profile,
    admission: Mutex<()>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    worker: Option<WorkerSession>,
    stopped: bool,
}

struct WorkerSession {
    guardian: Guardian,
    client: Client,
}

impl ManagedWorker {
    pub(crate) fn alias(&self) -> &str {
        self.profile.alias()
    }

    /// Returns whether this worker's model file is still present.
    #[must_use]
    pub fn weights_exist(&self) -> bool {
        self.runtime
            .admits_weights(&self.model, self.profile.alias())
    }

    /// Finds locally staged assets in the app's inference directory.
    #[must_use]
    pub fn configured(directory: &Path, profile: Profile) -> Option<Self> {
        let runtime = Arc::new(Runtime::preview(directory));
        if !runtime.is_available() {
            return None;
        }
        Self::with_runtime(directory, profile, runtime)
    }

    #[must_use]
    pub fn with_runtime(directory: &Path, profile: Profile, runtime: Arc<Runtime>) -> Option<Self> {
        let path = directory.join(profile.file());
        let model = if runtime.managed_weights() {
            path
        } else {
            path.canonicalize().ok()?
        };
        runtime
            .admits_weights(&model, profile.alias())
            .then_some(Self {
                runtime,
                model,
                profile,
                admission: Mutex::new(()),
                state: Mutex::default(),
            })
    }

    pub(crate) fn run<T, E>(&self, operation: impl FnOnce(&Client) -> Result<T, E>) -> Result<T, E>
    where
        E: From<WorkerError>,
    {
        self.run_interruptible(None, operation)
    }

    pub(crate) fn run_warm<T, E>(
        &self,
        operation: impl FnOnce(&Client) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<WorkerError>,
    {
        let _permit = self.admission.try_lock().map_err(|_| WorkerError::Busy)?;
        let client = {
            let state = self.state.try_lock().map_err(|_| WorkerError::Busy)?;
            if state.stopped {
                return Err(WorkerError::Unavailable.into());
            }
            state
                .worker
                .as_ref()
                .ok_or(WorkerError::Unavailable)?
                .client
                .clone()
        };
        operation(&client)
    }

    pub(crate) fn run_interruptible<T, E>(
        &self,
        cancelled: Option<&AtomicBool>,
        operation: impl FnOnce(&Client) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<WorkerError>,
    {
        self.run_until(cancelled, None, operation)
    }

    pub(crate) fn run_until<T, E>(
        &self,
        cancelled: Option<&AtomicBool>,
        deadline: Option<Instant>,
        operation: impl FnOnce(&Client) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<WorkerError>,
    {
        let _permit = self.admission.try_lock().map_err(|_| WorkerError::Busy)?;
        if deadline.is_some_and(|limit| Instant::now() >= limit) {
            return Err(WorkerError::Deadline.into());
        }
        let (client, cold) = self.client(cancelled)?;
        let client = client.with_deadline(deadline);
        if cold && let Err(error) = self.wait_ready(&client, deadline) {
            if let Ok(mut state) = self.state.lock() {
                state.worker.take();
            }
            return Err(error.into());
        }
        let result = operation(&client);
        // The HTTP connection can close before the guardian observes the worker exit.
        if result.is_err() && !client.ready(self.profile.alias()) {
            self.unload();
        }
        result
    }

    fn client(&self, cancelled: Option<&AtomicBool>) -> Result<(Client, bool), WorkerError> {
        let mut state = self.state.lock().map_err(|_| WorkerError::Unavailable)?;
        if state.stopped || cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err(WorkerError::Unavailable);
        }
        if !self.weights_exist() {
            state.worker.take();
            return Err(WorkerError::Unavailable);
        }
        if let Some(worker) = &mut state.worker
            && worker.guardian.is_running().unwrap_or(false)
        {
            return Ok((worker.client.clone(), false));
        }
        let binary = self.runtime.binary().ok_or(WorkerError::Unavailable)?;
        let (guardian, endpoint) =
            Guardian::spawn(self.runtime.guardian(), binary, &self.model, &self.profile)
                .map_err(|_| WorkerError::Unavailable)?;
        let timeout = match self.profile {
            Profile::Embedding | Profile::EmbeddingF16 | Profile::ImportedEmbedding(_) => {
                Duration::from_secs(15)
            }
            Profile::Generation(_) => Duration::from_secs(45),
        };
        let client = Client::new(endpoint.address, &endpoint.secret, timeout)
            .ok_or(WorkerError::Unavailable)?;
        state.worker = Some(WorkerSession {
            guardian,
            client: client.clone(),
        });
        Ok((client, true))
    }

    fn wait_ready(&self, client: &Client, limit: Option<Instant>) -> Result<(), WorkerError> {
        let normal = Instant::now() + Duration::from_secs(25);
        let deadline = limit.map_or(normal, |limit| limit.min(normal));
        while Instant::now() < deadline {
            {
                let mut state = self.state.lock().map_err(|_| WorkerError::Unavailable)?;
                let worker = state.worker.as_mut().ok_or(WorkerError::Unavailable)?;
                if !worker
                    .guardian
                    .is_running()
                    .map_err(|_| WorkerError::Unavailable)?
                {
                    return Err(WorkerError::Unavailable);
                }
            }
            if client.ready(self.profile.alias()) {
                return Ok(());
            }
            thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(75)),
            );
        }
        if limit.is_some_and(|limit| limit <= normal) {
            Err(WorkerError::Deadline)
        } else {
            Err(WorkerError::Unavailable)
        }
    }

    pub(crate) fn warm(&self) -> Result<(), WorkerError> {
        self.run(|_| Ok(()))
    }

    pub(crate) fn interrupt(&self, cancelled: &AtomicBool) {
        if let Ok(mut state) = self.state.lock() {
            cancelled.store(true, Ordering::Release);
            state.worker.take();
        }
    }

    /// Closes the current worker process while keeping the profile available for a later start.
    pub fn unload(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.worker.take();
        }
    }

    /// Stops the worker and closes any active inference transport.
    pub fn shutdown(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.stopped = true;
            state.worker.take();
        }
    }
}
