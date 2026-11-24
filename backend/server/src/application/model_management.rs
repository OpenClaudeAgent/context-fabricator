//! Catalog, discovery, and installation jobs for the running app session.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use ctxfab_inference::{
    ALTERNATE_EMBEDDING_PROFILE, ALTERNATE_EMBEDDING_SHA256, PROFILE,
    generation::{
        ALTERNATE_PROFILE, ALTERNATE_SHA256, GENERATION_PROFILE, GENERATION_SHA256, Model,
    },
};

use crate::application::inference_setup::SetupLease;
use crate::domain::model::{
    Artifact, ArtifactKind, DownloadError, InstallationStage, SetupRole, Snapshot, State,
    discovery::{DiscoveryError, Repository, Variant, approved_imported_embedding, hex},
};

mod durable;
mod job;
pub use durable::{DownloadJobStore, RemovalIntent, StoredDownload};
use job::{Job, Phase, SavedPhase, Transfer};

pub type ArtifactResult<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, DownloadError>> + Send + 'a>>;
pub type DiscoveryResult<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, DiscoveryError>> + Send + 'a>>;

#[derive(Clone, serde::Serialize)]
pub struct RemovalPlan {
    pub profile: String,
    pub bytes: u64,
    pub identity: String,
}

pub struct LocalArtifactState {
    pub installed: bool,
    pub staged_bytes: u64,
}

/// Storage and installation of a catalog entry.
pub trait ArtifactStore: Send + Sync {
    fn install<'a>(&'a self, artifact: &'a Artifact) -> ArtifactResult<'a, ()>;
    fn install_controlled<'a>(
        &'a self,
        artifact: &'a Artifact,
        pause: tokio::sync::watch::Receiver<bool>,
    ) -> ArtifactResult<'a, ()>;
    fn install_tracked<'a>(
        &'a self,
        artifact: &'a Artifact,
        pause: tokio::sync::watch::Receiver<bool>,
        _stage: tokio::sync::watch::Sender<InstallationStage>,
    ) -> ArtifactResult<'a, ()> {
        self.install_controlled(artifact, pause)
    }
    fn local_state<'a>(&'a self, artifact: &'a Artifact) -> ArtifactResult<'a, LocalArtifactState>;
    /// # Errors
    /// Returns an error if the managed artifact cannot safely be removed.
    fn remove(&self, artifact: &Artifact) -> Result<(), DownloadError>;
    fn requires_guarded_removal(&self) -> bool {
        false
    }
    /// # Errors
    /// Returns an error if file identity cannot be read safely.
    fn removal_identity(
        &self,
        _artifact: &Artifact,
    ) -> Result<Option<(u64, String)>, DownloadError> {
        Err(DownloadError::Unavailable)
    }
    /// # Errors
    /// Returns an error if identity differs or deletion fails.
    fn remove_if_identity(
        &self,
        _artifact: &Artifact,
        _identity: &str,
    ) -> Result<(), DownloadError> {
        Err(DownloadError::Unavailable)
    }
    /// # Errors
    /// Returns an error if the managed partial cannot safely be removed.
    fn discard_partial(&self, artifact: &Artifact) -> Result<(), DownloadError>;
}

/// Looks up compatible artifacts and pins their transfer metadata.
pub trait Discovery: Send + Sync {
    fn search<'a>(
        &'a self,
        query: &'a str,
        kind: ArtifactKind,
    ) -> DiscoveryResult<'a, Vec<Repository>>;
    fn variants<'a>(
        &'a self,
        repository: &'a str,
        revision: &'a str,
        kind: ArtifactKind,
    ) -> DiscoveryResult<'a, Vec<Variant>>;
    fn resolve<'a>(
        &'a self,
        repository: &'a str,
        revision: &'a str,
        file: &'a str,
        kind: ArtifactKind,
    ) -> DiscoveryResult<'a, Artifact>;
}

pub struct RetainedModel {
    pub artifact: Artifact,
    pub verified: bool,
}

