//! Coordinates an explicitly confirmed, local first-run installation.

use std::{path::PathBuf, sync::Arc};

use ctxfab_inference::generation::SelectionStoreError;
use serde::Deserialize;
use tokio::{
    sync::{Mutex, watch},
    task::JoinHandle,
};
use uuid::Uuid;

use super::{
    answers::Answers,
    artifact_storage::StorageReport,
    model_management::ModelManagement,
    retrieval::{IndexState, Retrieval},
    runtime_setup::RuntimeSetup,
};

mod embedding_selection;
mod execution;
mod intent;
mod plan;
mod resume;

pub use embedding_selection::{EmbeddingModels, EmbeddingSelectionError, embedding_models};
pub use intent::{SetupAsset, SetupIntent, SetupLease, SetupRun};

pub use plan::{PlanError, RuntimeSource, SetupPhase, SetupPlan, SetupSelection, SetupStatus};

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("Setup state could not be stored.")]
    Storage,
    #[error("Another setup operation is active, or this operation ID was already used.")]
    Conflict,
}

pub enum Claim {
    Started(SetupLease),
    Existing(SetupStatus),
}

/// The durable receipt joins the generation commit to the setup step that owns it.
pub trait SetupJournal: Send + Sync {
    /// # Errors
    /// Returns an error if unfinished operations cannot be checkpointed.
    fn recover(&self) -> Result<Option<SetupRun>, JournalError>;
    /// # Errors
    /// Returns an error if the previous runner cannot be fenced.
    fn suspend(&self, lease: SetupLease) -> Result<(), JournalError>;
    /// # Errors
    /// Returns an error if the archive wait state cannot be stored.
    fn set_archive_wait(&self, lease: SetupLease, waiting: bool) -> Result<bool, JournalError>;
    /// # Errors
    /// Returns an error if the generation authority cannot be read.
    fn selection_unchanged(&self, lease: SetupLease) -> Result<bool, JournalError>;
    /// # Errors
    /// Returns an error if the index authority cannot be read.
    fn embedding_unchanged(&self, lease: SetupLease, profile: &str) -> Result<bool, JournalError>;
    /// # Errors
    /// Returns an error if status cannot be read.
    fn current(&self) -> Result<Option<SetupStatus>, JournalError>;
    /// # Errors
    /// Returns an error if the operation fingerprint conflicts or status cannot be read.
    fn lookup(&self, id: Uuid, fingerprint: &str) -> Result<Option<SetupStatus>, JournalError>;
    /// # Errors
    /// Returns an error if another operation owns admission or the intent cannot be stored.
    fn claim(
        &self,
        id: Uuid,
        fingerprint: &str,
        embedding_epoch: Option<i64>,
        intent: &SetupIntent,
    ) -> Result<Claim, JournalError>;
    /// # Errors
    /// Returns an error if the phase change cannot be stored.
    fn transition(
        &self,
        lease: SetupLease,
        from: SetupPhase,
        to: SetupPhase,
        error: Option<&str>,
    ) -> Result<bool, JournalError>;
    /// # Errors
    /// Returns an error if the operation or selection is stale or the commit fails.
    fn commit_activation(
        &self,
        lease: SetupLease,
        profile: &str,
        fingerprint: &str,
        next: SetupPhase,
    ) -> Result<(), SelectionStoreError>;
    /// # Errors
    /// Returns an error if the optional index policy and setup receipt cannot be committed.
    fn commit_embedding(
        &self,
        lease: SetupLease,
        previous_profile: &str,
        profile: &str,
    ) -> Result<Option<IndexState>, JournalError>;

