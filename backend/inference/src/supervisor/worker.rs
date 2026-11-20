//! Worker admission, lazy startup, cancellation, and recovery for one profile.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::guardian::Guardian;
use super::{Client, Profile, WorkerError};

pub struct ManagedWorker {
    binary: PathBuf,
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
    /// Finds locally staged assets in the app's inference directory.
    #[must_use]
    pub fn configured(directory: &Path, profile: Profile) -> Option<Self> {
        let binary = directory.join("llama-server").canonicalize().ok()?;
        let model = directory.join(profile.file()).canonicalize().ok()?;
        (binary.is_file() && model.is_file()).then_some(Self {
            binary,
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

    pub(crate) fn run_interruptible<T, E>(
        &self,
        cancelled: Option<&AtomicBool>,
        operation: impl FnOnce(&Client) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<WorkerError>,
    {
        let _permit = self.admission.try_lock().map_err(|_| WorkerError::Busy)?;
        let (client, cold) = self.client(cancelled)?;
        if cold && let Err(error) = self.wait_ready(&client) {
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
        if let Some(worker) = &mut state.worker
            && worker.guardian.is_running().unwrap_or(false)
        {
            return Ok((worker.client.clone(), false));
        }
        let (guardian, endpoint) = Guardian::spawn(&self.binary, &self.model, self.profile)
            .map_err(|_| WorkerError::Unavailable)?;
        let timeout = match self.profile {
            Profile::Embedding => Duration::from_secs(15),
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

    fn wait_ready(&self, client: &Client) -> Result<(), WorkerError> {
        let deadline = Instant::now() + Duration::from_secs(25);
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
            thread::sleep(Duration::from_millis(75));
        }
        Err(WorkerError::Unavailable)
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

    pub(crate) fn unload(&self) {
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