/// Retains installation choices and their last verified state across app sessions.
pub trait ModelInventory: Send + Sync {
    /// # Errors
    /// Returns a storage error if the catalog cannot be loaded.
    fn load_models(&self) -> Result<Vec<RetainedModel>, DownloadError>;

    /// # Errors
    /// Returns a storage error if the choice cannot be persisted.
    fn retain_model(&self, model: &Artifact) -> Result<(), DownloadError>;

    /// # Errors
    /// Returns a storage error if the selected model cannot be updated.
    fn mark_verified(&self, profile: &str, verified: bool) -> Result<(), DownloadError>;
}

pub trait ModelUsage: Send + Sync {
    /// Make a successfully verified installation eligible for activation.
    ///
    /// # Errors
    /// Returns an error if the model cannot be registered with inference.
    fn installed(&self, _artifact: &Artifact) -> Result<(), DownloadError> {
        Ok(())
    }

    /// # Errors
    /// Returns an error if the current inference selection cannot be read.
    fn deletion_block(&self, profile: &str) -> Result<Option<&'static str>, DownloadError>;

    /// # Errors
    /// Rejects a profile in use or returns the artifact removal error.
    fn with_removal(
        &self,
        profile: &str,
        remove: &dyn Fn() -> Result<(), DownloadError>,
    ) -> Result<(), DownloadError>;
}

struct Jobs {
    accepting: bool,
    entries: Vec<Job>,
}

struct Hub {
    discovery: Arc<dyn Discovery>,
    inventory: Arc<dyn ModelInventory>,
}

pub struct ModelManagement {
    artifacts: Arc<dyn ArtifactStore>,
    usage: Option<Arc<dyn ModelUsage>>,
    hub: Option<Hub>,
    job_store: Option<Arc<dyn DownloadJobStore>>,
    jobs: Mutex<Jobs>,
}

impl ModelManagement {
    #[must_use]
    pub fn new(catalog: Vec<Artifact>, artifacts: Arc<dyn ArtifactStore>) -> Self {
        Self {
            artifacts,
            usage: None,
            hub: None,
            job_store: None,
            jobs: Mutex::new(Jobs {
                accepting: true,
                entries: catalog
                    .into_iter()
                    .map(|model| Job::new(model, true))
                    .collect(),
            }),
        }
    }

    #[must_use]
    pub fn with_usage(mut self, usage: Arc<dyn ModelUsage>) -> Self {
        self.usage = Some(usage);
        self
    }

    /// # Errors
    /// Returns an error if retained model jobs cannot be registered.
    pub fn with_discovery(
        mut self,
        discovery: Arc<dyn Discovery>,
        inventory: Arc<dyn ModelInventory>,
        retained: Vec<RetainedModel>,
    ) -> Result<Self, DownloadError> {
        self.jobs
            .get_mut()
            .map_err(|_| DownloadError::Unavailable)?
            .entries
            .extend(
                retained
                    .into_iter()
                    .map(|model| Job::new(model.artifact, model.verified)),
            );
        self.hub = Some(Hub {
            discovery,
            inventory,
        });
        Ok(self)
    }

    /// Restores paused/interrupted intent after catalog and inventory are loaded.
    /// Running work is made interrupted; no transfer starts without a new command.
    /// # Errors
    /// Returns an error if durable job state cannot be loaded or reconciled.
    pub fn with_jobs(mut self, store: Arc<dyn DownloadJobStore>) -> Result<Self, DownloadError> {
        let jobs = self
            .jobs
            .get_mut()
            .map_err(|_| DownloadError::Unavailable)?;
        for mut saved in store.load_download_jobs()? {
            let Some(job) = jobs.entries.iter_mut().find(|job| {
                job.artifact.profile == saved.profile && job.artifact.sha256 == saved.fingerprint
            }) else {
                continue;
            };
            if uuid::Uuid::parse_str(&saved.operation_id).is_err() {
                continue;
            }
            let Some(phase) = SavedPhase::parse(&saved) else {
                continue;
            };
            if matches!(phase, SavedPhase::Running) {
                saved.phase = SavedPhase::Interrupted.code().into();
                store.save_download_job(&saved)?;
            }
            job.phase = phase.restore();
            job.operation_id = Some(saved.operation_id);
        }
        for intent in store.load_removals()? {
            if let Err(error) = self.reconcile_removal(store.as_ref(), &intent) {
                tracing::warn!(code = "model_removal_reconciliation_deferred", profile = %intent.profile, error = %error, "Model removal reconciliation deferred");
            }
        }
        self.job_store = Some(store);
        Ok(self)
    }