    /// # Errors
    /// Returns an error when the settings selection cannot be stored.
    fn select_embedding(
        &self,
        previous: &IndexState,
        profile: &str,
    ) -> Result<Option<IndexState>, JournalError>;
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Confirmation {
    pub selection: SetupSelection,
    pub fingerprint: String,
    pub operation_id: Uuid,
    pub archive_path: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error(transparent)]
    Journal(#[from] JournalError),
    #[error("Review an updated installation estimate before continuing.")]
    StalePlan,
    #[error("The reviewed installation cannot start with the current library state.")]
    Blocked,
    #[error("Choose a local runtime ZIP for this installation.")]
    ArchiveRequired,
    #[error("The local inference setup is unavailable.")]
    Unavailable,
}

pub struct InferenceSetup {
    capacity: Arc<dyn StorageReport>,
    runtime: Arc<RuntimeSetup>,
    downloads: Arc<ModelManagement>,
    answers: Arc<Answers>,
    retrieval: Arc<Retrieval>,
    journal: Arc<dyn SetupJournal>,
    admission: Mutex<SetupSession>,
    closing: watch::Sender<bool>,
}

struct SetupSession {
    pending: Option<SetupRun>,
    lease: Option<SetupLease>,
    task: Option<JoinHandle<()>>,
}

impl InferenceSetup {
    /// # Errors
    /// Returns a storage error if interrupted setup cannot be recovered.
    pub fn open(
        capacity: Arc<dyn StorageReport>,
        runtime: Arc<RuntimeSetup>,
        downloads: Arc<ModelManagement>,
        answers: Arc<Answers>,
        retrieval: Arc<Retrieval>,
        journal: Arc<dyn SetupJournal>,
    ) -> Result<Self, JournalError> {
        let pending = journal.recover()?;
        let lease = pending.as_ref().map(|run| run.lease);
        Ok(Self {
            capacity,
            runtime,
            downloads,
            answers,
            retrieval,
            journal,
            admission: Mutex::new(SetupSession {
                pending,
                lease,
                task: None,
            }),
            closing: watch::channel(false).0,
        })
    }

    /// # Errors
    /// Returns the capacity or selection error when the plan cannot be calculated.
    pub async fn preview(&self, selection: SetupSelection) -> Result<SetupPlan, SetupError> {
        let capacity = self.capacity_now().await?;
        Ok(plan::plan(capacity, &self.runtime, &self.downloads, selection).await?)
    }

    async fn capacity_now(&self) -> Result<super::artifact_storage::StorageCapacity, SetupError> {
        let report = self.capacity.clone();
        tokio::task::spawn_blocking(move || report.capacity())
            .await
            .map_err(|_| SetupError::Unavailable)?
            .map_err(|_| SetupError::Unavailable)
    }

    /// # Errors
    /// Returns a typed rejection for stale consent, concurrent work, or unavailable storage.
    pub async fn start(
        self: &Arc<Self>,
        confirmed: Confirmation,
    ) -> Result<SetupStatus, SetupError> {
        let mut session = self.admission.lock().await;
        if *self.closing.borrow() {
            return Err(SetupError::Unavailable);
        }
        let id = confirmed.operation_id;
        if let Some(current) = self.journal.lookup(id, &confirmed.fingerprint)? {
            return Ok(current);
        }
        let plan = self.preview(confirmed.selection).await?;
        if plan.fingerprint != confirmed.fingerprint {
            return Err(SetupError::StalePlan);
        }
        if !plan.can_start {
            return Err(SetupError::Blocked);
        }
        let archive = match (plan.selection.runtime_source, confirmed.archive_path) {
            (RuntimeSource::LocalArchive, Some(path)) if valid_archive(&path) => Some(path),
            (RuntimeSource::Hosted, None) => None,
            (RuntimeSource::LocalArchive, None) if !plan.runtime_required => None,
            _ => return Err(SetupError::ArchiveRequired),
        };
        let embedding_epoch = if plan.selection.embedding {
            Some(
                self.retrieval
                    .index_state()
                    .map_err(|_| SetupError::Unavailable)?
                    .epoch,
            )
        } else {
            None
        };
        let lease =
            match self
                .journal
                .claim(id, &plan.fingerprint, embedding_epoch, &plan.intent)?
            {
                Claim::Started(lease) => lease,
                Claim::Existing(current) => return Ok(current),
            };
        let status = SetupStatus {
            operation_id: id,
            phase: SetupPhase::Runtime,
            answer_ready: false,
            selection: Some(plan.intent.selection.clone()),
            error: None,
        };
        self.launch(
            &mut session,
            SetupRun {
                lease,
                status: status.clone(),
                intent: plan.intent,
            },
            archive,
        );
        Ok(status)
    }

    /// # Errors
    /// Returns an error if the durable status cannot be read.
    pub fn status(&self) -> Result<Option<SetupStatus>, JournalError> {
        self.journal.current()
    }

    pub async fn stop(&self) {
        let mut session = self.admission.lock().await;
        self.closing.send_replace(true);
        if let Some(lease) = session.lease
            && matches!(self.journal.suspend(lease), Err(JournalError::Storage))
        {
            tracing::warn!(
                code = "inference_setup_checkpoint_failed",
                "Setup will recover on restart"
            );
        }
        let task = session.task.take();
        drop(session);
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

fn valid_archive(path: &std::path::Path) -> bool {
    path.is_absolute()
        && path.as_os_str().len() <= 2048
        && !path.as_os_str().as_encoded_bytes().contains(&0)
}