    fn reconcile_removal(
        &self,
        store: &dyn DownloadJobStore,
        intent: &RemovalIntent,
    ) -> Result<(), DownloadError> {
        let artifact = self
            .jobs
            .lock()
            .map_err(|_| DownloadError::Unavailable)?
            .entries
            .iter()
            .find(|job| {
                job.artifact.profile == intent.profile && job.artifact.sha256 == intent.fingerprint
            })
            .map(|job| job.artifact.clone());
        if let Some(artifact) = artifact {
            let current = self.artifacts.removal_identity(&artifact)?;
            if current
                .as_ref()
                .is_none_or(|(_, identity)| *identity == intent.identity)
            {
                let finish = || {
                    if current.is_some() {
                        self.artifacts
                            .remove_if_identity(&artifact, &intent.identity)?;
                    }
                    self.mark_verified(&artifact, false)?;
                    store.remove_download_job(&intent.profile)
                };
                if current.is_some() {
                    if let Some(usage) = &self.usage {
                        usage.with_removal(&intent.profile, &finish)?;
                    } else {
                        finish()?;
                    }
                } else {
                    finish()?;
                }
                if let Some(job) = self
                    .jobs
                    .lock()
                    .map_err(|_| DownloadError::Unavailable)?
                    .entries
                    .iter_mut()
                    .find(|job| job.artifact.profile == intent.profile)
                {
                    job.verified = job.artifact.remote_file.is_none();
                    job.clear();
                }
            }
        }
        store.clear_removal(&intent.profile)
    }

    fn persist(
        &self,
        artifact: &Artifact,
        operation_id: Option<&str>,
        phase: SavedPhase,
    ) -> Result<(), DownloadError> {
        if let Some(store) = &self.job_store {
            let operation_id = operation_id.ok_or(DownloadError::Unavailable)?;
            store.save_download_job(&phase.record(artifact, operation_id))?;
        }
        Ok(())
    }

    fn clear_job(&self, profile: &str) -> Result<(), DownloadError> {
        self.job_store
            .as_ref()
            .map_or(Ok(()), |store| store.remove_download_job(profile))
    }

    /// # Errors
    /// Rejects unsupported queries or reports a Hub failure.
    pub async fn search(
        &self,
        query: &str,
        kind: ArtifactKind,
    ) -> Result<Vec<Repository>, DiscoveryError> {
        self.hub
            .as_ref()
            .ok_or(DiscoveryError::Unavailable)?
            .discovery
            .search(query, kind)
            .await
    }

    /// # Errors
    /// Rejects unsupported repositories or reports a Hub failure.
    pub async fn variants(
        &self,
        repository: &str,
        revision: &str,
        kind: ArtifactKind,
    ) -> Result<Vec<Variant>, DiscoveryError> {
        self.hub
            .as_ref()
            .ok_or(DiscoveryError::Unavailable)?
            .discovery
            .variants(repository, revision, kind)
            .await
    }

    /// Resolves a chosen file on the Hub, retains its pinned metadata, and starts the transfer.
    ///
    /// # Errors
    /// Rejects unsupported files or reports that discovery or installation is unavailable.
    pub async fn discover_and_request(
        self: &Arc<Self>,
        repository: &str,
        revision: &str,
        file: &str,
        kind: ArtifactKind,
        sha256: Option<&str>,
    ) -> Result<String, DiscoveryError> {
        if (kind == ArtifactKind::Embedding && sha256.is_none())
            || sha256.is_some_and(|pin| !hex(pin, 64))
        {
            return Err(DiscoveryError::Invalid);
        }
        let hub = self.hub.as_ref().ok_or(DiscoveryError::Unavailable)?;
        let model = hub
            .discovery
            .resolve(repository, revision, file, kind)
            .await?;
        if sha256.is_some_and(|pin| pin != model.sha256) {
            return Err(DiscoveryError::StaleApproval);
        }
        let downloads = Arc::clone(self);
        tokio::task::spawn_blocking(move || downloads.request_resolved(model))
            .await
            .map_err(|_| DiscoveryError::Unavailable)?
            .map_err(|_| DiscoveryError::Unavailable)
    }

    fn request_resolved(self: &Arc<Self>, model: Artifact) -> Result<String, DownloadError> {
        let mut jobs = self.jobs.lock().map_err(|_| DownloadError::Unavailable)?;
        if !jobs.accepting {
            return Err(DownloadError::Unavailable);
        }
        let existing = jobs
            .entries
            .iter()
            .position(|job| job.artifact.profile == model.profile);
        let index = if let Some(index) = existing {
            if jobs.entries[index].artifact.sha256 != model.sha256 {
                return Err(DownloadError::Integrity);
            }
            index
        } else {
            let hub = self.hub.as_ref().ok_or(DownloadError::Unavailable)?;
            hub.inventory.retain_model(&model)?;
            let index = jobs.entries.len();
            jobs.entries.push(Job::new(model, false));
            index
        };
        let profile = jobs.entries[index].artifact.profile.to_string();
        self.start(&mut jobs.entries[index], index, None, None)?;
        Ok(profile)
    }

    /// Removes a catalog artifact after checking that installation and inference are idle.
    /// # Errors
    /// Returns a typed error for an unknown, busy, or inaccessible artifact.
    pub fn removal_plan(&self, profile: &str) -> Result<RemovalPlan, DownloadError> {
        let jobs = self.jobs.lock().map_err(|_| DownloadError::Unavailable)?;
        let job = jobs
            .entries
            .iter()
            .find(|job| job.artifact.profile == profile)
            .ok_or(DownloadError::UnknownProfile)?;
        if job.selecting || matches!(job.phase, Phase::Active(_)) || job.phase.can_discard() {
            return Err(DownloadError::InUse);
        }
        if let Some(usage) = &self.usage
            && usage.deletion_block(profile)?.is_some()
        {
            return Err(DownloadError::InUse);
        }
        let (bytes, identity) = self
            .artifacts
            .removal_identity(&job.artifact)?
            .ok_or(DownloadError::StaleRemoval)?;
        Ok(RemovalPlan {
            profile: profile.to_owned(),
            bytes,
            identity,
        })
    }

    /// # Errors
    /// Rejects managed files without an identity or any busy or inaccessible model.
    pub fn remove(&self, profile: &str) -> Result<(), DownloadError> {
        if self.artifacts.requires_guarded_removal() {
            return Err(DownloadError::StaleRemoval);
        }
        self.remove_guarded(profile, None)
    }

    /// # Errors
    /// Rejects stale identities, busy models, or storage failures.
    pub fn remove_guarded(
        &self,
        profile: &str,
        identity: Option<&str>,
    ) -> Result<(), DownloadError> {
        let mut jobs = self.jobs.lock().map_err(|_| DownloadError::Unavailable)?;
        if !jobs.accepting {
            return Err(DownloadError::Unavailable);
        }
        let job = jobs
            .entries
            .iter_mut()
            .find(|job| job.artifact.profile == profile)
            .ok_or(DownloadError::UnknownProfile)?;
        if job.selecting || matches!(job.phase, Phase::Active(_)) || job.phase.can_discard() {
            return Err(DownloadError::InUse);
        }
        let remove = || {
            if self.artifacts.requires_guarded_removal() || identity.is_some() {
                let identity = identity.ok_or(DownloadError::StaleRemoval)?;
                let current = self.artifacts.removal_identity(&job.artifact)?;
                if let Some((_, current)) = current {
                    if current != identity {
                        return Err(DownloadError::StaleRemoval);
                    }
                    if self.artifacts.requires_guarded_removal() {
                        let store = self.job_store.as_ref().ok_or(DownloadError::Unavailable)?;
                        store.save_removal(&RemovalIntent {
                            profile: profile.to_owned(),
                            fingerprint: job.artifact.sha256.to_string(),
                            identity: identity.to_owned(),
                        })?;
                    }
                    self.artifacts.remove_if_identity(&job.artifact, identity)?;
                }
                self.mark_verified(&job.artifact, false)?;
            } else {
                self.artifacts.remove(&job.artifact)?;
                self.mark_verified(&job.artifact, false)?;
            }
            Ok(())
        };
        if let Some(usage) = &self.usage {
            usage.with_removal(profile, &remove)?;
        } else {
            remove()?;
        }
        self.clear_job(profile)?;
        if self.artifacts.requires_guarded_removal() {
            self.job_store
                .as_ref()
                .ok_or(DownloadError::Unavailable)?
                .clear_removal(profile)?;
        }
        job.verified = job.artifact.remote_file.is_none();
        job.clear();
        Ok(())
    }

    /// Starts one supervised installation for a catalog profile.
    /// # Errors
    /// Returns an error for an unknown profile or unavailable session.
    pub fn request(self: &Arc<Self>, profile: &str) -> Result<(), DownloadError> {
        self.request_install(profile, None, None)
    }

    pub(super) fn request_for_setup(
        self: &Arc<Self>,
        profile: &str,
        additional_bytes: u64,
        lease: SetupLease,
        approved_previous_job: Option<&str>,
    ) -> Result<(), DownloadError> {
        self.request_install(
            profile,
            Some(additional_bytes),
            Some((lease, approved_previous_job)),
        )
    }

    fn request_install(
        self: &Arc<Self>,
        profile: &str,
        setup_allowance: Option<u64>,
        setup_claim: Option<(SetupLease, Option<&str>)>,
    ) -> Result<(), DownloadError> {
        let mut jobs = self.jobs.lock().map_err(|_| DownloadError::Unavailable)?;
        if !jobs.accepting {
            return Err(DownloadError::Unavailable);
        }
        let (index, job) = jobs
            .entries
            .iter_mut()
            .enumerate()
            .find(|(_, job)| job.artifact.profile == profile)
            .ok_or(DownloadError::UnknownProfile)?;
        if setup_allowance.is_some() && job.phase.is_paused() {
            return Err(DownloadError::Paused);
        }
        self.start(job, index, setup_allowance, setup_claim)
    }

    /// Discard only the partial bytes of a stopped operation.
    ///
    /// # Errors
    /// Rejects active, committing, unavailable, or unknown operations.
    pub fn discard_partial(&self, profile: &str) -> Result<(), DownloadError> {
        let mut jobs = self.jobs.lock().map_err(|_| DownloadError::Unavailable)?;
        if !jobs.accepting {
            return Err(DownloadError::Unavailable);
        }
        let job = jobs
            .entries
            .iter_mut()
            .find(|job| job.artifact.profile == profile)
            .ok_or(DownloadError::UnknownProfile)?;
        if !job.phase.can_discard() {
            return Err(DownloadError::InUse);
        }
        self.artifacts.discard_partial(&job.artifact)?;
        self.clear_job(profile)?;
        job.clear();
        Ok(())
    }

    /// Persist explicit pause before signalling the cooperative transfer.
    /// # Errors
    /// Returns an error for an unknown, inactive, or unavailable operation.
    pub fn pause(&self, profile: &str) -> Result<(), DownloadError> {
        let mut jobs = self.jobs.lock().map_err(|_| DownloadError::Unavailable)?;
        if !jobs.accepting {
            return Err(DownloadError::Unavailable);
        }
        let job = jobs
            .entries
            .iter_mut()
            .find(|job| job.artifact.profile == profile)
            .ok_or(DownloadError::UnknownProfile)?;
        if job.phase.is_paused() {
            return Ok(());
        }
        let Phase::Active(transfer) = &job.phase else {
            return Err(DownloadError::InUse);
        };
        self.persist(
            &job.artifact,
            job.operation_id.as_deref(),
            SavedPhase::Paused,
        )?;
        transfer.pause.send_replace(true);
        Ok(())
    }

    /// Resume a paused, interrupted, or failed operation after explicit user intent.
    /// # Errors
    /// Rejects a profile without an eligible stopped operation.
    pub fn resume(self: &Arc<Self>, profile: &str) -> Result<(), DownloadError> {
        let mut jobs = self.jobs.lock().map_err(|_| DownloadError::Unavailable)?;
        if !jobs.accepting {
            return Err(DownloadError::Unavailable);
        }
        let (index, job) = jobs
            .entries
            .iter_mut()
            .enumerate()
            .find(|(_, job)| job.artifact.profile == profile)
            .ok_or(DownloadError::UnknownProfile)?;
        if !job.phase.can_resume() {
            return Err(DownloadError::InUse);
        }
        self.start(job, index, None, None)
    }

    fn start(
        self: &Arc<Self>,
        job: &mut Job,
        index: usize,
        setup_allowance: Option<u64>,
        setup_claim: Option<(SetupLease, Option<&str>)>,
    ) -> Result<(), DownloadError> {
        if job.selecting {
            return Err(DownloadError::InUse);
        }
        if let Some((lease, approved_previous_job)) = setup_claim {
            self.claim_setup_job(job, lease, approved_previous_job)?;
        }
        if let Phase::Active(transfer) = &job.phase {
            return if transfer.is_pausing() {
                Err(DownloadError::InUse)
            } else {
                Ok(())
            };
        }
        let operation_id = match &job.operation_id {
            Some(id) => id.clone(),
            None => crate::runtime::entropy::random_uuid()
                .map_err(|_| DownloadError::Unavailable)?
                .to_string(),
        };
        if setup_claim.is_none() {
            self.persist(&job.artifact, Some(&operation_id), SavedPhase::Running)?;
        }
        job.operation_id = Some(operation_id.clone());
        let (pause, receiver) = tokio::sync::watch::channel(false);
        let (stage_sender, stage) = tokio::sync::watch::channel(InstallationStage::Preparing);
        let downloads = self.clone();
        let artifact = job.artifact.clone();
        let task = tokio::spawn(async move {
            let outcome = downloads
                .install(&artifact, receiver, stage_sender, setup_allowance)
                .await;
            if let Ok(mut jobs) = downloads.jobs.lock() {
                downloads.finish(&mut jobs.entries[index], &operation_id, outcome);
            }
        });
        job.phase = Phase::Active(Transfer { task, pause, stage });
        Ok(())
    }

    fn finish(&self, job: &mut Job, operation_id: &str, outcome: Result<(), DownloadError>) {
        if !matches!(job.phase, Phase::Active(_))
            || job.operation_id.as_deref() != Some(operation_id)
        {
            return;
        }
        let outcome_code = if outcome == Err(DownloadError::Paused) {
            job.phase = Phase::Paused;
            "paused"
        } else {
            let result = outcome
                .and_then(|()| self.mark_verified(&job.artifact, true))
                .and_then(|()| {
                    self.usage
                        .as_ref()
                        .map_or(Ok(()), |usage| usage.installed(&job.artifact))
                })
                .and_then(|()| self.clear_job(&job.artifact.profile));
            match result {
                Ok(()) => {
                    job.verified = true;
                    job.clear();
                    "installed"
                }
                Err(error) => {
                    let visible_error = if self
                        .persist(&job.artifact, Some(operation_id), SavedPhase::Failed(error))
                        .is_ok()
                    {
                        error
                    } else {
                        DownloadError::Unavailable
                    };
                    job.phase = Phase::Failed(visible_error);
                    visible_error.code()
                }
            }
        };
        tracing::info!(
            code = "model_installation",
            profile = %job.artifact.profile,
            outcome = outcome_code,
            "Model installation finished"
        );
    }

    fn claim_setup_job(
        &self,
        job: &mut Job,
        lease: SetupLease,
        approved_previous_job: Option<&str>,
    ) -> Result<(), DownloadError> {
        let owner = lease.id.to_string();
        let current = job.operation_id.as_deref();
        if job.phase.is_paused() {
            return Err(DownloadError::Paused);
        }
        let permitted = if current == Some(owner.as_str()) {
            !matches!(job.phase, Phase::Failed(_))
        } else {
            !matches!(job.phase, Phase::Active(_))
                && current == approved_previous_job
                && (current.is_none() || matches!(job.phase, Phase::Interrupted | Phase::Failed(_)))
        };
        if !permitted {
            return Err(DownloadError::InUse);
        }
        let saved = SavedPhase::Running.record(&job.artifact, &owner);
        self.job_store
            .as_ref()
            .ok_or(DownloadError::Unavailable)?
            .save_setup_job(&saved, lease, approved_previous_job)?;
        job.operation_id = Some(owner);
        Ok(())
    }

    async fn install(
        &self,
        artifact: &Artifact,
        pause: tokio::sync::watch::Receiver<bool>,
        stage: tokio::sync::watch::Sender<InstallationStage>,
        setup_allowance: Option<u64>,
    ) -> Result<(), DownloadError> {
        if let Some(allowance) = setup_allowance {
            let observed = self.artifacts.local_state(artifact).await?;
            if !observed.installed
                && crate::domain::model::staged_weight_peak(artifact.bytes, observed.staged_bytes)
                    > allowance
            {
                return Err(DownloadError::StorageLimit);
            }
        }
        self.artifacts.install_tracked(artifact, pause, stage).await
    }

    fn mark_verified(&self, model: &Artifact, verified: bool) -> Result<(), DownloadError> {
        if let (Some(hub), Some(_)) = (&self.hub, &model.remote_file) {
            hub.inventory.mark_verified(&model.profile, verified)?;
        }
        Ok(())
    }

    pub(super) fn reserve_embedding(
        self: &Arc<Self>,
        profile: &str,
    ) -> Result<EmbeddingSelectionLease, DownloadError> {
        let mut jobs = self.jobs.lock().map_err(|_| DownloadError::Unavailable)?;
        if !jobs.accepting {
            return Err(DownloadError::Unavailable);
        }
        let job = jobs
            .entries
            .iter_mut()
            .find(|job| {
                job.artifact.profile == profile && Self::setup_embedding_allowed(&job.artifact)
            })
            .ok_or(DownloadError::UnknownProfile)?;
        if job.selecting || matches!(job.phase, Phase::Active(_)) {
            return Err(DownloadError::InUse);
        }
        if !job.verified || !matches!(job.phase, Phase::Idle) {
            return Err(DownloadError::UnknownProfile);
        }
        job.selecting = true;
        Ok(EmbeddingSelectionLease {
            downloads: self.clone(),
            artifact: job.artifact.clone(),
        })
    }

    pub(super) fn setup_embedding(&self, profile: &str) -> Result<Artifact, DownloadError> {
        let artifact = self.artifact(profile)?;
        if !Self::setup_embedding_allowed(&artifact) {
            return Err(DownloadError::UnknownProfile);
        }
        Ok(artifact)
    }

    fn setup_embedding_allowed(artifact: &Artifact) -> bool {
        artifact.profile == PROFILE
            || (artifact.profile == ALTERNATE_EMBEDDING_PROFILE
                && artifact.sha256 == ALTERNATE_EMBEDDING_SHA256)
            || approved_imported_embedding(artifact)
    }

    pub(super) fn setup_answer(&self, profile: &str) -> Result<Artifact, DownloadError> {
        let artifact = self.artifact(profile)?;
        if Self::setup_answer_role(&artifact).is_none() {
            return if matches!(profile, GENERATION_PROFILE | ALTERNATE_PROFILE) {
                Err(DownloadError::Unavailable)
            } else {
                Err(DownloadError::UnknownProfile)
            };
        }
        Ok(artifact)
    }

    fn setup_answer_role(artifact: &Artifact) -> Option<SetupRole> {
        match artifact.profile.as_ref() {
            GENERATION_PROFILE if artifact.sha256 == GENERATION_SHA256 => {
                Some(SetupRole::RecommendedAnswer)
            }
            ALTERNATE_PROFILE if artifact.sha256 == ALTERNATE_SHA256 => {
                Some(SetupRole::AlternativeAnswer)
            }
            profile
                if matches!(artifact.kind, Some(ArtifactKind::Generation))
                    && artifact.remote_file.is_some()
                    && Model::managed(
                        profile,
                        artifact.label.as_deref().unwrap_or_default(),
                        &artifact.sha256,
                    )
                    .is_some() =>
            {
                Some(SetupRole::AlternativeAnswer)
            }
            _ => None,
        }
    }

    pub(super) fn artifact(&self, profile: &str) -> Result<Artifact, DownloadError> {
        self.jobs
            .lock()
            .map_err(|_| DownloadError::Unavailable)?
            .entries
            .iter()
            .find(|job| job.artifact.profile == profile)
            .map(|job| job.artifact.clone())
            .ok_or(DownloadError::UnknownProfile)
    }

    /// # Errors
    /// Returns an error if the session state or model directory is unreadable.
    pub async fn snapshot(&self) -> Result<Vec<Snapshot>, DownloadError> {
        let entries = self
            .jobs
            .lock()
            .map_err(|_| DownloadError::Unavailable)?
            .entries
            .iter()
            .map(|job| {
                (
                    job.artifact.clone(),
                    job.phase.status(),
                    job.phase.installation_stage(),
                    job.phase.failure(),
                    job.verified,
                    job.operation_id.clone(),
                )
            })
            .collect::<Vec<_>>();
        let mut catalog = Vec::with_capacity(entries.len());
        for (model, phase, stage, failure, verified, operation_id) in entries {
            catalog.push(
                self.snapshot_model(model, phase, stage, failure, verified, operation_id)
                    .await?,
            );
        }
        Ok(catalog)
    }

    async fn snapshot_model(
        &self,
        model: Artifact,
        phase: State,
        installation_stage: Option<InstallationStage>,
        failure: Option<DownloadError>,
        verified: bool,
        operation_id: Option<String>,
    ) -> Result<Snapshot, DownloadError> {
        let disk = self.artifacts.local_state(&model).await?;
        let state = if phase == State::NotInstalled && verified && disk.installed {
            State::Installed
        } else {
            phase
        };
        let deletion_block = self
            .usage
            .as_ref()
            .map(|usage| usage.deletion_block(&model.profile))
            .transpose()?
            .flatten();
        let setup_role = if Self::setup_embedding_allowed(&model) {
            Some(SetupRole::OptionalEmbedding)
        } else {
            Self::setup_answer_role(&model)
        };
        Ok(Snapshot {
            profile: model.profile,
            label: model.label,
            kind: model.kind,
            source: model.source,
            setup_role,
            deletion_block,
            state,
            installation_stage,
            operation_id,
            downloaded_bytes: if state == State::Installed {
                model.bytes
            } else {
                disk.staged_bytes
            },
            total_bytes: model.bytes,
            error: failure.map(DownloadError::code),
        })
    }

    pub async fn stop(&self) {
        let tasks = if let Ok(mut jobs) = self.jobs.lock() {
            jobs.accepting = false;
            jobs.entries
                .iter_mut()
                .filter_map(|job| {
                    let (task, phase) = job.interrupt()?;
                    if let Some(phase) = phase
                        && self
                            .persist(&job.artifact, job.operation_id.as_deref(), phase)
                            .is_err()
                    {
                        tracing::warn!(
                            code = "model_job_persistence_failed",
                            "Could not save interrupted model download"
                        );
                    }
                    Some(task)
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
    }
}

pub(super) struct EmbeddingSelectionLease {
    downloads: Arc<ModelManagement>,
    artifact: Artifact,
}

impl EmbeddingSelectionLease {
    pub(super) async fn is_installed(&self) -> Result<bool, DownloadError> {
        Ok(self
            .downloads
            .artifacts
            .local_state(&self.artifact)
            .await?
            .installed)
    }
}

impl Drop for EmbeddingSelectionLease {
    fn drop(&mut self) {
        if let Ok(mut jobs) = self.downloads.jobs.lock()
            && let Some(job) = jobs
                .entries
                .iter_mut()
                .find(|job| job.artifact.profile == self.artifact.profile)
        {
            job.selecting = false;
        }
    }
}
